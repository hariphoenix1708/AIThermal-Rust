//! Battery-Aware FPS Cap — dynamically limit max FPS based on battery level,
//! thermal state, and charging status to balance performance and battery life.

use std::fs;

/// Sysfs path for SurfaceFlinger max FPS (varies by device/Android version).
/// On Android 14+, SurfaceFlinger supports max_frame_rate via SurfaceFlinger properties.
/// We'll use the standard approach via setprop or SurfaceFlinger transaction.
const SF_MAX_FPS_PROP: &str = "debug.sf.max_fps";

/// Battery sysfs paths.
const BATTERY_CAPACITY_PATH: &str = "/sys/class/power_supply/battery/capacity";
const BATTERY_STATUS_PATH: &str = "/sys/class/power_supply/battery/status";

/// Thermal zone path for CPU temp (composite temp used for FPS decisions).
/// Scan for cpu_therm/quiet_therm dynamically — thermal_zone0 is not always CPU on peridot.
fn thermal_zone_path() -> String {
    if let Ok(entries) = std::fs::read_dir("/sys/class/thermal") {
        for e in entries.flatten() {
            let p = e.path();
            let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
            let ty = ty.trim();
            if ty == "cpu_therm" || ty == "quiet_therm" || ty == "soc_thermal" {
                let temp = p.join("temp");
                if temp.exists() {
                    return temp.to_string_lossy().to_string();
                }
            }
        }
    }
    "/sys/class/thermal/thermal_zone0/temp".to_string()
}

/// Battery thresholds for FPS scaling.
const BATTERY_CRITICAL: u32 = 15;  // Below this: 60 FPS cap
const BATTERY_LOW: u32 = 30; // Below this: 90 FPS cap

/// Thermal thresholds for FPS scaling — tuned for Low+Ultra120 Ranked.
/// At 120Hz budget is 8.3ms; keep 120 until 48C (was 45) to avoid Ranked stutter.
const THERMAL_WARM: u32 = 48;      // Above this: start reducing FPS (Ultra 120)
const THERMAL_HOT: u32 = 50;       // Above this: aggressive FPS reduction

pub struct FpsCapManager {
    available: bool,
    original_max_fps: Option<u32>,
    current_cap: Option<u32>,
}

impl FpsCapManager {
    pub fn new() -> Self {
        // Check if SurfaceFlinger property approach works.
        // In practice, we use `setprop` or write to /sys/class/graphics/fb0/...
        // For now, mark as available and try at runtime.
        let available = true;
        Self {
            available,
            original_max_fps: None,
            current_cap: None,
        }
    }

    /// Activate: read current FPS cap and set battery/thermal-aware cap.
    pub fn activate(&mut self) {
        if !self.available {
            return;
        }

        // Read battery info directly from sysfs.
        let battery_percent = Self::read_battery_percent();
        let is_charging = Self::read_charging_status();
        let thermal_temp = Self::read_thermal_temp();

        // Read current max FPS from property.
        if let Ok(output) = std::process::Command::new("getprop")
            .arg(SF_MAX_FPS_PROP)
            .output()
            && output.status.success() {
                let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !val.is_empty() && val != "0" {
                    self.original_max_fps = val.parse().ok();
                }
            }

        let cap = self.calculate_cap(battery_percent, is_charging, thermal_temp);
        self.apply_cap(cap);
        self.current_cap = Some(cap);
    }

    /// Per-tick: adjust FPS cap based on battery/thermal.
    pub fn tick(&mut self) {
        if !self.available {
            return;
        }

        // Read battery info directly from sysfs.
        let battery_percent = Self::read_battery_percent();
        let is_charging = Self::read_charging_status();
        let thermal_temp = Self::read_thermal_temp();

        let cap = self.calculate_cap(battery_percent, is_charging, thermal_temp);
        if self.current_cap != Some(cap) {
            self.apply_cap(cap);
            self.current_cap = Some(cap);
            tracing::debug!(
                target: "game_turbo",
                "FPS Cap: adjusted to {} (bat={}%, charging={}, temp={}°C)",
                cap, battery_percent, is_charging, thermal_temp
            );
        }
    }

    /// Read battery percentage from sysfs.
    fn read_battery_percent() -> u32 {
        fs::read_to_string(BATTERY_CAPACITY_PATH)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(100)
    }

    /// Read charging status from sysfs.
    fn read_charging_status() -> bool {
        fs::read_to_string(BATTERY_STATUS_PATH)
            .ok()
            .map(|s| {
                let status = s.trim().to_lowercase();
                status == "charging" || status == "full"
            })
            .unwrap_or(false)
    }

    /// Read thermal temperature from sysfs (millidegrees C -> degrees C).
    fn read_thermal_temp() -> u32 {
        fs::read_to_string(thermal_zone_path())
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map(|millideg| millideg / 1000) // Convert millidegrees to degrees
            .unwrap_or(40) // Default to 40°C if unavailable
    }

    /// Calculate target FPS cap based on battery, charging, and thermal.
    fn calculate_cap(&self, battery: u32, charging: bool, thermal: u32) -> u32 {
        // If charging and not hot, allow max FPS.
        if charging && thermal < THERMAL_HOT {
            return 120; // Max supported
        }

        // Base cap from battery level.
        let battery_cap = if battery <= BATTERY_CRITICAL {
            60
        } else if battery <= BATTERY_LOW {
            90
        } else {
            120
        };

        // Thermal reduction.
        let thermal_cap = if thermal >= THERMAL_HOT {
            60
        } else if thermal >= THERMAL_WARM {
            90
        } else {
            120
        };

        // Take the minimum (most restrictive).
        battery_cap.min(thermal_cap)
    }

    /// Apply FPS cap via SurfaceFlinger property.
    fn apply_cap(&self, cap: u32) {
        // Use setprop to set SurfaceFlinger max FPS.
        // Note: This requires appropriate SELinux permissions.
        let _ = std::process::Command::new("setprop")
            .args([SF_MAX_FPS_PROP, &cap.to_string()])
            .output();

        tracing::info!(
            target: "game_turbo",
            "FPS Cap: set max_fps to {}",
            cap
        );
    }

    pub fn current_cap(&self) -> Option<u32> {
        self.current_cap
    }

    /// Deactivate: restore original FPS cap.
    pub fn deactivate(&mut self) {
        if !self.available {
            return;
        }

        if let Some(original) = self.original_max_fps.take() {
            let _ = std::process::Command::new("setprop")
                .args([SF_MAX_FPS_PROP, &original.to_string()])
                .output();
            tracing::info!(
                target: "game_turbo",
                "FPS Cap: restored max_fps to {}",
                original
            );
        } else {
            // Reset to 0 (unlimited).
            let _ = std::process::Command::new("setprop")
                .args([SF_MAX_FPS_PROP, "0"])
                .output();
            tracing::info!(
                target: "game_turbo",
                "FPS Cap: reset to unlimited"
            );
        }
        self.current_cap = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fps_cap_new() {
        let mgr = FpsCapManager::new();
        let _ = mgr.available;
    }

    #[test]
    fn test_calculate_cap() {
        let mgr = FpsCapManager::new();
        // Charging + cool = max
        assert_eq!(mgr.calculate_cap(50, true, 35), 120);
        // Low battery = 60
        assert_eq!(mgr.calculate_cap(10, false, 35), 60);
        // Hot thermal = 60
        assert_eq!(mgr.calculate_cap(80, false, 55), 60);
        // Normal = 120
        assert_eq!(mgr.calculate_cap(60, false, 35), 120);
    }
}