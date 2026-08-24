//! Network optimizer — WiFi power-save disable and buffer tuning
//! during gaming to reduce latency and packet loss.
//!
//! WiFi PS is a sysfs toggle. All values saved for restoration.

use std::fs;

/// Candidate paths for WiFi power-save sysfs knobs (varies by kernel/driver).
const WIFI_PS_PATHS: &[&str] = &[
    "/sys/class/net/wlan0/power_save",
    "/sys/bus/platform/drivers WLAN(power_save)",
    "/sys/kernel/debug/ieee80211/phy0/power_save",
];

pub struct NetworkState {
    wifi_ps_original: Option<String>,
    wifi_ps_path: Option<String>,
}

impl NetworkState {
    pub fn new() -> Self {
        Self {
            wifi_ps_original: None,
            wifi_ps_path: None,
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
                self.wifi_ps_original = Some(orig);
                self.wifi_ps_path = Some(path.to_string());
                if write_file(path, "0") {
                    tracing::info!(
                        target: "game_turbo",
                        "WiFi PS: {} -> 0 (disabled for gaming)",
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
    }
}
