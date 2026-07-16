use crate::thermal::ThermalEngine;

#[derive(Debug, Clone, PartialEq)]
pub struct PredictionResult {
    pub current_smoothed: i32,
    pub predicted_temp: i32,
    pub trend_slope: f32,
    pub trend_score: i32,
    pub confidence: u32,
}

pub struct PredictionEngine {
    window_size: usize,
    prediction_steps: usize,
}

impl PredictionEngine {
    pub fn new(window_size: usize, prediction_steps: usize) -> Self {
        Self {
            window_size,
            prediction_steps,
        }
    }

    pub fn predict(&self, engine: &ThermalEngine) -> Option<PredictionResult> {
        let history: Vec<i32> = engine.get_history().iter().copied().collect();
        let n = history.len().min(self.window_size);
        if n < 2 {
            return None; // Not enough data for trend
        }

        let smoothed = engine.get_smoothed_temp();

        // Simple linear regression over the recent window
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_xy = 0.0;
        let mut sum_xx = 0.0;

        let samples: Vec<i32> = history.into_iter().rev().take(n).collect();

        // reverse back to chronological for correct slope
        let samples: Vec<i32> = samples.into_iter().rev().collect();

        for (x, &y) in samples.iter().enumerate() {
            let x_f = x as f32;
            let y_f = y as f32;

            sum_x += x_f;
            sum_y += y_f;
            sum_xy += x_f * y_f;
            sum_xx += x_f * x_f;
        }

        let n_f = n as f32;
        let denominator = n_f * sum_xx - sum_x * sum_x;

        let slope = if denominator.abs() > 1e-6 {
            (n_f * sum_xy - sum_x * sum_y) / denominator
        } else {
            0.0
        };

        // Predict future temp
        let predicted_temp = smoothed + (slope * self.prediction_steps as f32).round() as i32;

        // Confidence heuristic based on data points available vs window,
        // penalized if the variance is extremely high (noisy data).
        // For now, keep it simple but improve it slightly from before.
        let coverage_score = (n as f32 / self.window_size as f32) * 100.0;

        let mut variance = 0.0;
        let mean_y = sum_y / n_f;
        for &y in &samples {
            variance += (y as f32 - mean_y) * (y as f32 - mean_y);
        }
        variance /= n_f;

        let stability_penalty = if variance > 25.0 {
            30.0
        } else if variance > 10.0 {
            15.0
        } else {
            0.0
        };
        let mut confidence = (coverage_score - stability_penalty) as i32;
        if confidence < 0 {
            confidence = 0;
        }

        let trend_score = (slope * 25.0).round().clamp(-50.0, 50.0) as i32;

        Some(PredictionResult {
            current_smoothed: smoothed,
            predicted_temp,
            trend_slope: slope,
            trend_score,
            confidence: confidence as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_prediction() {
        let mut model = ThermalEngine::new(10);
        let engine = PredictionEngine::new(5, 3);

        // Constant trend
        for _ in 0..5 {
            model.update(40);
        }

        let p = engine.predict(&model).unwrap();
        assert_eq!(p.current_smoothed, 40);
        assert_eq!(p.predicted_temp, 40);
        assert!(p.trend_slope.abs() < 0.001);

        // Rising trend (40, 41, 42, 43, 44)
        let mut model2 = ThermalEngine::new(10);
        model2.update(40);
        model2.update(41);
        model2.update(42);
        model2.update(43);
        model2.update(44);

        let p2 = engine.predict(&model2).unwrap();
        assert!(p2.trend_slope > 0.9 && p2.trend_slope < 1.1); // slope ~ 1.0 per tick

        // Predict 3 steps ahead from smoothed
        let expected_pred = p2.current_smoothed + 3;
        assert_eq!(p2.predicted_temp, expected_pred);
    }
}
