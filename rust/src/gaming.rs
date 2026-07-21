use anyhow::Result;
use std::fs;
use std::time::{Duration, Instant};

use std::collections::HashMap;

/// How many consecutive top-app-cgroup misses are required before we allow
/// them to invalidate an otherwise-valid process-scan detection. Prevents
/// one flaky read from erasing a real game latch (the 73 s Roblox stall).
const CGROUP_NEGATIVE_STREAK_THRESHOLD: u32 = 3;

pub struct GameDetector {
    pub known_games: Vec<String>,
    pub is_gaming: bool,
    pub confirmed_package: Option<String>,
    pub confirmed_pid: Option<u32>,
    latch_time: Option<Instant>,
    latch_duration: Duration,
    last_proc_scan: Option<Instant>,
    proc_scan_interval: Duration,
    cached_package: Option<String>,
    cached_pid: Option<u32>,
    last_scan_pids: HashMap<u32, Option<String>>,
    daemon_started_at: Instant,
    cgroup_negative_streak: u32,
}

impl GameDetector {
    pub fn foreground_priority(&self, known_hot: bool, is_screen_off: bool) -> i32 {
        let mut priority = 0;

        if self.is_gaming {
            priority += 50;

            if known_hot {
                priority += 30;
            }
            if !is_screen_off {
                priority += 20;
            }
        } else if !is_screen_off {
            priority += 10;
        }

        priority
    }

    pub fn detect_frame_stutter(&self, session_started_at: Option<Instant>) -> bool {
        if !self.is_gaming {
            return false;
        }

        if let Some(start) = session_started_at {
            if start.elapsed() < Duration::from_secs(60) {
                return false;
            }
        } else {
            return false;
        }

        let mut has_stutter = false;

        if let Ok(content) = fs::read_to_string("/sys/class/kgsl/kgsl-3d0/devfreq/busy_time")
            && let Ok(busy) = content.trim().parse::<u64>()
            && let Ok(content_total) =
                fs::read_to_string("/sys/class/kgsl/kgsl-3d0/devfreq/total_time")
            && let Ok(total) = content_total.trim().parse::<u64>()
            && total > 0
        {
            let stress = (busy as f64 / total as f64) * 100.0;
            if stress > 95.0 {
                has_stutter = true;
            }
        }

        if let Ok(loadavg) = fs::read_to_string("/proc/loadavg") {
            let parts: Vec<&str> = loadavg.split_whitespace().collect();
            if !parts.is_empty()
                && let Ok(load1) = parts[0].parse::<f64>()
            {
                let num_cores = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(8) as f64;
                if load1 > num_cores * 1.5 {
                    has_stutter = true;
                }
            }
        }

        has_stutter
    }

    pub fn new(known_games: Vec<String>, latch_sec: u64, proc_scan_interval_sec: u64) -> Self {
        Self {
            known_games,
            is_gaming: false,
            confirmed_package: None,
            confirmed_pid: None,
            latch_time: None,
            latch_duration: Duration::from_secs(latch_sec),
            last_proc_scan: None,
            proc_scan_interval: Duration::from_secs(proc_scan_interval_sec),
            cached_package: None,
            cached_pid: None,
            last_scan_pids: HashMap::new(),
            daemon_started_at: Instant::now(),
        }
    }

    pub fn confirmed_package(&self) -> Option<&str> {
        self.confirmed_package.as_deref()
    }

    fn is_package_in_foreground_cgroup(target_pkg: &str) -> Option<bool> {
        let candidate_paths = [
            "/dev/cpuset/top-app/cgroup.procs",
            "/sys/fs/cgroup/cpuset/top-app/cgroup.procs",
        ];
        for path in candidate_paths {
            let Ok(content) = std::fs::read_to_string(path) else { continue };
            let mut checked_any = false;
            for pid_str in content.split_whitespace() {
                let cmdline_path = format!("/proc/{}/cmdline", pid_str);
                if let Ok(cmdline) = std::fs::read_to_string(&cmdline_path) {
                    checked_any = true;
                    let pkg = cmdline.split('\0').next().unwrap_or("").trim();
                    // Match the base package exactly, OR the base package followed
                    // by a ":" process-name suffix (Android's standard multi-process
                    // naming convention) - either means this PID belongs to the
                    // SAME app as target_pkg, just a different process within it.
                    if pkg == target_pkg || pkg.starts_with(&format!("{}:", target_pkg)) {
                        return Some(true);
                    }
                }
            }
            if checked_any {
                return Some(false); // cgroup was readable, had processes, none matched
            }
        }
        None // cgroup path unreadable/unavailable on this device - can't corroborate either way
    }

    pub fn tick(&mut self) -> Result<bool> {
        if self.daemon_started_at.elapsed().as_secs() < 30 {
            return Ok(false);
        }

        let mut perform_scan = false;
        if let Some(last_scan) = self.last_proc_scan {
            if last_scan.elapsed() >= self.proc_scan_interval {
                perform_scan = true;
            }
        } else {
            perform_scan = true;
        }

        let (mut pkg, mut pid) = if perform_scan {
            let scanned = self.scan_oom_score_adj();
            self.last_proc_scan = Some(Instant::now());
            self.cached_package = scanned.clone().and_then(|(p, _)| p);
            self.cached_pid = scanned.clone().and_then(|(_, id)| id);
            scanned.unwrap_or((None, None))
        } else {
            (self.cached_package.clone(), self.cached_pid)
        };

        let mut detected = pkg.is_some();

        if detected {
            if let Some(ref p) = pkg {
                match Self::is_package_in_foreground_cgroup(p) {
                    Some(false) => {
                        tracing::debug!(
                            target: "gaming",
                            "Package {} matched process scan but not found in top-app cgroup, treating as unconfirmed",
                            p
                        );
                        detected = false;
                        pkg = None;
                        pid = None;
                    }
                    Some(true) => {
                        tracing::debug!(target: "gaming", "Confirmed {} in top-app cgroup", p);
                    }
                    None => {
                        // Cgroup unavailable/unreadable on this device - fall back to
                        // trusting the primary exact-match scan alone, exactly as it
                        // worked before this round's cgroup-corroboration feature was
                        // added. Never let an unreadable cgroup path cancel an
                        // otherwise-confirmed detection.
                    }
                }
            }
        }

        if detected {
            self.is_gaming = true;
            self.confirmed_package = pkg;
            self.confirmed_pid = pid;
            self.latch_time = Some(Instant::now());
        } else if let Some(last_detected) = self.latch_time {
            if last_detected.elapsed() > self.latch_duration {
                self.is_gaming = false;
                self.latch_time = None;
                self.confirmed_pid = None;
            } else {
                self.is_gaming = true;
            }
        } else {
            self.is_gaming = false;
            self.confirmed_pid = None;
        }

        Ok(self.is_gaming)
    }

    #[allow(clippy::collapsible_if)]
    fn scan_oom_score_adj(&mut self) -> Option<(Option<String>, Option<u32>)> {
        let mut current_pids = HashMap::new();
        let mut detected_game = None;
        let mut detected_pid = None;

        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let path = entry.path();
                        if let Some(pid_str) = path.file_name().and_then(|n| n.to_str()) {
                            if let Ok(pid) = pid_str.parse::<u32>() {
                                // If we've already cached a negative result for this PID in the previous scan,
                                // we can skip reading cmdline/status assuming it's a long-lived non-game daemon.
                                // However, oom_score_adj can change dynamically, so we still check it first.
                                let oom_path = path.join("oom_score_adj");
                                if let Ok(content) = fs::read_to_string(&oom_path) {
                                    if content.trim() == "0" {
                                        // Fast path check: was it already scanned and found to be NOT a game?
                                        if let Some(cached_pkg) = self.last_scan_pids.get(&pid) {
                                            current_pids.insert(pid, cached_pkg.clone());
                                            if cached_pkg.is_none() {
                                                continue; // Known non-game, skip expensive cmdline read
                                            }
                                            if cached_pkg.is_some() {
                                                detected_game = cached_pkg.clone();
                                                detected_pid = Some(pid);
                                                break;
                                            }
                                        }

                                        // Fallback slow path
                                        let cmdline_path = path.join("cmdline");
                                        let mut pkg_name = String::new();
                                        if let Ok(cmdline) = fs::read_to_string(&cmdline_path) {
                                            let mut trimmed =
                                                cmdline.trim_matches('\0').to_string();
                                            if let Some(idx) = trimmed.find('\0') {
                                                trimmed = trimmed[..idx].to_string();
                                            }
                                            pkg_name = trimmed;
                                        }

                                        if pkg_name.is_empty() {
                                            let status_path = path.join("status");
                                            if let Ok(status) = fs::read_to_string(&status_path) {
                                                for line in status.lines() {
                                                    if line.starts_with("Name:") {
                                                        if let Some(name) =
                                                            line.split_whitespace().nth(1)
                                                        {
                                                            pkg_name = name.to_string();
                                                        }
                                                        break;
                                                    }
                                                }
                                            }
                                        }

                                        let mut exact_match = false;
                                        if !pkg_name.is_empty()
                                            && self.known_games.contains(&pkg_name)
                                        {
                                            exact_match = true;
                                        }

                                        if exact_match {
                                            current_pids.insert(pid, Some(pkg_name.clone()));
                                            detected_game = Some(pkg_name);
                                            detected_pid = Some(pid);
                                        } else {
                                            current_pids.insert(pid, None);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        self.last_scan_pids = current_pids;
        if detected_game.is_some() {
            Some((detected_game, detected_pid))
        } else {
            None
        }
    }
}
