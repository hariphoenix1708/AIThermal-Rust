//! Performance Hint integration — use `sched_setattr` with uclamp
//! to signal the scheduler that game-critical threads need high CPU
//! frequency.
//!
//! This is the Rust equivalent of Android's PerformanceHintManager API,
//! but works at the kernel level via `sched_setattr`. The kernel's
//! Energy-Aware Scheduler (EAS/WALT) uses these uclamp values to
//! decide CPU frequency and placement.
//!
//! Key insight: `uclamp_min` forces the scheduler to select a CPU
//! frequency that can deliver at least the requested utilization.
//! For gaming, setting uclamp_min=600 on a big core ensures the
//! scheduler keeps frequencies high without the HAL roundtrip.
//!
//! All values are saved for restoration on game exit.

use std::collections::HashMap;
use std::fs;

/// sched_attr flags for uclamp
const SCHED_FLAG_UTIL_CLAMP_MIN: u64 = 1 << 10;
const SCHED_FLAG_UTIL_CLAMP_MAX: u64 = 1 << 11;

/// Uclamp range: 0-1024 (kernel default).
/// 1024 = 100% of max capacity.
const UCLAMP_MIN_GAME_RENDER: u32 = 896;  // 87.5% — render threads need max
const UCLAMP_MIN_SYSTEM_CRITICAL: u32 = 640; // 62.5% — SF/composer
const UCLAMP_MAX_UNLIMITED: u32 = 1024;

/// Thread names that are critical for game rendering.
const GAME_CRITICAL_THREADS: &[&str] = &[
    "RenderThread",
    "GPU completion",
    "hwuiTask0",
    "hwuiTask1",
];

/// System threads critical for display pipeline.
const DISPLAY_CRITICAL_THREADS: &[&str] = &[
    "surfaceflinger",
    "HwBinder:",
    "composer-service",
];

#[repr(C)]
struct SchedAttr {
    size: u32,
    sched_policy: u32,
    sched_flags: u64,
    sched_nice: i32,
    sched_priority: u32,
    sched_runtime: u64,
    sched_deadline: u64,
    sched_period: u64,
    sched_util_min: u32,
    sched_util_max: u32,
}

pub struct PerfHintState {
    /// tid -> original uclamp_min
    saved_uclamp_min: HashMap<u32, u32>,
    /// tid -> original uclamp_max
    saved_uclamp_max: HashMap<u32, u32>,
    /// TIDs we boosted this session.
    boosted_tids: Vec<u32>,
    /// Whether sched_setattr with uclamp is supported by this kernel.
    uclamp_supported: Option<bool>,
}

impl PerfHintState {
    pub fn new() -> Self {
        Self {
            saved_uclamp_min: HashMap::new(),
            saved_uclamp_max: HashMap::new(),
            boosted_tids: Vec::new(),
            uclamp_supported: None,
        }
    }

    /// Activate: find game-critical threads and set uclamp_min.
    pub fn activate(&mut self, game_pid: u32) {
        if self.uclamp_supported == Some(false) {
            return;
        }

        // First, test if sched_setattr with uclamp is supported.
        if self.uclamp_supported.is_none() {
            // Test with our own PID (safe no-op if it fails).
            let test_attr = SchedAttr {
                size: std::mem::size_of::<SchedAttr>() as u32,
                sched_policy: 0,
                sched_flags: SCHED_FLAG_UTIL_CLAMP_MIN,
                sched_nice: 0,
                sched_priority: 0,
                sched_runtime: 0,
                sched_deadline: 0,
                sched_period: 0,
                sched_util_min: 512,
                sched_util_max: UCLAMP_MAX_UNLIMITED,
            };
            let ret = unsafe {
                libc::syscall(libc::SYS_sched_setattr, 0i32, &test_attr as *const SchedAttr, 0u32)
            };
            self.uclamp_supported = Some(ret == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM));
            if !self.uclamp_supported.unwrap_or(false) {
                tracing::debug!(
                    target: "game_turbo",
                    "PerfHint: sched_setattr uclamp not supported by kernel, falling back to nice/RT"
                );
                return;
            }
        }

        // Scan game process threads.
        let task_dir = format!("/proc/{}/task", game_pid);
        if let Ok(entries) = fs::read_dir(&task_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Ok(tid) = name.parse::<u32>() {
                    // Read thread name from /proc/<pid>/task/<tid>/comm.
                    let comm_path = entry.path().join("comm");
                    if let Ok(comm) = fs::read_to_string(&comm_path) {
                        let comm = comm.trim().to_string();
                        if GAME_CRITICAL_THREADS.contains(&comm.as_str()) {
                            self.boost_thread(tid, UCLAMP_MIN_GAME_RENDER, "game-critical");
                        } else if comm.starts_with("UnityMain")
                            || comm.starts_with("UnityGfx")
                            || comm.starts_with("UnityMultiR")
                        {
                            // Unity engine threads.
                            self.boost_thread(tid, UCLAMP_MIN_GAME_RENDER, "unity-critical");
                        }
                    }
                }
            }
        }

        // Also boost display pipeline threads (surfaceflinger, composer).
        for name in DISPLAY_CRITICAL_THREADS {
            if let Some(pid) = find_pid_by_prefix(name) {
                self.boost_thread(pid, UCLAMP_MIN_SYSTEM_CRITICAL, "display-critical");
            }
        }

        if !self.boosted_tids.is_empty() {
            tracing::info!(
                target: "game_turbo",
                "PerfHint: boosted {} threads via sched_setattr uclamp",
                self.boosted_tids.len()
            );
        }
    }

    /// Per-tick: re-boost threads that may have spawned after activate.
    pub fn tick(&mut self, game_pid: u32) {
        if self.uclamp_supported == Some(false) {
            return;
        }

        let task_dir = format!("/proc/{}/task", game_pid);
        if let Ok(entries) = fs::read_dir(&task_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Ok(tid) = name.parse::<u32>()
                    && !self.boosted_tids.contains(&tid)
                {
                    let comm_path = entry.path().join("comm");
                    if let Ok(comm) = fs::read_to_string(&comm_path) {
                        let comm = comm.trim().to_string();
                        if GAME_CRITICAL_THREADS.contains(&comm.as_str()) {
                            self.boost_thread(tid, UCLAMP_MIN_GAME_RENDER, "game-critical");
                        }
                    }
                }
            }
        }
    }

    /// Restore all saved uclamp values.
    pub fn deactivate(&mut self) {
        for tid in &self.boosted_tids {
            let min = self.saved_uclamp_min.get(tid).copied().unwrap_or(0);
            let max = self.saved_uclamp_max.get(tid).copied().unwrap_or(UCLAMP_MAX_UNLIMITED);
            set_uclamp(*tid, min, max);
        }

        tracing::info!(
            target: "game_turbo",
            "PerfHint: restored {} threads",
            self.boosted_tids.len()
        );

        self.saved_uclamp_min.clear();
        self.saved_uclamp_max.clear();
        self.boosted_tids.clear();
    }

    fn boost_thread(&mut self, tid: u32, uclamp_min: u32, tag: &str) {
        if self.boosted_tids.contains(&tid) {
            return;
        }

        // Read current uclamp values for restoration.
        let (cur_min, cur_max) = get_uclamp(tid);
        self.saved_uclamp_min.insert(tid, cur_min);
        self.saved_uclamp_max.insert(tid, cur_max);

        if set_uclamp(tid, uclamp_min, UCLAMP_MAX_UNLIMITED) {
            self.boosted_tids.push(tid);
            tracing::debug!(
                target: "game_turbo",
                "PerfHint [{}]: tid {} uclamp_min {} -> {}",
                tag, tid, cur_min, uclamp_min
            );
        }
    }
}

/// Set uclamp_min and uclamp_max for a thread via `sched_setattr`.
fn set_uclamp(tid: u32, uclamp_min: u32, uclamp_max: u32) -> bool {
    let attr = SchedAttr {
        size: std::mem::size_of::<SchedAttr>() as u32,
        sched_policy: 0,
        sched_flags: SCHED_FLAG_UTIL_CLAMP_MIN | SCHED_FLAG_UTIL_CLAMP_MAX,
        sched_nice: 0,
        sched_priority: 0,
        sched_runtime: 0,
        sched_deadline: 0,
        sched_period: 0,
        sched_util_min: uclamp_min,
        sched_util_max: uclamp_max,
    };

    // libc doesn't expose sched_setattr — use the raw syscall directly.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_sched_setattr,
            tid as libc::pid_t,
            &attr as *const SchedAttr,
            0u32,
        )
    };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        tracing::debug!(
            target: "game_turbo",
            "PerfHint: sched_setattr failed for tid {}: {}",
            tid, err
        );
        return false;
    }
    true
}

/// Read current uclamp values for a thread.
fn get_uclamp(tid: u32) -> (u32, u32) {
    // Read from /proc/<tid>/sched is the most reliable way on Linux.
    let sched_path = format!("/proc/{}/sched", tid);
    if let Ok(content) = fs::read_to_string(&sched_path) {
        let mut min = 0u32;
        let mut max = 1024u32;
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("uclamp.min")
                && let Ok(v) = val.trim().parse::<u32>()
            {
                min = v;
            } else if let Some(val) = line.strip_prefix("uclamp.max")
                && let Ok(v) = val.trim().parse::<u32>()
            {
                max = v;
            }
        }
        return (min, max);
    }
    (0, 1024)
}

fn find_pid_by_prefix(prefix: &str) -> Option<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return None;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
            continue;
        };
        let comm_path = entry.path().join("comm");
        if let Ok(comm) = fs::read_to_string(&comm_path)
            && comm.trim().starts_with(prefix)
        {
            return Some(pid);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perf_hint_state_new() {
        let state = PerfHintState::new();
        assert!(state.boosted_tids.is_empty());
        assert!(state.uclamp_supported.is_none());
    }

    #[test]
    fn test_sched_attr_size() {
        // sched_attr must be at least 56 bytes (without uclamp) on all platforms.
        // With uclamp flags, the kernel reads 64 bytes but the struct itself
        // may be smaller due to alignment differences between x86_64 and aarch64.
        assert!(std::mem::size_of::<SchedAttr>() >= 56);
    }
}
