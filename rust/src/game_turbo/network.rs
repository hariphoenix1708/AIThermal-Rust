//! Network optimizer — WiFi power-save disable, RPS steering, UDP/TCP buffer
//! tuning, and socket prioritization during gaming.
//!
//! All values saved for restoration on game exit.

use std::collections::HashMap;
use std::fs;

/// Candidate paths for WiFi power-save sysfs knobs (varies by kernel/driver).
/// v3.3.7: Removed bogus `/sys/bus/platform/drivers WLAN(power_save)` path.
const WIFI_PS_PATHS: &[&str] = &[
    "/sys/class/net/wlan0/power_save",
    "/sys/kernel/debug/ieee80211/phy0/power_save",
];

/// /proc/sys/net tunables to boost for gaming network performance.
/// (path, gaming_value, description)
const GAMING_NET_TUNABLES: &[(&str, &str, &str)] = &[
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
    saved_tunables: HashMap<String, String>,
    rps_saved: HashMap<String, String>,
    wifi_ps_cmd_used: bool,
    rps_flow_original: Option<String>,
}

impl NetworkState {
    pub fn new() -> Self {
        Self {
            wifi_ps_original: None,
            wifi_ps_path: None,
            saved_tunables: HashMap::new(),
            rps_saved: HashMap::new(),
            wifi_ps_cmd_used: false,
            rps_flow_original: None,
        }
    }

    /// Disable WiFi power-save to reduce TX latency.
    pub fn activate_wifi_ps(&mut self) {
        // v3.3.7: On Qualcomm WCN6750, /sys/class/net/wlan0/power_save doesn't exist.
        // Use Android framework command to set low-latency mode which disables PS.
        if std::process::Command::new("su")
            .args(["-c", "cmd wifi force-low-latency-mode enabled"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            self.wifi_ps_cmd_used = true;
            tracing::info!(
                target: "game_turbo",
                "WiFi PS: disabled via cmd wifi force-low-latency-mode"
            );
            return;
        }

        for path in WIFI_PS_PATHS {
            if !std::path::Path::new(path).exists() {
                continue;
            }
            if let Ok(orig) = fs::read_to_string(path) {
                let orig = orig.trim().to_string();
                if orig == "0" {
                    return;
                }
                if write_file(path, "0") {
                    self.wifi_ps_original = Some(orig);
                    self.wifi_ps_path = Some(path.to_string());
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
        if self.wifi_ps_cmd_used {
            let _ = std::process::Command::new("su")
                .args(["-c", "cmd wifi force-low-latency-mode disabled"])
                .output();
            tracing::info!(
                target: "game_turbo",
                "WiFi PS: restored via cmd wifi force-low-latency-mode"
            );
            self.wifi_ps_cmd_used = false;
            return;
        }
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

    /// v3.3.8: Enable RPS (Receive Packet Steering) on all active network
    /// interfaces. On SM8635, all WLAN and modem interrupts land on CPU0 —
    /// without RPS this causes softirq storms and ping spikes.
    /// v3.3.8: Also enables RPS on mobile data (rmnet) interfaces.
    pub fn activate_rps(&mut self) {
        let cpus_all = "ff";
        let mut ifaces: Vec<String> = Vec::new();

        // Discover active interfaces: wlan0 + all up rmnet_data*
        for name in &["wlan0"] {
            if std::path::Path::new(&format!("/sys/class/net/{}/operstate", name)).exists() {
                ifaces.push(name.to_string());
            }
        }
        // Also check all rmnet_data* interfaces for mobile data gaming
        if let Ok(entries) = fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("rmnet_data") {
                    let operstate = format!("/sys/class/net/{}/operstate", name);
                    if std::path::Path::new(&operstate).exists() {
                        ifaces.push(name);
                    }
                }
            }
        }

        let mut total_rps_cpus = 0usize;

        for iface in &ifaces {
            let rx_queues_dir = format!("/sys/class/net/{}/queues", iface);

            // Pass 1: Set rps_cpus on each rx queue
            if let Ok(entries) = fs::read_dir(&rx_queues_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if !name_str.starts_with("rx-") {
                        continue;
                    }
                    let rps_path = format!("{}/{}/rps_cpus", rx_queues_dir, name_str);
                    if !std::path::Path::new(&rps_path).exists() {
                        continue;
                    }
                    if !self.rps_saved.contains_key(&rps_path)
                        && let Ok(orig) = fs::read_to_string(&rps_path)
                    {
                        self.rps_saved
                            .insert(rps_path.clone(), orig.trim().to_string());
                    }
                    if write_file(&rps_path, cpus_all) {
                        total_rps_cpus += 1;
                        tracing::debug!(target: "game_turbo", "RPS {} -> {}", rps_path, cpus_all);
                    }
                }
            }

            // Pass 2: Set rps_flow_cnt on each rx queue
            if let Ok(entries) = fs::read_dir(&rx_queues_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if !name_str.starts_with("rx-") {
                        continue;
                    }
                    let flow_cnt_path = format!("{}/{}/rps_flow_cnt", rx_queues_dir, name_str);
                    if std::path::Path::new(&flow_cnt_path).exists() {
                        if !self.rps_saved.contains_key(&flow_cnt_path)
                            && let Ok(orig) = fs::read_to_string(&flow_cnt_path)
                        {
                            self.rps_saved
                                .insert(flow_cnt_path.clone(), orig.trim().to_string());
                        }
                        write_file(&flow_cnt_path, "4096");
                    }
                }
            }
        }

        // Set global RPS sock flow entries
        let flow_path = "/proc/sys/net/core/rps_sock_flow_entries";
        if std::path::Path::new(flow_path).exists() {
            if let Ok(orig) = fs::read_to_string(flow_path) {
                self.rps_flow_original = Some(orig.trim().to_string());
            }
            write_file(flow_path, "32768");
        }

        if !self.rps_saved.is_empty() {
            tracing::info!(
                target: "game_turbo",
                "RPS: enabled across {} interfaces ({} rx queues, cpus={})",
                ifaces.len(), total_rps_cpus, cpus_all
            );
        }
    }

    /// Restore RPS to original values.
    pub fn deactivate_rps(&mut self) {
        for (path, orig) in &self.rps_saved {
            write_file(path, orig);
        }
        if let Some(orig) = &self.rps_flow_original {
            write_file("/proc/sys/net/core/rps_sock_flow_entries", orig);
        }
        if !self.rps_saved.is_empty() {
            tracing::info!(
                target: "game_turbo",
                "RPS: restored {} sysfs nodes",
                self.rps_saved.len()
            );
        }
        self.rps_saved.clear();
        self.rps_flow_original = None;
    }

    /// Tune UDP/TCP buffers for gaming network performance.
    pub fn activate_buffers(&mut self) {
        for &(path, value, desc) in GAMING_NET_TUNABLES {
            if !std::path::Path::new(path).exists() {
                continue;
            }
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
            write_file(path, orig);
            tracing::debug!(
                target: "game_turbo",
                "NET-BUF {} -> {} (restored)",
                path, orig
            );
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
        assert!(state.rps_saved.is_empty());
        assert!(!state.wifi_ps_cmd_used);
        assert!(state.rps_flow_original.is_none());
    }
}
