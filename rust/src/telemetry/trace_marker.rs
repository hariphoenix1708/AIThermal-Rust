use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static HANDLE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

fn get_handle() -> &'static Mutex<Option<File>> {
    HANDLE.get_or_init(|| {
        let paths = [
            "/sys/kernel/tracing/trace_marker",
            "/sys/kernel/debug/tracing/trace_marker",
        ];
        let f = paths.iter()
            .find(|p| Path::new(p).exists())
            .and_then(|p| OpenOptions::new().write(true).open(p).ok());
        Mutex::new(f)
    })
}

/// Emit a single trace marker line. Silently no-ops if the marker
/// file does not exist or the write fails. NEVER logs on the hot
/// path (would be worse than the feature itself).
pub fn emit(enabled: bool, msg: &str) {
    if !enabled { return; }
    if let Ok(mut guard) = get_handle().lock() {
        if let Some(f) = guard.as_mut() {
            let _ = writeln!(f, "{}", msg);
        }
    }
}
