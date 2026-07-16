use anyhow::Result;
use std::time::Instant;
use tracing::warn;

pub struct Watchdog {
    pub heartbeat_failures: u32,
    pub last_heartbeat: Option<Instant>,
    pub poll_interval: u64,
}

impl Watchdog {
    pub fn new(poll_interval: u64) -> Self {
        Self {
            heartbeat_failures: 0,
            last_heartbeat: None,
            poll_interval,
        }
    }

    pub fn check(&mut self, is_running_properly: bool) -> Result<bool> {
        let current_time = Instant::now();
        let mut stalled = !is_running_properly;

        #[allow(clippy::collapsible_if)]
        if let Some(last) = self.last_heartbeat {
            let limit = (self.poll_interval * 5).max(15); // Stalled if missed 5 intervals or 15s absolute min
            if current_time.duration_since(last).as_secs() > limit {
                stalled = true; // Stalled for over 60 seconds
            }
        }

        if is_running_properly {
            self.last_heartbeat = Some(current_time);
            stalled = false;
        }

        if stalled {
            self.heartbeat_failures += 1;
            if self.heartbeat_failures > 5 {
                warn!("Watchdog triggered. System state unstable. Requesting emergency restore.");
                return Ok(true);
            }
        } else {
            self.heartbeat_failures = 0;
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watchdog_triggers() {
        let mut wd = Watchdog::new(2);
        for _ in 0..6 {
            wd.check(false).unwrap();
        }
        assert_eq!(wd.heartbeat_failures, 6);
        wd.check(true).unwrap();
        assert_eq!(wd.heartbeat_failures, 0);
    }

    #[test]
    fn test_watchdog_recovers_from_stale_timer() {
        let mut wd = Watchdog::new(2);

        // Force a stall
        for _ in 0..6 {
            wd.check(false).unwrap();
        }
        assert_eq!(wd.heartbeat_failures, 6);

        // Manually backdate the last_heartbeat to simulate a stale timer
        wd.last_heartbeat = Some(Instant::now() - std::time::Duration::from_secs(60));

        // Subsequent check with true health should not trigger the time limit and should fully recover
        for _ in 0..3 {
            assert!(!wd.check(true).unwrap());
            assert_eq!(wd.heartbeat_failures, 0);
        }
    }
}
