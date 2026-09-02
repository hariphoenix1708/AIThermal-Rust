//! GPU Busy Hints via KGSL — keep GPU clocks up and bus active during gaming.
//!
//! On Qualcomm Adreno (SM8635), KGSL exposes sysfs interfaces to control
//! GPU power management:
//! - `idle_timer`: ms before GPU enters idle (lower = more responsive)
//! - `force_bus_on`: keep GPU bus active (1=on)
//! - `force_clk_on`: keep GPU clock active (1=on)
//! - `rt_bus_hint`: real-time bus hint for priority bandwidth
//! - `gpu_busy_percentage`: read current GPU utilization

use std::fs;
use std::path::Path;

const KGSL_BASE: &str = "/sys/class/kgsl/kgsl-3d0";

/// Default idle timer (ms) - kernel default is typically 100-200ms.
const DEFAULT_IDLE_TIMER: &str = "100";
/// Gaming idle timer (ms) - keep GPU responsive.
const GAMING_IDLE_TIMER: &str = "10";

pub struct GpuBusyHints {
    available: bool,
    saved_idle_timer: Option<String>,
    saved_force_bus_on: Option<String>,
    saved_force_clk_on: Option<String>,
    saved_rt_bus_hint: Option<String>,
}

impl GpuBusyHints {
    pub fn new() -> Self {
        let available = Path::new(KGSL_BASE).exists();
        Self {
            available,
            saved_idle_timer: None,
            saved_force_bus_on: None,
            saved_force_clk_on: None,
            saved_rt_bus_hint: None,
        }
    }

    /// Activate: set gaming-friendly GPU power management.
    pub fn activate(&mut self) {
        if !self.available {
            return;
        }

        // Save original values.
        self.saved_idle_timer = read_sysfs("idle_timer");
        self.saved_force_bus_on = read_sysfs("force_bus_on");
        self.saved_force_clk_on = read_sysfs("force_clk_on");
        self.saved_rt_bus_hint = read_sysfs("rt_bus_hint");

        // Set gaming values.
        write_sysfs("idle_timer", GAMING_IDLE_TIMER);
        write_sysfs("force_bus_on", "1");
        write_sysfs("force_clk_on", "1");
        write_sysfs("rt_bus_hint", "1"); // Request RT bus priority

        tracing::info!(
            target: "game_turbo",
            "GPU Hints: idle_timer={}ms, force_bus_on=1, force_clk_on=1, rt_bus_hint=1",
            GAMING_IDLE_TIMER
        );
    }

    /// Per-tick: could adjust idle_timer based on GPU load.
    pub fn tick(&mut self, gpu_load: u32) {
        if !self.available {
            return;
        }

        // Adjust idle timer based on GPU load:
        // - High load (>70%): very aggressive (5ms)
        // - Medium load (30-70%): standard gaming (10ms)
        // - Low load (<30%): moderate (20ms)
        let target = if gpu_load > 70 {
            "5"
        } else if gpu_load > 30 {
            "10"
        } else {
            "20"
        };

        // Only write if different from current.
        if let Ok(current) = fs::read_to_string(format!("{}/idle_timer", KGSL_BASE))
            && current.trim() != target {
                write_sysfs("idle_timer", target);
                tracing::debug!(
                    target: "game_turbo",
                    "GPU Hints: idle_timer adjusted to {}ms (gpu_load={}%)",
                    target, gpu_load
                );
            }
    }

    /// Deactivate: restore original values.
    pub fn deactivate(&mut self) {
        if !self.available {
            return;
        }

        if let Some(v) = self.saved_idle_timer.take() {
            write_sysfs("idle_timer", &v);
        } else {
            write_sysfs("idle_timer", DEFAULT_IDLE_TIMER);
        }
        if let Some(v) = self.saved_force_bus_on.take() {
            write_sysfs("force_bus_on", &v);
        } else {
            write_sysfs("force_bus_on", "0");
        }
        if let Some(v) = self.saved_force_clk_on.take() {
            write_sysfs("force_clk_on", &v);
        } else {
            write_sysfs("force_clk_on", "0");
        }
        if let Some(v) = self.saved_rt_bus_hint.take() {
            write_sysfs("rt_bus_hint", &v);
        } else {
            write_sysfs("rt_bus_hint", "0");
        }

        tracing::info!(
            target: "game_turbo",
            "GPU Hints: restored idle_timer={}, force_bus_on={}, force_clk_on={}, rt_bus_hint={}",
            self.saved_idle_timer.as_deref().unwrap_or(DEFAULT_IDLE_TIMER),
            self.saved_force_bus_on.as_deref().unwrap_or("0"),
            self.saved_force_clk_on.as_deref().unwrap_or("0"),
            self.saved_rt_bus_hint.as_deref().unwrap_or("0")
        );
    }
}

fn read_sysfs(name: &str) -> Option<String> {
    fs::read_to_string(format!("{}/{}", KGSL_BASE, name))
        .map(|s| s.trim().to_string())
        .ok()
}

fn write_sysfs(name: &str, value: &str) {
    let _ = fs::write(format!("{}/{}", KGSL_BASE, name), value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_hints_new() {
        let hints = GpuBusyHints::new();
        let _ = hints.available;
    }
}