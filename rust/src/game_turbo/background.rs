//! Background CPU lockdown — clamp non-game cgroups via UCLAMP_MAX
//! during gaming to reduce CPU contention for the game process.
//!
//! Writes `uclamp.max` to cgroup CPU controllers. Values are saved
//! for restoration on game exit. Degrades silently if the kernel
//! doesn't expose uclamp knobs.

use std::collections::HashMap;
use std::fs;

/// 50% of max capacity — enough for background services to function
/// but not enough to steal cycles from the game.
const BG_UCLAMP_MAX: &str = "512";
const BG_UCLAMP_MIN: &str = "0";
const UCLAMP_MAX_DEFAULT: &str = "max";

/// Fallback value if the kernel uses 0-100 range instead of 0-1024.
const BG_UCLAMP_MAX_PCT: &str = "50";

const CGROUP_PATHS: &[&str] = &[
    "/dev/cpuctl/background",
    "/dev/cpuctl/sys-background",
    "/sys/fs/cgroup/cpu/background",
    "/sys/fs/cgroup/cpu/system-background",
];

pub struct BackgroundState {
    /// path -> saved original uclamp.max value.
    saved_uclamp_max: HashMap<String, String>,
}

impl BackgroundState {
    pub fn new() -> Self {
        Self {
            saved_uclamp_max: HashMap::new(),
        }
    }

    /// Activate: clamp all discovered background cgroups.
    pub fn activate(&mut self, _game_pid: u32) {
        let mut clamped = 0u32;
        let mut skipped = 0u32;

        for base in CGROUP_PATHS {
            let uclamp_max_path = format!("{}/cpu.uclamp.max", base);
            let uclamp_min_path = format!("{}/cpu.uclamp.min", base);

            if !std::path::Path::new(&uclamp_max_path).exists() {
                continue;
            }

            // Read the current value before writing — needed for restoration
            // and to diagnose ERANGE issues.
            let orig_val = fs::read_to_string(&uclamp_max_path)
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| UCLAMP_MAX_DEFAULT.to_string());

            self.saved_uclamp_max
                .insert(uclamp_max_path.clone(), orig_val.clone());

            // If current value is "max" (unlimited), a numerical clamp may not
            // be supported by this kernel. Try anyway but handle ERANGE.
            let wrote = if write_str(&uclamp_max_path, BG_UCLAMP_MAX) {
                tracing::debug!(
                    target: "game_turbo",
                    "Background lockdown: {} '{}' -> {}",
                    uclamp_max_path, orig_val, BG_UCLAMP_MAX
                );
                true
            } else if write_str(&uclamp_max_path, BG_UCLAMP_MAX_PCT) {
                tracing::debug!(
                    target: "game_turbo",
                    "Background lockdown: {} '{}' -> {} (pct fallback)",
                    uclamp_max_path, orig_val, BG_UCLAMP_MAX_PCT
                );
                true
            } else {
                false
            };

            if wrote {
                clamped += 1;
                // Also set min to 0 to allow full dynamic range down to zero.
                let _ = write_str(&uclamp_min_path, BG_UCLAMP_MIN);
            } else {
                // Kernel rejected both values — don't count this cgroup.
                // The uclamp controller may not be available or the kernel
                // uses a non-standard range.
                tracing::warn!(
                    target: "game_turbo",
                    "Background lockdown: {} current='{}' — kernel rejected both {} and {}. Skipping.",
                    uclamp_max_path, orig_val, BG_UCLAMP_MAX, BG_UCLAMP_MAX_PCT
                );
                skipped += 1;
                // Remove from saved map since we didn't actually change it.
                self.saved_uclamp_max.remove(&uclamp_max_path);
            }
        }

        if clamped > 0 || skipped > 0 {
            let mut msg = format!("Background lockdown: clamped {} cgroups", clamped);
            if skipped > 0 {
                msg.push_str(&format!(", skipped {} (kernel rejected uclamp writes)", skipped));
            }
            tracing::info!(target: "game_turbo", "{}", msg);
        }
    }

    /// Restore all saved uclamp values.
    pub fn deactivate(&mut self) {
        let mut restored = 0u32;
        let mut failed = 0u32;
        for (path, orig_val) in &self.saved_uclamp_max {
            let ok = write_str(path, orig_val) || write_str(path, "max");
            if ok {
                restored += 1;
            } else {
                failed += 1;
                tracing::warn!(
                    target: "game_turbo",
                    "Background lockdown: failed to restore {} to '{}' — cgroup may remain clamped",
                    path, orig_val
                );
            }
        }
        tracing::info!(
            target: "game_turbo",
            "Background lockdown: restored {} cgroups{}",
            restored,
            if failed > 0 { format!(", {} failed to restore", failed) } else { String::new() }
        );
        self.saved_uclamp_max.clear();
    }
}

fn write_str(path: &str, value: &str) -> bool {
    use std::io::Write;
    match fs::OpenOptions::new().write(true).truncate(true).open(path) {
        Ok(mut file) => {
            if file.write_all(value.as_bytes()).is_ok() && file.flush().is_ok() {
                true
            } else {
                let err = std::io::Error::last_os_error();
                tracing::debug!(
                    target: "game_turbo",
                    "Write failed for {}: {}",
                    path, err
                );
                false
            }
        }
        Err(e) => {
            tracing::debug!(
                target: "game_turbo",
                "Cannot open {}: {}",
                path, e
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_background_state_new() {
        let state = BackgroundState::new();
        assert!(state.saved_uclamp_max.is_empty());
    }
}
