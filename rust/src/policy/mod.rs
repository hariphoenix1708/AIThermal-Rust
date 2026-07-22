use crate::config::ProfilesConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyState {
    Performance,
    Balanced,
    Conservative,
    Powersave,
    EmergencyCool,
    Suspend,
}

pub struct PolicyEngine {
    pub current_policy: PolicyState,
    pub debounce: std::time::Duration,
    pub last_change_at: std::time::Instant,
    startup_time: std::time::Instant,
    startup_grace_secs: u64,
}

impl PolicyEngine {
    pub fn new(debounce_sec: u64, _poll_interval_sec: u64) -> Self {
        let debounce = std::time::Duration::from_secs(debounce_sec.max(1));

        Self {
            current_policy: PolicyState::Balanced,
            debounce,
            last_change_at: std::time::Instant::now(),
            startup_time: std::time::Instant::now(),
            startup_grace_secs: 30, // Default 30s grace period for inputs to stabilize
        }
    }

    /// Evaluates the temperature and requested hints to emit the desired policy.
    /// Does NOT perform side effects (no sysfs writes).
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &mut self,
        composite_temp: i32,
        predicted_temp: i32,
        trend_score: i32,
        is_gaming: bool,
        is_screen_off: bool, // Passed in but handled via context_weight mostly, left here for explicit threshold logic
        context_weight: f64,
        game_modifier: f64,
        comfort_weight: f64,
        config: &ProfilesConfig,
    ) -> PolicyState {
        //
        let s_temp = (composite_temp as f64 - config.temp_cool as f64).max(0.0) * 2.0;
        let s_pred = (predicted_temp as f64 - config.temp_cool as f64).max(0.0) * 1.5;
        let s_game = if is_gaming {
            -(config.gaming_score_boost as f64)
        } else {
            0.0
        };

        // Trend score is scaled: > 0 means heating rapidly, < 0 means cooling
        let s_trend = (trend_score as f64).clamp(-10.0, 10.0) * 2.5;

        // Total evaluation score
        let total_score =
            s_temp + s_pred + s_game + s_trend + context_weight + game_modifier + comfort_weight;

        // Threshold evaluation (recalibrated based on the new total_score ranges)
        // With screen_weight removed and comfort_weight no longer *10, the score is tighter.
        // A typical hot score might be: temp diff (45-35)=10 * 2 = 20, pred (45-35)=15, game=10, trend=5, context=..., comfort=...
        // Let's calibrate:
        let desired = if composite_temp >= config.temp_critical
            || predicted_temp >= config.temp_critical
            || total_score > 90.0
        {
            PolicyState::EmergencyCool
        } else if is_screen_off && !is_gaming && total_score < -5.0 && self.last_change_at.elapsed().as_secs() > 10
        {
            PolicyState::Suspend
        } else if total_score > 65.0 {
            PolicyState::Powersave
        } else if total_score > 40.0 {
            PolicyState::Conservative
        } else if total_score > 15.0 {
            PolicyState::Balanced
        } else {
            PolicyState::Performance
        };

        self.apply_transition(desired, total_score)
    }

    fn apply_transition(&mut self, desired: PolicyState, total_score: f64) -> PolicyState {
        // Immediate escalate for Emergency or Suspend
        if desired == PolicyState::EmergencyCool || desired == PolicyState::Suspend {
            if self.current_policy != desired {
                self.current_policy = desired.clone();
                self.last_change_at = std::time::Instant::now();
            }
            return desired;
        }

        // Startup grace period: hold at Balanced to prevent early instability
        if self.startup_time.elapsed().as_secs() < self.startup_grace_secs {
            self.current_policy = PolicyState::Balanced;
            return self.current_policy.clone();
        }

        // Apply debounce for normal transitions to prevent rapid flapping
        if desired != self.current_policy && self.last_change_at.elapsed() >= self.debounce {
            const HYSTERESIS_MARGIN: f64 = 8.0;

            let desired_rank = policy_rank(&desired);
            let current_rank = policy_rank(&self.current_policy);

            let allowed = if desired_rank >= current_rank {
                true // always allow becoming MORE conservative immediately (safety)
            } else {
                // Becoming LESS conservative - require clearing the margin.
                total_score < threshold_for_rank(current_rank) - HYSTERESIS_MARGIN
            };

            if allowed {
                self.current_policy = desired.clone();
                self.last_change_at = std::time::Instant::now();
            }
        }

        self.current_policy.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_evaluation_and_debounce() {
        let mut engine = PolicyEngine::new(10, 2); // 10s debounce
        // bypass startup grace period for normal tests
        engine.startup_grace_secs = 0;
        let config = ProfilesConfig::default();

        // Screen off doesn't override immediately unless score is low and time elapsed > 10
        engine.last_change_at = std::time::Instant::now() - std::time::Duration::from_secs(11);
        // With temps at 30, they are likely cool, giving 0 for s_temp and s_pred.
        // We pass -10.0 for context_weight to drop the score below -5.0.
        assert_eq!(
            engine.evaluate(30, 30, 0, false, true, -10.0, 0.0, 0.0, &config),
            PolicyState::Suspend
        );

        // Emergency cool overrides immediately (high temp)
        assert_eq!(
            engine.evaluate(80, 80, 2, false, false, 0.0, 0.0, 0.0, &config),
            PolicyState::EmergencyCool
        );

        // Drop to cool should debounce
        assert_eq!(
            engine.evaluate(30, 30, 0, false, false, 0.0, 0.0, 0.0, &config),
            PolicyState::EmergencyCool // still emergency because time elapsed is < 10
        );

        // Fast forward time
        engine.last_change_at = std::time::Instant::now() - std::time::Duration::from_secs(10);
        assert_eq!(
            engine.evaluate(30, 30, 0, false, false, 0.0, 0.0, 0.0, &config),
            PolicyState::Performance
        );

        // Rise to warm
        engine.last_change_at = std::time::Instant::now() - std::time::Duration::from_secs(10);
        let _res = engine.evaluate(50, 50, 0, false, false, 0.0, 0.0, 0.0, &config);
    }
}

fn policy_rank(policy: &PolicyState) -> u8 {
    match policy {
        PolicyState::Performance => 0,
        PolicyState::Balanced => 1,
        PolicyState::Conservative => 2,
        PolicyState::Powersave => 3,
        PolicyState::Suspend => 4,
        PolicyState::EmergencyCool => 5,
    }
}

fn threshold_for_rank(rank: u8) -> f64 {
    match rank {
        0 => f64::MIN,
        1 => 15.0,
        2 => 40.0,
        3 => 65.0,
        _ => f64::MAX,
    }
}
