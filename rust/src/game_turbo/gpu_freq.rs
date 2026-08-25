//! GPU frequency management during gaming.
//!
//! Sets GPU to best power level during gaming for minimum rendering
//! latency. Saves and restores the original level on game exit.
//! When thermal-throttled, eases back to allow the GPU to cool.

use std::fs;

pub struct GpuFreqState {
    /// Original power level before gaming.
    saved_power_level: Option<u32>,
    /// Path to the writable power level knob.
    power_level_path: Option<String>,
    /// Best (highest performance = lowest index) power level from GPU discovery.
    best_level: u32,
}

impl GpuFreqState {
    pub fn new() -> Self {
        Self {
            saved_power_level: None,
            power_level_path: None,
            best_level: 0,
        }
    }

    /// Activate: save current GPU power level and set to best.
    pub fn activate(
        &mut self,
        power_level_path: Option<&str>,
        current_level: Option<u32>,
        best_level: u32,
    ) {
        let Some(path) = power_level_path else {
            tracing::debug!(
                target: "game_turbo",
                "GPU freq: no writable power_level_path, skipping"
            );
            return;
        };

        self.power_level_path = Some(path.to_string());
        self.best_level = best_level;

        // Save current level for restoration.
        self.saved_power_level = current_level;

        // Set to best (lowest index = highest performance).
        if write_level(path, best_level) {
            tracing::info!(
                target: "game_turbo",
                "GPU freq: {} -> {} (best for gaming)",
                current_level.map(|l| l.to_string()).unwrap_or_else(|| "?".to_string()),
                best_level
            );
        } else {
            tracing::debug!(
                target: "game_turbo",
                "GPU freq: write to {} failed",
                path
            );
        }
    }

    /// Deactivate: restore original GPU power level.
    pub fn deactivate(&mut self) {
        if let (Some(path), Some(orig)) = (&self.power_level_path, self.saved_power_level) {
            if write_level(path, orig) {
                tracing::info!(
                    target: "game_turbo",
                    "GPU freq: restored to {}",
                    orig
                );
            } else {
                tracing::debug!(
                    target: "game_turbo",
                    "GPU freq: restore to {} failed",
                    orig
                );
            }
        }
        self.saved_power_level = None;
        self.power_level_path = None;
    }
}

fn write_level(path: &str, level: u32) -> bool {
    use std::io::Write;
    match fs::OpenOptions::new().write(true).truncate(true).open(path) {
        Ok(mut file) => {
            file.write_all(level.to_string().as_bytes()).is_ok() && file.flush().is_ok()
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_freq_state_new() {
        let state = GpuFreqState::new();
        assert!(state.saved_power_level.is_none());
        assert!(state.power_level_path.is_none());
    }
}
