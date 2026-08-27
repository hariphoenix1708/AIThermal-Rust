//! CPU Idle Control — disable deep C-states during gaming to reduce
//! wake latency and prevent frame pacing jitter.
//!
//! On modern ARM SoCs (SM8635), the CPU idle driver exposes C-states via
//! `/sys/devices/system/cpu/cpu*/cpuidle/state*/disable`. C0=active, C1=WFI,
//! C2=retention, C3+=power collapse. Disabling C3+ during gaming ensures
//! big cores wake fast enough for render thread deadlines.

use std::fs;
use std::path::Path;

const CPU_IDLE_BASE: &str = "/sys/devices/system/cpu";

/// C-state index threshold: disable states >= this value (0=C0, 1=C1, 2=C2, 3=C3...).
/// Keep C0-C2 enabled (active, WFI, retention), disable C3+ (power collapse).
const MAX_ENABLED_CSTATE: u32 = 2;

pub struct CpuIdleControl {
    /// Track which states we disabled for restoration.
    disabled_states: Vec<(String, String)>, // (path, original_value)
    available: bool,
}

impl CpuIdleControl {
    pub fn new() -> Self {
        // Check if cpuidle interface exists.
        let available = Path::new("/sys/devices/system/cpu/cpu0/cpuidle").exists();
        Self {
            disabled_states: Vec::new(),
            available,
        }
    }

    /// Activate: disable deep C-states (C3+) on all cores.
    pub fn activate(&mut self) {
        if !self.available {
            return;
        }

        let mut disabled = 0;
        if let Ok(entries) = fs::read_dir(CPU_IDLE_BASE) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("cpu") && name[3..].chars().all(|c| c.is_ascii_digit()) {
                    let cpu_idle_dir = entry.path().join("cpuidle");
                    if cpu_idle_dir.exists() {
                        disabled += self.disable_cpu_states(&cpu_idle_dir);
                    }
                }
            }
        }

        if disabled > 0 {
            tracing::info!(
                target: "game_turbo",
                "CPU Idle: disabled {} deep C-states (C3+) on all cores",
                disabled
            );
        }
    }

    /// Deactivate: restore all C-states to original values.
    pub fn deactivate(&mut self) {
        if !self.available {
            return;
        }

        let mut restored = 0;
        for (path, original) in self.disabled_states.drain(..) {
            if let Err(e) = fs::write(&path, original.as_bytes()) {
                tracing::warn!(
                    target: "game_turbo",
                    "CPU Idle: failed to restore {}: {}",
                    path, e
                );
            } else {
                restored += 1;
            }
        }

        if restored > 0 {
            tracing::info!(
                target: "game_turbo",
                "CPU Idle: restored {} C-states",
                restored
            );
        }
    }

    fn disable_cpu_states(&mut self, cpu_idle_dir: &Path) -> u32 {
        let mut count = 0;
        if let Ok(entries) = fs::read_dir(cpu_idle_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("state") && name[5..].chars().all(|c| c.is_ascii_digit()) {
                    let state_idx: u32 = name[5..].parse().unwrap_or(99);
                    if state_idx > MAX_ENABLED_CSTATE {
                        let disable_path = entry.path().join("disable");
                        if disable_path.exists() {
                            // Read original value for restoration.
                            let original = fs::read_to_string(&disable_path)
                                .map(|s| s.trim().to_string())
                                .unwrap_or_else(|_| "0".to_string());

                            // Only disable if not already disabled.
                            if original == "0" {
                                if fs::write(&disable_path, b"1").is_ok() {
                                    self.disabled_states.push((disable_path.to_string_lossy().to_string(), original));
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_idle_new() {
        let ctrl = CpuIdleControl::new();
        // Just verify it constructs
        let _ = ctrl.available;
    }
}