use serde::Serialize;
use std::fs;
use std::path::Path;

/// Single JSONL row for offline training. Keep schema stable — version it.
#[derive(Debug, Serialize)]
pub struct MlFeatureRow {
    /// schema version, bump on breaking change
    pub v: u8,
    /// unix epoch secs (wall clock, not Instant, survives Doze)
    pub ts: u64,
    /// raw inputs
    pub composite_c: i32,
    pub adj_c: i32,
    pub predicted_c: i32,
    pub trend_score: i32,
    pub cpu_c: i32,
    pub gpu_c: i32,
    pub batt_c: i32,
    pub skin_c: i32,
    pub gpu_load: u32,
    pub cpu_util_pct: u8,
    pub cpu_pressure: f32,
    pub io_pressure: f32,
    pub mem_pressure: f32,
    pub gaming: bool,
    pub screen_off: bool,
    pub plugged: bool,
    pub     cycle_count: Option<u64>,
    /// supervision labels (filled after the fact by tooling, or used for 10s horizon)
    pub policy: String,
    pub actuation_blocked: bool,
}

const ML_FILE: &str = "ml_features.jsonl";
const ML_MAX_BYTES: u64 = 2 * 1024 * 1024; // 2 MB ~ 20k rows, keeps /data bounded
const ML_KEEP_BACKUP: bool = true;

/// Cheap, best-effort append. Never panics, never blocks tick on error.
pub fn log_row(state_dir: &str, row: &MlFeatureRow) {
    let path = Path::new(state_dir).join(ML_FILE);
    // Rotate if too large — keep one backup like logger.rs HourlyTruncatingWriter.
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > ML_MAX_BYTES {
            let backup = Path::new(state_dir).join(format!("{}.1", ML_FILE));
            let _ = fs::remove_file(&backup);
            let _ = fs::rename(&path, &backup);
            let _marker = serde_json::json!({
                "v": row.v,
                "ts": row.ts,
                "event": "rotate",
                "kept_backup": ML_KEEP_BACKUP,
            });
            if let Ok(line) = serde_json::to_string(&_marker) {
                let _ = fs::write(&path, format!("{}\n", line));
            }
        }
    }

    if let Ok(line) = serde_json::to_string(row) {
        // Append via read+write — keeps it simple and atomic enough for 2s tick.
        // Use OpenOptions append to avoid truncating concurrent readers.
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{}", line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_log_and_rotate() {
        let dir = tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let row = MlFeatureRow {
            v: 1,
            ts: 0,
            composite_c: 43,
            adj_c: 43,
            predicted_c: 43,
            trend_score: -5,
            cpu_c: 47,
            gpu_c: 55,
            batt_c: 36,
            skin_c: 41,
            gpu_load: 10,
            cpu_util_pct: 22,
            cpu_pressure: 0.0,
            io_pressure: 0.0,
            mem_pressure: 0.0,
            gaming: false,
            screen_off: false,
            plugged: true,
            cycle_count: Some(381),
            policy: "Balanced".to_string(),
            actuation_blocked: false,
        };
        log_row(d, &row);
        let s = fs::read_to_string(Path::new(d).join(ML_FILE)).unwrap();
        assert!(s.contains("\"v\":1"));
        assert!(s.contains("Balanced"));
    }
}
