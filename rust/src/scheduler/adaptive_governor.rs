use std::time::{Duration, Instant};

pub struct AdaptiveGovernorState {
    pub last_sample_at: Option<Instant>,
    pub sample_interval: Duration,      // e.g. 1.5s - tunable
    pub current_tier: FrequencyTier,
    pub consecutive_good_samples: u32,  // for controlled step-down
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

        let next_tier = if let Some(stats) = frame_stats {
            let jank_ratio = stats.jank_ratio();
            // Thresholds are intentionally coarse and conservative - tune only
            // after observing real jank_ratio values from this device in a
            // logged session, do not assume these exact numbers are optimal.
            if jank_ratio > 0.15 || stats.worst_frame_ns > 50_000_000 {
                // More than 15% of recent frames missed budget, or at least
                // one frame took over 50ms - clear, real stutter. Go to max.
                FrequencyTier::Max
            } else if jank_ratio > 0.05 {
                // Some jank, not severe - step up but not to max.
                FrequencyTier::High
            } else if jank_ratio == 0.0 && cluster_utilization < 0.35 {
                // Perfectly smooth AND low measured load - safe to ease off.
                FrequencyTier::Eco
            } else {
                FrequencyTier::Balanced
            }
        } else {
            // No frame data available this sample (dumpsys call failed, or no
            // game currently detected) - fall back to pure load-based decision.
            if cluster_utilization > 0.75 {
                FrequencyTier::High
            } else if cluster_utilization < 0.25 {
                FrequencyTier::Eco
            } else {
                FrequencyTier::Balanced
            }
        };

        // Rate limiting: only allow stepping DOWN one tier at a time, but
        // allow stepping UP immediately to any tier (react fast to stutter,
        // back off cautiously to avoid oscillating right back into jank).
        let stepped_tier = if tier_rank(next_tier) < tier_rank(self.current_tier) {
            // Stepping down - only move one rank down per sample, and only
            // after 2 consecutive samples agree it's safe to do so.
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
            next_tier // stepping up or staying - apply immediately
        };

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
