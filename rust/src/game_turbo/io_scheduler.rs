//! I/O scheduler boost — switch block device I/O schedulers to
//! low-latency mode and increase read-ahead during gaming.
//!
//! Uses `mq-deadline` (lower latency than `bfq`) and bumps
//! `read_ahead_kb` for sequential game asset loading. All values
//! saved for restoration on game exit.

use std::collections::HashMap;
use std::fs;

/// Target I/O scheduler for gaming (lower latency for random I/O).
const GAMING_SCHEDULER: &str = "mq-deadline";
/// Read-ahead in KB for gaming (sequential asset streaming).
const GAMING_READ_AHEAD: &str = "2048";

/// Block devices to skip (virtual / not real storage).
const SKIP_PREFIXES: &[&str] = &["dm-", "loop", "zram"];

pub struct IoSchedulerState {
    /// dev_name -> original scheduler name.
    saved_schedulers: HashMap<String, String>,
    /// dev_name -> original read_ahead_kb.
    saved_read_ahead: HashMap<String, u64>,
}

impl IoSchedulerState {
    pub fn new() -> Self {
        Self {
            saved_schedulers: HashMap::new(),
            saved_read_ahead: HashMap::new(),
        }
    }

    /// Activate: switch all real block devices to gaming I/O profile.
    pub fn activate(&mut self) {
        let Ok(entries) = fs::read_dir("/sys/block") else {
            return;
        };

        for entry in entries.flatten() {
            let dev_name = entry.file_name();
            let dev_str = dev_name.to_string_lossy();

            if SKIP_PREFIXES.iter().any(|p| dev_str.starts_with(p)) {
                continue;
            }

            let queue_dir = entry.path().join("queue");
            let sched_path = queue_dir.join("scheduler");
            let ra_path = queue_dir.join("read_ahead_kb");

            // Switch scheduler.
            if sched_path.exists()
                && let Ok(orig) = fs::read_to_string(&sched_path)
            {
                let orig_str = normalize_active_scheduler(&orig);
                // Check if mq-deadline is even available.
                if orig.contains(GAMING_SCHEDULER) {
                    self.saved_schedulers
                        .insert(dev_str.to_string(), orig_str);
                    let path_str = sched_path.to_string_lossy().to_string();
                    write_str(&path_str, GAMING_SCHEDULER);
                    tracing::debug!(
                        target: "game_turbo",
                        "I/O scheduler: {} -> {} (gaming)",
                        dev_str, GAMING_SCHEDULER
                    );
                }
            }

            // Bump read-ahead.
            if ra_path.exists()
                && let Ok(orig) = fs::read_to_string(&ra_path)
            {
                let orig_val: u64 = orig.trim().parse().unwrap_or(128);
                self.saved_read_ahead
                    .insert(dev_str.to_string(), orig_val);
                let path_str = ra_path.to_string_lossy().to_string();
                write_str(&path_str, GAMING_READ_AHEAD);
                tracing::debug!(
                    target: "game_turbo",
                    "Read-ahead: {} {} -> {} (gaming)",
                    dev_str, orig_val, GAMING_READ_AHEAD
                );
            }
        }

        let count = self.saved_schedulers.len().max(self.saved_read_ahead.len());
        if count > 0 {
            tracing::info!(
                target: "game_turbo",
                "I/O scheduler: boosted {} block devices",
                count
            );
        }
    }

    /// Restore all saved I/O scheduler and read-ahead values.
    pub fn deactivate(&mut self) {
        for (dev, orig_sched) in &self.saved_schedulers {
            let path = format!("/sys/block/{}/queue/scheduler", dev);
            write_str(&path, orig_sched);
            tracing::debug!(
                target: "game_turbo",
                "I/O scheduler: {} -> {} (restored)",
                dev, orig_sched
            );
        }

        for (dev, orig_ra) in &self.saved_read_ahead {
            let path = format!("/sys/block/{}/queue/read_ahead_kb", dev);
            write_str(&path, &orig_ra.to_string());
            tracing::debug!(
                target: "game_turbo",
                "Read-ahead: {} -> {} (restored)",
                dev, orig_ra
            );
        }

        let count = self.saved_schedulers.len().max(self.saved_read_ahead.len());
        if count > 0 {
            tracing::info!(
                target: "game_turbo",
                "I/O scheduler: restored {} block devices",
                count
            );
        }

        self.saved_schedulers.clear();
        self.saved_read_ahead.clear();
    }
}

/// Extract the active scheduler from the bracketed kernel format:
/// "none [mq-deadline] kyber" -> "mq-deadline"
fn normalize_active_scheduler(raw: &str) -> String {
    raw.split_ascii_whitespace()
        .find_map(|t| {
            t.strip_prefix('[')
                .and_then(|t| t.strip_suffix(']'))
                .map(String::from)
        })
        .unwrap_or_else(|| raw.trim().to_string())
}

fn write_str(path: &str, value: &str) -> bool {
    use std::io::Write;
    match fs::OpenOptions::new().write(true).truncate(true).open(path) {
        Ok(mut file) => {
            if file.write_all(value.trim().as_bytes()).is_ok() && file.flush().is_ok() {
                true
            } else {
                let err = std::io::Error::last_os_error();
                tracing::debug!(target: "game_turbo", "Write failed {}: {}", path, err);
                false
            }
        }
        Err(e) => {
            tracing::debug!(target: "game_turbo", "Cannot open {}: {}", path, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_active_scheduler() {
        assert_eq!(
            normalize_active_scheduler("none [mq-deadline] kyber"),
            "mq-deadline"
        );
        assert_eq!(
            normalize_active_scheduler("none [bfq] mq-deadline"),
            "bfq"
        );
        assert_eq!(normalize_active_scheduler("[mq-deadline]"), "mq-deadline");
    }

    #[test]
    fn test_io_scheduler_state_new() {
        let state = IoSchedulerState::new();
        assert!(state.saved_schedulers.is_empty());
        assert!(state.saved_read_ahead.is_empty());
    }
}
