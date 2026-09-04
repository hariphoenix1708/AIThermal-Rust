use std::time::{Duration, Instant};

pub struct AdaptiveGovernorState {
    pub last_sample_at: Option<Instant>,
    pub sample_interval: Duration, // e.g. 1.5s - tunable
    pub current_tier: FrequencyTier,
    pub consecutive_good_samples: u32, // for controlled step-down
    pub promotion_streak: u8,
    pub demotion_streak: u8,
    /// Consecutive decisions with no usable frame signal (None or under
    /// MIN_JANK_SAMPLES). Holding Max forever on zero signal cooked a full
    /// 9-min CODM match (51C composite, 47C skin) with no benefit — past the
    /// grace window we fall back to utilization, floored at High so the
    /// v3.2.15 starvation (Balanced mid-cap all session) cannot recur.
    pub no_signal_streak: u32,
}

// Decisions (~1s cadence while gaming) to tolerate total frame-signal
// blindness before easing off Max. Covers lobby/loading; a whole match
// with zero valid samples means the signal path is broken, not the game
// smooth — holding Fmax beyond this only adds heat.
const NO_SIGNAL_GRACE: u32 = 90;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrequencyTier {
    Max,      // use cluster's max_freq() - heavy, sustained jank
    High,     // use a frequency between mid and max
    Balanced, // use cluster's mid_freq() - the steady state
    Eco,      // use a frequency between min and mid - consistently smooth
}

// Minimum parsed frames before a FrameStats jank ratio is considered
// statistically meaningful for tier decisions. The Android 16 framestats
// windows on this device only yield ~5-9 durations per capture (often 3-4 in
// lobbies/menus), so this must sit at or below that floor. Below this
// threshold we cannot PROVE the game is running smoothly, so the governor
// holds Max instead of capping on low utilization (which previously starved
// COD at the Balanced mid-frequency all session).
const MIN_JANK_SAMPLES: usize = 5;

impl AdaptiveGovernorState {
    pub fn new(sample_interval_secs: u64) -> Self {
        Self {
            last_sample_at: None,
            sample_interval: Duration::from_millis(sample_interval_secs * 1000), // use millis internally for fractional support if wanted
            current_tier: FrequencyTier::Balanced,
            consecutive_good_samples: 0,
            promotion_streak: 0,
            demotion_streak: 0,
            no_signal_streak: 0,
        }
    }

    pub fn nudge_on_screen_on(&mut self) {
        if matches!(self.current_tier, FrequencyTier::Eco) {
            self.current_tier = FrequencyTier::Balanced;
        }
    }

    pub fn should_sample(&self) -> bool {
        match self.last_sample_at {
            Some(t) => t.elapsed() >= self.sample_interval,
            None => true,
        }
    }

    /// Core decision logic. Called once per sample_interval with fresh
    /// FrameStats (if available), current cluster utilization, and GPU load.
    /// Returns the tier to apply until the next sample.
    pub fn decide_tier(
        &mut self,
        frame_stats: Option<&crate::monitor::frame_sampler::FrameStats>,
        cluster_utilization: f32,
        gpu_load: f32,
    ) -> FrequencyTier {
        self.last_sample_at = Some(Instant::now());

        // decide_tier is only ever called while a game is running (the
        // orchestrator gates it on is_gaming + Performance policy). Jank from
        // 2-4 recovered frames is statistical noise, so we never trust a
        // handful of durations to DEMOTE the tier; but equally, a missing or
        // too-thin frame signal must not be treated as "idle". The old
        // utilization fallback capped COD at the Balanced mid-frequency
        // (1.2-1.65 GHz) for entire sessions because lobbies keep CPU util
        // low, and the frame windows on this device never yielded >=5 samples
        // for jank to fire. When we cannot prove the game is smooth we run at
        // Max; only jank==0 over a real sample count is allowed to step down.
        let has_signal = matches!(frame_stats,
            Some(stats) if stats.sample_count >= MIN_JANK_SAMPLES);
        if has_signal {
            self.no_signal_streak = 0;
        } else {
            self.no_signal_streak = self.no_signal_streak.saturating_add(1);
        }
        let raw_next_tier = if let Some(stats) = frame_stats
            && stats.sample_count >= MIN_JANK_SAMPLES {
            let jank_ratio = stats.jank_ratio();
            if jank_ratio > 0.15 || stats.worst_frame_ns > 50_000_000 {
                FrequencyTier::Max
            } else if jank_ratio > 0.05 {
                FrequencyTier::High
            } else if jank_ratio == 0.0 && cluster_utilization < 0.55 {
                FrequencyTier::Eco
            } else {
                FrequencyTier::Balanced
            }
        } else if self.no_signal_streak > NO_SIGNAL_GRACE {
            // Signal blind for minutes: the frame path is broken for this
            // title, not the game idle. Ease to utilization-driven tier
            // floored at High — sheds Max heat while keeping headroom the
            // old Balanced cap lacked. GPU override below can still lift.
            if cluster_utilization >= 0.8 {
                FrequencyTier::Max
            } else {
                FrequencyTier::High
            }
        } else {
            FrequencyTier::Max
        };

        // GPU load override: if the GPU is the bottleneck, CPU frequency
        // demotion won't help rendering. Force Max when GPU is saturated,
        // and block Eco/Balanced when GPU load is high.
        let raw_next_tier = if gpu_load > 0.90 {
            FrequencyTier::Max
        } else if gpu_load > 0.80 && matches!(raw_next_tier, FrequencyTier::Eco | FrequencyTier::Balanced) {
            FrequencyTier::High
        } else {
            raw_next_tier
        };

        let next_tier = if self.current_tier == FrequencyTier::Eco
            && raw_next_tier == FrequencyTier::Balanced
        {
            self.promotion_streak += 1;
            if self.promotion_streak >= 2 {
                self.promotion_streak = 0;
                FrequencyTier::Balanced
            } else {
                FrequencyTier::Eco
            }
        } else if self.current_tier == FrequencyTier::Balanced
            && raw_next_tier == FrequencyTier::Eco
        {
            self.demotion_streak += 1;
            if self.demotion_streak >= 2 {
                self.demotion_streak = 0;
                FrequencyTier::Eco
            } else {
                FrequencyTier::Balanced
            }
        } else {
            if raw_next_tier == FrequencyTier::Eco {
                self.promotion_streak = 0;
            }
            if raw_next_tier == FrequencyTier::Balanced {
                self.demotion_streak = 0;
            }
            raw_next_tier
        };

        let stepped_tier = if tier_rank(next_tier) < tier_rank(self.current_tier) {
            if next_tier == self.current_tier {
                self.consecutive_good_samples = 0;
            } else {
                self.consecutive_good_samples += 1;
            }
            if self.consecutive_good_samples >= 2 {
                self.consecutive_good_samples = 0;
                step_down_one(self.current_tier)
            } else {
                self.current_tier
            }
        } else {
            self.consecutive_good_samples = 0;
            next_tier
        };

        if self.current_tier != stepped_tier {
            let jank = frame_stats.map(|s| s.jank_ratio()).unwrap_or(0.0);
            tracing::info!(target: "thermal",
                "Adaptive tier {:?} -> {:?} (util={:.0}%, jank={:.2}%, streak={})",
                self.current_tier, stepped_tier, cluster_utilization*100.0, jank*100.0, self.promotion_streak);
            tracing::debug!(target: "thermal", "Adaptive tier {:?} -> {:?}", self.current_tier, stepped_tier);
        }

        self.current_tier = stepped_tier;
        stepped_tier
    }
}

fn tier_rank(t: FrequencyTier) -> u8 {
    match t {
        FrequencyTier::Eco => 0,
        FrequencyTier::Balanced => 1,
        FrequencyTier::High => 2,
        FrequencyTier::Max => 3,
    }
}

fn step_down_one(current: FrequencyTier) -> FrequencyTier {
    match current {
        FrequencyTier::Max => FrequencyTier::High,
        FrequencyTier::High => FrequencyTier::Balanced,
        FrequencyTier::Balanced => FrequencyTier::Eco,
        FrequencyTier::Eco => FrequencyTier::Eco,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_rank() {
        assert_eq!(tier_rank(FrequencyTier::Eco), 0);
        assert_eq!(tier_rank(FrequencyTier::Balanced), 1);
        assert_eq!(tier_rank(FrequencyTier::High), 2);
        assert_eq!(tier_rank(FrequencyTier::Max), 3);
    }

    #[test]
    fn test_step_down_one() {
        assert_eq!(step_down_one(FrequencyTier::Max), FrequencyTier::High);
        assert_eq!(step_down_one(FrequencyTier::High), FrequencyTier::Balanced);
        assert_eq!(step_down_one(FrequencyTier::Balanced), FrequencyTier::Eco);
        assert_eq!(step_down_one(FrequencyTier::Eco), FrequencyTier::Eco);
    }

    #[test]
    fn test_decide_tier_without_frame_stats() {
        let mut gov = AdaptiveGovernorState::new(0);
        gov.current_tier = FrequencyTier::Max; // Start at max to test step-down

        // Step down logic requires TWO consecutive good samples where tier_rank(next) < tier_rank(current).
        // Current: Max, Util: 0.8 -> Next: High.
        let tier = gov.decide_tier(None, 0.8, 0.0);
        assert_eq!(tier, FrequencyTier::Max); // 1st good sample, stays Max

        let tier = gov.decide_tier(None, 0.8, 0.0);
        assert_eq!(tier, FrequencyTier::Max); // 2nd good sample, stays Max

        // Update struct's current tier as the orchestrator would.
        gov.current_tier = tier;

        // Med util -> Next is Balanced.
        let tier = gov.decide_tier(None, 0.6, 0.0);
        assert_eq!(tier, FrequencyTier::Max); // 1st good sample, stays Max

        let tier = gov.decide_tier(None, 0.6, 0.0);
        assert_eq!(tier, FrequencyTier::Max); // 2nd good sample, stays Max

        gov.current_tier = tier;

        // Low util (< 0.55) -> Next is Eco.
        let tier = gov.decide_tier(None, 0.5, 0.0);
        assert_eq!(tier, FrequencyTier::Max); // no signal -> Max, not Eco
        gov.current_tier = tier;

        let tier = gov.decide_tier(None, 0.5, 0.0);
        assert_eq!(tier, FrequencyTier::Max);
        gov.current_tier = tier;

        // A missing frame signal is never enough to DEMOTE a gaming session;
        // only proven-clean samples are allowed to walk the tier back down.
        let _ = gov.current_tier;
    }

    fn frame_stats(n: usize, janky: usize) -> crate::monitor::frame_sampler::FrameStats {
        crate::monitor::frame_sampler::FrameStats {
            sample_count: n,
            janky_frames: janky,
            p50_frame_ns: 8_000_000,
            p90_frame_ns: 8_000_000,
            worst_frame_ns: 12_000_000,
            max_consecutive_jank: 0,
            captured_at: None,
        }
    }

    #[test]
    fn test_no_signal_grace_then_high_floor() {
        let mut gov = AdaptiveGovernorState::new(0);
        gov.current_tier = FrequencyTier::Max;

        // Within grace: missing signal holds Max (v3.2.15 behavior).
        for _ in 0..10 {
            let tier = gov.decide_tier(None, 0.3, 0.0);
            assert_eq!(tier, FrequencyTier::Max);
            gov.current_tier = tier;
        }
        // Past grace with low util: eases to High, never below (no starvation).
        for _ in 0..100 {
            let tier = gov.decide_tier(None, 0.3, 0.0);
            gov.current_tier = tier;
        }
        assert_eq!(gov.current_tier, FrequencyTier::High);
        // High util keeps Max even past grace.
        let mut gov2 = AdaptiveGovernorState::new(0);
        gov2.current_tier = FrequencyTier::Max;
        gov2.no_signal_streak = NO_SIGNAL_GRACE + 1;
        let tier = gov2.decide_tier(None, 0.9, 0.0);
        assert_eq!(tier, FrequencyTier::Max);
        // A real signal resets the streak.
        let clean = frame_stats(8, 0);
        gov2.decide_tier(Some(&clean), 0.6, 0.0);
        assert_eq!(gov2.no_signal_streak, 0);
    }

    #[test]
    fn test_steps_down_from_max_only_on_clean_samples() {
        let mut gov = AdaptiveGovernorState::new(0);
        gov.current_tier = FrequencyTier::Max;

        // Enough clean samples (>= MIN_JANK_SAMPLES, jank==0) allow a step
        // down after two consecutive good samples.
        let clean = frame_stats(8, 0);

        let tier = gov.decide_tier(Some(&clean), 0.6, 0.0);
        assert_eq!(tier, FrequencyTier::Max); // 1st good sample, holds Max
        gov.current_tier = tier;

        let tier = gov.decide_tier(Some(&clean), 0.6, 0.0);
        assert_eq!(tier, FrequencyTier::High); // 2nd good sample, step to High
        gov.current_tier = tier;

        let tier = gov.decide_tier(Some(&clean), 0.6, 0.0);
        assert_eq!(tier, FrequencyTier::High); // 1st good sample from High
        gov.current_tier = tier;

        let tier = gov.decide_tier(Some(&clean), 0.6, 0.0);
        assert_eq!(tier, FrequencyTier::Balanced); // 2nd good sample, step to Balanced
        gov.current_tier = tier;

        // A noisy few-frame window (3-4 samples, under the threshold) cannot
        // demote from Balanced; it escalates to Max instead of trusting jank.
        let noisy = frame_stats(3, 1);
        let tier = gov.decide_tier(Some(&noisy), 0.6, 0.0);
        assert_eq!(tier, FrequencyTier::Max);
    }
}
