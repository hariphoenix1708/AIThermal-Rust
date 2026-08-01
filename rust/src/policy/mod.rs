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
    pub(crate) powersave_arm_count: u8,
    startup_time: std::time::Instant,
    startup_grace_secs: u64,
    last_total_score: f64,
    trend_history: std::collections::VecDeque<i32>,
}

impl PolicyEngine {
    pub fn new(debounce_sec: u64, _poll_interval_sec: u64) -> Self {
        let debounce = std::time::Duration::from_secs(debounce_sec.max(1));

        Self {
            current_policy: PolicyState::Balanced,
            debounce,
            last_change_at: std::time::Instant::now(),
            powersave_arm_count: 0,
            startup_time: std::time::Instant::now(),
            startup_grace_secs: 30, // Default 30s grace period for inputs to stabilize
            last_total_score: 0.0,
            trend_history: std::collections::VecDeque::with_capacity(5),
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
        cpu_pressure: f32,
        io_pressure: f32,
        config: &ProfilesConfig,
    ) -> PolicyState {
        // Smooth trend_score over the last 5 ticks to damp out single-tick derivative noise
        self.trend_history.push_back(trend_score);
        if self.trend_history.len() > 5 {
            self.trend_history.pop_front();
        }
        let smoothed_trend = (self.trend_history.iter().sum::<i32>() as f64 / self.trend_history.len() as f64).round() as i32;

        //
        let s_temp = (composite_temp as f64 - config.temp_cool as f64).max(0.0) * 2.0;
        let s_pred = (predicted_temp as f64 - config.temp_cool as f64).max(0.0) * 1.5;
        let s_game = if is_gaming {
            -(config.gaming_score_boost as f64)
        } else {
            0.0
        };

        // Trend score is scaled: > 0 means heating rapidly, < 0 means cooling
        let s_trend = (smoothed_trend as f64).clamp(-10.0, 10.0) * 2.5;

        // PSI dampener: if the system is thermally warm but NOT under
        // CPU or IO pressure, subtract from the score so we don't
        // unnecessarily tighten. This addresses the case where a warm
        // pocket / warm ambient causes SoC temp to hover without any
        // real load.
        let psi_dampener: f64 = if cpu_pressure < 5.0 && io_pressure < 5.0 {
            // Idle relief is temperature-independent.
            -4.0
        } else if (cpu_pressure > 50.0 || io_pressure > 30.0)
            && composite_temp >= config.temp_warm
        {
            // Amplify tightening ONLY when the device is actually warm.
            // Reduced from +4.0 to +3.0 so it cannot single-handedly
            // cross the Powersave threshold.
            3.0
        } else {
            0.0
        };

        let mut normal_use_guard = 0.0;
        if !is_gaming && !is_screen_off {
            if composite_temp >= config.temp_hot - 2 || smoothed_trend > 18 {
                normal_use_guard += 12.0; // Apply measured pressure only for real heat or sustained ramp.
            } else if composite_temp >= config.temp_warm && smoothed_trend > 9 {
                normal_use_guard += 6.0; // Warm-but-rising: nudge, don't cliff the UI.
            }
        }

        // Total evaluation score
        let total_score = s_temp
            + s_pred
            + s_game
            + s_trend
            + context_weight
            + game_modifier
            + comfort_weight
            + psi_dampener
            + normal_use_guard;

        tracing::debug!(
            target: "thermal",
            "Policy score components: s_temp={:.1} s_pred={:.1} s_trend={:.1} s_game={:.1} normal_use_guard={:.1} psi_dampener={:.1} context_weight={:.1} comfort_weight={:.1} game_modifier={:.1} total_score={:.1}",
            s_temp, s_pred, s_trend, s_game, normal_use_guard, psi_dampener, context_weight, comfort_weight, game_modifier, total_score
        );

        // Threshold evaluation (recalibrated based on the new total_score ranges)
        // With screen_weight removed and comfort_weight no longer *10, the score is tighter.
        // A typical hot score might be: temp diff (45-35)=10 * 2 = 20, pred (45-35)=15, game=10, trend=5, context=..., comfort=...
        // Let's calibrate:
        let tentative = if composite_temp >= config.temp_critical
            || predicted_temp >= config.temp_critical
            || total_score > 90.0
        {
            PolicyState::EmergencyCool
        } else if is_screen_off
            && !is_gaming
            && total_score < -5.0
            && self.last_change_at.elapsed().as_secs() > 10
        {
            PolicyState::Suspend
        } else if total_score > 65.0 {
            PolicyState::Powersave
        } else if total_score > 40.0 {
            PolicyState::Conservative
        } else if total_score > 15.0 {
            PolicyState::Balanced
        } else if is_gaming {
            PolicyState::Performance
        } else {
            PolicyState::Balanced
        };

        // P5: require sustained pressure before entering Powersave from
        // a lighter state. Single-tick trend spikes must not cliff the UI.
        let next_state = if tentative == PolicyState::Powersave
            && !matches!(self.current_state(),
                PolicyState::Powersave
                    | PolicyState::EmergencyCool
                    | PolicyState::Suspend)
        {
            let hot_enough = composite_temp >= config.temp_powersave;
            if hot_enough {
                // Composite already at/above temp_powersave — enter now.
                self.powersave_arm_count = 0;
                PolicyState::Powersave
            } else {
                self.powersave_arm_count = self.powersave_arm_count.saturating_add(1);
                if self.powersave_arm_count >= 2 {
                    self.powersave_arm_count = 0;
                    PolicyState::Powersave
                } else {
                    // Stay one step softer for one more tick.
                    PolicyState::Conservative
                }
            }
        } else {
            // R5: preserve the arm counter across a placeholder-Conservative
            // step. Only clear when the tentative itself is NOT Powersave.
            if tentative != PolicyState::Powersave {
                self.powersave_arm_count = 0;
            }
            tentative
        };

        self.apply_transition(next_state, total_score)
    }

    fn current_state(&self) -> &PolicyState {
        &self.current_policy
    }

    pub fn last_score(&self) -> f64 {
        self.last_total_score
    }

    fn apply_transition(&mut self, desired: PolicyState, total_score: f64) -> PolicyState {
        self.last_total_score = total_score;
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

            let allowed = if desired_rank > current_rank {
                // Becoming MORE conservative - require a smaller margin to avoid flap on noise, but react fast
                total_score > threshold_for_rank(desired_rank) + (HYSTERESIS_MARGIN / 2.0)
            } else if desired_rank < current_rank {
                // Becoming LESS conservative - require clearing the margin.
                total_score < threshold_for_rank(current_rank) - HYSTERESIS_MARGIN
            } else {
                true
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
            engine.evaluate(30, 30, 0, false, true, -10.0, 0.0, 0.0, 0.0, 0.0, &config),
            PolicyState::Suspend
        );

        // Emergency cool overrides immediately (high temp)
        assert_eq!(
            engine.evaluate(80, 80, 2, false, false, 0.0, 0.0, 0.0, 0.0, 0.0, &config),
            PolicyState::EmergencyCool
        );

        // Drop to cool should debounce
        assert_eq!(
            engine.evaluate(30, 30, 0, false, false, 0.0, 0.0, 0.0, 0.0, 0.0, &config),
            PolicyState::EmergencyCool // still emergency because time elapsed is < 10
        );

        // Fast forward time
        engine.last_change_at = std::time::Instant::now() - std::time::Duration::from_secs(10);
        assert_eq!(
            engine.evaluate(30, 30, 0, false, false, 0.0, 0.0, 0.0, 0.0, 0.0, &config),
            PolicyState::Balanced
        );

        // Rise to warm
        engine.last_change_at = std::time::Instant::now() - std::time::Duration::from_secs(10);
        let _res = engine.evaluate(50, 50, 0, false, false, 0.0, 0.0, 0.0, 0.0, 0.0, &config);
    }

    #[test]
    fn warm_stable_interactive_use_stays_balanced() {
        let mut engine = PolicyEngine::new(1, 2);
        engine.startup_grace_secs = 0;
        engine.last_change_at = std::time::Instant::now() - std::time::Duration::from_secs(2);
        let config = ProfilesConfig::default();

        let policy = engine.evaluate(
            config.temp_warm,
            config.temp_warm,
            0,
            false,
            false,
            5.0,
            0.0,
            10.0,
            20.0,
            0.0,
            &config,
        );

        assert_eq!(policy, PolicyState::Balanced);
    }
}

pub fn policy_rank(policy: &PolicyState) -> u8 {
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
