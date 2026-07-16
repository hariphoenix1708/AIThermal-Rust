use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use tracing::info;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct CalibrationState {
    pub active_offset: i32,
    pub hot_ticks: u32,
    pub cool_ticks: u32,
    pub slow_cooler_persistent: bool,
    pub consecutive_cool_sessions: u32,
}

pub struct CalibrationManager {
    calibration_file: PathBuf,
    temp_file: PathBuf,
    pub active_offset: i32,
    pub hot_ticks: u32,
    pub cool_ticks: u32,
    pub slow_cooler_persistent: bool,
    pub consecutive_cool_sessions: u32,
}

impl CalibrationManager {
    pub fn new(state_dir: &str) -> Self {
        let base = PathBuf::from(state_dir);
        let mut manager = Self {
            calibration_file: base.join("calibration.json"),
            temp_file: base.join("calibration.json.tmp"),
            active_offset: 0,
            hot_ticks: 0,
            cool_ticks: 0,
            slow_cooler_persistent: false,
            consecutive_cool_sessions: 0,
        };
        manager.load_persistent_state();

        // Cleanup old split file if exists
        let old_offset = base.join("calibration_offset.json");
        if old_offset.exists() {
            let _ = fs::remove_file(old_offset);
        }

        manager
    }

    fn load_persistent_state(&mut self) {
        if let Ok(content) = fs::read_to_string(&self.calibration_file)
            && let Ok(state) = serde_json::from_str::<CalibrationState>(&content)
        {
            self.active_offset = state.active_offset.clamp(-6, 6); // Ensure limits
            self.hot_ticks = state.hot_ticks;
            self.cool_ticks = state.cool_ticks;
            self.slow_cooler_persistent = state.slow_cooler_persistent;
            self.consecutive_cool_sessions = state.consecutive_cool_sessions;
            info!("Loaded calibration offset: {}C", self.active_offset);
        }
    }

    pub fn save_state(&self) -> Result<()> {
        let state = CalibrationState {
            active_offset: self.active_offset,
            hot_ticks: self.hot_ticks,
            cool_ticks: self.cool_ticks,
            slow_cooler_persistent: self.slow_cooler_persistent,
            consecutive_cool_sessions: self.consecutive_cool_sessions,
        };

        if let Ok(content) = serde_json::to_string_pretty(&state) {
            fs::write(&self.temp_file, content)?;
            fs::rename(&self.temp_file, &self.calibration_file)?;
        }
        Ok(())
    }

    pub fn evaluate_post_game_cooling(&mut self, peak_temp: i32, current_temp: i32) {
        if peak_temp > 45 {
            let drop = peak_temp - current_temp;
            if drop < 1 {
                // Poor cooling
                self.slow_cooler_persistent = true;
                self.consecutive_cool_sessions = 0;
                info!("Marked as persistent slow cooler (drop was {}C).", drop);
                let _ = self.save_state();
            } else if drop >= 3 {
                // Good cooling
                self.consecutive_cool_sessions += 1;
                if self.consecutive_cool_sessions >= 3 {
                    self.slow_cooler_persistent = false;
                    self.consecutive_cool_sessions = 0;
                    info!("Cleared persistent slow cooler flag.");
                    let _ = self.save_state();
                }
            }
            // If drop is between 1 and 3, it's neutral, do nothing.
        }
    }
    pub fn apply_calibration(&mut self, is_heating_or_flat: bool) {
        let mut changed = false;

        if is_heating_or_flat {
            self.hot_ticks += 1;
            self.cool_ticks = 0; // Reset cool ticks

            // Shift offset after sustained heat
            if self.hot_ticks > 60 {
                if self.active_offset > -6 {
                    self.active_offset -= 1;
                    info!(
                        "Calibration adjustment triggered. New offset: {}",
                        self.active_offset
                    );
                    changed = true;
                }
                self.hot_ticks = 0; // Reset after adjustment
            }
        } else {
            if self.hot_ticks > 0 {
                self.hot_ticks -= 1;
            }

            // Apply gradual decay if we have a negative offset and aren't heating
            if self.active_offset < 0 {
                self.cool_ticks += 1;

                // Decay slowly back to 0
                if self.cool_ticks > 120 {
                    self.active_offset += 1;
                    info!(
                        "Calibration decay triggered. New offset: {}",
                        self.active_offset
                    );
                    self.cool_ticks = 0;
                    changed = true;
                }
            } else {
                self.cool_ticks = 0;
            }
        }

        // Also ensure positive bounds (e.g. if we decay too far, though we only +=1 if < 0 above).
        self.active_offset = self.active_offset.clamp(-6, 6);

        if changed {
            let _ = self.save_state();
        }
    }
}
