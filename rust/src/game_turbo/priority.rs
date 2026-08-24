//! Priority elevator — boost scheduling priority for game-critical
//! system threads (surfaceflinger, composer, kgsl, crtc).
//!
//! Uses `nice` for CFS threads and `SCHED_FIFO` for kernel RT workers.
//! All values are saved for restoration on game exit.

use std::collections::HashMap;
use std::fs;

const SF_NICE_BOOST: i32 = -10;
const COMPOSER_NICE_BOOST: i32 = -8;
const RT_PRIORITY: i32 = 2;

pub struct PriorityState {
    /// pid -> original nice value.
    saved_nice: HashMap<u32, i32>,
    /// pid -> original scheduling policy (libc constants).
    saved_policy: HashMap<u32, i32>,
    /// pid -> original scheduling priority.
    saved_sched_priority: HashMap<u32, i32>,
    /// PIDs we boosted this session.
    boosted_pids: Vec<u32>,
}

impl PriorityState {
    pub fn new() -> Self {
        Self {
            saved_nice: HashMap::new(),
            saved_policy: HashMap::new(),
            saved_sched_priority: HashMap::new(),
            boosted_pids: Vec::new(),
        }
    }

    /// Initial activation: find and boost surfaceflinger, composer, kgsl, crtc.
    pub fn activate(&mut self, _game_pid: u32) {
        // SurfaceFlinger — boost with nice.
        if let Some(sf_pid) = find_pid_by_name("surfaceflinger") {
            self.boost_nice(sf_pid, SF_NICE_BOOST);
        }

        // HWComposer — boost with nice.
        if let Some(composer_pid) = find_pid_by_name("composer-service") {
            self.boost_nice(composer_pid, COMPOSER_NICE_BOOST);
        }

        // Also try "android.hardware.graphics.composer" variant.
        if let Some(composer_pid) = find_pid_by_prefix("android.hardware.graphics.composer") {
            self.boost_nice(composer_pid, COMPOSER_NICE_BOOST);
        }

        // KGSL GPU worker threads — boost to SCHED_FIFO for lower GPU scheduling latency.
        self.boost_kgsl_workers();

        // DRM/CRTC commit threads — boost to SCHED_FIFO for display commit latency.
        self.boost_crtc_workers();

        tracing::info!(
            target: "game_turbo",
            "Priority elevator: boosted {} system threads",
            self.boosted_pids.len()
        );
    }

    /// Per-tick: re-boost threads that may have been restarted (rare but
    /// surfaceflinger occasionally respawns on display errors).
    pub fn tick(&mut self, _game_pid: u32) {
        // Re-check surfaceflinger (it's the most likely to restart).
        if let Some(sf_pid) = find_pid_by_name("surfaceflinger")
            && !self.saved_nice.contains_key(&sf_pid)
        {
            self.boost_nice(sf_pid, SF_NICE_BOOST);
        }
    }

    /// Restore all saved scheduling parameters.
    pub fn deactivate(&mut self) {
        // Restore nice values.
        for (pid, orig_nice) in &self.saved_nice {
            set_nice(*pid, *orig_nice);
        }

        // Restore scheduling policies.
        for (pid, orig_policy) in &self.saved_policy {
            let orig_prio = self.saved_sched_priority.get(pid).copied().unwrap_or(0);
            set_scheduler(*pid, *orig_policy, orig_prio);
        }

        tracing::info!(
            target: "game_turbo",
            "Priority elevator: restored {} threads",
            self.boosted_pids.len()
        );

        self.saved_nice.clear();
        self.saved_policy.clear();
        self.saved_sched_priority.clear();
        self.boosted_pids.clear();
    }

    fn boost_nice(&mut self, pid: u32, target_nice: i32) {
        if self.saved_nice.contains_key(&pid) {
            return;
        }

        let orig_nice = get_nice(pid);
        self.saved_nice.insert(pid, orig_nice);
        set_nice(pid, target_nice);
        self.boosted_pids.push(pid);

        tracing::debug!(
            target: "game_turbo",
            "Boosted pid {} nice {} -> {}",
            pid, orig_nice, target_nice
        );
    }

    fn boost_kgsl_workers(&mut self) {
        // kgsl_worker_thread and kgsl-events are kernel RT threads for GPU command submission.
        for name in &["kgsl-worker", "kgsl-events"] {
            for pid in find_pids_by_prefix(name) {
                self.boost_rt(pid);
            }
        }
    }

    fn boost_crtc_workers(&mut self) {
        // DRM CRTC commit threads handle display page-flipping.
        for pid in find_pids_by_prefix("crtc_commit") {
            self.boost_rt(pid);
        }
        for pid in find_pids_by_prefix("crtc.EVENT") {
            self.boost_rt(pid);
        }
    }

    fn boost_rt(&mut self, pid: u32) {
        if self.saved_policy.contains_key(&pid) {
            return;
        }

        let (orig_policy, orig_prio) = get_scheduler(pid);
        self.saved_policy.insert(pid, orig_policy);
        self.saved_sched_priority.insert(pid, orig_prio);

        if set_scheduler(pid, libc::SCHED_FIFO, RT_PRIORITY) {
            self.boosted_pids.push(pid);
            tracing::debug!(
                target: "game_turbo",
                "Boosted pid {} to SCHED_FIFO prio {}",
                pid, RT_PRIORITY
            );
        }
    }
}

/// Find PID by exact process name match (reads /proc/<pid>/comm).
fn find_pid_by_name(name: &str) -> Option<u32> {
    find_pid_by(|comm| comm == name)
}

/// Find PID by prefix match on process name.
fn find_pid_by_prefix(prefix: &str) -> Option<u32> {
    find_pid_by(|comm| comm.starts_with(prefix))
}

fn find_pid_by(pred: impl Fn(&str) -> bool) -> Option<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return None;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
            continue;
        };
        let comm_path = entry.path().join("comm");
        if let Ok(comm) = fs::read_to_string(&comm_path) {
            let comm = comm.trim();
            if pred(comm) {
                return Some(pid);
            }
        }
    }
    None
}

/// Find all PIDs whose name starts with the given prefix.
fn find_pids_by_prefix(prefix: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
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
            pids.push(pid);
        }
    }
    pids
}

fn get_nice(pid: u32) -> i32 {
    unsafe { libc::getpriority(libc::PRIO_PROCESS, pid as libc::id_t) }
}

fn set_nice(pid: u32, nice: i32) {
    unsafe {
        if libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice) != 0 {
            let err = std::io::Error::last_os_error();
            tracing::debug!(
                target: "game_turbo",
                "setpriority failed for pid {}: {}",
                pid, err
            );
        }
    }
}

fn get_scheduler(pid: u32) -> (i32, i32) {
    unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        let policy = libc::sched_getscheduler(pid as libc::pid_t);
        libc::sched_getparam(pid as libc::pid_t, &mut param);
        (policy, param.sched_priority)
    }
}

fn set_scheduler(pid: u32, policy: i32, priority: i32) -> bool {
    unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = priority;
        let ret = libc::sched_setscheduler(pid as libc::pid_t, policy, &param);
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            tracing::debug!(
                target: "game_turbo",
                "sched_setscheduler failed for pid {}: {}",
                pid, err
            );
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_state_new() {
        let state = PriorityState::new();
        assert!(state.saved_nice.is_empty());
        assert!(state.saved_policy.is_empty());
        assert!(state.boosted_pids.is_empty());
    }
}
