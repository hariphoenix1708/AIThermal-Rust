use crate::policy::PolicyState;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum RecoveryPhase {
    None,
    Thermal,
    Charging,
    GameExit,
}

pub struct RecoveryManager {
    pub in_recovery: bool,
    pub phase: RecoveryPhase,
    pub recovery_started_at: Option<std::time::Instant>,
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryManager {
    pub fn new() -> Self {
        Self {
            in_recovery: false,
            phase: RecoveryPhase::None,
            recovery_started_at: None,
        }
    }

    pub fn check_recovery(
        &mut self,
        current_policy: &PolicyState,
        was_gaming: bool,
        is_gaming: bool,
    ) -> bool {
        // Did we just exit a game?
        if was_gaming && !is_gaming {
            self.in_recovery = true;
            self.phase = RecoveryPhase::GameExit;
            tracing::info!(target: "thermal", "Recovery -> {:?}", self.phase);
            tracing::info!("Recovery -> {:?}", self.phase);
            self.recovery_started_at = Some(std::time::Instant::now());
            return true;
        }

        // If we hit emergency cool, we enter thermal recovery
        if *current_policy == PolicyState::EmergencyCool {
            self.in_recovery = true;
            self.phase = RecoveryPhase::Thermal;
            tracing::info!(target: "thermal", "Recovery -> {:?}", self.phase);
            tracing::info!("Recovery -> {:?}", self.phase);
            self.recovery_started_at = Some(std::time::Instant::now());
            return true;
        }

        // Keep recovery active until we've been out of emergency for a while (gradual)
        if self.in_recovery {
            let elapsed_secs = self.recovery_started_at
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(u64::MAX);

            let threshold_secs = match self.phase {
                RecoveryPhase::GameExit => 20,
                RecoveryPhase::Thermal => 45,
                _ => 25,
            };

            if elapsed_secs > threshold_secs && *current_policy != PolicyState::EmergencyCool {
                self.in_recovery = false;
                self.phase = RecoveryPhase::None;
                self.recovery_started_at = None;
                tracing::info!(target: "thermal", "Recovery cleared after {}s", elapsed_secs);
                tracing::info!("Recovery cleared after {}s", elapsed_secs);
                return false;
            }
            return true; // Still recovering
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recovery_transitions() {
        let mut rm = RecoveryManager::new();

        // Enter thermal recovery
        assert!(rm.check_recovery(&PolicyState::EmergencyCool, false, false));
        assert_eq!(rm.phase, RecoveryPhase::Thermal);

        // Stay in recovery (elapsed time < threshold)
        assert!(rm.check_recovery(&PolicyState::Performance, false, false));

        // Fast forward time to exit recovery
        rm.recovery_started_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(46));
        assert!(!rm.check_recovery(&PolicyState::Performance, false, false));
        assert_eq!(rm.phase, RecoveryPhase::None);

        // Enter game exit recovery
        assert!(rm.check_recovery(&PolicyState::Performance, true, false));
        assert_eq!(rm.phase, RecoveryPhase::GameExit);
    }
}
