//! SCHED_DEADLINE Support — give game render threads guaranteed CPU time.
//!
//! SCHED_DEADLINE (policy 6) provides hard real-time guarantees:
//! - runtime: guaranteed CPU time per period
//! - deadline: must complete within this time
//! - period: replenishment period
//!
//! For gaming, we can give the render thread a guaranteed slice
//! (e.g., 2ms runtime / 4ms period = 50% CPU guaranteed).

use std::fs;

const SCHED_DEADLINE: u32 = 6;

pub struct SchedDeadlineManager {
    available: bool,
    applied_tids: Vec<u32>,
}

impl SchedDeadlineManager {
    pub fn new() -> Self {
        // Test if SCHED_DEADLINE is supported by trying to set it on self.
        let available = Self::test_support();
        Self {
            available,
            applied_tids: Vec::new(),
        }
    }

    fn test_support() -> bool {
        // Try to set SCHED_DEADLINE on current thread.
        // If it fails with EPERM, the kernel supports it but we lack permission.
        // If it fails with EINVAL, the kernel doesn't support it.
        #[cfg(target_os = "linux")]
        {
            
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

            let attr = SchedAttr {
                size: std::mem::size_of::<SchedAttr>() as u32,
                sched_policy: 6, // SCHED_DEADLINE
                sched_flags: 0,
                sched_nice: 0,
                sched_priority: 0,
                sched_runtime: 1_000_000,  // 1ms runtime
                sched_deadline: 2_000_000, // 2ms deadline
                sched_period: 4_000_000,   // 4ms period
                sched_util_min: 0,
                sched_util_max: 1024,
            };

            let ret = unsafe {
                libc::syscall(
                    libc::SYS_sched_setattr,
                    0i32, // current thread
                    &attr as *const SchedAttr,
                    0u32,
                )
            };

            // Success (0) or EPERM (1) means supported.
            // EINVAL (22) means not supported.
            ret == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Activate: apply SCHED_DEADLINE to game render threads.
    pub fn activate(&mut self, game_pid: u32) {
        if !self.available {
            return;
        }

        let task_dir = format!("/proc/{}/task", game_pid);
        if let Ok(entries) = fs::read_dir(&task_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Ok(tid) = name.parse::<u32>() {
                    let comm_path = entry.path().join("comm");
                    if let Ok(comm) = fs::read_to_string(&comm_path) {
                        let comm = comm.trim().to_string();
                        // Target render threads.
                        if comm == "RenderThread"
                            || comm == "GPU completion"
                            || comm.starts_with("UnityMain")
                            || comm.starts_with("UnityGfx")
                        {
                            self.apply_deadline(tid);
                        }
                    }
                }
            }
        }
    }

    fn apply_deadline(&mut self, tid: u32) {
        if self.applied_tids.contains(&tid) {
            return;
        }

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

        // 2ms runtime / 4ms period = 50% CPU guaranteed.
        // This ensures the render thread gets consistent CPU time.
        let attr = SchedAttr {
            size: std::mem::size_of::<SchedAttr>() as u32,
            sched_policy: SCHED_DEADLINE,
            sched_flags: 0,
            sched_nice: 0,
            sched_priority: 0,
            sched_runtime: 2_000_000, // 2ms
            sched_deadline: 4_000_000, // 4ms
            sched_period: 4_000_000,  // 4ms
            sched_util_min: 0,
            sched_util_max: 1024,
        };

        let ret = unsafe {
            libc::syscall(
                libc::SYS_sched_setattr,
                tid as libc::pid_t,
                &attr as *const SchedAttr,
                0u32,
            )
        };

        if ret == 0 {
            self.applied_tids.push(tid);
            tracing::info!(
                target: "game_turbo",
                "SCHED_DEADLINE: applied to tid {} (2ms/4ms)",
                tid
            );
        } else {
            let err = std::io::Error::last_os_error();
            tracing::debug!(
                target: "game_turbo",
                "SCHED_DEADLINE: failed for tid {}: {}",
                tid, err
            );
        }
    }

    /// Per-tick: re-scan for new threads.
    pub fn tick(&mut self, game_pid: u32) {
        if !self.available {
            return;
        }
        self.activate(game_pid);
    }

    /// Deactivate: restore threads to SCHED_NORMAL.
    pub fn deactivate(&mut self) {
        if !self.available {
            return;
        }

        let restored_count = self.applied_tids.len();
        for tid in self.applied_tids.drain(..) {
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

            let attr = SchedAttr {
                size: std::mem::size_of::<SchedAttr>() as u32,
                sched_policy: 0, // SCHED_NORMAL
                sched_flags: 0,
                sched_nice: 0,
                sched_priority: 0,
                sched_runtime: 0,
                sched_deadline: 0,
                sched_period: 0,
                sched_util_min: 0,
                sched_util_max: 1024,
            };

            let _ = unsafe {
                libc::syscall(
                    libc::SYS_sched_setattr,
                    tid as libc::pid_t,
                    &attr as *const SchedAttr,
                    0u32,
                )
            };
        }

        tracing::info!(
            target: "game_turbo",
            "SCHED_DEADLINE: restored {} threads to SCHED_NORMAL",
            restored_count
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sched_deadline_new() {
        let mgr = SchedDeadlineManager::new();
        let _ = mgr.available;
    }
}