//! Thread affinity binder — pin game UI/render threads to big cores.
//!
//! Scans `/proc/<pid>/task` for known critical thread names and binds
//! them to the big-core mask via `sched_setaffinity`. Original masks
//! are saved for full restoration on game exit.

use std::collections::HashMap;
use std::fs;

/// Thread names that are critical for game frame pacing.
const CRITICAL_THREAD_NAMES: &[&str] = &[
    "RenderThread",
    "GLThread",
    "hwuiTask0",
    "hwuiTask1",
    "UnityMain",
    "UnityGfxDeviceW",
    "UnityMultiRend",
    "mali-event-hnd",
    "GPU completion",
    "sg_main_thread",
    "Thread-1",        // CODM render thread
    "Thread-2",        // CODM worker
];

pub struct AffinityState {
    /// tid -> saved original CPU mask (as a bitmask).
    saved_affinities: HashMap<u32, u64>,
    /// Tids we already pinned this session (to avoid redundant saves).
    pinned_tids: HashMap<u32, ()>,
}

impl AffinityState {
    pub fn new() -> Self {
        Self {
            saved_affinities: HashMap::new(),
            pinned_tids: HashMap::new(),
        }
    }

    /// Initial activation: scan all threads and pin critical ones.
    pub fn activate(&mut self, game_pid: u32, big_mask: u64) {
        let task_dir = format!("/proc/{}/task", game_pid);
        let Ok(entries) = fs::read_dir(&task_dir) else {
            tracing::debug!(target: "game_turbo", "Cannot read task dir: {}", task_dir);
            return;
        };

        for entry in entries.flatten() {
            let tid_str = entry.file_name();
            let Ok(tid) = tid_str.to_string_lossy().parse::<u32>() else {
                continue;
            };

            let comm_path = format!("{}/{}/comm", task_dir, tid_str.to_string_lossy());
            let Ok(comm) = fs::read_to_string(&comm_path) else {
                continue;
            };
            let comm = comm.trim().to_string();

            if !is_critical_thread(&comm) {
                continue;
            }

            self.pin_thread(tid, big_mask);
        }

        tracing::info!(
            target: "game_turbo",
            "Thread affinity: pinned {} critical threads to big cores",
            self.pinned_tids.len()
        );
    }

    /// Per-tick: re-scan for newly spawned threads (game engines delay
    /// thread creation by seconds after process start).
    pub fn tick(&mut self, game_pid: u32, big_mask: u64) {
        let task_dir = format!("/proc/{}/task", game_pid);
        let Ok(entries) = fs::read_dir(&task_dir) else {
            return;
        };

        for entry in entries.flatten() {
            let tid_str = entry.file_name();
            let Ok(tid) = tid_str.to_string_lossy().parse::<u32>() else {
                continue;
            };

            if self.pinned_tids.contains_key(&tid) {
                continue;
            }

            let comm_path = format!("{}/{}/comm", task_dir, tid_str.to_string_lossy());
            let Ok(comm) = fs::read_to_string(&comm_path) else {
                continue;
            };
            let comm = comm.trim().to_string();

            if is_critical_thread(&comm) {
                self.pin_thread(tid, big_mask);
            }
        }
    }

    /// Restore all saved affinities.
    pub fn deactivate(&mut self) {
        for (tid, orig_mask) in &self.saved_affinities {
            restore_affinity(*tid, *orig_mask);
        }

        tracing::info!(
            target: "game_turbo",
            "Thread affinity: restored {} threads",
            self.saved_affinities.len()
        );
        self.saved_affinities.clear();
        self.pinned_tids.clear();
    }

    /// Re-pin all currently pinned threads to a new affinity mask.
    /// Used by thermal-aware mode to expand (big→all) or contract
    /// (all→big) the allowed core set without losing track of originals.
    pub fn update_mask(&self, _game_pid: u32, new_mask: u64) {
        for tid in self.pinned_tids.keys() {
            if write_affinity(*tid, new_mask) {
                tracing::debug!(
                    target: "game_turbo",
                    "Re-pinned tid {} to mask {:#x}",
                    tid, new_mask
                );
            }
        }
    }

    fn pin_thread(&mut self, tid: u32, big_mask: u64) {
        if self.pinned_tids.contains_key(&tid) {
            return;
        }

        // Save original affinity before overwriting.
        if let Some(orig) = read_affinity(tid) {
            self.saved_affinities.insert(tid, orig);
        }

        if write_affinity(tid, big_mask) {
            self.pinned_tids.insert(tid, ());
            tracing::debug!(
                target: "game_turbo",
                "Pinned tid {} to big cores {:#x}",
                tid, big_mask
            );
        }
    }
}

fn is_critical_thread(comm: &str) -> bool {
    CRITICAL_THREAD_NAMES.contains(&comm)
}

/// Read the current CPU affinity mask for a thread.
fn read_affinity(tid: u32) -> Option<u64> {
    let status_path = format!("/proc/{}/status", tid);
    let content = fs::read_to_string(&status_path).ok()?;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Cpus_allowed:") {
            let hex = rest.trim().trim_start_matches("0x").trim();
            return u64::from_str_radix(hex, 16).ok();
        }
    }
    None
}

/// Write a CPU affinity bitmask to a thread using sched_setaffinity.
fn write_affinity(tid: u32, mask: u64) -> bool {
    unsafe {
        let mut cpu_set: libc::cpu_set_t = std::mem::zeroed();
        for bit in 0..64u32 {
            if (mask >> bit) & 1 == 1 {
                libc::CPU_SET(bit as usize, &mut cpu_set);
            }
        }
        let ret = libc::sched_setaffinity(
            tid as libc::pid_t,
            std::mem::size_of::<libc::cpu_set_t>(),
            &cpu_set,
        );
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            tracing::debug!(
                target: "game_turbo",
                "sched_setaffinity failed for tid {}: {}",
                tid, err
            );
            return false;
        }
        true
    }
}

/// Restore a thread's CPU affinity from a saved bitmask.
fn restore_affinity(tid: u32, mask: u64) {
    if write_affinity(tid, mask) {
        tracing::debug!(
            target: "game_turbo",
            "Restored affinity for tid {} to {:#x}",
            tid, mask
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_critical_thread() {
        assert!(is_critical_thread("RenderThread"));
        assert!(is_critical_thread("GLThread"));
        assert!(is_critical_thread("UnityMain"));
        assert!(is_critical_thread("hwuiTask0"));
        assert!(!is_critical_thread("binder"));
        assert!(!is_critical_thread("HeapTaskDaemon"));
    }

    #[test]
    fn test_affinity_state_new() {
        let state = AffinityState::new();
        assert!(state.saved_affinities.is_empty());
        assert!(state.pinned_tids.is_empty());
    }
}
