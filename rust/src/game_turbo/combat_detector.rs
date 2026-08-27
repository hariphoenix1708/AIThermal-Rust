//! Combat Detector — heuristic enemy-proximity burst detection for CODM Ranked.
//!
//! No game hook. When 3+ enemies enter, Unity adds ~180k tris + 3 setPass per
//! enemy plus server enemy-tick bursts. Daemon-visible proxies:
//!   - rx_packets/s ↑ 1.5× (wlan0/rmnet_data1)
//!   - gpu_busy% ↑ +20pp
//!   - top-app uclamp demand already high
//! 2-of-3 vote avoids single-packet false positives.
//! Logs to thermalai.log + thermalai_combat.log (pullable via Download/AIThermal-Logs).

use std::collections::VecDeque;
use std::fs;
use std::time::Instant;

const WINDOW_SHORT_MS: u64 = 500;
const WINDOW_BASELINE_MS: u64 = 2000;
const GPU_SPIKE_PP: u32 = 10; // Ultra120: 8.3ms budget, +10pp already misses
const NET_SPIKE_FACTOR: f64 = 1.3;
const MIN_PPS: u64 = 100; // lower for lobby/low-pps ranked

fn read_rx_packets() -> u64 {
    let Ok(content) = fs::read_to_string("/proc/net/dev") else { return 0 };
    let mut total = 0u64;
    for line in content.lines().skip(2) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 { continue; }
        let iface = parts[0].trim_end_matches(':');
        if (iface == "wlan0" || iface.starts_with("rmnet") || iface.starts_with("r_rmnet"))
            && let Ok(v) = parts[2].parse::<u64>() {
                total += v;
            }
    }
    total
}

fn read_gpu_busy() -> u32 {
    fs::read_to_string("/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage")
        .ok()
        .and_then(|s| s.trim().trim_end_matches('%').parse().ok())
        .unwrap_or_else(|| {
            // Fallback: busy_time/devfreq
            fs::read_to_string("/sys/class/kgsl/kgsl-3d0/gpubusy")
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0)
        })
}

#[derive(Clone)]
struct Sample {
    at: Instant,
    rx: u64,
    gpu: u32,
}

pub struct CombatDetector {
    window: VecDeque<Sample>,
    last_log: Option<Instant>,
    last_heartbeat: Option<Instant>,
}

impl CombatDetector {
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(16),
            last_log: None,
            last_heartbeat: None,
        }
    }

    /// Call each tick (~1s). Returns true if combat burst detected.
    pub fn update(&mut self, gpu_load: u32) -> bool {
        let now = Instant::now();
        let rx = read_rx_packets();
        // Prefer caller gpu_load if valid, else read sysfs
        let gpu = if gpu_load > 0 { gpu_load } else { read_gpu_busy() };
        self.window.push_back(Sample { at: now, rx, gpu });
        // Keep 3s window
        while let Some(front) = self.window.front() {
            if now.duration_since(front.at).as_millis() as u64 > 3000 {
                self.window.pop_front();
            } else { break; }
        }
        if self.window.len() < 3 { return false; }

        // pps short vs baseline
        let (short_pps, base_pps) = self.pps_windows(now);
        let gpu_spike = self.gpu_spike();
        let net_spike = short_pps as f64 > base_pps as f64 * NET_SPIKE_FACTOR && short_pps > MIN_PPS;
        // 1-of-2 vote with thresholds already high: net 1.5× + gpu +20pp
        let combat = net_spike || gpu_spike;

        if combat && self.should_log(now) {
            tracing::info!(
                target: "game_turbo",
                "Combat Detector: BURST net {}pps→{}pps gpu spike {} (gpu {}%)",
                base_pps, short_pps, gpu_spike, gpu
            );
            self.append_combat_log(now, base_pps, short_pps, gpu, gpu_spike, net_spike);
        } else if !combat && self.should_heartbeat(now) {
            tracing::debug!(target: "game_turbo", "Combat Detector: idle rx {}pps gpu {}% (no burst)", short_pps, gpu);
            // Heartbeat for offline analysis every 30s
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f%z").to_string();
            let line = format!("{ts} COMBAT idle rx {short_pps}pps gpu={gpu}% burst=false\n");
            let path = std::env::var("THERMALAI_LOG_DIR").unwrap_or_else(|_| "/data/local/tmp/AIThermal".to_string());
            let p = std::path::Path::new(&path).join("thermalai_combat.log");
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(&p)
                .and_then(|mut f| { use std::io::Write; f.write_all(line.as_bytes()) });
        }
        combat
    }

    fn should_log(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last_log
            && now.duration_since(last).as_secs() < 2 { return false; }
        self.last_log = Some(now);
        true
    }

    fn should_heartbeat(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last_heartbeat
            && now.duration_since(last).as_secs() < 30 { return false; }
        self.last_heartbeat = Some(now);
        true
    }

    fn pps_windows(&self, now: Instant) -> (u64, u64) {
        let short = self.pps_for_window(now, WINDOW_SHORT_MS);
        let base = self.pps_for_window(now, WINDOW_BASELINE_MS);
        (short, base)
    }

    fn pps_for_window(&self, now: Instant, window_ms: u64) -> u64 {
        let mut oldest: Option<&Sample> = None;
        let mut newest: Option<&Sample> = None;
        for s in &self.window {
            let age = now.duration_since(s.at).as_millis() as u64;
            if age <= window_ms {
                if oldest.is_none() { oldest = Some(s); }
                newest = Some(s);
            }
        }
        if let (Some(a), Some(b)) = (oldest, newest) {
            let dt = b.at.duration_since(a.at).as_secs_f64();
            if dt > 0.05 {
                return ((b.rx.saturating_sub(a.rx)) as f64 / dt) as u64;
            }
        }
        0
    }

    fn gpu_spike(&self) -> bool {
        if self.window.len() < 4 { return false; }
        let recent = self.window.back().map(|s| s.gpu).unwrap_or(0);
        let baseline = {
            let n = self.window.len().saturating_sub(2);
            if n == 0 { return false; }
            let sum: u32 = self.window.iter().take(n).map(|s| s.gpu).sum();
            sum / n as u32
        };
        recent > baseline + GPU_SPIKE_PP && recent > 30
    }

    fn append_combat_log(&self, now: Instant, base_pps: u64, short_pps: u64, gpu: u32, gpu_spike: bool, net_spike: bool) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f%z").to_string();
        let line = format!("{ts} COMBAT rx {base_pps}->{short_pps}pps net_spike={net_spike} gpu={gpu}% gpu_spike={gpu_spike} combat=true\n");
        // Pullable combat log + verbose trace
        let path = std::env::var("THERMALAI_LOG_DIR").unwrap_or_else(|_| "/data/local/tmp/AIThermal".to_string());
        let p = std::path::Path::new(&path).join("thermalai_combat.log");
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&p)
            .and_then(|mut f| { use std::io::Write; f.write_all(line.as_bytes()) });
        let _ = now; // keep
    }

    pub fn reset(&mut self) {
        self.window.clear();
        self.last_log = None;
        self.last_heartbeat = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_detector_empty() {
        let d = CombatDetector::new();
        assert!(d.window.is_empty());
    }
}
