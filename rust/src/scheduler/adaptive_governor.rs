use std::time::{Duration, Instant};

pub struct AdaptiveGovernorState {
    pub last_sample_at: Option<Instant>,
    pub sample_interval: Duration,      // e.g. 1.5s - tunable
    pub current_tier: FrequencyTier,
    pub consecutive_good_samples: u32,  // for controlled step-down
    pub promotion_streak: u8,
    pub demotion_streak: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrequencyTier {
    Max,      // use cluster's max_freq() - heavy, sustained jank
    High,     // use a frequency between mid and max
    Balanced, // use cluster's mid_freq() - the steady state
    Eco,      // use a frequency between min and mid - consistently smooth
}

impl AdaptiveGovernorState {
    pub fn new(sample_interval_secs: u64) -> Self {
        Self {
            last_sample_at: None,
            sample_interval: Duration::from_millis(sample_interval_secs * 1000), // use millis internally for fractional support if wanted
            current_tier: FrequencyTier::Balanced,
            consecutive_good_samples: 0,
            promotion_streak: 0,
            demotion_streak: 0,
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
    /// FrameStats (if available) and current cluster utilization (always
    /// available as a fallback signal). Returns the tier to apply until the
    /// next sample.
    pub fn decide_tier(
        &mut self,
        frame_stats: Option<&crate::monitor::frame_sampler::FrameStats>,
        cluster_utilization: f32,
    ) -> FrequencyTier {
        self.last_sample_at = Some(Instant::now());

        let raw_next_tier = if let Some(stats) = frame_stats {
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
        } else {
            if cluster_utilization > 0.75 {
                FrequencyTier::High
            } else if cluster_utilization < 0.55 {
                FrequencyTier::Eco
            } else {
                FrequencyTier::Balanced
            }
        };

        let next_tier = if self.current_tier == FrequencyTier::Eco && raw_next_tier == FrequencyTier::Balanced {
            self.promotion_streak += 1;
            if self.promotion_streak >= 2 {
                self.promotion_streak = 0;
                FrequencyTier::Balanced
            } else {
                FrequencyTier::Eco
            }
        } else if self.current_tier == FrequencyTier::Balanced && raw_next_tier == FrequencyTier::Eco {
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
