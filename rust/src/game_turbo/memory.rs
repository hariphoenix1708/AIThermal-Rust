//! Memory Management — ZRAM compaction and OOM score adjustment for gaming.
//!
//! During gaming, we want to:
//! 1. Compact ZRAM to free memory for game allocations.
//! 2. Lower OOM score of game process to prevent OOM kills.
//! 3. Optionally raise OOM score of background apps.

use std::fs;
use std::path::Path;

const ZRAM_COMPACT: &str = "/sys/block/zram0/compact";

pub struct MemoryManager {
    available: bool,
    game_pid: Option<u32>,
    saved_oom_score_adj: Option<i32>,
}

impl MemoryManager {
    pub fn new() -> Self {
        let available = Path::new(ZRAM_COMPACT).exists();
        Self {
            available,
            game_pid: None,
            saved_oom_score_adj: None,
        }
    }

    /// Activate: compact ZRAM and protect game process from OOM.
    pub fn activate(&mut self, game_pid: u32) {
        if !self.available {
            return;
        }

        self.game_pid = Some(game_pid);

        // Compact ZRAM to free memory.
        if let Err(e) = fs::write(ZRAM_COMPACT, b"1") {
            tracing::warn!(
                target: "game_turbo",
                "Memory: ZRAM compact failed: {}",
                e
            );
        } else {
            tracing::info!(
                target: "game_turbo",
                "Memory: ZRAM compacted for gaming"
            );
        }

        // Lower OOM score of game process (make it less likely to be killed).
        // -1000 = maximum protection, -500 = strong protection.
        let oom_score_path = format!("/proc/{}/oom_score_adj", game_pid);
        if Path::new(&oom_score_path).exists() {
            if let Ok(current) = fs::read_to_string(&oom_score_path) {
                self.saved_oom_score_adj = current.trim().parse().ok();
            }
            if fs::write(&oom_score_path, b"-500").is_ok() {
                tracing::info!(
                    target: "game_turbo",
                    "Memory: game PID {} oom_score_adj set to -500",
                    game_pid
                );
            }
        }

        // Optionally: increase OOM score of background cgroup apps.
        // This is handled by the background lockdown cgroup uclamp.max.
    }

    /// Per-tick: no-op for now.
    pub fn tick(&mut self) {}

    /// Deactivate: restore OOM score.
    pub fn deactivate(&mut self) {
        if let Some(pid) = self.game_pid {
            let oom_score_path = format!("/proc/{}/oom_score_adj", pid);
            if Path::new(&oom_score_path).exists() {
                if let Some(saved) = self.saved_oom_score_adj.take() {
                    let _ = fs::write(&oom_score_path, saved.to_string().as_bytes());
                    tracing::info!(
                        target: "game_turbo",
                        "Memory: game PID {} oom_score_adj restored to {}",
                        pid, saved
                    );
                } else {
                    // Default restore to 0.
                    let _ = fs::write(&oom_score_path, b"0");
                    tracing::info!(
                        target: "game_turbo",
                        "Memory: game PID {} oom_score_adj restored to 0",
                        pid
                    );
                }
            }
        }
        self.game_pid = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_manager_new() {
        let mgr = MemoryManager::new();
        let _ = mgr.available;
    }
}