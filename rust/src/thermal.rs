use std::collections::VecDeque;

pub struct ThermalEngine {
    history: VecDeque<i32>,
    history_size: usize,
    ema_temp: f64,
    alpha: f64,
}

impl ThermalEngine {
    pub fn new(history_size: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(history_size),
            history_size,
            ema_temp: 0.0,
            alpha: 0.2, // EMA smoothing factor
        }
    }

    pub fn reset_after_long_sleep(&mut self) {
        self.history.clear();
        self.ema_temp = 0.0;
    }

    pub fn update(&mut self, current_temp: i32) {
        if self.history.len() >= self.history_size {
            self.history.pop_front();
        }
        self.history.push_back(current_temp);

        if self.ema_temp == 0.0 {
            self.ema_temp = current_temp as f64;
        } else {
            self.ema_temp =
                (current_temp as f64 * self.alpha) + (self.ema_temp * (1.0 - self.alpha));
        }
    }

    pub fn get_history(&self) -> &VecDeque<i32> {
        &self.history
    }

    pub fn get_smoothed_temp(&self) -> i32 {
        if self.history.is_empty() {
            return 0;
        }
        self.ema_temp.round() as i32
    }

    pub fn composite_temp(cpu: i32, gpu: i32, battery: i32, skin: i32, gpu_load: u32) -> i32 {
        let cpu_w = 0.4;
        let mut gpu_w = 0.3 * (gpu_load as f64 / 100.0);
        if gpu_w < 0.03 {
            gpu_w = 0.03;
        }
        let bat_w = 0.2;
        let skin_w = 0.1;

        let total = cpu_w + gpu_w + bat_w + skin_w;
        let cpu_n = cpu_w / total;
        let gpu_n = gpu_w / total;
        let bat_n = bat_w / total;
        let skin_n = skin_w / total;

        let weighted = (cpu as f64 * cpu_n)
            + (gpu as f64 * gpu_n)
            + (battery as f64 * bat_n)
            + (skin as f64 * skin_n);
        weighted.round() as i32
    }

    pub fn is_cooling(&self) -> bool {
        if self.history.len() < 3 {
            return false;
        }
        if let (Some(&last), Some(&first)) = (self.history.back(), self.history.front()) {
            (first - last) >= 2
        } else {
            false
        }
    }

    /// True when temperature has genuinely RISEN by >= 2C over the history
    /// window. Used by calibration so that warm-but-flat idle is not mistaken
    /// for active heating (which previously drove the calibration offset down
    /// every ~2 minutes during normal use).
    pub fn is_heating(&self) -> bool {
        if self.history.len() < 3 {
            return false;
        }
        if let (Some(&last), Some(&first)) = (self.history.back(), self.history.front()) {
            (last - first) >= 2
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thermal_history_and_smoothing() {
        let mut engine = ThermalEngine::new(3);
        engine.update(40);
        engine.update(42);
        engine.update(44);

        assert_eq!(engine.history.len(), 3);
        assert!(!engine.is_cooling());

        engine.update(38); // This pushes out 40
        assert_eq!(engine.history.len(), 3); // [42, 44, 38]
        assert!(engine.is_cooling()); // 42 - 38 = 4 >= 2 -> true

        // Noisy but flat case
        let mut engine2 = ThermalEngine::new(3);
        engine2.update(52);
        engine2.update(53);
        engine2.update(51); // 52 - 51 = 1 >= 2 -> false
        assert_eq!(engine2.history.len(), 3); // [52, 53, 51]
        assert!(!engine2.is_cooling());

        // Rising case: is_heating true, is_cooling false.
        let mut engine3 = ThermalEngine::new(3);
        engine3.update(44);
        engine3.update(46);
        engine3.update(48); // 48 - 44 = 4 >= 2 -> rising
        assert!(engine3.is_heating());
        assert!(!engine3.is_cooling());

        // Warm-but-flat: neither heating nor cooling (the idle case that
        // previously drove the calibration offset down).
        let mut engine4 = ThermalEngine::new(3);
        engine4.update(40);
        engine4.update(40);
        engine4.update(41); // +1 rise < 2
        assert!(!engine4.is_heating());
        assert!(!engine4.is_cooling());

        // EMA check
        let smoothed = engine.get_smoothed_temp();
        assert!(smoothed > 38 && smoothed < 44); // basic boundary check

        // Composite check
        let comp = ThermalEngine::composite_temp(50, 40, 30, 35, 100);
        assert_eq!(comp, 42); // gpu load 100 restores base weights
    }
}
