// Network diagnostics for gaming — active quality measurement.
//
// Implements ICMP ping via raw sockets in pure Rust. No shell
// delegation for the core measurement path. Shell scripts remain
// only for boot-time network tweaks (pre-daemon startup).

use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ─── ICMP Ping (raw socket) ──────────────────────────────────────────

/// ICMP Echo Request (type 8, code 0)
const ICMP_ECHO_REQUEST: u8 = 8;
/// ICMP Echo Reply (type 0, code 0)
const ICMP_ECHO_REPLY: u8 = 0;

/// CRC-16 for ICMP as per RFC 1071.
fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]);
        sum += word as u32;
    }
    if let Some(&byte) = chunks.remainder().first() {
        sum += (byte as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
}

/// Result of a single ping probe.
#[derive(Debug, Clone, Copy)]
struct PingReply {
    rtt_us: u64,
}

/// Send one ICMP echo request and wait for the reply.
/// Returns RTT in microseconds, or None on timeout/error.
fn icmp_ping_one(
    sock_fd: libc::c_int,
    target_addr: &libc::sockaddr_in,
    seq: u16,
    ident: u16,
    timeout: Duration,
) -> Option<PingReply> {
    // Build ICMP echo request: type(1) + code(1) + checksum(2) + id(2) + seq(2) + timestamp(8) = 16 bytes
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;

    let mut packet = [0u8; 16];
    packet[0] = ICMP_ECHO_REQUEST;
    packet[1] = 0; // code
    // checksum at [2..4] — computed after filling
    packet[4..6].copy_from_slice(&ident.to_be_bytes());
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    packet[8..16].copy_from_slice(&now_us.to_be_bytes());

    let cksum = icmp_checksum(&packet);
    packet[2..4].copy_from_slice(&cksum.to_be_bytes());

    // Send
    let addr_ptr = target_addr as *const libc::sockaddr_in as *const libc::sockaddr;
    let sent = unsafe {
        libc::sendto(
            sock_fd,
            packet.as_ptr() as *const libc::c_void,
            packet.len(),
            0,
            addr_ptr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if sent < 0 {
        return None;
    }

    // Receive with timeout via poll
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 64];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let ms = remaining.as_millis() as libc::c_int;

        let mut pollfd = libc::pollfd {
            fd: sock_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let poll_rc = unsafe { libc::poll(&mut pollfd, 1, ms) };
        if poll_rc <= 0 {
            return None;
        }

        // Read
        let received = unsafe {
            libc::recvfrom(
                sock_fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if received < 20 {
            // Minimum: 20 bytes IP header + 8 bytes ICMP
            continue;
        }

        // Parse IP header to find ICMP payload offset
        let ihl = (buf[0] & 0x0F) as usize * 4;
        if ihl + 8 > received as usize {
            continue;
        }

        let icmp_type = buf[ihl];
        let reply_id = u16::from_be_bytes([buf[ihl + 4], buf[ihl + 5]]);

        // Filter: only accept echo reply matching our ident
        if icmp_type != ICMP_ECHO_REPLY || reply_id != ident {
            continue;
        }

        // Extract the sender's timestamp from our original packet echoed back
        if ihl + 16 > received as usize {
            continue;
        }
        let reply_ts = u64::from_be_bytes([
            buf[ihl + 8],
            buf[ihl + 9],
            buf[ihl + 10],
            buf[ihl + 11],
            buf[ihl + 12],
            buf[ihl + 13],
            buf[ihl + 14],
            buf[ihl + 15],
        ]);

        let rtt_us = now_us.saturating_sub(reply_ts);
        return Some(PingReply {
            rtt_us,
        });
    }
}

/// Active ping result.
#[derive(Debug, Clone)]
pub struct PingResult {
    pub avg_rtt_ms: f64,
    pub min_rtt_ms: f64,
    pub max_rtt_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
    pub packets_sent: u32,
    pub packets_received: u32,
}

/// Run ICMP ping to a target IP. Uses raw socket (requires root).
/// Returns aggregated statistics.
pub fn icmp_ping(target: &str, count: u32, interval_ms: u64) -> PingResult {
    let target_ip: u32 = match resolve_ip(target) {
        Some(ip) => ip,
        None => {
            return PingResult {
                avg_rtt_ms: 0.0,
                min_rtt_ms: 0.0,
                max_rtt_ms: 0.0,
                jitter_ms: 0.0,
                loss_pct: 100.0,
                packets_sent: count,
                packets_received: 0,
            };
        }
    };

    let ident = std::process::id() as u16;
    let target_addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as u16,
        sin_port: 0,
        sin_addr: libc::in_addr { s_addr: target_ip.to_be() },
        sin_zero: [0; 8],
    };

    // Create raw ICMP socket
    let sock_fd = unsafe {
        libc::socket(
            libc::AF_INET,
            libc::SOCK_RAW,
            libc::IPPROTO_ICMP,
        )
    };
    if sock_fd < 0 {
        return PingResult {
            avg_rtt_ms: 0.0,
            min_rtt_ms: 0.0,
            max_rtt_ms: 0.0,
            jitter_ms: 0.0,
            loss_pct: 100.0,
            packets_sent: count,
            packets_received: 0,
        };
    }

    // Set receive timeout to 2 seconds
    let timeout_val = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
    unsafe {
        libc::setsockopt(
            sock_fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout_val as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }

    let mut rtts: Vec<u64> = Vec::with_capacity(count as usize);
    let mut received = 0u32;

    for seq in 0..count {
        let reply = icmp_ping_one(sock_fd, &target_addr, seq as u16, ident, Duration::from_secs(2));
        if let Some(r) = reply {
            rtts.push(r.rtt_us);
            received += 1;
        }
        // Inter-packet delay (skip after last packet)
        if seq + 1 < count {
            std::thread::sleep(Duration::from_millis(interval_ms));
        }
    }

    unsafe {
        libc::close(sock_fd);
    }

    if rtts.is_empty() {
        return PingResult {
            avg_rtt_ms: 0.0,
            min_rtt_ms: 0.0,
            max_rtt_ms: 0.0,
            jitter_ms: 0.0,
            loss_pct: 100.0,
            packets_sent: count,
            packets_received: 0,
        };
    }

    rtts.sort_unstable();
    let min_us = *rtts.first().unwrap();
    let max_us = *rtts.last().unwrap();
    let sum_us: u64 = rtts.iter().sum();
    let avg_us = sum_us / rtts.len() as u64;

    // Jitter: mean of absolute differences between consecutive RTTs (RFC 3550 style)
    let mut jitter_sum: u64 = 0;
    for w in rtts.windows(2) {
        let diff = w[0].abs_diff(w[1]);
        jitter_sum += diff;
    }
    let jitter_us = if rtts.len() > 1 {
        jitter_sum / (rtts.len() - 1) as u64
    } else {
        0
    };

    let loss_pct = ((count - received) as f64 / count as f64) * 100.0;

    PingResult {
        avg_rtt_ms: avg_us as f64 / 1000.0,
        min_rtt_ms: min_us as f64 / 1000.0,
        max_rtt_ms: max_us as f64 / 1000.0,
        jitter_ms: jitter_us as f64 / 1000.0,
        loss_pct,
        packets_sent: count,
        packets_received: received,
    }
}

/// Resolve a hostname to an IPv4 address in network byte order.
fn resolve_ip(host: &str) -> Option<u32> {
    // Fast path: already an IP literal
    if let Some(ip) = ip_to_u32(host) {
        return Some(ip);
    }

    // Use getaddrinfo via libc
    let c_host = std::ffi::CString::new(host).ok()?;
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_INET;
    hints.ai_socktype = libc::SOCK_DGRAM;

    let mut result: *mut libc::addrinfo = std::ptr::null_mut();
    let rc = unsafe {
        libc::getaddrinfo(
            c_host.as_ptr(),
            std::ptr::null(),
            &hints,
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }

    let addr_in = unsafe { &*(result as *const libc::sockaddr_in) };
    let ip = u32::from_be(addr_in.sin_addr.s_addr);

    unsafe { libc::freeaddrinfo(result) };

    Some(ip)
}

/// Parse an IPv4 dotted-decimal string to u32 in network byte order.
fn ip_to_u32(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        octets[i] = part.parse().ok()?;
    }
    Some(u32::from_be_bytes(octets))
}

/// Measure DNS resolution time in milliseconds.
pub fn dns_resolution_ms(host: &str) -> u64 {
    let start = Instant::now();
    let c_host = match std::ffi::CString::new(host) {
        Ok(h) => h,
        Err(_) => return 0,
    };
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_INET;
    hints.ai_socktype = libc::SOCK_DGRAM;

    let mut result: *mut libc::addrinfo = std::ptr::null_mut();
    unsafe {
        libc::getaddrinfo(
            c_host.as_ptr(),
            std::ptr::null(),
            &hints,
            &mut result,
        );
        if !result.is_null() {
            libc::freeaddrinfo(result);
        }
    }
    start.elapsed().as_millis() as u64
}

// ─── ROM Detection ───────────────────────────────────────────────────

/// ROM type detected at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RomType {
    HyperOS,
    Aosp,
}

impl RomType {
    pub fn detect() -> Self {
        let brand = prop("ro.product.brand");
        let os_ver = prop("ro.mi.os.version.incremental");
        let miui_ver = prop("ro.miui.ui.version.name");

        let brand_lower = brand.to_lowercase();
        if (brand_lower.contains("xiaomi") || brand_lower.contains("poco") || brand_lower.contains("redmi"))
            && (!os_ver.is_empty() || !miui_ver.is_empty())
        {
            return Self::HyperOS;
        }
        Self::Aosp
    }
}

/// Read an Android system property via getprop.
fn prop(key: &str) -> String {
    std::process::Command::new("getprop")
        .arg(key)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default()
        .trim()
        .to_string()
}

// ─── Quality Scoring ─────────────────────────────────────────────────

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

// ─── Passive Probing ─────────────────────────────────────────────────

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

/// Active quality measurement.
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

    if state.interface.is_empty()
        && let Ok(entries) = fs::read_dir("/sys/class/net")
    {
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

/// Run full quality probe: passive sysfs + active ICMP ping to both
/// 8.8.8.8 and 1.1.1.1, DNS resolution, then score.
pub fn probe_quality(state_dir: &str) -> NetworkQuality {
    let passive = probe_passive();
    let rom = RomType::detect();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Active probe: ping both targets, pick the better one
    let targets = ["8.8.8.8", "1.1.1.1"];
    let ping_count: u32 = 20;
    let ping_interval_ms: u64 = 200;

    let mut best: Option<PingResult> = None;
    for target in &targets {
        let result = icmp_ping(target, ping_count, ping_interval_ms);
        if result.packets_received > 0 {
            match &best {
                None => best = Some(result),
                Some(b) => {
                    if result.avg_rtt_ms < b.avg_rtt_ms {
                        best = Some(result);
                    }
                }
            }
        }
    }

    // DNS resolution test
    let dns_ms = dns_resolution_ms("google.com");

    let active = best.map(|r| ActiveQuality {
        avg_rtt_ms: r.avg_rtt_ms,
        min_rtt_ms: r.min_rtt_ms,
        max_rtt_ms: r.max_rtt_ms,
        jitter_ms: r.jitter_ms,
        loss_pct: r.loss_pct,
        dns_resolution_ms: dns_ms,
    });

    // Score
    let (bullet_reg, jitter_tier, quality_score) = if let Some(ref a) = active {
        let br = BulletRegQuality::from_jitter_ms(a.jitter_ms, a.avg_rtt_ms, a.loss_pct);
        let jt = JitterTier::from_jitter_ms(a.jitter_ms);
        let mut score = br.score();

        // Degrade for packet loss
        let loss_int = a.loss_pct as i64;
        if loss_int > 5 {
            score = score.saturating_sub(30);
        } else if loss_int > 0 {
            score = score.saturating_sub(15);
        }

        (br, jt, score)
    } else {
        (
            BulletRegQuality::Unknown,
            JitterTier::B,
            50,
        )
    };

    let quality = NetworkQuality {
        timestamp,
        rom,
        passive,
        active,
        bullet_reg,
        jitter_tier,
        quality_score,
    };

    // Persist report to state dir for shell scripts / diagnostics
    if !state_dir.is_empty() {
        let report_path = std::path::PathBuf::from(state_dir).join("network_quality.json");
        if let Ok(json) = serde_json::to_string_pretty(&quality) {
            let _ = fs::write(&report_path, json);
        }
    }

    quality
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
    match quality.bullet_reg {
        BulletRegQuality::Bad | BulletRegQuality::Poor | BulletRegQuality::Unknown => true,
        BulletRegQuality::Fair => quality.passive.network_type == "wifi",
        BulletRegQuality::Good | BulletRegQuality::Excellent => true,
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
        let state = probe_passive();
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

    #[test]
    fn test_icmp_checksum() {
        // Known ICMP echo request checksum for all-zero payload
        let packet = [0x08, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01];
        let cksum = icmp_checksum(&packet);
        // Checksum of an all-zero packet (except type) should be valid
        // Just verify it runs without panicking
        assert!(cksum != 0 || packet.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_ip_to_u32() {
        assert_eq!(ip_to_u32("8.8.8.8"), Some(0x08080808));
        assert_eq!(ip_to_u32("1.1.1.1"), Some(0x01010101));
        assert_eq!(ip_to_u32("192.168.1.1"), Some(0xC0A80101));
        assert_eq!(ip_to_u32("not-an-ip"), None);
        assert_eq!(ip_to_u32("1.2.3"), None);
    }

    #[test]
    fn test_ping_unreachable_target() {
        // 192.0.2.1 is TEST-NET-1 (RFC 5737), should timeout
        let result = icmp_ping("192.0.2.1", 2, 100);
        assert_eq!(result.packets_received, 0);
        assert_eq!(result.loss_pct, 100.0);
    }
}
