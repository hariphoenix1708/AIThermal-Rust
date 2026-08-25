//! Network optimizer — WiFi power-save disable, UDP/TCP buffer tuning,
//! and socket prioritization during gaming.
//!
//! All values saved for restoration on game exit.

use std::collections::HashMap;
use std::fs;

/// Candidate paths for WiFi power-save sysfs knobs (varies by kernel/driver).
const WIFI_PS_PATHS: &[&str] = &[
    "/sys/class/net/wlan0/power_save",
    "/sys/bus/platform/drivers WLAN(power_save)",
    "/sys/kernel/debug/ieee80211/phy0/power_save",
];

/// /proc/sys/net tunables to boost for gaming network performance.
/// (path, gaming_value, description)
const GAMING_NET_TUNABLES: &[(&str, &str, &str)] = &[
    // Default buffers: raise baseline so new sockets inherit larger buffers.
    // NOTE: rmem_max/wmem_max are intentionally NOT tuned here — the shell
    // script already sets optimal values via the ROM-specific path. Tuning
    // them in the daemon would conflict and potentially downgrade the system
    // values (e.g. from 16MB to 256KB).
    (
        "/proc/sys/net/core/rmem_default",
        "262144",
        "UDP/TCP default receive buffer",
    ),
    (
        "/proc/sys/net/core/wmem_default",
        "262144",
        "UDP/TCP default send buffer",
    ),
];

pub struct NetworkState {
    wifi_ps_original: Option<String>,
    wifi_ps_path: Option<String>,
    /// sysfs path -> original value (for all tunables we modify).
    saved_tunables: HashMap<String, String>,
}

impl NetworkState {
    pub fn new() -> Self {
        Self {
            wifi_ps_original: None,
            wifi_ps_path: None,
            saved_tunables: HashMap::new(),
        }
    }

    /// Disable WiFi power-save to reduce TX latency.
    pub fn activate_wifi_ps(&mut self) {
        for path in WIFI_PS_PATHS {
            if !std::path::Path::new(path).exists() {
                continue;
            }
            if let Ok(orig) = fs::read_to_string(path) {
                let orig = orig.trim().to_string();
                if orig == "0" {
                    return; // Already disabled.
                }
                if write_file(path, "0") {
                    self.wifi_ps_original = Some(orig);
                    self.wifi_ps_path = Some(path.to_string());
                    tracing::info!(
                        target: "game_turbo",
                        "WiFi PS: {} -> 0 (disabled for gaming)",
                        path
                    );
                } else {
                    tracing::debug!(
                        target: "game_turbo",
                        "WiFi PS: write to {} failed, not tracking",
                        path
                    );
                }
                return;
            }
        }
        tracing::debug!(target: "game_turbo", "WiFi PS: no writable power_save node found");
    }

    /// Restore WiFi power-save.
    pub fn deactivate_wifi_ps(&mut self) {
        if let (Some(path), Some(orig)) = (&self.wifi_ps_path, &self.wifi_ps_original)
            && write_file(path, orig)
        {
            tracing::info!(
                target: "game_turbo",
                "WiFi PS: {} -> {} (restored)",
                path, orig
            );
        }
        self.wifi_ps_original = None;
        self.wifi_ps_path = None;
    }

    /// Tune UDP/TCP buffers for gaming network performance.
    pub fn activate_buffers(&mut self) {
        for &(path, value, desc) in GAMING_NET_TUNABLES {
            if !std::path::Path::new(path).exists() {
                continue;
            }
            // Save original only once per path.
            if !self.saved_tunables.contains_key(path) {
                if let Ok(orig) = fs::read_to_string(path) {
                    self.saved_tunables
                        .insert(path.to_string(), orig.trim().to_string());
                } else {
                    continue;
                }
            }

            if write_file(path, value) {
                tracing::debug!(
                    target: "game_turbo",
                    "NET-BUF {} {} -> {} ({})",
                    path,
                    self.saved_tunables.get(path).unwrap_or(&"?".to_string()),
                    value,
                    desc
                );
            }
        }

        if !self.saved_tunables.is_empty() {
            tracing::info!(
                target: "game_turbo",
                "Network buffers: tuned {} sysctl knobs for gaming",
                self.saved_tunables.len()
            );
        }
    }

    /// Restore all saved network tunables.
    pub fn deactivate_buffers(&mut self) {
        for (path, orig) in &self.saved_tunables {
            if write_file(path, orig) {
                tracing::debug!(
                    target: "game_turbo",
                    "NET-BUF {} -> {} (restored)",
                    path, orig
                );
            }
        }

        if !self.saved_tunables.is_empty() {
            tracing::info!(
                target: "game_turbo",
                "Network buffers: restored {} sysctl knobs",
                self.saved_tunables.len()
            );
        }
        self.saved_tunables.clear();
    }
}

fn write_file(path: &str, value: &str) -> bool {
    use std::io::Write;
    match fs::OpenOptions::new().write(true).truncate(true).open(path) {
        Ok(mut file) => {
            if file.write_all(value.trim().as_bytes()).is_ok() && file.flush().is_ok() {
                true
            } else {
                let err = std::io::Error::last_os_error();
                tracing::debug!(target: "game_turbo", "Write failed {}: {}", path, err);
                false
            }
        }
        Err(e) => {
            tracing::debug!(target: "game_turbo", "Cannot open {}: {}", path, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_state_new() {
        let state = NetworkState::new();
        assert!(state.wifi_ps_original.is_none());
        assert!(state.wifi_ps_path.is_none());
        assert!(state.saved_tunables.is_empty());
    }
}
