//! Touch latency reducer — boost touch controller IRQ threads to
//! SCHED_FIFO during gaming to minimize input-to-photon latency.
//!
//! Scans `/proc/interrupts` for known touch controller IRQ names,
//! resolves their IRQ threads, and elevates to RT scheduling.
//! All values saved for restoration.

use std::collections::HashMap;
use std::fs;

/// Touch controller IRQ names commonly found on Qualcomm/MTK platforms.
const TOUCH_IRQ_NAMES: &[&str] = &[
    "fts_ts",
    "nvt_ts",
    "goodix_ts",
    "atmel_mxt_ts",
    "synaptics_ts",
    "sec_ts",
    "himax",
    "ilitek",
    "focaltech",
    "novatek",
    "xiaomi_ts",
    "goodix",
    "himax_touch",
];

/// IRQ thread names to search in /proc (pattern match on comm).
const TOUCH_THREAD_PREFIXES: &[&str] = &[
    "irq/",
    "fts_wq",
    "nvt_ts_work",
    "goodix_ts_work",
];

const RT_PRIORITY: i32 = 1;

pub struct TouchState {
    /// tid -> original scheduling policy.
    saved_policy: HashMap<u32, i32>,
    /// tid -> original scheduling priority.
    saved_priority: HashMap<u32, i32>,
    boosted_tids: Vec<u32>,
}

impl TouchState {
    pub fn new() -> Self {
        Self {
            saved_policy: HashMap::new(),
            saved_priority: HashMap::new(),
            boosted_tids: Vec::new(),
        }
    }

    /// Initial activation: scan for touch IRQ threads and boost them.
    pub fn activate(&mut self) {
        self.scan_and_boost();

        if !self.boosted_tids.is_empty() {
            tracing::info!(
                target: "game_turbo",
                "Touch boost: elevated {} IRQ threads to SCHED_FIFO",
                self.boosted_tids.len()
            );
        }
    }

    /// Per-tick: re-scan for threads that may have been recreated.
    pub fn tick(&mut self) {
        self.scan_and_boost();
    }

    /// Restore all saved scheduling parameters.
    pub fn deactivate(&mut self) {
        for tid in &self.boosted_tids {
            let policy = self.saved_policy.get(tid).copied().unwrap_or(libc::SCHED_OTHER);
            let prio = self.saved_priority.get(tid).copied().unwrap_or(0);
            restore_scheduler(*tid, policy, prio);
        }

        tracing::info!(
            target: "game_turbo",
            "Touch boost: restored {} IRQ threads",
            self.boosted_tids.len()
        );

        self.saved_policy.clear();
        self.saved_priority.clear();
        self.boosted_tids.clear();
    }

    fn scan_and_boost(&mut self) {
        // Strategy 1: scan /proc/interrupts for touch-related IRQ lines
        // and find their kernel threads.
        if let Ok(content) = fs::read_to_string("/proc/interrupts") {
            for line in content.lines() {
                for name in TOUCH_IRQ_NAMES {
                    if line.contains(name) {
                        // Extract IRQ number from the line.
                        if let Some(irq_num) = extract_irq_number(line) {
                            let thread_name = format!("irq/{}", irq_num);
                            self.boost_thread_by_name(&thread_name);
                        }
                    }
                }
            }
        }

        // Strategy 2: scan /proc for known touch worker thread names.
        for prefix in TOUCH_THREAD_PREFIXES {
            self.boost_threads_by_prefix(prefix);
        }

        // Strategy 3: scan all processes for touch-related comm names.
        for name in TOUCH_IRQ_NAMES {
            self.boost_thread_by_name(name);
        }
    }

    fn boost_thread_by_name(&mut self, name: &str) {
        let Ok(entries) = fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let pid_str = entry.file_name();
            let Ok(tid) = pid_str.to_string_lossy().parse::<u32>() else {
                continue;
            };
            if self.saved_policy.contains_key(&tid) {
                continue;
            }
            let comm_path = entry.path().join("comm");
            if let Ok(comm) = fs::read_to_string(&comm_path)
                && (comm.trim() == name || comm.trim().contains(name))
            {
                self.boost_tid(tid);
            }
        }
    }

    fn boost_threads_by_prefix(&mut self, prefix: &str) {
        let Ok(entries) = fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let pid_str = entry.file_name();
            let Ok(tid) = pid_str.to_string_lossy().parse::<u32>() else {
                continue;
            };
            if self.saved_policy.contains_key(&tid) {
                continue;
            }
            let comm_path = entry.path().join("comm");
            if let Ok(comm) = fs::read_to_string(&comm_path) {
                let comm = comm.trim();
                if comm.starts_with(prefix) {
                    // Check if this thread is touch-related by cross-referencing
                    // with known IRQ names.
                    let is_touch = comm.contains("touch")
                        || TOUCH_IRQ_NAMES.iter().any(|&name| comm.contains(name));
                    if is_touch {
                        self.boost_tid(tid);
                    }
                }
            }
        }
    }

    fn boost_tid(&mut self, tid: u32) {
        if self.saved_policy.contains_key(&tid) {
            return;
        }

        let (policy, prio) = get_scheduler(tid);
        self.saved_policy.insert(tid, policy);
        self.saved_priority.insert(tid, prio);

        if set_scheduler(tid, libc::SCHED_FIFO, RT_PRIORITY) {
            self.boosted_tids.push(tid);
            tracing::debug!(
                target: "game_turbo",
                "Touch boost: tid {} -> SCHED_FIFO prio {}",
                tid, RT_PRIORITY
            );
        }
    }
}

fn extract_irq_number(line: &str) -> Option<u32> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if let Some(first) = parts.first() {
        // IRQ lines in /proc/interrupts look like:
        //  123:  12345  GICv3  xxx  xxx  fts_ts
        if let Ok(num) = first.trim_end_matches(':').parse::<u32>() {
            return Some(num);
        }
    }
    None
}

fn get_scheduler(tid: u32) -> (i32, i32) {
    unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        let policy = libc::sched_getscheduler(tid as libc::pid_t);
        libc::sched_getparam(tid as libc::pid_t, &mut param);
        (policy, param.sched_priority)
    }
}

fn set_scheduler(tid: u32, policy: i32, priority: i32) -> bool {
    unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = priority;
        let ret = libc::sched_setscheduler(tid as libc::pid_t, policy, &param);
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            tracing::debug!(
                target: "game_turbo",
                "sched_setscheduler failed for tid {}: {}",
                tid, err
            );
            return false;
        }
        true
    }
}

fn restore_scheduler(tid: u32, policy: i32, priority: i32) {
    set_scheduler(tid, policy, priority);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_touch_state_new() {
        let state = TouchState::new();
        assert!(state.boosted_tids.is_empty());
    }
}
