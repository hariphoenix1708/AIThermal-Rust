use anyhow::Result;

use lazy_static::lazy_static;
use std::collections::HashSet;
use std::sync::Mutex;

lazy_static! {
    static ref SYSFS_BLACKLIST: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
}

pub fn blacklist_sysfs_node(path: &str) {
    if let Ok(mut blacklist) = SYSFS_BLACKLIST.lock() {
        blacklist.insert(path.to_string());
    }
}

pub fn is_sysfs_blacklisted(path: &str) -> bool {
    if let Ok(blacklist) = SYSFS_BLACKLIST.lock() {
        blacklist.contains(path)
    } else {
        false
    }
}

use std::fs;
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub struct LoggerGuards {
    pub _normal: WorkerGuard,
    pub _verbose: WorkerGuard,
}

const LOG_TRUNCATE_INTERVAL_SECS: u64 = 2 * 60 * 60;

// Periodic truncating writer truncates logs in place every two hours.
// NOTE: This intentionally loses all historical logs from previous intervals.
// If historical back-ups are needed, consider renaming to a '.1' backup
// instead of truncating in place.
struct HourlyTruncatingWriter {
    path: std::path::PathBuf,
    file: std::fs::File,
    opened_at: std::time::Instant,
}

impl HourlyTruncatingWriter {
    fn new(path: impl Into<std::path::PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            opened_at: std::time::Instant::now(),
        })
    }

    fn maybe_rotate(&mut self) -> std::io::Result<()> {
        if self.opened_at.elapsed().as_secs() >= LOG_TRUNCATE_INTERVAL_SECS {
            self.file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?;
            self.opened_at = std::time::Instant::now();
        }
        Ok(())
    }
}

impl std::io::Write for HourlyTruncatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.maybe_rotate()?;
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub fn init_logger(
    level: &str,
    log_dir: &str,
    _rotate_mb: u64,
    _retain_count: u32,
) -> Result<LoggerGuards> {
    let log_level = match level.to_uppercase().as_str() {
        "DEBUG" => LevelFilter::DEBUG,
        "INFO" => LevelFilter::INFO,
        "WARN" => LevelFilter::WARN,
        "ERROR" => LevelFilter::ERROR,
        "TRACE" => LevelFilter::TRACE,
        _ => LevelFilter::INFO,
    };

    let _ = fs::create_dir_all(log_dir);

    // Normal log: thermalai.log
    let normal_path = std::path::Path::new(log_dir).join("thermalai.log");
    let normal_appender = HourlyTruncatingWriter::new(&normal_path)?;
    let (normal_writer, normal_guard) = tracing_appender::non_blocking(normal_appender);

    // Verbose log: thermalai_verbose.log
    let verbose_path = std::path::Path::new(log_dir).join("thermalai_verbose.log");
    let verbose_appender = HourlyTruncatingWriter::new(&verbose_path)?;
    let (verbose_writer, verbose_guard) = tracing_appender::non_blocking(verbose_appender);

    let format = fmt::format()
        .with_level(true)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_ansi(false)
        .compact();

    let normal_filter = EnvFilter::from_default_env().add_directive(log_level.into());
    let verbose_filter = EnvFilter::from_default_env().add_directive(LevelFilter::TRACE.into());

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .event_format(format.clone())
                .with_writer(normal_writer)
                .with_filter(normal_filter),
        )
        .with(
            fmt::layer()
                .event_format(format)
                .with_writer(verbose_writer)
                .with_filter(verbose_filter),
        )
        .init();

    Ok(LoggerGuards {
        _normal: normal_guard,
        _verbose: verbose_guard,
    })
}
