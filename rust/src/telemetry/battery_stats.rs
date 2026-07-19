pub struct BatterySample {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub batt_temp_c: i32,
    pub soc_percent: u8,
    pub current_now_ua: Option<i64>,
    pub screen_on: bool,
    pub is_gaming: bool,
    pub is_charging: bool,
}

pub struct DrainRateSample {
    pub percent_per_hour: f64,
    pub was_screen_on: bool,
    pub was_gaming: bool,
    pub was_charging: bool,
}

pub struct BatteryStatsTracker {
    last_sample: Option<BatterySample>,
    screen_on_secs: u64,
    screen_off_secs: u64,
    deep_sleep_secs: u64,
    awake_secs: u64,
}

impl BatteryStatsTracker {
    pub fn new() -> Self {
        Self {
            last_sample: None,
            screen_on_secs: 0,
            screen_off_secs: 0,
            deep_sleep_secs: 0,
            awake_secs: 0,
        }
    }

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
        let now = chrono::Utc::now();

        if screen_on {
            self.screen_on_secs += tick_interval_secs;
        } else {
            self.screen_off_secs += tick_interval_secs;
            if is_long_idle {
                self.deep_sleep_secs += tick_interval_secs;
            } else {
                self.awake_secs += tick_interval_secs;
            }
        }

        let drain_rate = self.last_sample.as_ref().and_then(|prev| {
            let elapsed_secs = (now - prev.timestamp).num_seconds();
            if elapsed_secs <= 0 {
                return None;
            }
            let soc_delta = prev.soc_percent as i32 - soc_percent as i32;
            let percent_per_hour = (soc_delta as f64) * 3600.0 / elapsed_secs as f64;
            Some(DrainRateSample {
                percent_per_hour,
                was_screen_on: prev.screen_on,
                was_gaming: prev.is_gaming,
                was_charging: prev.is_charging,
            })
        });

        self.last_sample = Some(BatterySample {
            timestamp: now,
            batt_temp_c,
            soc_percent,
            current_now_ua,
            screen_on,
            is_gaming,
            is_charging,
        });

        drain_rate
    }

    pub fn summary_line(&self) -> String {
        format!(
            "screen_on={}s screen_off={}s deep_sleep={}s awake={}s",
            self.screen_on_secs, self.screen_off_secs, self.deep_sleep_secs, self.awake_secs
        )
    }
}
