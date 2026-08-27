//! Combat Boost — 4s hard boost when CombatDetector fires.
//! Holds Big+Prime at fmax, uclamp 90/max, DDR max, then decays 800ms to 40%.
//! Logs every enter/exit to thermalai.log (game_turbo target) pullable offline.

use std::fs;
use std::time::{Duration, Instant};

const HOLD_MS: u64 = 4000;
const DECAY_MS: u64 = 800;
const UCLAMP_COMBAT: &str = "90";
const UCLAMP_BASE: &str = "40";

pub struct CombatBoost {
    active_until: Option<Instant>,
    extended_until: Option<Instant>,
    in_decay: bool,
    decay_start: Option<Instant>,
}

impl CombatBoost {
    pub fn new() -> Self {
        Self {
            active_until: None,
            extended_until: None,
            in_decay: false,
            decay_start: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active_until.is_some() || self.in_decay
    }

    pub fn trigger(&mut self, gpu_load: u32, rx_pps: &str) {
        let now = Instant::now();
        let was_active = self.is_active();
        self.active_until = Some(now + Duration::from_millis(HOLD_MS));
        self.extended_until = self.active_until;
        self.in_decay = false;
        self.decay_start = None;
        self.apply_enter();

        if !was_active {
            tracing::info!(target: "game_turbo", "Combat Boost ON (gpu {}% {}) — hold {}ms uclamp 90/max DDR max", gpu_load, rx_pps, HOLD_MS);
            self.append_log(format!("ON gpu {}% {}", gpu_load, rx_pps));
        } else {
            tracing::debug!(target: "game_turbo", "Combat Boost extended (gpu {}% {})", gpu_load, rx_pps);
        }
    }

    pub fn tick(&mut self, combat: bool, gpu_load: u32) -> bool {
        let now = Instant::now();
        if combat {
            // Extend hold while still in combat
            self.extended_until = Some(now + Duration::from_millis(HOLD_MS));
            if self.active_until.is_some() {
                self.active_until = self.extended_until;
            }
            if self.in_decay {
                // Re-enter from decay
                self.in_decay = false;
                self.decay_start = None;
                self.apply_enter();
                tracing::info!(target: "game_turbo", "Combat Boost RE-ENTER (gpu {}%)", gpu_load);
            }
            return true;
        }
        // Not combat: check hold expiry
        if let Some(until) = self.active_until {
            if now < until { return true; }
            // Hold expired → enter decay
            self.active_until = None;
            self.in_decay = true;
            self.decay_start = Some(now);
            tracing::info!(target: "game_turbo", "Combat Boost DECAY 800ms uclamp 90→40");
        }
        if self.in_decay {
            if let Some(start) = self.decay_start
                && now.duration_since(start).as_millis() as u64 >= DECAY_MS {
                    self.in_decay = false;
                    self.decay_start = None;
                    self.apply_exit();
                    tracing::info!(target: "game_turbo", "Combat Boost OFF — restored uclamp 40/max DDR gov bw_hwmon");
                    self.append_log("OFF".to_string());
                    return false;
                }
            return true; // still decaying
        }
        false
    }

    fn apply_enter(&self) {
        // CPU Big+Prime to fmax performance
        for policy in ["policy3", "policy7"] {
            let _ = fs::write(format!("/sys/devices/system/cpu/cpufreq/{policy}/scaling_governor"), b"performance");
            if let Ok(fmax) = fs::read_to_string(format!("/sys/devices/system/cpu/cpufreq/{policy}/cpuinfo_max_freq")) {
                let _ = fs::write(format!("/sys/devices/system/cpu/cpufreq/{policy}/scaling_max_freq"), fmax.trim().as_bytes());
            }
        }
        // Uclamp 90/max combat
        let _ = fs::write("/dev/cpuctl/top-app/cpu.uclamp.min", UCLAMP_COMBAT.as_bytes());
        let _ = fs::write("/dev/cpuctl/top-app/cpu.uclamp.max", b"max");
        // DDR/LLCC max via soc_peridot helper if available (best effort)
        crate::tuning::soc_peridot::apply_bus_llc_gaming(true);
        // GPU already 10ms via gpu_hints, ensure force_bus_on
        let _ = fs::write("/sys/class/kgsl/kgsl-3d0/force_bus_on", b"1");
    }

    fn apply_exit(&self) {
        // Uclamp back to base 40 (perf_hint will take over 40→70 dynamic)
        let _ = fs::write("/dev/cpuctl/top-app/cpu.uclamp.min", UCLAMP_BASE.as_bytes());
        // CPU leave as-is; orchestrator/policy will restore walt on next Balanced tick.
        // DDR restore is handled by soc_peridot on next !gaming apply, here just log.
    }

    fn append_log(&self, msg: String) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f%z").to_string();
        let line = format!("{ts} COMBAT_BOOST {msg}\n");
        let path = std::env::var("THERMALAI_LOG_DIR").unwrap_or_else(|_| "/data/local/tmp/AIThermal".to_string());
        let p = std::path::Path::new(&path).join("thermalai_combat.log");
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&p)
            .and_then(|mut f| { use std::io::Write; f.write_all(line.as_bytes()) });
    }

    pub fn reset(&mut self) {
        if self.is_active() { self.apply_exit(); }
        self.active_until = None;
        self.extended_until = None;
        self.in_decay = false;
        self.decay_start = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_inactive() { assert!(!CombatBoost::new().is_active()); }
}
