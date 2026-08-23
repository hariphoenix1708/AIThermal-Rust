// Network diagnostics for gaming — active quality measurement.
//
// Reads the JSON reports produced by the shell scripts and provides
// a scored NetworkQuality struct to the orchestrator. Also does
// lightweight passive checks (interface state, buffer sizes, power-save).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// ROM type detected at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RomType {
    HyperOS,
    Aosp,
}

impl RomType {
    pub fn detect() -> Self {
        let brand = std::process::Command::new("getprop")
            .arg("ro.product.brand")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
            .trim()
            .to_lowercase();

        let os_ver = std::process::Command::new("getprop")
            .arg("ro.mi.os.version.incremental")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
            .trim()
            .to_string();

        let miui_ver = std::process::Command::new("getprop")
            .arg("ro.miui.ui.version.name")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
            .trim()
            .to_string();

        if (brand.contains("xiaomi") || brand.contains("poco") || brand.contains("redmi"))
            && (!os_ver.is_empty() || !miui_ver.is_empty())
        {
            return Self::HyperOS;
        }
        Self::Aosp
    }
}

/// Quality rating for bullet registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BulletRegQuality {
    Excellent,
    Good,
    Fair,
    Poor,
    Bad,
    Unknown,
}

impl BulletRegQuality {
    pub fn from_jitter_ms(jitter: f64, avg_rtt: f64, loss_pct: f64) -> Self {
        if jitter <= 5.0 && avg_rtt <= 80.0 && loss_pct == 0.0 {
            Self::Excellent
        } else if jitter <= 10.0 && avg_rtt <= 120.0 && loss_pct <= 2.0 {
            Self::Good
        } else if jitter <= 15.0 && avg_rtt <= 150.0 {
            Self::Fair
        } else if jitter <= 25.0 {
            Self::Poor
        } else {
            Self::Bad
        }
    }

    pub fn score(&self) -> u32 {
        match self {
            Self::Excellent => 100,
            Self::Good => 80,
            Self::Fair => 60,
            Self::Poor => 40,
            Self::Bad => 20,
            Self::Unknown => 50,
        }
    }
}

/// Jitter quality tier (S+/S/A/B/C/D).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JitterTier {
    SPlus,
    S,
    A,
    B,
    C,
    D,
}

impl JitterTier {
    pub fn from_jitter_ms(jitter: f64) -> Self {
        let j = jitter as i64;
        if j <= 3 {
            Self::SPlus
        } else if j <= 5 {
            Self::S
        } else if j <= 8 {
            Self::A
        } else if j <= 12 {
            Self::B
        } else if j <= 20 {
            Self::C
        } else {
            Self::D
        }
    }
}

/// Passive network state read from sysfs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PassiveNetworkState {
    pub interface: String,
    pub network_type: String,
    pub wifi_power_save: Option<String>,
    pub wifi_rssi: Option<String>,
    pub wifi_freq: Option<String>,
    pub tcp_rmem: Option<String>,
    pub tcp_wmem: Option<String>,
    pub udp_mem: Option<String>,
    pub rmem_max: Option<u64>,
    pub wmem_max: Option<u64>,
    pub busy_poll: Option<u64>,
    pub netdev_backlog: Option<u64>,
    pub fastopen: Option<String>,
    pub tcp_keepalive: Option<u32>,
    pub tx_queue_len: Option<u64>,
}

/// Active quality measurement from shell script reports.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActiveQuality {
    pub avg_rtt_ms: f64,
    pub min_rtt_ms: f64,
    pub max_rtt_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
    pub dns_resolution_ms: u64,
}

/// Combined network quality assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkQuality {
    pub timestamp: u64,
    pub rom: RomType,
    pub passive: PassiveNetworkState,
    pub active: Option<ActiveQuality>,
    pub bullet_reg: BulletRegQuality,
    pub jitter_tier: JitterTier,
    pub quality_score: u32,
}

/// Cached quality state for the daemon to read without re-probing.
static QUALITY_CACHE: OnceLock<Mutex<Option<NetworkQuality>>> = OnceLock::new();

/// Read the network quality report produced by detect_network_quality.sh.
pub fn read_quality_report(state_dir: &str) -> Option<NetworkQuality> {
    let report_path = Path::new(state_dir).join("network_quality.json");
    let content = fs::read_to_string(report_path).ok()?;
    let raw: serde_json::Value = serde_json::from_str(&content).ok()?;

    let timestamp = raw["timestamp"].as_u64().unwrap_or(0);

    let rom = match raw["rom"].as_str().unwrap_or("aosp") {
        "hyperos" => RomType::HyperOS,
        _ => RomType::Aosp,
    };

    let active = Some(ActiveQuality {
        avg_rtt_ms: raw["ping"]["google_dns"]["avg_ms"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        min_rtt_ms: raw["ping"]["google_dns"]["min_ms"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        max_rtt_ms: raw["ping"]["google_dns"]["max_ms"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        jitter_ms: raw["ping"]["google_dns"]["jitter_ms"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        loss_pct: raw["ping"]["google_dns"]["loss_pct"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        dns_resolution_ms: raw["dns_resolution_ms"].as_u64().unwrap_or(0),
    });

    let a = active.as_ref().unwrap();
    let bullet_reg = BulletRegQuality::from_jitter_ms(a.jitter_ms, a.avg_rtt_ms, a.loss_pct);
    let jitter_tier = JitterTier::from_jitter_ms(a.jitter_ms);
    let quality_score = raw["quality_score"].as_u64().unwrap_or(50) as u32;

    Some(NetworkQuality {
        timestamp,
        rom,
        passive: PassiveNetworkState::default(),
        active,
        bullet_reg,
        jitter_tier,
        quality_score,
    })
}

/// Read passive network state from sysfs (cheap, no network I/O).
pub fn probe_passive() -> PassiveNetworkState {
    let mut state = PassiveNetworkState::default();

    // Detect active interface
    for iface in &["wlan0", "rmnet_data0"] {
        let path = format!("/sys/class/net/{}/operstate", iface);
        if let Ok(content) = fs::read_to_string(&path)
            && content.trim() == "up"
        {
            state.interface = iface.to_string();
            state.network_type = if iface.starts_with("wlan") {
                "wifi".to_string()
            } else {
                "mobile".to_string()
            };
            break;
        }
    }

    if state.interface.is_empty() {
        // Fallback: find any up interface
        if let Ok(entries) = fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name == "lo" {
                    continue;
                }
                let path = entry.path().join("operstate");
                if let Ok(content) = fs::read_to_string(&path)
                    && content.trim() == "up"
                {
                    state.interface = name.to_string();
                    state.network_type = if name.starts_with("wlan") {
                        "wifi".to_string()
                    } else if name.starts_with("rmnet") || name.starts_with("ccmni") {
                        "mobile".to_string()
                    } else {
                        "other".to_string()
                    };
                    break;
                }
            }
        }
    }

    // WiFi-specific
    if state.network_type == "wifi" {
        state.wifi_rssi = fs::read_to_string("/sys/class/net/wlan0/wireless/link/level")
            .ok()
            .map(|s| s.trim().to_string());
        state.wifi_freq = fs::read_to_string("/sys/class/net/wlan0/wireless/freq")
            .ok()
            .map(|s| s.trim().to_string());
        state.wifi_power_save = fs::read_to_string("/sys/class/net/wlan0/power_save")
            .ok()
            .map(|s| s.trim().to_string());
    }

    // TCP/UDP buffers
    state.tcp_rmem = fs::read_to_string("/proc/sys/net/ipv4/tcp_rmem")
        .ok()
        .map(|s| s.trim().to_string());
    state.tcp_wmem = fs::read_to_string("/proc/sys/net/ipv4/tcp_wmem")
        .ok()
        .map(|s| s.trim().to_string());
    state.udp_mem = fs::read_to_string("/proc/sys/net/ipv4/udp_mem")
        .ok()
        .map(|s| s.trim().to_string());
    state.rmem_max = fs::read_to_string("/proc/sys/net/core/rmem_max")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    state.wmem_max = fs::read_to_string("/proc/sys/net/core/wmem_max")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    state.busy_poll = fs::read_to_string("/proc/sys/net/core/busy_poll")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    state.netdev_backlog = fs::read_to_string("/proc/sys/net/core/netdev_max_backlog")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    state.fastopen = fs::read_to_string("/proc/sys/net/ipv4/tcp_fastopen")
        .ok()
        .map(|s| s.trim().to_string());
    state.tcp_keepalive = fs::read_to_string("/proc/sys/net/ipv4/tcp_keepalive_time")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    state.tx_queue_len = fs::read_to_string(format!(
        "/sys/class/net/{}/tx_queue_len",
        if state.interface.is_empty() {
            "wlan0"
        } else {
            &state.interface
        }
    ))
    .ok()
    .and_then(|s| s.trim().parse().ok());

    state
}

/// Run full quality probe: passive sysfs + read shell script report.
pub fn probe_quality(state_dir: &str) -> NetworkQuality {
    let passive = probe_passive();
    let rom = RomType::detect();

    // Try to read the active report if it exists
    if let Some(mut quality) = read_quality_report(state_dir) {
        quality.passive = passive;
        quality.rom = rom;
        return quality;
    }

    // No active report: synthesize from passive data
    NetworkQuality {
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        rom,
        passive,
        active: None,
        bullet_reg: BulletRegQuality::Unknown,
        jitter_tier: JitterTier::B,
        quality_score: 50,
    }
}

/// Cache the latest quality reading for the orchestrator to read.
pub fn cache_quality(quality: NetworkQuality) {
    let cache = QUALITY_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut locked) = cache.lock() {
        *locked = Some(quality);
    }
}

/// Get the cached quality (returns None if never probed).
pub fn cached_quality() -> Option<NetworkQuality> {
    let cache = QUALITY_CACHE.get_or_init(|| Mutex::new(None));
    cache.lock().ok().and_then(|locked| locked.clone())
}

/// Determine if gaming network tweaks should be applied based on quality.
pub fn should_apply_gaming_tweaks(quality: &NetworkQuality) -> bool {
    // Always apply tweaks if quality is poor or unknown
    match quality.bullet_reg {
        BulletRegQuality::Bad | BulletRegQuality::Poor | BulletRegQuality::Unknown => true,
        BulletRegQuality::Fair => {
            // Apply if jitter is borderline and we're on WiFi (more tweakable)
            quality.passive.network_type == "wifi" || quality.jitter_tier as u8 >= JitterTier::B as u8
        }
        BulletRegQuality::Good | BulletRegQuality::Excellent => {
            // Still apply WiFi power save disable and buffer tweaks
            // (they help even on good connections)
            true
        }
    }
}

/// Get a human-readable summary of the quality assessment.
pub fn quality_summary(q: &NetworkQuality) -> String {
    let active_part = if let Some(ref a) = q.active {
        format!(
            "RTT={:.1}ms jitter={:.1}ms loss={:.1}%",
            a.avg_rtt_ms, a.jitter_ms, a.loss_pct
        )
    } else {
        "no active measurement".to_string()
    };

    format!(
        "[{}] rom={:?} net={} {} bullet_reg={:?} jitter={:?} score={}",
        q.passive.interface,
        q.rom,
        q.passive.network_type,
        active_part,
        q.bullet_reg,
        q.jitter_tier,
        q.quality_score,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bullet_reg_quality() {
        assert_eq!(
            BulletRegQuality::from_jitter_ms(3.0, 40.0, 0.0),
            BulletRegQuality::Excellent
        );
        assert_eq!(
            BulletRegQuality::from_jitter_ms(7.0, 80.0, 0.0),
            BulletRegQuality::Good
        );
        assert_eq!(
            BulletRegQuality::from_jitter_ms(12.0, 100.0, 0.0),
            BulletRegQuality::Fair
        );
        assert_eq!(
            BulletRegQuality::from_jitter_ms(20.0, 200.0, 0.0),
            BulletRegQuality::Poor
        );
        assert_eq!(
            BulletRegQuality::from_jitter_ms(30.0, 300.0, 10.0),
            BulletRegQuality::Bad
        );
    }

    #[test]
    fn test_jitter_tier() {
        assert_eq!(JitterTier::from_jitter_ms(2.0), JitterTier::SPlus);
        assert_eq!(JitterTier::from_jitter_ms(4.0), JitterTier::S);
        assert_eq!(JitterTier::from_jitter_ms(6.0), JitterTier::A);
        assert_eq!(JitterTier::from_jitter_ms(10.0), JitterTier::B);
        assert_eq!(JitterTier::from_jitter_ms(15.0), JitterTier::C);
        assert_eq!(JitterTier::from_jitter_ms(25.0), JitterTier::D);
    }

    #[test]
    fn test_quality_score() {
        assert_eq!(BulletRegQuality::Excellent.score(), 100);
        assert_eq!(BulletRegQuality::Good.score(), 80);
        assert_eq!(BulletRegQuality::Fair.score(), 60);
        assert_eq!(BulletRegQuality::Poor.score(), 40);
        assert_eq!(BulletRegQuality::Bad.score(), 20);
        assert_eq!(BulletRegQuality::Unknown.score(), 50);
    }

    #[test]
    fn test_should_apply_gaming_tweaks() {
        let mut quality = NetworkQuality {
            timestamp: 0,
            rom: RomType::Aosp,
            passive: PassiveNetworkState::default(),
            active: None,
            bullet_reg: BulletRegQuality::Unknown,
            jitter_tier: JitterTier::B,
            quality_score: 50,
        };
        assert!(should_apply_gaming_tweaks(&quality));

        quality.bullet_reg = BulletRegQuality::Excellent;
        assert!(should_apply_gaming_tweaks(&quality));
    }

    #[test]
    fn test_quality_summary() {
        let quality = NetworkQuality {
            timestamp: 0,
            rom: RomType::HyperOS,
            passive: PassiveNetworkState {
                interface: "wlan0".to_string(),
                network_type: "wifi".to_string(),
                ..Default::default()
            },
            active: Some(ActiveQuality {
                avg_rtt_ms: 45.0,
                min_rtt_ms: 30.0,
                max_rtt_ms: 80.0,
                jitter_ms: 5.0,
                loss_pct: 0.0,
                dns_resolution_ms: 25,
            }),
            bullet_reg: BulletRegQuality::Good,
            jitter_tier: JitterTier::S,
            quality_score: 85,
        };
        let summary = quality_summary(&quality);
        assert!(summary.contains("wlan0"));
        assert!(summary.contains("RTT=45.0ms"));
        assert!(summary.contains("HyperOS"));
    }

    #[test]
    fn test_probe_passive_runs() {
        // This runs on any system; just verify it doesn't panic
        let state = probe_passive();
        // On a dev machine we won't have wlan0 but the function should still work
        assert!(state.network_type.is_empty() || !state.network_type.is_empty());
    }

    #[test]
    fn test_cache_quality_roundtrip() {
        let quality = NetworkQuality {
            timestamp: 12345,
            rom: RomType::Aosp,
            passive: PassiveNetworkState::default(),
            active: None,
            bullet_reg: BulletRegQuality::Fair,
            jitter_tier: JitterTier::B,
            quality_score: 60,
        };
        cache_quality(quality.clone());
        let cached = cached_quality().unwrap();
        assert_eq!(cached.timestamp, 12345);
        assert_eq!(cached.bullet_reg, BulletRegQuality::Fair);
    }
}
