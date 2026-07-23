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
    pub _battery: WorkerGuard,
    pub _thermal: WorkerGuard,
    pub _charging: WorkerGuard,
    pub _gaming: WorkerGuard,
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
    let _ = fs::create_dir_all(log_dir);

    let normal_path = std::path::Path::new(log_dir).join("thermalai.log");
    let normal_appender = HourlyTruncatingWriter::new(&normal_path)?;
    let (normal_writer, normal_guard) = tracing_appender::non_blocking(normal_appender);

    let verbose_path = std::path::Path::new(log_dir).join("thermalai_verbose.log");
    let verbose_appender = HourlyTruncatingWriter::new(&verbose_path)?;
    let (verbose_writer, verbose_guard) = tracing_appender::non_blocking(verbose_appender);

    let battery_path = std::path::Path::new(log_dir).join("thermalai_battery.log");
    let battery_appender = HourlyTruncatingWriter::new(&battery_path)?;
    let (battery_writer, battery_guard) = tracing_appender::non_blocking(battery_appender);

    let thermal_path = std::path::Path::new(log_dir).join("thermalai_thermal.log");
    let thermal_appender = HourlyTruncatingWriter::new(&thermal_path)?;
    let (thermal_writer, thermal_guard) = tracing_appender::non_blocking(thermal_appender);

    let charging_path = std::path::Path::new(log_dir).join("thermalai_charging.log");
    let charging_appender = HourlyTruncatingWriter::new(&charging_path)?;
    let (charging_writer, charging_guard) = tracing_appender::non_blocking(charging_appender);

    let gaming_path = std::path::Path::new(log_dir).join("thermalai_gaming.log");
    let gaming_appender = HourlyTruncatingWriter::new(&gaming_path)?;
    let (gaming_writer, gaming_guard) = tracing_appender::non_blocking(gaming_appender);

    let format = fmt::format()
        .with_level(true)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_ansi(false)
        .compact();

    // ---- Main log: high-signal lifecycle + warnings + errors, NEVER the
    //      per-tick domain firehose. RUST_LOG wins; else fall back to the
    //      `log_level` string from profiles.conf; else "info".
    let main_env = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| level.to_lowercase());
    let main_filter = EnvFilter::try_new(&main_env)
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("battery=off".parse().unwrap())
        .add_directive("thermal=off".parse().unwrap())
        .add_directive("charging=off".parse().unwrap())
        .add_directive("gaming=off".parse().unwrap())
        .add_directive("wake=off".parse().unwrap());

    // ---- Verbose: everything, always.
    let verbose_filter = EnvFilter::from_default_env()
        .add_directive(LevelFilter::TRACE.into());

    // ---- Domain writers: default OFF, admit only their own target at INFO+.
    //      Anchor with a leading module-off directive so untargeted
    //      thermalai_daemon::* events cannot leak in.
    let battery_filter  = EnvFilter::new("off,thermalai_daemon=off,lifecycle=off,battery=info");
    let thermal_filter  = EnvFilter::new("off,thermalai_daemon=off,lifecycle=off,thermal=info");
    let charging_filter = EnvFilter::new("off,thermalai_daemon=off,lifecycle=off,charging=info");
    let gaming_filter   = EnvFilter::new("off,thermalai_daemon=off,lifecycle=off,gaming=info");

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .event_format(format.clone())
                .with_writer(normal_writer)
                .with_filter(main_filter),
        )
        .with(
            fmt::layer()
                .event_format(format.clone())
                .with_writer(verbose_writer)
                .with_filter(verbose_filter),
        )
        .with(
            fmt::layer()
                .event_format(format.clone())
                .with_writer(battery_writer)
                .with_filter(battery_filter),
        )
        .with(
            fmt::layer()
                .event_format(format.clone())
                .with_writer(thermal_writer)
                .with_filter(thermal_filter),
        )
        .with(
            fmt::layer()
                .event_format(format.clone())
                .with_writer(charging_writer)
                .with_filter(charging_filter),
        )
        .with(
            fmt::layer()
                .event_format(format)
                .with_writer(gaming_writer)
                .with_filter(gaming_filter),
        )
        .init();

    Ok(LoggerGuards {
        _normal: normal_guard,
        _verbose: verbose_guard,
        _battery: battery_guard,
        _thermal: thermal_guard,
        _charging: charging_guard,
        _gaming: gaming_guard,
    })
}
