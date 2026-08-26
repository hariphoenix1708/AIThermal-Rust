use super::profile::NetworkProfile;
use std::fs;

pub fn probe_network() -> NetworkProfile {
    let mut profile = NetworkProfile::default();

    if let Ok(content) = fs::read_to_string("/proc/sys/net/core/default_qdisc") {
        profile.default_qdisc = content.trim().to_string();
    }

    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_congestion_control") {
        profile.tcp_congestion_control = content.trim().to_string();
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_available_congestion_control") {
        profile.available_congestion_controls =
            content.split_whitespace().map(|s| s.to_string()).collect();
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_ecn") {
        profile.ecn_enabled = Some(content.trim() != "0");
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_fastopen") {
        profile.fast_open = Some(content.trim().to_string());
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_mtu_probing") {
        profile.mtu_probing = content.trim().parse::<u32>().ok();
    }

    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_keepalive_time") {
        profile.tcp_keepalive_time = content.trim().parse::<u32>().ok();
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_syn_retries") {
        profile.tcp_syn_retries = content.trim().parse::<u32>().ok();
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_synack_retries") {
        profile.tcp_synack_retries = content.trim().parse::<u32>().ok();
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_window_scaling") {
        profile.tcp_window_scaling = content.trim().parse::<u32>().ok();
    }
    if let Ok(content) = fs::read_to_string("/proc/sys/net/ipv4/tcp_timestamps") {
        profile.tcp_timestamps = content.trim().parse::<u32>().ok();
    }

    profile
}

use std::sync::Mutex;
use std::sync::OnceLock;

static ACTIVE_INTERFACE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// Check if an interface is active: operstate=="up" OR carrier=="1".
/// v3.3.9: Added carrier check — rmnet interfaces on SM8635 report
/// operstate="unknown" even when the link is functionally up.
fn iface_is_active(iface: &str) -> bool {
    let operstate = format!("/sys/class/net/{}/operstate", iface);
    let carrier = format!("/sys/class/net/{}/carrier", iface);
    if let Ok(content) = fs::read_to_string(&operstate)
        && content.trim() == "up"
    {
        return true;
    }
    if let Ok(content) = fs::read_to_string(&carrier)
        && content.trim() == "1"
    {
        return true;
    }
    false
}

#[allow(clippy::collapsible_if)]
pub fn read_wifi_active() -> bool {
    let cache = ACTIVE_INTERFACE.get_or_init(|| Mutex::new(None));

    // 1. Check cached interface first if it exists
    if let Ok(mut locked_cache) = cache.lock() {
        if let Some(iface) = locked_cache.as_ref() {
            if iface_is_active(iface) {
                return true;
            }
            // Interface is no longer up, clear cache
            *locked_cache = None;
        }

        // 2. Check prioritized interfaces
        let priorities = ["wlan0", "rmnet_data0"];
        for iface in priorities {
            if iface_is_active(iface) {
                *locked_cache = Some(iface.to_string());
                return true;
            }
        }

        // 3. Fallback: Check any other interface (excluding lo)
        if let Ok(entries) = fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if name == "lo" || priorities.contains(&name.as_str()) {
                        continue;
                    }
                    if iface_is_active(&name) {
                        *locked_cache = Some(name);
                        return true;
                    }
                }
            }
        }
    }

    false
}
