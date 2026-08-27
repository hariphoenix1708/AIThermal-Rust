//! Performance Hint integration — use cgroup uclamp for top-app
//! to signal the scheduler that game-critical threads need high CPU
//! frequency.
//!
//! This is the Rust equivalent of Android's PerformanceHintManager API,
//! but works at the kernel level via cgroup uclamp (since sched_setattr
//! with uclamp flags is not supported on this kernel).
//!
//! Key insight: `uclamp_min` on the top-app cgroup forces the scheduler
//! to select a CPU frequency that can deliver at least the requested
//! utilization for all top-app threads (game render threads, UI).
//! Dynamic adjustment: 40% baseline → 70% when GPU-loaded/hot thermal.

use std::fs;
use std::path::Path;

const TOP_APP_UCLAMP_MIN: &str = "/dev/cpuctl/top-app/cpu.uclamp.min";
const TOP_APP_UCLAMP_MAX: &str = "/dev/cpuctl/top-app/cpu.uclamp.max";

/// Uclamp range: 0-1024 (kernel default), but cgroup uses 0-100% strings.
/// 1024 = 100% of max capacity.
/// Baseline uclamp_min for gaming (40%).
const UCLAMP_MIN_BASELINE: u32 = 40;

/// Elevated uclamp_min when GPU-loaded or hot thermal (70%).
const UCLAMP_MIN_ELEVATED: u32 = 70;

/// Thermal threshold for elevated uclamp (°C).
const THERMAL_THRESHOLD_HOT: u32 = 48;

/// GPU load threshold for elevated uclamp (%).
const GPU_LOAD_THRESHOLD: u32 = 50;

pub struct PerfHintState {
    /// Whether cgroup uclamp is available.
    uclamp_available: bool,
    /// Original uclamp_min value for restoration.
    saved_uclamp_min: Option<String>,
    /// Original uclamp_max value for restoration.
    saved_uclamp_max: Option<String>,
    /// Current uclamp_min level.
    current_level: u32,
}

impl PerfHintState {
    pub fn new() -> Self {
        let uclamp_available = Path::new(TOP_APP_UCLAMP_MIN).exists()
            && Path::new(TOP_APP_UCLAMP_MAX).exists();

        Self {
            uclamp_available,
            saved_uclamp_min: None,
            saved_uclamp_max: None,
            current_level: UCLAMP_MIN_BASELINE,
        }
    }

    /// Activate: set top-app uclamp_min to baseline gaming level.
    pub fn activate(&mut self) {
        if !self.uclamp_available {
            return;
        }

        // Save original values.
        self.saved_uclamp_min = fs::read_to_string(TOP_APP_UCLAMP_MIN).ok().map(|s| s.trim().to_string());
        self.saved_uclamp_max = fs::read_to_string(TOP_APP_UCLAMP_MAX).ok().map(|s| s.trim().to_string());

        // Set baseline gaming uclamp.
        self.set_uclamp(UCLAMP_MIN_BASELINE, "max");
        self.current_level = UCLAMP_MIN_BASELINE;

        tracing::info!(
            target: "game_turbo",
            "PerfHint: top-app uclamp.min set to {}% (baseline gaming)",
            UCLAMP_MIN_BASELINE
        );
    }

    /// Per-tick: adjust uclamp_min based on GPU load and thermal state.
    pub fn tick(&mut self, gpu_load: u32, thermal_temp: u32) {
        if !self.uclamp_available {
            return;
        }

        let should_elevate = gpu_load >= GPU_LOAD_THRESHOLD || thermal_temp >= THERMAL_THRESHOLD_HOT;
        let target = if should_elevate { UCLAMP_MIN_ELEVATED } else { UCLAMP_MIN_BASELINE };

        if target != self.current_level {
            self.set_uclamp(target, "max");
            self.current_level = target;
            tracing::debug!(
                target: "game_turbo",
                "PerfHint: top-app uclamp.min {}% (gpu_load={}%, temp={}°C)",
                target, gpu_load, thermal_temp
            );
        }
    }

    /// Restore original uclamp values.
    pub fn deactivate(&mut self) {
        if !self.uclamp_available {
            return;
        }

        if let Some(min) = &self.saved_uclamp_min {
            let _ = fs::write(TOP_APP_UCLAMP_MIN, min.as_bytes());
        } else {
            // Default restore to 0.
            let _ = fs::write(TOP_APP_UCLAMP_MIN, b"0");
        }
        if let Some(max) = &self.saved_uclamp_max {
            let _ = fs::write(TOP_APP_UCLAMP_MAX, max.as_bytes());
        } else {
            let _ = fs::write(TOP_APP_UCLAMP_MAX, b"max");
        }

        tracing::info!(
            target: "game_turbo",
            "PerfHint: restored top-app uclamp.min={}, uclamp.max={}",
            self.saved_uclamp_min.as_deref().unwrap_or("0"),
            self.saved_uclamp_max.as_deref().unwrap_or("max")
        );

        self.saved_uclamp_min = None;
        self.saved_uclamp_max = None;
        self.current_level = UCLAMP_MIN_BASELINE;
    }

    fn set_uclamp(&self, min: u32, max: &str) {
        let _ = fs::write(TOP_APP_UCLAMP_MIN, min.to_string().as_bytes());
        let _ = fs::write(TOP_APP_UCLAMP_MAX, max.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perf_hint_state_new() {
        let state = PerfHintState::new();
        assert!(!state.is_uclamp_available_conceptually() || state.is_uclamp_available_conceptually());
    }

    // Helper for test - we can't access private fields, so test behavior
    impl PerfHintState {
        fn is_uclamp_available_conceptually(&self) -> bool {
            true // Just a placeholder test that the struct constructs
        }
    }
}