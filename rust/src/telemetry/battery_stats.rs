use std::time::Instant;

pub struct BatterySample {
    pub timestamp: Instant,
    pub batt_temp_c: i32,
    pub soc_percent: u8,
    pub current_now_ua: Option<i64>,
    pub screen_on: bool,
    pub is_gaming: bool,
    pub is_charging: bool,
}

#[derive(Clone, Copy)]
pub struct DrainRateSample {
    pub percent_per_hour: f64,
    pub was_screen_on: bool,
    pub was_gaming: bool,
    pub was_charging: bool,
}

pub struct BatteryStatsTracker {
    /// Most recent sample (used for screen-on/off time tracking).
    last_sample: Option<BatterySample>,
    /// Sample from the last time SOC changed (used for drain calculation).
    /// This avoids the per-tick comparison where SOC rarely changes between
    /// consecutive 1-second ticks, causing drain to always show as "?".
    last_drain_sample: Option<BatterySample>,
    /// The most recently computed drain rate (cached for logging).
    cached_drain: Option<DrainRateSample>,
    screen_on_secs: u64,
    screen_off_secs: u64,
    deep_sleep_secs: u64,
    awake_secs: u64,
}

impl Default for BatteryStatsTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl BatteryStatsTracker {
    pub fn new() -> Self {
        Self {
            last_sample: None,
            last_drain_sample: None,
            cached_drain: None,
            screen_on_secs: 0,
            screen_off_secs: 0,
            deep_sleep_secs: 0,
            awake_secs: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_sample(
        &mut self,
        batt_temp_c: i32,
        soc_percent: u8,
        current_now_ua: Option<i64>,
        screen_on: bool,
        is_gaming: bool,
        is_charging: bool,
        is_long_idle: bool,
        tick_interval_secs: u64,
    ) -> Option<DrainRateSample> {
        let now = Instant::now();

        let real_elapsed_secs = self
            .last_sample
            .as_ref()
            .map(|prev| now.duration_since(prev.timestamp).as_secs())
            .unwrap_or(tick_interval_secs);

        if screen_on {
            self.screen_on_secs += real_elapsed_secs;
        } else {
            self.screen_off_secs += real_elapsed_secs;
            if is_long_idle {
                self.deep_sleep_secs += real_elapsed_secs;
            } else {
                self.awake_secs += real_elapsed_secs;
            }
        }

        // Compute drain rate: compare with last_drain_sample (only resets
        // when SOC changes). This gives meaningful values even when ticks
        // are 1-second apart and SOC only changes every few minutes.
        let drain_rate = match self.last_drain_sample {
            Some(ref prev) => {
                let elapsed_secs = now.duration_since(prev.timestamp).as_secs();
                let soc_delta = prev.soc_percent as i32 - soc_percent as i32;
                if elapsed_secs == 0 || soc_delta == 0 {
                    None
                } else {
                    let percent_per_hour = (soc_delta as f64) * 3600.0 / elapsed_secs as f64;
                    Some(DrainRateSample {
                        percent_per_hour,
                        was_screen_on: prev.screen_on,
                        was_gaming: prev.is_gaming,
                        was_charging: prev.is_charging,
                    })
                }
            }
            None => None,
        };

        // Update drain anchor when SOC changes or on first sample.
        let should_reset_anchor = self
            .last_drain_sample
            .as_ref()
            .map(|p| p.soc_percent != soc_percent)
            .unwrap_or(true);

        if should_reset_anchor {
            self.last_drain_sample = Some(BatterySample {
                timestamp: now,
                batt_temp_c,
                soc_percent,
                current_now_ua,
                screen_on,
                is_gaming,
                is_charging,
            });
        }

        // Cache the latest drain rate for display between SOC changes
        if let Some(d) = drain_rate {
            self.cached_drain = Some(d);
        }

        self.last_sample = Some(BatterySample {
            timestamp: now,
            batt_temp_c,
            soc_percent,
            current_now_ua,
            screen_on,
            is_gaming,
            is_charging,
        });

        // Return drain_rate if SOC changed, otherwise return cached drain
        // so the log always shows the most recent known rate.
        drain_rate.or(self.cached_drain)
    }

    pub fn summary_line(&self) -> String {
        format!(
            "screen_on={}s screen_off={}s deep_sleep={}s awake={}s",
            self.screen_on_secs, self.screen_off_secs, self.deep_sleep_secs, self.awake_secs
        )
    }
}
