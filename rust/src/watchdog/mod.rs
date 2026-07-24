use anyhow::Result;
use std::time::Instant;
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogVerdict {
    Healthy,
    DegradedRestoreRecommended,
    StalledRecoverNow,
}

pub struct Watchdog {
    pub heartbeat_failures: u32,
    pub last_heartbeat: Option<Instant>,
    pub poll_interval: u64,
    pub stall_threshold: u32,
    last_legacy_write_failures: u64,
    last_healthy_at: Option<Instant>,
    pub tick_count: u64,
}

impl Watchdog {
    pub fn new(poll_interval: u64) -> Self {
        Self::with_threshold(poll_interval, 5)
    }

    pub fn with_threshold(poll_interval: u64, stall_threshold: u32) -> Self {
        Self {
            heartbeat_failures: 0,
            last_heartbeat: None,
            poll_interval,
            stall_threshold,
            last_legacy_write_failures: 0,
            last_healthy_at: None,
            tick_count: 0,
        }
    }

    pub fn mark_healthy(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);
        self.last_healthy_at = Some(Instant::now());
    }

    pub fn check(&mut self, is_running_properly: bool) -> Result<WatchdogVerdict> {
        let current_time = Instant::now();
        let mut stalled = !is_running_properly;

        #[allow(clippy::collapsible_if)]
        if let Some(last) = self.last_heartbeat {
            let limit = (self.poll_interval * 5).max(15);
            if current_time.duration_since(last).as_secs() > limit {
                stalled = true;
            }
        }

        if is_running_properly {
            self.last_heartbeat = Some(current_time);
            // last_healthy_at is intentionally NOT set here; mark_healthy()
            // must be called explicitly at the end of a successful loop tick.
            stalled = false;
        }

        if stalled {
            self.heartbeat_failures = self.heartbeat_failures.saturating_add(1);
        } else {
            self.heartbeat_failures = 0;
        }

        // Elevate on sysfs write flood: if legacy write failures jumped by
        // more than N since last healthy tick, most writes are being rejected
        // by the kernel or SELinux and we should back off to safe defaults.
        let current_failures = crate::tuning::backend::TuningBackend::legacy_write_failure_count();
        let jumped_by = current_failures.saturating_sub(self.last_legacy_write_failures);
        let jumped = jumped_by > 20;
        if is_running_properly {
            self.last_legacy_write_failures = current_failures;
        }

        let verdict = if self.heartbeat_failures == 0 && !jumped {
            WatchdogVerdict::Healthy
        } else if self.heartbeat_failures > self.stall_threshold {
            warn!(
                "Watchdog stall (failures={}) — recommending full recovery",
                self.heartbeat_failures
            );
            WatchdogVerdict::StalledRecoverNow
        } else {
            if jumped {
                warn!(
                    "Watchdog: sysfs write failures jumped by {} — degraded restore",
                    jumped_by
                );
            }
            WatchdogVerdict::DegradedRestoreRecommended
        };

        let stall_secs = self.last_healthy_at.map(|t| current_time.duration_since(t).as_secs()).unwrap_or(0);
        tracing::trace!("Watchdog: tick_count={}, last_healthy_at={:?}, stall_secs={}",
            self.tick_count, self.last_healthy_at, stall_secs);

        Ok(verdict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watchdog_triggers() {
        let mut wd = Watchdog::new(2);
        for _ in 0..6 {
            let _ = wd.check(false).unwrap();
        }
        assert!(wd.heartbeat_failures >= 6);
        let v = wd.check(true).unwrap();
        assert_eq!(v, WatchdogVerdict::Healthy);
        assert_eq!(wd.heartbeat_failures, 0);
    }
}
