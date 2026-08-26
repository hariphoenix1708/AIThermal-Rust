pub mod advanced;
pub mod backend;
use crate::hardware::HardwareProfile;
use crate::sysfs;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::tuning::backend::{BackendError, VmBackend};

const TUNING_ACTIVE_FILE: &str = "tuning_active.json";
const LOCKED_NODES_FILE: &str = "locked_sysfs_nodes.json";

pub struct RuntimeTuner {
    hardware: HardwareProfile,
    original_state: std::sync::Mutex<HashMap<String, String>>,
    last_gpu_boost: std::sync::Mutex<Option<std::time::Instant>>,
    last_drop_cache: std::sync::Mutex<Option<std::time::Instant>>,
    unsupported_cooling_nodes: std::sync::Mutex<std::collections::HashSet<String>>,
    locked_sysfs_nodes: std::sync::Mutex<Vec<String>>,
    state_dir: Option<PathBuf>,
    tcp_cc_gaming: String,
    touch_network_stack: bool,
    unsupported_touch_nodes: std::sync::Mutex<std::collections::HashSet<String>>,
}

fn write_if_changed(path: &str, value: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(cur) => {
            // Normalize the kernel's bracketed active-list format used by
            // nodes like queue/scheduler: "none [mq-deadline] kyber" -> the
            // active token "mq-deadline". Falls back to trimmed compare.
            let active = cur
                .split_ascii_whitespace()
                .find_map(|t| t.strip_prefix('[').and_then(|t| t.strip_suffix(']')))
                .unwrap_or_else(|| cur.trim());
            if active == value.trim() {
                false
            } else {
                let _ = crate::tuning::backend::TuningBackend::try_write_string(path, value);
                true
            }
        }
        Err(_) => {
            let _ = crate::tuning::backend::TuningBackend::try_write_string(path, value);
            true
        }
    }
}

impl RuntimeTuner {
    pub fn new(hardware: HardwareProfile) -> Self {
        Self {
            hardware,
            original_state: std::sync::Mutex::new(HashMap::new()),
            last_gpu_boost: std::sync::Mutex::new(None),
            last_drop_cache: std::sync::Mutex::new(None),
            unsupported_cooling_nodes: std::sync::Mutex::new(std::collections::HashSet::new()),
            locked_sysfs_nodes: std::sync::Mutex::new(Vec::new()),
            state_dir: None,
            tcp_cc_gaming: "kernel_default".to_string(),
            touch_network_stack: false,
            unsupported_touch_nodes: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn with_state_dir(mut self, dir: &str) -> Self {
        if !dir.is_empty() {
            self.state_dir = Some(PathBuf::from(dir));
        }
        self
    }

    pub fn with_network_config(mut self, tcp_cc_gaming: &str, touch_network_stack: bool) -> Self {
        self.tcp_cc_gaming = tcp_cc_gaming.to_string();
        self.touch_network_stack = touch_network_stack;
        self
    }

    fn persist_active(&self) {
        let Some(dir) = &self.state_dir else { return };
        let Ok(state) = self.original_state.lock() else {
            return;
        };
        if state.is_empty() {
            return;
        }
        let path = dir.join(TUNING_ACTIVE_FILE);
        let tmp = dir.join(format!("{}.tmp", TUNING_ACTIVE_FILE));
        if let Ok(json) = serde_json::to_string(&*state) {
            let _ = std::fs::write(&tmp, json);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn persist_locked(&self) {
        let Some(dir) = &self.state_dir else { return };
        let Ok(nodes) = self.locked_sysfs_nodes.lock() else {
            return;
        };
        let path = dir.join(LOCKED_NODES_FILE);
        let tmp = dir.join(format!("{}.tmp", LOCKED_NODES_FILE));
        if let Ok(json) = serde_json::to_string(&*nodes) {
            let _ = std::fs::write(&tmp, json);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn clear_active(&self) {
        if let Some(dir) = &self.state_dir {
            let _ = std::fs::remove_file(dir.join(TUNING_ACTIVE_FILE));
        }
    }

    fn clear_locked(&self) {
        if let Some(dir) = &self.state_dir {
            let _ = std::fs::remove_file(dir.join(LOCKED_NODES_FILE));
        }
    }

    /// Startup helper: if the previous daemon run left a `tuning_active.json`
    /// (unclean exit), restore every saved (path, original_value) pair before
    /// the first tick. Called from `main.rs` immediately after Daemon::new.
    pub fn rehydrate_and_restore(state_dir: &str) {
        if state_dir.is_empty() {
            return;
        }
        let base = PathBuf::from(state_dir);
        let active = base.join(TUNING_ACTIVE_FILE);
        if active.exists() {
            if let Ok(content) = std::fs::read_to_string(&active)
                && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&content) {
                    tracing::warn!(
                        "Found stale tuning_active.json ({} entries) from previous run — restoring originals",
                        map.len()
                    );
                    for (path, val) in map {
                        if !std::path::Path::new(&path).exists() {
                            continue;
                        }
                        crate::tuning::backend::TuningBackend::write_string(&path, &val);
                    }
                }
            let _ = std::fs::remove_file(&active);
        }

        // D11: unlock any 0o444 nodes left behind by write_and_lock.
        let locked = base.join(LOCKED_NODES_FILE);
        if locked.exists() {
            if let Ok(content) = std::fs::read_to_string(&locked)
                && let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
                    for p in list {
                        let _ =
                            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
                    }
                }
            let _ = std::fs::remove_file(&locked);
        }
    }

    fn write_and_save(&self, path: &str, value: &str, save: bool) {
        let mut newly_saved = false;
        if save
            && let Ok(mut state) = self.original_state.lock()
                && !state.contains_key(path)
                    && let Ok(orig_val) = sysfs::read_string(path)
                {
                    state.insert(path.to_string(), orig_val);
                    newly_saved = true;
                }
        let _ = crate::tuning::backend::TuningBackend::try_write_string(path, value);
        if newly_saved {
            self.persist_active();
        }
    }

    fn restore_or_default(&self, path: &str, default: &str) {
        if let Ok(state) = self.original_state.lock()
            && let Some(orig) = state.get(path) {
                let _ = crate::tuning::backend::TuningBackend::try_write_string(path, orig);
                return;
            }
        let _ = crate::tuning::backend::TuningBackend::try_write_string(path, default);
    }

    fn write_and_lock(&self, path: &str, value: &str) {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
        if crate::tuning::backend::TuningBackend::try_write_string(path, value).is_ok() {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444));
            if let Ok(mut locked) = self.locked_sysfs_nodes.lock() {
                locked.push(path.to_string());
            }
            self.persist_locked();
        }
    }

    fn unsupported_block_device(dev: &str) -> bool {
        dev.starts_with("dm-") || dev.starts_with("loop") || dev.starts_with("zram")
    }
    pub fn check_network_quality(&self) -> bool {
        // v3.3.9: Check operstate OR carrier for each interface.
        // rmnet interfaces report operstate="unknown" even when active;
        // carrier==1 indicates the link is functionally up.
        let interfaces = ["wlan0", "rmnet_data0", "rmnet_data1", "rmnet_data2", "rmnet_data3"];
        for iface in &interfaces {
            let operstate = format!("/sys/class/net/{}/operstate", iface);
            let carrier = format!("/sys/class/net/{}/carrier", iface);
            let is_up = std::fs::read_to_string(&operstate)
                .ok()
                .map(|c| c.trim() == "up")
                .unwrap_or(false);
            let has_carrier = std::fs::read_to_string(&carrier)
                .ok()
                .map(|c| c.trim() == "1")
                .unwrap_or(false);
            if is_up || has_carrier {
                return true;
            }
        }
        false
    }

    pub fn apply_network_tweaks(
        &self,
        policy: &str,
        is_gaming: bool,
    ) -> Result<(), crate::tuning::backend::BackendError> {
        // Master off-switch: v3.1.0's TCP writes caused visible connectivity
        // regressions (DNS failures, stalled HTTPS streams). Off by default.
        if !self.touch_network_stack {
            return Ok(());
        }

        // Only apply network congestion controls if a network interface is active.
        if !self.check_network_quality() {
            return Ok(());
        }

        // Keep network tweaks active during gaming even if policy drops to
        // Balanced due to thermal pressure — game packets need low latency.
        let is_perf = policy == "Performance" || policy == "performance" || is_gaming;

        let path_keepalive = "/proc/sys/net/ipv4/tcp_keepalive_time";
        let path_congestion = "/proc/sys/net/ipv4/tcp_congestion_control";

        if is_perf {
            // Gaming: keepalive at 600s (10 min) — faster dead-connection
            // detection prevents stale sockets from blocking game state.
            let old_keepalive = sysfs::read_string(path_keepalive).ok().unwrap_or_default();
            self.write_and_save(path_keepalive, "600", true);
            tracing::info!(target: "network", "NET-01 tcp_keepalive_time {} -> 600", old_keepalive);

            // v3.3.6: TCP low-latency optimizations for gaming.
            // tcp_low_latency hints kernel to prioritize latency over throughput.
            if is_gaming {
                let path_low_latency = "/proc/sys/net/ipv4/tcp_low_latency";
                if let Ok(old_ll) = std::fs::read_to_string(path_low_latency) {
                    let old_ll = old_ll.trim().to_string();
                    if old_ll != "1" {
                        self.write_and_save(path_low_latency, "1", true);
                        tracing::info!(target: "network",
                            "NET-01 tcp_low_latency {} -> 1 (gaming low-latency)", old_ll);
                    }
                }
            }

            // B4: BBR/Westwood+ is opt-in via config, and only if available.
            let requested_cc = self.tcp_cc_gaming.trim();
            let should_write_cc = !requested_cc.is_empty()
                && requested_cc != "kernel_default"
                && self
                    .hardware
                    .network_profile
                    .available_congestion_controls
                    .iter()
                    .any(|c| c == requested_cc);

            if should_write_cc {
                let old_cc = sysfs::read_string(path_congestion).ok().unwrap_or_default();
                self.write_and_save(path_congestion, requested_cc, true);
                tracing::info!(target: "network",
                    "NET-01 tcp_congestion_control {} -> {}", old_cc, requested_cc);
            }
        } else {
            self.restore_or_default(path_keepalive, "7200");
            if is_gaming {
                self.restore_or_default("/proc/sys/net/ipv4/tcp_low_latency", "0");
            }
            if let Ok(state) = self.original_state.lock()
                && state.contains_key(path_congestion) {
                    drop(state);
                    self.restore_or_default(path_congestion, "cubic");
                }
        }
        Ok(())
    }

    pub fn restore_all(&self) {
        if let Ok(state) = self.original_state.lock() {
            for (path, val) in state.iter() {
                crate::tuning::backend::TuningBackend::write_string(path, val);
            }
        }
        self.clear_active();
    }

    pub fn apply_touch_display_tweaks(
        &self,
        policy: &str,
    ) -> Result<(), crate::tuning::backend::BackendError> {
        let is_perf = policy == "Performance" || policy == "performance";
        let is_game = policy == "Gaming" || policy == "gaming" || is_perf;

        if self
            .hardware
            .display_profile
            .touch_controller_name
            .is_none()
        {
            tracing::debug!("Skipping touch tuning: no verified touch controller discovered");
            return Ok(());
        }

        if self.hardware.display_profile.touch_nodes.is_empty() {
            tracing::debug!(
                "Skipping touch tuning: verified touch controller has no writable tuning nodes"
            );
            return Ok(());
        }

        for node in &self.hardware.display_profile.touch_nodes {
            let path = node.as_str();

            if let Ok(unsupported) = self.unsupported_touch_nodes.lock()
                && unsupported.contains(path) {
                    continue;
                }

            let Some(attr) = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
            else {
                if let Ok(mut unsupported) = self.unsupported_touch_nodes.lock() {
                    unsupported.insert(path.to_string());
                }
                tracing::debug!("Skipping malformed touch node path: {}", path);
                continue;
            };

            let value = if is_game { "1" } else { "0" };

            if matches!(
                attr,
                "game_mode" | "touch_boost" | "high_touch_rate" | "edge_reject" | "glove_mode"
            ) {
                if std::fs::OpenOptions::new().write(true).open(path).is_ok() {
                    self.write_and_save(path, value, true);
                    tracing::debug!(target: "tuning", "Applied touch tuning {}: {} via {}", attr, value, path);
                } else {
                    if let Ok(mut unsupported) = self.unsupported_touch_nodes.lock() {
                        unsupported.insert(path.to_string());
                    }
                    tracing::debug!("Touch node exists but not writable: {}", path);
                }
                continue;
            }

            if let Ok(mut unsupported) = self.unsupported_touch_nodes.lock() {
                unsupported.insert(path.to_string());
            }

            if matches!(attr, "touch_report_rate" | "report_rate") {
                tracing::debug!(
                    "Skipping rate-based touch attribute without an explicit numeric profile value: {}",
                    path
                );
                continue;
            }

            tracing::debug!("Skipping unsupported touch tuning attribute: {}", path);
        }

        Ok(())
    }

    pub fn apply_vm_params(&self, policy: &str) {
        let is_perf = policy == "Performance" || policy == "performance";
        let is_game = policy == "Gaming" || policy == "gaming" || is_perf;

        if is_game {
            if write_if_changed("/proc/sys/vm/swappiness", "60") {
                self.write_and_save("/proc/sys/vm/swappiness", "60", true);
            }
            if write_if_changed("/proc/sys/vm/vfs_cache_pressure", "100") {
                self.write_and_save("/proc/sys/vm/vfs_cache_pressure", "100", true);
            }
            if write_if_changed("/proc/sys/vm/dirty_ratio", "20") {
                self.write_and_save("/proc/sys/vm/dirty_ratio", "20", true);
            }
            if write_if_changed("/proc/sys/vm/dirty_background_ratio", "10") {
                self.write_and_save("/proc/sys/vm/dirty_background_ratio", "10", true);
            }
        } else {
            self.restore_or_default("/proc/sys/vm/swappiness", "100");
            self.restore_or_default("/proc/sys/vm/vfs_cache_pressure", "150");
            self.restore_or_default("/proc/sys/vm/dirty_ratio", "10");
            self.restore_or_default("/proc/sys/vm/dirty_background_ratio", "5");
        }
    }

    pub fn apply_io_scheduler(
        &self,
        policy: &str,
    ) -> Result<(), crate::tuning::backend::BackendError> {
        let is_perf = policy == "Performance" || policy == "performance";
        let is_game = policy == "Gaming" || policy == "gaming" || is_perf;

        let sched = if is_game { "mq-deadline" } else { "bfq" };
        let read_ahead = if is_game { "2048" } else { "128" };

        for dev in &self.hardware.storage_profile.block_devices {
            if Self::unsupported_block_device(dev) {
                tracing::debug!("Skipping unsupported block device during tuning: {}", dev);
                continue;
            }
            let p_sched = format!("/sys/block/{}/queue/scheduler", dev);
            let p_ra = format!("/sys/block/{}/queue/read_ahead_kb", dev);

            // Only apply if the scheduler is available
            if let Some(available) = self.hardware.storage_profile.available_schedulers.get(dev) {
                if is_game {
                    if available.contains(&sched.to_string()) {
                        write_if_changed(&p_sched, sched);
                    }
                } else {
                    if let Some(orig) = self.hardware.storage_profile.current_schedulers.get(dev) {
                        write_if_changed(&p_sched, orig);
                    } else if available.contains(&sched.to_string()) {
                        write_if_changed(&p_sched, sched);
                    }
                }
            }

            if is_game {
                write_if_changed(&p_ra, read_ahead);
            } else {
                if let Some(orig_ra) = self.hardware.storage_profile.read_ahead_kb.get(dev) {
                    write_if_changed(&p_ra, &orig_ra.to_string());
                } else {
                    write_if_changed(&p_ra, read_ahead);
                }
            }
        }
        Ok(())
    }

    pub fn drop_cache(&self, drop_slab: bool) -> Result<(), BackendError> {
        const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

        // Rate-limit drop_caches: writing "1" to drop_caches on every
        // Powersave tick forces immediate re-faulting and more I/O, which
        // is counterproductive when the device is already under memory
        // pressure. EmergencyCool (drop_caches=3) is rare and exempt.
        if !drop_slab
            && let Ok(last) = self.last_drop_cache.lock()
            && let Some(t) = *last
            && t.elapsed() < MIN_INTERVAL
        {
            return Ok(());
        }
        if let Ok(mut last) = self.last_drop_cache.lock() {
            *last = Some(std::time::Instant::now());
        }

        if drop_slab {
            crate::tuning::backend::TuningBackend::write_string("/proc/sys/vm/drop_caches", "3");
        } else {
            crate::tuning::backend::TuningBackend::write_string("/proc/sys/vm/drop_caches", "1");
        }
        Ok(())
    }

    pub fn disable_stock_thermal(&self) {
        // Disables standard qualcomm thermal engine
        crate::tuning::backend::TuningBackend::write_string(
            "/sys/class/thermal/thermal_message/sconfig",
            "10",
        );

        let cpu_limits_path = "/sys/class/thermal/thermal_message/cpu_limits";
        if std::fs::OpenOptions::new()
            .write(true)
            .open(cpu_limits_path)
            .is_ok()
        {
            let cpu_count = self
                .hardware
                .cpu_topology
                .clusters
                .iter()
                .flat_map(|c| c.cpus.iter())
                .count();
            for cpu in 0..cpu_count {
                let value = format!("cpu{} 2147483647", cpu); // i32::MAX as a no-limit sentinel
                let _ = crate::tuning::backend::TuningBackend::try_write_string(
                    cpu_limits_path,
                    &value,
                );
            }
            tracing::debug!(target: "tuning", "Applied unrestricted cpu_limits to {} cores via {}", cpu_count, cpu_limits_path);
        }

        for dev in &self.hardware.thermal_profile.cooling_devices {
            // D2: only silence CPU/GPU thermal caps. Never disarm modem,
            // charger, display, or battery cooling devices — those matter
            // for radio safety and battery health even when we own the CPU.
            let dtype_lower = dev.device_type.to_lowercase();
            let is_cpu_gpu = dtype_lower.contains("cpu")
                || dtype_lower.contains("cluster")
                || dtype_lower.contains("gpu")
                || dtype_lower.contains("kgsl")
                || dtype_lower.contains("thermal-cpufreq")
                || dtype_lower.contains("thermal-devfreq");
            if !is_cpu_gpu {
                tracing::debug!(
                    target: "tuning",
                    "Preserving non-CPU/GPU cooling device: {} (type={})",
                    dev.sysfs_path, dev.device_type
                );
                continue;
            }

            let cur_state = format!("{}/cur_state", dev.sysfs_path);
            if self
                .unsupported_cooling_nodes
                .lock()
                .map(|nodes| nodes.contains(&cur_state))
                .unwrap_or(false)
            {
                tracing::debug!(
                    "Skipping previously unsupported cooling device: {}",
                    cur_state
                );
                continue;
            }

            if std::fs::OpenOptions::new()
                .write(true)
                .open(&cur_state)
                .is_ok()
            {
                self.write_and_lock(&cur_state, "0");
            } else {
                tracing::warn!("Cooling device {} not writable, skipping", cur_state);
                if let Ok(mut nodes) = self.unsupported_cooling_nodes.lock() {
                    nodes.insert(cur_state);
                }
            }
        }

        self.disable_migt_if_present();
    }

    pub fn disable_migt_if_present(&self) {
        if !self.hardware.migt_present && !self.hardware.glk_present {
            return;
        }

        let migt_params: &[(&str, &str)] = &[
            (
                "/sys/module/migt/parameters/migt_freq",
                "0:0 1:0 2:0 3:0 4:0 5:0 6:0 7:0",
            ),
            ("/sys/module/migt/parameters/glk_disable", "1"),
            ("/sys/module/migt/parameters/mi_freq_enable", "0"),
            ("/sys/module/migt/parameters/force_stask_to_big", "0"),
        ];

        for (path, value) in migt_params {
            if std::fs::OpenOptions::new().write(true).open(path).is_ok() {
                self.write_and_lock(path, value);
            }
        }

        let glk_params: &[(&str, &str)] = &[
            ("/proc/sys/glk/glk_disable", "1"),
            ("/proc/sys/glk/freq_break_enable", "0"),
        ];

        for (path, value) in glk_params {
            if std::fs::OpenOptions::new().write(true).open(path).is_ok() {
                self.write_and_lock(path, value);
            }
        }
    }

    pub fn restore_stock_thermal(&self) {
        crate::tuning::backend::TuningBackend::write_string(
            "/sys/class/thermal/thermal_message/sconfig",
            "0",
        );

        if let Ok(mut locked) = self.locked_sysfs_nodes.lock() {
            for path in locked.iter() {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
            }
            locked.clear();
        }
        self.clear_locked();
        tracing::debug!(target: "tuning", "restore_stock_thermal: unlocked and cleared locked-node registry");
    }

    pub fn apply_universal_gpu_control(&self, policy: &str, is_gaming: bool) {
        let is_perf = policy == "Performance" || policy == "performance";
        // Boost for the whole active gaming session, not just when the policy
        // string happens to be "Performance". Mid-game Balanced/Conservative
        // flapping previously took the restore path and dropped the GPU to its
        // lowest power level (default_pwrlevel=10) in the middle of a frame.
        let is_game = policy == "Gaming" || policy == "gaming" || is_perf || is_gaming;

        if is_game && self.hardware.gpu_profile.is_kgsl {
            if self.hardware.gpu_profile.has_bus_split {
                let bus_split = format!("{}/bus_split", self.hardware.gpu_profile.path);
                if write_if_changed(&bus_split, "0") {
                    tracing::debug!(target: "tuning", "Applied GPU bus_split: 0 via {}", bus_split);
                }
            }
            if self.hardware.gpu_profile.has_force_clk_on {
                let force_clk = format!("{}/force_clk_on", self.hardware.gpu_profile.path);
                tracing::debug!(
                    target: "tuning",
                    "Skipping GPU force_clk_on: {} exists but is not a portable KGSL performance control",
                    force_clk
                );
            }
        }

        let Some(gpu) = &self.hardware.gpu_profile.power_level_path else {
            static GPU_PWR_MISSING_WARN: std::sync::Once = std::sync::Once::new();
            GPU_PWR_MISSING_WARN.call_once(|| {
                tracing::trace!(
                    target: "tuning",
                    "Skipping GPU power-level control: no writable KGSL power-level node"
                );
            });
            return;
        };
        let (Some(min_level), Some(max_level)) = (
            self.hardware.gpu_profile.min_power_level,
            self.hardware.gpu_profile.max_power_level,
        ) else {
            tracing::debug!("Skipping GPU power-level control: missing discovered KGSL bounds");
            return;
        };
        let low = min_level.min(max_level);
        let high = min_level.max(max_level);
        let target = if is_game {
            if let Ok(mut last) = self.last_gpu_boost.lock() {
                *last = Some(std::time::Instant::now());
            }
            // Boost = the best (lowest-index) level. This device reports
            // min=10/max=0, so the old `max_level` target (0) only worked by
            // coincidence; `low` is correct by construction on any KGSL node.
            low
        } else if let Some(current) = self.hardware.gpu_profile.current_power_level {
            if let Ok(last) = self.last_gpu_boost.lock()
                && let Some(t) = *last
                    && t.elapsed().as_secs() < 5 {
                        tracing::debug!(
                            "Skipping GPU power-level restore: holding boosted level for 5s debounce"
                        );
                        return;
                    }
            current
        } else {
            tracing::debug!(
                "Skipping GPU power-level restore: current KGSL power level was not discovered"
            );
            return;
        };

        if !(low..=high).contains(&target) {
            tracing::warn!(
                "Skipping GPU power-level write: target {} outside discovered bounds {}..={}",
                target,
                low,
                high
            );
            return;
        }
        self.write_and_save(gpu, &target.to_string(), true);
        tracing::debug!(target: "tuning", "Applied GPU power level: {} (mode: {}) via {}", target, if is_game { "boost" } else { "restore" }, gpu);
    }

    pub fn cpuset_tasks_file(hw: &crate::hardware::HardwareProfile, subgroup: &str) -> String {
        let base = &hw.cpuset_profile.root_path;
        if hw.cpuset_profile.is_cgroup_v2 {
            // On v2 threaded cgroups, thread migration uses cgroup.threads.
            // Fall back to cgroup.procs if cgroup.threads is not writable
            // (i.e. subgroup is not marked threaded).
            let threads = format!("{}/{}/cgroup.threads", base, subgroup);
            if std::path::Path::new(&threads).exists() {
                return threads;
            }
            format!("{}/{}/cgroup.procs", base, subgroup)
        } else {
            format!("{}/{}/tasks", base, subgroup)
        }
    }

    pub fn pin_critical_render_thread(&self, game_pid: u32, target_cpu_range: &str) {
        if self.hardware.cpuset_profile.root_path.is_empty() {
            return;
        }
        let task_dir = format!("/proc/{}/task", game_pid);
        let Ok(entries) = std::fs::read_dir(&task_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let tid = entry.file_name();
            let comm_path = format!("{}/{}/comm", task_dir, tid.to_string_lossy());
            let Ok(comm) = std::fs::read_to_string(&comm_path) else {
                continue;
            };
            let comm = comm.trim();
            if comm == "RenderThread" || comm == "GLThread" || comm.starts_with("hwuiTask") {
                let tasks_path = Self::cpuset_tasks_file(&self.hardware, target_cpu_range);
                let _ = crate::tuning::backend::TuningBackend::try_write_string(
                    &tasks_path,
                    tid.to_string_lossy().as_ref(),
                );
                tracing::debug!(
                    target: "tuning",
                    "Pinned {} (tid {}) to {}",
                    comm, tid.to_string_lossy(), target_cpu_range
                );
            }
        }
    }

    pub fn tune_walt(&self, policy: &str) {
        let is_perf = policy == "Performance" || policy == "performance";
        if self.hardware.scheduler_profile.has_walt {
            if is_perf {
                self.write_and_save("/proc/sys/kernel/sched_walt_rotate_capacity", "1", true);
            } else {
                self.restore_or_default("/proc/sys/kernel/sched_walt_rotate_capacity", "0");
            }
        }
    }

    fn snap_to_available_freq(&self, cluster_name: &str, target: u64) -> Option<u64> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/{}/scaling_available_frequencies",
            cluster_name
        );
        let s = std::fs::read_to_string(&path).ok()?;
        s.split_ascii_whitespace()
            .filter_map(|t| t.parse::<u64>().ok())
            .min_by_key(|f| f.abs_diff(target))
    }

    pub fn apply_cluster_settings(&self, policy: &str) {
        let (gov, max_pct): (&str, Option<u32>) = match policy {
            "Performance" | "performance" | "Gaming" | "gaming" =>
                ("performance", None),
            "Balanced"                       => ("walt", None),
            "Conservative"                   => ("schedutil", Some(85)),
            // Post-game / thermal recovery: gentler clamp than Conservative so
            // the exit animation isn't starved while the SoC cools down.
            "Recovery"                       => ("schedutil", Some(90)),
            "Powersave"                      => ("schedutil", Some(70)),
            "EmergencyCool"                  => ("schedutil", Some(55)),
            "Suspend"                        => ("powersave",  None),
            _                                => ("schedutil", None),
        };

        for cluster in &self.hardware.cpu_topology.clusters {
            // Governor write (existing capability check)
            let effective_gov = if cluster.available_governors.iter().any(|g| g == gov) {
                gov
            } else if cluster.available_governors.iter().any(|g| g == "schedutil") {
                "schedutil"
            } else {
                tracing::debug!(
                    "Skipping unsupported CPU governor {} on {}", gov, cluster.name
                );
                continue;
            };
            if let Err(e) = crate::tuning::backend::TuningBackend::write_capability(
                &cluster.governor_node,
                effective_gov,
            ) {
                tracing::warn!("Failed to apply CPU governor on {}: {}", cluster.name, e);
            }

            // Percent-of-Fmax clamp (only when max_pct is Some).
            // Reads cpuinfo_max_freq, computes target, snaps to nearest available
            // frequency, and writes scaling_max_freq. Restores on Balanced/Perf/Game.
            let scaling_max_path = format!(
                "/sys/devices/system/cpu/cpufreq/{}/scaling_max_freq",
                cluster.name
            );
            let cpuinfo_max_path = format!(
                "/sys/devices/system/cpu/cpufreq/{}/cpuinfo_max_freq",
                cluster.name
            );
            if let Some(pct) = max_pct {
                if let Ok(s) = std::fs::read_to_string(&cpuinfo_max_path)
                    && let Ok(fmax) = s.trim().parse::<u64>() {
                        let target = fmax.saturating_mul(pct as u64) / 100;
                        let snapped = self.snap_to_available_freq(&cluster.name, target)
                            .unwrap_or(target);
                        self.write_and_save(&scaling_max_path, &snapped.to_string(), true);
                    }
            } else {
                // Restore to Fmax when leaving a clamped state.
                if let Ok(s) = std::fs::read_to_string(&cpuinfo_max_path) {
                    self.write_and_save(&scaling_max_path, s.trim(), true);
                }
            }
        }
    }

    pub fn set_gpu_power_levels(&self, policy: &str) {
        self.apply_universal_gpu_control(policy, false);
    }

    pub fn apply_universal_cpu_tuning(&self, policy: &str) {
        self.apply_cluster_settings(policy);
        self.tune_walt(policy);
    }
}

impl VmBackend for RuntimeTuner {
    fn drop_caches(&self) -> Result<(), BackendError> {
        self.drop_cache(true)
    }
}

impl crate::tuning::backend::StorageBackend for RuntimeTuner {
    fn apply_scheduler(&self, sched: &str) -> Result<(), crate::tuning::backend::BackendError> {
        self.apply_io_scheduler(sched)
    }
}

impl crate::tuning::backend::NetworkBackend for RuntimeTuner {
    fn apply_congestion_control(
        &self,
        algo: &str,
    ) -> Result<(), crate::tuning::backend::BackendError> {
        self.apply_network_tweaks(algo, false)
    }
}
