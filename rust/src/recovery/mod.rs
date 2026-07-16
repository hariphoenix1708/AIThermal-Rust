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
    recovery_ticks: u64,
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
            recovery_ticks: 0,
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
            self.recovery_ticks = 0;
            return true;
        }

        // If we hit emergency cool, we enter thermal recovery
        if *current_policy == PolicyState::EmergencyCool {
            self.in_recovery = true;
            self.phase = RecoveryPhase::Thermal;
            self.recovery_ticks = 0;
            return true;
        }

        // Keep recovery active until we've been out of emergency for a while (gradual)
        if self.in_recovery {
            self.recovery_ticks += 1;

            let threshold = match self.phase {
                RecoveryPhase::GameExit => 5,
                RecoveryPhase::Thermal => 15,
                _ => 10,
            };

            if self.recovery_ticks > threshold && *current_policy != PolicyState::EmergencyCool {
                self.in_recovery = false;
                self.phase = RecoveryPhase::None;
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

        // Stay in recovery
        for _ in 0..15 {
            assert!(rm.check_recovery(&PolicyState::Performance, false, false));
        }

        // Exit recovery
        assert!(!rm.check_recovery(&PolicyState::Performance, false, false));
        assert_eq!(rm.phase, RecoveryPhase::None);

        // Enter game exit recovery
        assert!(rm.check_recovery(&PolicyState::Performance, true, false));
        assert_eq!(rm.phase, RecoveryPhase::GameExit);
    }
}
