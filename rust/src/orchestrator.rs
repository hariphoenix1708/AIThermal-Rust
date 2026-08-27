use crate::calibration::CalibrationManager;
use crate::charging::ChargingEngine;
use crate::cpuset::CpusetManager;
use crate::daemon::RuntimeTask;
use crate::gaming::GameDetector;
use crate::governors::GovernorManager;
use crate::hardware::HardwareProfile;
use crate::policy::{PolicyEngine, PolicyState};
use crate::prediction::PredictionEngine;
use crate::recovery::RecoveryManager;
use crate::runtime_context::RuntimeContext;
use crate::sensors::SensorManager;
use crate::snapshot::SnapshotManager;
use crate::thermal::ThermalEngine;
use crate::tuning::RuntimeTuner;
use crate::tuning::backend::CpusetBackend;
use crate::tuning::backend::StorageBackend;
use crate::watchdog::Watchdog;

use anyhow::Result;
use tracing::{info, warn};

pub struct SystemOrchestrator {
    adaptive_governor: crate::scheduler::adaptive_governor::AdaptiveGovernorState,
    last_load_sample: std::collections::HashMap<usize, crate::monitor::load_sampler::LoadSample>,
    background_frame_sampler: crate::monitor::frame_sampler::BackgroundFrameSampler,
    sensors: SensorManager,
    thermal: ThermalEngine,
    prediction: PredictionEngine,
    policy: PolicyEngine,
    governors: GovernorManager,
    cpuset: CpusetManager,
    charging: ChargingEngine,
    gaming: GameDetector,
    game_turbo: crate::game_turbo::GameTurboEngine,
    watchdog: Watchdog,
    recovery: RecoveryManager,
    calibration: CalibrationManager,
    snapshot: SnapshotManager,
    hardware: HardwareProfile,
    runtime_tuner: RuntimeTuner,
    game_profiles: crate::profiles::GameProfileManager,
    battery_stats: crate::telemetry::battery_stats::BatteryStatsTracker,
    last_battery_log_time: Option<std::time::Instant>,
    last_battery_summary_time: Option<std::time::Instant>,
    last_actuation_at: Option<std::time::Instant>,
    wake_defer_until: Option<std::time::Instant>,
    recovery_applied_this_stall: bool,
    pending_wake_nudge: bool,
    last_applied_cpu_gov: Option<String>,
    last_applied_gpu_gov: Option<String>,
    last_applied_gpu_level: Option<u32>,
    /// Some(true)  -> stock thermal is currently disabled by us
    /// Some(false) -> stock thermal is currently restored
    /// None        -> not yet decided this run
    stock_thermal_disabled: Option<bool>,
    last_telemetry_write_at: Option<std::time::Instant>,
    last_telemetry_policy: Option<String>,
    last_applied_policy: Option<String>,
    last_policy_change_at: Option<std::time::Instant>,
    last_network_probe: Option<std::time::Instant>,
    last_network_tweaks_applied: bool,
}

impl SystemOrchestrator {
    fn calculate_adaptive_sleep(
        &mut self,
        ctx: &mut RuntimeContext,
        trend_score: i32,
        is_screen_off_now: bool,
        is_gaming: bool,
        gpu_load: u32,
    ) -> (u64, bool) {
        let clamped_trend = (trend_score * 50).clamp(-50, 50);
        ctx.trend_score = clamped_trend;

        let long_idle = is_screen_off_now
            && !is_gaming
            && ctx.plugged_in_at.is_none()
            && ctx
                .screen_off_since
                .map(|t| t.elapsed().as_secs() > 30)
                .unwrap_or(false)
            && clamped_trend <= 0; // only back off further if not actively heating

        // Require BOTH a real heating trend AND two consecutive hot-trending
        // ticks before we run at high frequency; this stops the daemon from
        // spinning at 4 Hz on ordinary micro-fluctuations.
        let hot_trend_now = clamped_trend > 30;
        let sustained_hot_trend = hot_trend_now && ctx.prev_hot_trend;
        ctx.prev_hot_trend = hot_trend_now;

        let sleep_ms = if is_gaming {
            // ─── Adaptive gaming poll interval ─────────────────────────
            // Gaming needs faster polling for thermal response, but
            // we scale based on temperature trend and GPU load:
            // - Hot trend + high GPU load: 500ms (fastest — catch thermal spikes)
            // - Hot trend (moderate):       1000ms (fast — game_poll_interval)
            // - Stable + low GPU:           3000ms (slow — save power when safe)
            // - Rising trend:               1500ms (medium — watching closely)
            if sustained_hot_trend && gpu_load > 60 {
                500  // Hot + GPU-loaded: respond immediately to thermal events
            } else if sustained_hot_trend {
                1000 // Hot but low GPU: game_poll_interval default
            } else if clamped_trend > 15 {
                1500 // Rising trend: medium speed
            } else if gpu_load < 30 && (-5..=5).contains(&clamped_trend) {
                // Low GPU + stable temp: safe to poll slowly.
                // This saves significant CPU during cutscenes or menus.
                3000
            } else {
                ctx.config.profiles.game_poll_interval.saturating_mul(1000)
            }
        } else if sustained_hot_trend {
            750
        } else if clamped_trend > 15 {
            1500
        } else if long_idle {
            30_000
        } else if is_screen_off_now && (-2..=2).contains(&clamped_trend) {
            ctx.config.profiles.poll_interval.saturating_mul(4000)
        } else {
            ctx.config.profiles.poll_interval.saturating_mul(1000)
        };
        tracing::trace!(
            "adaptive sleep: base={}ms chosen={}ms trend={} sustained={} long_idle={} screen_off={} gaming={} gpu_load={}",
            ctx.config.profiles.poll_interval.saturating_mul(1000),
            sleep_ms,
            clamped_trend,
            sustained_hot_trend,
            long_idle,
            is_screen_off_now,
            is_gaming,
            gpu_load
        );

        (sleep_ms, long_idle)
    }

    fn check_throttle_limit(&self, base_ms: u64, is_gaming: bool) -> bool {
        if base_ms == 0 {
            return true;
        }
        // While a game is running, hold the floor at 3s. Burst-
        // rewriting governors mid-frame is worse than a slightly
        // stale policy, but 8s was too coarse to respond to thermal swings.
        let min_ms = if is_gaming {
            base_ms.max(3_000)
        } else {
            base_ms
        };
        match self.last_actuation_at {
            None => true,
            Some(t) => t.elapsed().as_millis() as u64 >= min_ms,
        }
    }

    fn actuation_allowed(&self, ctx: &RuntimeContext, is_gaming: bool) -> bool {
        if let Some(defer) = self.wake_defer_until
            && std::time::Instant::now() < defer {
                return false;
            }
        self.check_throttle_limit(ctx.config.profiles.min_actuation_interval_ms, is_gaming)
    }

    fn actuation_allowed_bypass_wake(&self, ctx: &RuntimeContext, is_gaming: bool) -> bool {
        // Same throttle as actuation_allowed but ignores wake_defer_until.
        self.check_throttle_limit(ctx.config.profiles.min_actuation_interval_ms, is_gaming)
    }
    fn get_context_score(
        wifi_active: bool,
        screen_brightness: i32,
        ambient_temp: i32,
        is_screen_off: bool,
        is_gaming: bool,
    ) -> f64 {
        let mut score = 0.0;

        // Incorporate screen state weight natively here
        if is_screen_off {
            score -= 30.0;
        } else if is_gaming {
            score += 15.0;
        } else {
            score += 5.0; // Base foreground weight when screen is on but not gaming
        }

        if wifi_active {
            score -= 2.0;
        } // Active radio generates heat
        if screen_brightness > 80 {
            score -= 3.0;
        } // High brightness generates heat
        if ambient_temp > 35 {
            score -= 5.0;
        } // High ambient temp reduces cooling efficiency
        score
    }

    fn get_cooling_efficiency(ema_trend: i32, gpu_load: u32, is_cooling: bool) -> f64 {
        let mut efficiency: f64 = 1.0;
        if is_cooling {
            efficiency += 0.5;
        }
        if ema_trend > 5 {
            efficiency -= 0.5;
        } // Heating rapidly
        if gpu_load > 80 {
            efficiency -= 0.3;
        } // High GPU load reduces efficiency
        efficiency.max(0.1)
    }

    fn compute_comfort_weight(
        skin_temp: i32,
        bat_temp: i32,
        is_cooling_slowly: bool,
        mem_pressure: f32,
    ) -> f64 {
        let mut base = 5.0;

        // Skin comfort only adds measured pressure once the phone is genuinely
        // hot to the touch. The old +15 for skin >= 42 alone inflated the
        // score by 25 and, together with a +25 trend, pushed a 50C device
        // into Powersave/EmergencyCool territory right after boot.
        if skin_temp >= 45 {
            base += 10.0;
        } else if skin_temp >= 42 {
            base += 5.0;
        }

        if bat_temp >= 45 {
            base += 8.0;
        } else if bat_temp >= 42 {
            base += 4.0;
        }

        if is_cooling_slowly {
            base += 3.0;
        }

        if mem_pressure > 80.0 {
            base += 3.0; // Memory pressure increases heat generation risk
        }

        base
    }
    pub fn new(ctx: &RuntimeContext, hardware: HardwareProfile) -> Self {
        let adaptive_governor = crate::scheduler::adaptive_governor::AdaptiveGovernorState::new(1);
        // Initialize subsystems
        let mut sensors = SensorManager::new();
        sensors.discover_hardware(&hardware);

        let mut governors = GovernorManager::new();
        governors.discover_hardware(&hardware);

        let mut cpuset = CpusetManager::new();
        cpuset.discover_hardware(&hardware);

        let gaming = GameDetector::new(
            ctx.config.games.packages.clone(),
            ctx.config.profiles.game_latch_sec,
            ctx.config.profiles.proc_scan_interval,
            ctx.config.profiles.game_poll_interval,
            ctx.config.profiles.pkg_cache_ttl,
        );

        let thermal = ThermalEngine::new(ctx.config.profiles.temp_history_size);
        let policy = PolicyEngine::new(
            ctx.config.profiles.policy_debounce_sec,
            ctx.config.profiles.poll_interval,
        );
        let prediction = PredictionEngine::new(ctx.config.profiles.prediction_window, 3); // 3 steps ahead

        let charging = ChargingEngine::new(&hardware, ctx.config.profiles.temp_warm, ctx.config.profiles.temp_hot);
        let watchdog = Watchdog::with_threshold(ctx.config.profiles.poll_interval, ctx.config.profiles.watchdog_stall_threshold);
        let recovery = RecoveryManager::new();
        let calibration = CalibrationManager::new(&ctx.state_dir);
        let snapshot = SnapshotManager::new(&ctx.state_dir, hardware.clone());

        // Restore snapshot early in startup if it exists, and verify policy
        if let Some(_snap) = snapshot.load_snapshot()
            && snapshot.verify_policy("Performance")
        {
            snapshot.restore_snapshot();
        }

        // Rehydrate any stale tuning state from a previous unclean exit
        // before we take our own baselines this run. (Done AFTER snapshot
        // restore so snapshot covers baseline cleanly).
        crate::tuning::RuntimeTuner::rehydrate_and_restore(&ctx.state_dir);

        let runtime_tuner = RuntimeTuner::new(hardware.clone())
            .with_state_dir(&ctx.state_dir)
            .with_network_config(
                &ctx.config.profiles.tcp_congestion_control_gaming,
                ctx.config.profiles.touch_network_stack,
            );

        Self {
            sensors,
            thermal,
            prediction,
            policy,
            governors,
            cpuset,
            charging,
            gaming,
            game_turbo: {
                let mut gt = crate::game_turbo::GameTurboEngine::new();
                gt.init_profiles(&ctx.state_dir);
                gt
            },
            watchdog,
            recovery,
            calibration,
            snapshot,
            hardware,
            runtime_tuner,
            game_profiles: crate::profiles::GameProfileManager::new(&ctx.state_dir),
            adaptive_governor,
            last_load_sample: std::collections::HashMap::new(),
            background_frame_sampler: crate::monitor::frame_sampler::BackgroundFrameSampler::new(),
            battery_stats: crate::telemetry::battery_stats::BatteryStatsTracker::new(),
            last_battery_log_time: None,
            last_battery_summary_time: None,
            last_actuation_at: None,
            wake_defer_until: None,
            recovery_applied_this_stall: false,
            pending_wake_nudge: false,
            last_applied_cpu_gov: None,
            last_applied_gpu_gov: None,
            last_applied_gpu_level: None,
            stock_thermal_disabled: None,
            last_telemetry_write_at: None,
            last_telemetry_policy: None,
            last_applied_policy: None,
            last_policy_change_at: None,
            last_network_probe: None,
            last_network_tweaks_applied: false,
        }
    }

    fn compute_game_modifier(
        &mut self,
        pkg: Option<&str>,
        ctx: &crate::runtime_context::RuntimeContext,
        is_gaming: bool,
    ) -> f64 {
        // Game modifier must only ever shape the score while a game is ACTIVE.
        // The confirmed package lingers a few ticks after game exit; keying on
        // the package alone leaked a -11 (known-hot) modifier into the
        // post-game cooldown scoring window.
        if !is_gaming {
            return 0.0;
        }

        let mut modifier = 0.0;

        // Frame stutter mitigation applies to every game, not just profiled
        // ones. The profile is only written at session end, so a game's first
        // session got zero stutter protection: once heat pushed the score over
        // the Balanced boundary the engine flip-flopped the governor mid-match.
        if self.gaming.detect_frame_stutter(ctx.game_session_started_at) {
            modifier -= 15.0;
        }

        if let Some(p) = pkg.and_then(|name| self.game_profiles.get_profile(name)) {
            if p.known_hot {
                modifier -= 12.0;
            }

            // Active foreground gaming priority influence
            let is_screen_off = crate::hardware::display::is_screen_off();
            let fg_priority = self.gaming.foreground_priority(p.known_hot, is_screen_off) as f64;
            modifier += fg_priority / 10.0;

            let active_secs = ctx
                .game_session_started_at
                .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if active_secs >= 1800 {
                modifier -= ((active_secs / 1800).saturating_sub(1) as f64) * 5.0;
                if let Some(policy) = &ctx.current_policy {
                    if policy == "EmergencyCool" || policy == "Powersave" {
                        modifier -= 5.0;
                    } else if policy == "Performance" {
                        modifier += 5.0;
                    }
                }
            }
        }

        modifier
    }

    fn policy_state_name(policy: &PolicyState) -> &'static str {
        match policy {
            PolicyState::Performance => "Performance",
            PolicyState::Balanced => "Balanced",
            PolicyState::Conservative => "Conservative",
            PolicyState::Powersave => "Powersave",
            PolicyState::EmergencyCool => "EmergencyCool",
            PolicyState::Suspend => "Suspend",
        }
    }

    fn resolve_sensor_name(&self, path_opt: Option<&String>) -> Option<String> {
        path_opt.and_then(|path| {
            self.hardware
                .thermal_profile
                .all_zones
                .iter()
                .find(|(_, p)| *p == path)
                .map(|(name, _)| name.clone())
        })
    }

    fn read_thermal_source(&self, path_opt: Option<&String>) -> Option<i32> {
        self.resolve_sensor_name(path_opt)
            .and_then(|name| self.sensors.read_sensor(&name))
    }

    fn select_gpu_governor(&self, preferred: &[&str]) -> Option<String> {
        for gov in preferred {
            if self
                .hardware
                .gpu_profile
                .available_governors
                .iter()
                .any(|g| g == gov)
            {
                return Some((*gov).to_string());
            }
        }
        if !self.hardware.gpu_profile.current_governor.is_empty()
            && self
                .hardware
                .gpu_profile
                .available_governors
                .iter()
                .any(|g| g == &self.hardware.gpu_profile.current_governor)
        {
            return Some(self.hardware.gpu_profile.current_governor.clone());
        }
        self.hardware
            .gpu_profile
            .available_governors
            .first()
            .cloned()
    }

    fn select_cpu_governor(&self, preferred: &[&str]) -> Option<String> {
        for gov in preferred {
            if self.hardware.cpu_topology.clusters.iter().all(|cluster| {
                cluster.governor_node.valid
                    && cluster.governor_node.writable
                    && cluster.available_governors.iter().any(|g| g == gov)
            }) {
                return Some((*gov).to_string());
            }
        }
        None
    }

    fn plug_state(&self) -> (bool, bool) {
        let known = &self.hardware.charging_profile.path;
        if !known.is_empty() {
            if let Ok(online) = crate::sysfs::read_i64(format!("{}/online", known)) {
                return (online > 0, true);
            }
            if let Ok(present) = crate::sysfs::read_i64(format!("{}/present", known)) {
                return (present > 0, true);
            }
        }

        let mut saw_power_supply = false;
        if let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") {
            for entry in entries.flatten() {
                let path = entry.path();
                let type_name = crate::sysfs::read_string(path.join("type"))
                    .unwrap_or_default()
                    .to_lowercase();
                if !(type_name.contains("usb")
                    || type_name.contains("mains")
                    || type_name.contains("wireless"))
                {
                    continue;
                }
                saw_power_supply = true;
                if let Ok(online) = crate::sysfs::read_i64(path.join("online"))
                    && online > 0
                {
                    return (true, true);
                }
                if let Ok(present) = crate::sysfs::read_i64(path.join("present"))
                    && present > 0
                {
                    return (true, true);
                }
            }
        }

        if saw_power_supply {
            return (false, true);
        }

        let status_path = format!("{}/status", self.hardware.battery_profile.path);
        let status =
            crate::sysfs::read_string(&status_path).unwrap_or_else(|_| "Discharging".to_string());
        (
            status.contains("Charging")
                || status.contains("Full")
                || status.contains("Not charging"),
            false,
        )
    }

    #[cfg(test)]
    fn new_for_test(hardware: HardwareProfile) -> Self {
        let (config, _) = crate::config::AppConfig::load_or_default("missing", "missing");
        let ctx = RuntimeContext {
            config: config.clone(),
            state_dir: String::new(),
            snapshot_taken: false,
            recovery_mode: false,
            initialized: false,
            runtime_health: true,
            battery_temp_c: 0,
            trend_score: 0,
            prev_hot_trend: false,
            sleep_ms: config.profiles.poll_interval.saturating_mul(1000),
            current_policy: None,
            current_game: None,
            cooldown_active: false,
            cooldown_until: None,
            cooldown_source_pkg: None,
            game_session_started_at: None,
            game_session_peak_temp: 0,
            last_session_peak_temp: 0,
            last_gaming_state: false,
            plugged_in_at: None,
            screen_off_since: None,
            game_session_worst_jank_pct: 0.0,
            game_session_worst_p90_ms: 0.0,
        };
        Self {
            sensors: SensorManager::new(),
            thermal: ThermalEngine::new(ctx.config.profiles.temp_history_size),
            prediction: PredictionEngine::new(ctx.config.profiles.prediction_window, 3),
            policy: PolicyEngine::new(
                ctx.config.profiles.policy_debounce_sec,
                ctx.config.profiles.poll_interval,
            ),
            governors: GovernorManager::new(),
            cpuset: CpusetManager::new(),
            charging: ChargingEngine::new(&hardware, 48, 58),
            gaming: GameDetector::new(Vec::new(), 0, 1, 1, 1),
            game_turbo: crate::game_turbo::GameTurboEngine::new(),
            watchdog: Watchdog::with_threshold(ctx.config.profiles.poll_interval, ctx.config.profiles.watchdog_stall_threshold),
            recovery: RecoveryManager::new(),
            calibration: CalibrationManager::new(""),
            snapshot: SnapshotManager::new("", hardware.clone()),
            hardware,
            runtime_tuner: RuntimeTuner::new(HardwareProfile::default()),
            game_profiles: crate::profiles::GameProfileManager::new(""),
            adaptive_governor: crate::scheduler::adaptive_governor::AdaptiveGovernorState::new(1),
            last_load_sample: std::collections::HashMap::new(),
            background_frame_sampler: crate::monitor::frame_sampler::BackgroundFrameSampler::new(),
            battery_stats: crate::telemetry::battery_stats::BatteryStatsTracker::new(),
            last_battery_log_time: None,
            last_battery_summary_time: None,
            last_actuation_at: None,
            wake_defer_until: None,
            recovery_applied_this_stall: false,
            pending_wake_nudge: false,
            last_applied_cpu_gov: None,
            last_applied_gpu_gov: None,
            last_applied_gpu_level: None,
            stock_thermal_disabled: None,
            last_telemetry_write_at: None,
            last_telemetry_policy: None,
            last_applied_policy: None,
            last_policy_change_at: None,
            last_network_probe: None,
            last_network_tweaks_applied: false,
        }
    }

    pub fn bootstrap(&mut self) -> Result<()> {
        info!("Bootstrapping SystemOrchestrator...");
        let mut paths = Vec::new();

        for cluster in &self.hardware.cpu_topology.clusters {
            paths.push(format!("{}/scaling_governor", cluster.policy_path));
        }

        if !self.hardware.charging_profile.path.is_empty() {
            paths.push(self.hardware.charging_profile.path.clone());
        }

        if !self.hardware.gpu_profile.path.is_empty() {
            paths.push(format!("{}/governor", self.hardware.gpu_profile.path));
            if self.hardware.gpu_profile.is_kgsl {
                paths.push(format!("{}/max_pwrlevel", self.hardware.gpu_profile.path));
            }
        }

        self.snapshot.take_snapshot(paths)?;
        Ok(())
    }

    // ─── v3.2.29: Network Diagnostics ─────────────────────────────────

    fn last_network_probe_interface(&self) -> Option<String> {
        crate::network_diag::cached_quality().map(|q| q.passive.interface)
    }

    /// Run the network quality probe in pure Rust (ICMP ping + sysfs).
    fn probe_network_quality(&mut self, state_dir: &str) {
        let quality = crate::network_diag::probe_quality(state_dir);
        let summary = crate::network_diag::quality_summary(&quality);
        tracing::info!(target: "network", "NET-PROBE {}", summary);
        crate::network_diag::cache_quality(quality);
    }

    /// Apply or restore gaming network tweaks via the shell script.
    fn apply_gaming_network_tweaks(&mut self, enable: bool) {
        let module_dir = std::env::var("THERMALAI_MODULE_DIR")
            .unwrap_or_else(|_| "/data/adb/modules/thermalai_rust".to_string());
        let script = std::path::PathBuf::from(&module_dir)
            .join("scripts/tweak_network_gaming.sh");

        if !script.exists() {
            tracing::debug!("Network tweak script not found: {}", script.display());
            return;
        }

        let action = if enable { "enable" } else { "disable" };
        let state_dir = std::env::var("THERMALAI_STATE_DIR")
            .unwrap_or_else(|_| "/data/local/tmp/AIThermal/state".to_string());
        let log_dir = std::env::var("THERMALAI_LOG_DIR")
            .unwrap_or_else(|_| "/data/local/tmp/AIThermal".to_string());

        let result = std::process::Command::new("sh")
            .arg(script.to_string_lossy().to_string())
            .arg(action)
            .arg(&state_dir)
            .arg(&log_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait());

        match result {
            Ok(status) if status.success() => {
                self.last_network_tweaks_applied = enable;
                tracing::info!(target: "network",
                    "NET-TWEAK {} network tweaks {}", if enable { "Applied" } else { "Restored" },
                    if enable { "for gaming" } else { "on game exit" }
                );
            }
            Ok(status) => {
                tracing::warn!(target: "network",
                    "NET-TWEAK {} exited with status: {}", action, status
                );
            }
            Err(e) => {
                tracing::warn!(target: "network", "NET-TWEAK failed to run {}: {}", action, e);
            }
        }
    }
}

impl RuntimeTask for SystemOrchestrator {
    fn cleanup(&mut self) {
        self.game_turbo.deactivate();
        self.charging.release_voters_on_shutdown();
        self.runtime_tuner.restore_all();
        self.runtime_tuner.restore_stock_thermal();
        self.stock_thermal_disabled = Some(false);
        self.last_applied_policy = None;
    }

    fn execute(&mut self, ctx: &mut RuntimeContext) -> Result<()> {
        let bat_temp_c = {
            let mut val = 350; // Assume tenths by default for power_supply
            let candidates = [
                format!("{}/temp", self.hardware.battery_profile.path),
                "/sys/class/power_supply/battery/temp".to_string(),
                "/sys/class/power_supply/bms/temp".to_string(),
                "/sys/class/power_supply/main/temp".to_string(),
            ];

            let mut found = false;
            for node in &candidates {
                if let Ok(v) = crate::sysfs::read_i64(node)
                    && v > 0
                {
                    val = v as i32;
                    found = true;
                    break;
                }
            }

            if found {
                val / 10 // Convert power_supply raw tenths to whole degrees
            } else {
                let bat_name =
                    self.resolve_sensor_name(self.hardware.thermal_profile.battery_zone.as_ref());
                bat_name
                    .and_then(|name| self.sensors.read_sensor(&name))
                    .unwrap_or(35)
            }
        };
        ctx.battery_temp_c = bat_temp_c;

        let is_running = ctx.runtime_health;

        let is_screen_off_now = crate::hardware::display::is_screen_off();
        let just_woke = ctx.screen_off_since.is_some() && !is_screen_off_now;
        if just_woke {
            // Wake burst protection: keep the defer window for TIGHTENING
            // transitions (Powersave/EmergencyCool/Suspend), but do NOT push
            // last_actuation_at forward - the loosening-bypass helper needs
            // it clean so the first post-wake tick can flip the governor
            // from powersave back to schedutil immediately.
            self.wake_defer_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(800));
            self.pending_wake_nudge = true;
            tracing::info!(target: "wake", "Screen wake detected; deferring actuation for 800ms");
        }

        // Defense-in-depth: the policy engine already exits Suspend instantly
        // on screen-on, but a wake must never leave the device on the bare
        // powersave governor even if the engine is mid-debounce/override or
        // the actuation throttle (1.5 s) hasn't cleared. Every non-Suspend
        // policy uses schedutil, so restoring it here can never conflict with
        // the policy this tick settles on.
        if just_woke
            && self.last_applied_cpu_gov.as_deref() == Some("powersave")
            && let Some(gov) = self.select_cpu_governor(&["schedutil"])
        {
            if let Err(e) = self.governors.apply_cpu_governor(&gov) {
                tracing::warn!("Failed to restore interactive governor on wake: {}", e);
            } else {
                self.last_applied_cpu_gov = Some(gov);
                tracing::debug!(target: "thermal", "Restored schedutil governor on wake");
            }
        }

        if just_woke
            && ctx
                .screen_off_since
                .map(|t| t.elapsed().as_secs() >= 10)
                .unwrap_or(false)
        {
            self.thermal.reset_after_long_sleep();
        }

        // 1. Watchdog
        match self.watchdog.check(is_running) {
            Ok(crate::watchdog::WatchdogVerdict::Healthy) => {
                self.recovery_applied_this_stall = false;
            }
            Ok(crate::watchdog::WatchdogVerdict::DegradedRestoreRecommended) => {
                warn!("Watchdog: degraded — restoring stock thermal governance");
                self.runtime_tuner.restore_stock_thermal();
            }
            Ok(crate::watchdog::WatchdogVerdict::StalledRecoverNow) => {
                warn!("Watchdog: stalled — restoring all sysfs originals");
                if !self.recovery_applied_this_stall {
                    self.runtime_tuner.restore_all();
                    self.runtime_tuner.restore_stock_thermal();
                    self.recovery_applied_this_stall = true;
                }
                ctx.recovery_mode = true;
            }
            Err(e) => tracing::debug!("Watchdog check error: {}", e),
        }

        // 2. Gaming state
        let was_gaming = ctx.last_gaming_state;
        let is_gaming = self.gaming.tick().unwrap_or(false);
        ctx.last_gaming_state = is_gaming;
        let confirmed_pkg = self.gaming.confirmed_package().map(|s| s.to_string());
        let now = std::time::Instant::now();

        if is_gaming && !was_gaming {
            ctx.game_session_started_at = Some(std::time::SystemTime::now());
            tracing::info!(
                target: "gaming",
                "Game detected: {}",
                confirmed_pkg.as_deref().unwrap_or("unknown")
            );
            // A stale negative calibration offset must not blind the thermal
            // model during a session: start each game with honest temps.
            self.calibration.reset_for_gaming_session();

            // v3.2.29: Run network quality detection and apply gaming tweaks
            if ctx.config.profiles.network_diagnostics_enabled {
                self.probe_network_quality(&ctx.state_dir);
            }
            if ctx.config.profiles.gaming_network_tweaks_enabled {
                self.apply_gaming_network_tweaks(true);
            }
            self.last_network_probe = Some(now);

            // v3.3.0: Activate GameTurbo engine for runtime gaming optimizations.
            if ctx.config.profiles.game_turbo_enabled
                && let Some(pid) = self.gaming.confirmed_pid
            {
                let gpu = &self.hardware.gpu_profile;
                // Use per-game GPU profile recommendation if available.
                let gpu_min = gpu.min_power_level;
                let gpu_max = gpu.max_power_level;
                let gpu_best = gpu_min.map(|m| m.min(gpu_max.unwrap_or(m))).unwrap_or(0);
                let gpu_worst = gpu_min.map(|m| m.max(gpu_max.unwrap_or(m))).unwrap_or(4);
                let recommended = confirmed_pkg.as_deref()
                    .and_then(|pkg| self.game_turbo.recommended_gpu_level(pkg, gpu_best, gpu_worst))
                    .unwrap_or(gpu_best);
                self.game_turbo.set_gpu_power_info(
                    gpu.power_level_path.clone(),
                    gpu.current_power_level,
                    recommended,
                );
                self.game_turbo.activate(pid, &ctx.config.profiles);
            }
        }

        if is_gaming {
            ctx.current_game = confirmed_pkg.clone();
            if ctx.cooldown_source_pkg != confirmed_pkg {
                ctx.cooldown_source_pkg = confirmed_pkg.clone();
                ctx.cooldown_until = None;
            }
        } else {
            ctx.current_game = None;
        }

        // v3.2.29: Periodic network quality re-probe during gaming
        if is_gaming && ctx.config.profiles.network_diagnostics_enabled {
            let probe_interval = ctx.config.profiles.network_probe_interval_sec;
            if probe_interval > 0 {
                let should_probe = self
                    .last_network_probe
                    .map(|t| t.elapsed().as_secs() >= probe_interval)
                    .unwrap_or(true);
                if should_probe {
                    self.probe_network_quality(&ctx.state_dir);
                    self.last_network_probe = Some(now);
                }
            }
        }

        // v3.3.0: GameTurbo per-tick refresh (re-scan newly spawned threads).
        // Also handles deferred activation when PID was unavailable at game detection.
        if is_gaming && ctx.config.profiles.game_turbo_enabled
            && let Some(pid) = self.gaming.confirmed_pid
            && !self.game_turbo.is_active()
        {
                // Deferred activation — PID became available after initial detection.
                let gpu = &self.hardware.gpu_profile;
                let gpu_min = gpu.min_power_level;
                let gpu_max = gpu.max_power_level;
                let gpu_best = gpu_min.map(|m| m.min(gpu_max.unwrap_or(m))).unwrap_or(0);
                let gpu_worst = gpu_min.map(|m| m.max(gpu_max.unwrap_or(m))).unwrap_or(4);
                let recommended = confirmed_pkg.as_deref()
                    .and_then(|pkg| self.game_turbo.recommended_gpu_level(pkg, gpu_best, gpu_worst))
                    .unwrap_or(gpu_best);
                self.game_turbo.set_gpu_power_info(
                    gpu.power_level_path.clone(),
                    gpu.current_power_level,
                    recommended,
                );
                self.game_turbo.activate(pid, &ctx.config.profiles);
            }
            // Note: game_turbo.tick() moved below to have access to gpu_load and comp_temp.

        // 3. Sensors & Thermal
        // Find the node name from the path stored in the thermal_profile.
        // `read_sensor` expects the `type_name` (e.g. "cpu_therm"), not the path.
        let cpu_temp = self
            .read_thermal_source(self.hardware.thermal_profile.cpu_zone.as_ref())
            .unwrap_or(40);

        let gpu_temp = self
            .read_thermal_source(self.hardware.thermal_profile.gpu_zone.as_ref())
            .unwrap_or(40);

        let bat_temp = ctx.battery_temp_c;
        let skin_temp = self
            .read_thermal_source(self.hardware.thermal_profile.skin_zone.as_ref())
            .unwrap_or(bat_temp); // Fallback to bat

        let gpu_load = crate::hardware::display::gpu_load_percent().unwrap_or({
            // No fake substitute. When GPU load truly can't be read, fall back
            // to CPU utilization-derived estimate if it was computed (we don't have it at this exact spot so 0),
            // or 0 if unavailable — never a fabricated "typical"
            // value that can drive false policy transitions.
            0
        });

        // Record GPU load for per-game profile learning (needs gpu_load which is now in scope).
        if is_gaming && ctx.config.profiles.game_turbo_enabled {
            self.game_turbo.record_gpu_load(gpu_load);
        }

        let comp_temp =
            ThermalEngine::composite_temp(cpu_temp, gpu_temp, bat_temp, skin_temp, gpu_load);

        // Apply calibration
        let adj_temp = comp_temp + self.calibration.active_offset;

        self.thermal.update(adj_temp);

        // v3.3.0: GameTurbo per-tick refresh (re-scan newly spawned threads).
        // Also handles deferred activation when PID was unavailable at game detection.
        if is_gaming
            && let Some(pid) = self.gaming.confirmed_pid
        {
            // adj_temp is always positive (temp in Celsius), safe to cast.
            let adj_temp_u32 = adj_temp.max(0) as u32;
            self.game_turbo.tick(pid, gpu_load, adj_temp_u32);
        }

        if is_gaming {
            if !was_gaming {
                ctx.game_session_peak_temp = adj_temp;
            } else {
                ctx.game_session_peak_temp = ctx.game_session_peak_temp.max(adj_temp);
            }

            // v3.3.0: Thermal-aware GameTurbo — ease constraints when hot.
            if ctx.config.profiles.game_turbo_enabled
                && let Some(pid) = self.gaming.confirmed_pid
            {
                self.game_turbo.thermal_adjust(
                    adj_temp,
                    ctx.config.profiles.temp_hot,
                    pid,
                );
            }
        }
        let is_cooling = self.thermal.is_cooling();
        // Calibration shifts offset only on a genuine rising ramp, not on
        // warm-but-flat idle (the latter drove offset to -6C during normal
        // use and masked all gaming heat).
        self.calibration.apply_calibration(self.thermal.is_heating());

        // 4. Prediction
        let mut predicted_temp = self.thermal.get_smoothed_temp();
        let mut trend_score = 0;
        #[allow(clippy::collapsible_if)]
        if let Some(pred) = self.prediction.predict(&self.thermal) {
            trend_score = pred.trend_score;
            if pred.confidence > 50 {
                predicted_temp = pred.predicted_temp;
            }
        }

        // 5. Policy
        if is_screen_off_now {
            if ctx.screen_off_since.is_none() {
                ctx.screen_off_since = Some(std::time::Instant::now());
            }
        } else {
            ctx.screen_off_since = None;
        }

        let game_modifier = self.compute_game_modifier(confirmed_pkg.as_deref(), ctx, is_gaming);
        let mem_pressure = self
            .hardware
            .memory_profile
            .memory_pressure_avg10
            .unwrap_or(0.0);

        let cpu_pressure = self
            .hardware
            .memory_profile
            .cpu_pressure_some_avg10
            .unwrap_or(0.0);

        let io_pressure = self
            .hardware
            .memory_profile
            .io_pressure_full_avg10
            .unwrap_or(0.0);

        let comfort_weight =
            Self::compute_comfort_weight(skin_temp, bat_temp, is_cooling, mem_pressure);

        //
        let wifi_active = crate::hardware::network::read_wifi_active();
        let screen_brightness = crate::hardware::display::read_screen_brightness_percent(
            self.hardware.display_profile.brightness_path.as_deref(),
            self.hardware.display_profile.max_brightness_path.as_deref(),
        );
        let ambient_temp = self.sensors.read_ambient_temp_c();

        let context_score = Self::get_context_score(
            wifi_active,
            screen_brightness,
            ambient_temp,
            is_screen_off_now,
            is_gaming,
        );
        let cooling_eff = Self::get_cooling_efficiency(trend_score, gpu_load, is_cooling);

        let final_context = context_score + cooling_eff;

        let desired_policy = self.policy.evaluate(
            adj_temp,
            predicted_temp,
            trend_score,
            is_gaming,
            is_screen_off_now,
            final_context,
            game_modifier,
            comfort_weight,
            cpu_pressure,
            io_pressure,
            &ctx.config.profiles,
            bat_temp_c,
            skin_temp,
        );

        // 6. Recovery overrides
        // 6. Post-game cooldown and session updates
        if was_gaming && !is_gaming {
            tracing::info!(
                target: "gaming",
                "Game session ended: {} (peak {}C) - clearing active game boosts",
                ctx.cooldown_source_pkg.as_deref().unwrap_or("unknown"),
                ctx.game_session_peak_temp
            );

            // v3.3.0: Deactivate GameTurbo engine — restore all runtime state.
            // Record per-game GPU profile before deactivating.
            let gpu_level_used = self.last_applied_gpu_level.unwrap_or(0);
            let pkg = ctx.cooldown_source_pkg.as_deref().unwrap_or("");
            self.game_turbo.record_session(
                pkg,
                gpu_level_used,
                ctx.game_session_worst_jank_pct,
            );
            // Wire per-app learning: FPS cap / network / thermal feedback
            // FPS cap learned from current cap (if jank low, keep it)
            if let Some(fps_cap) = self.game_turbo.recommended_fps_cap(pkg) {
                // Re-record with current jank to refine optimal_fps_cap
                self.game_turbo.record_fps_cap(pkg, fps_cap, ctx.game_session_worst_jank_pct);
            } else {
                // Seed fps cap from actual max_fps if no profile yet
                let cur_fps = self.game_turbo.current_fps_cap().unwrap_or(0);
                if cur_fps > 0 {
                    self.game_turbo.record_fps_cap(pkg, cur_fps, ctx.game_session_worst_jank_pct);
                }
            }
            // Network preference from last probe
            let net_type = match self.last_network_probe_interface() {
                Some(i) if i.starts_with("rmnet") => 2u8, // cellular
                Some(i) if i == "wlan0" => 1u8,
                _ => 0u8,
            };
            if net_type != 0 {
                self.game_turbo.record_network_profile(pkg, net_type);
            }
            // Thermal policy at session peak
            let therm_policy = if ctx.game_session_peak_temp >= ctx.config.profiles.temp_hot { 2 } else if ctx.game_session_peak_temp >= 50 { 1 } else { 0 };
            self.game_turbo.record_thermal_policy(pkg, therm_policy);
            self.game_turbo.deactivate();

            // Actively restore the normal usage profile to drop heat quickly
            self.runtime_tuner.restore_all();

            // Re-apply baseline policies for screen-on idle immediately
            if let Err(e) = self.cpuset.apply_cpuset("balanced", adj_temp, ctx.config.profiles.temp_hot) {
                tracing::warn!("Failed to apply cpuset during game exit restore: {}", e);
            }
            if let Some(gov) = self.select_cpu_governor(&["schedutil"]) {
                if let Err(e) = self.governors.apply_cpu_governor(&gov) {
                    tracing::warn!("Failed to apply CPU governor during game exit restore: {}", e);
                } else {
                    self.last_applied_cpu_gov = Some(gov);
                }
            }
            // Mild post-game GPU clamp to help the SoC shed heat. Uses the
            // WORST power level (max of the discovered pair — this device:
            // min=10, max=0 -> worst=10) so the old code's raw max (0) did
            // not leave the GPU boosted after game exit.
            let gpu_worst_at_exit = self
                .hardware
                .gpu_profile
                .min_power_level
                .unwrap_or(0)
                .max(self.hardware.gpu_profile.max_power_level.unwrap_or(4));
            if self.hardware.gpu_profile.max_power_level.is_some() {
                if let Err(e) = self
                    .governors
                    .apply_gpu_power_level(gpu_worst_at_exit.saturating_sub(1))
                {
                    tracing::warn!(
                        "GPU power level write failed at game exit: {}",
                        e
                    );
                }
                self.last_applied_gpu_level = Some(gpu_worst_at_exit.saturating_sub(1));
            }

            self.last_applied_policy = None;

            let pkg = ctx.cooldown_source_pkg.clone().unwrap_or_default();
            let cd_sec = self
                .game_profiles
                .get_profile(&pkg)
                .map(|p| p.cooldown_sec)
                .unwrap_or(90);
            ctx.cooldown_until = Some(now + std::time::Duration::from_secs(cd_sec));

            let session_secs = ctx
                .game_session_started_at
                .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if let Err(e) = self.game_profiles.update_session(
                &pkg,
                ctx.game_session_peak_temp,
                Self::policy_state_name(&desired_policy), // Using desired before overrides
                session_secs,
            ) {
                tracing::warn!("Failed to save game profile for {}: {}", pkg, e);
            }

            // Record GameTurbo-specific session stats for per-game learning.
            let turbo_throttled = self.game_turbo.was_thermally_throttled();
            if let Err(e) = self.game_profiles.record_game_turbo_session(
                &pkg,
                turbo_throttled,
                ctx.game_session_peak_temp,
                ctx.game_session_worst_jank_pct,
                ctx.game_session_worst_p90_ms,
            ) {
                tracing::warn!("Failed to record GameTurbo stats for {}: {}", pkg, e);
            }

            ctx.last_session_peak_temp = ctx.game_session_peak_temp;
            ctx.game_session_peak_temp = 0;
            ctx.game_session_worst_jank_pct = 0.0;
            ctx.game_session_worst_p90_ms = 0.0;
            ctx.game_session_started_at = None;
            ctx.current_game = None;

            // v3.2.29: Restore network tweaks on game exit
            if ctx.config.profiles.gaming_network_tweaks_enabled && self.last_network_tweaks_applied {
                self.apply_gaming_network_tweaks(false);
            }
        }

        let is_cooldown = ctx.cooldown_until.is_some_and(|t| t > now);

        // Evaluate post-game cooling when cooldown expires
        if !is_cooldown && ctx.cooldown_until.is_some() {
            self.calibration
                .evaluate_post_game_cooling(ctx.last_session_peak_temp, bat_temp);
            ctx.cooldown_until = None;
            ctx.cooldown_source_pkg = None;
        }

        // Cooldown only holds the Conservative clamp while the SoC is actually
        // still warm. A time-only 120s cooldown kept the CPU at 85% Fmax for
        // two full minutes after game exit even at 44C — the major UI stutter
        // the user hit after closing the game. Once the SoC drops below
        // temp_warm, release the clamp (cooldown_until stays armed in case it
        // reheats within the window).
        ctx.cooldown_active =
            is_cooldown && !is_gaming && adj_temp >= ctx.config.profiles.temp_warm;

        // 7. Recovery overrides & Final Policy Computation
        // desired_policy already reflects the PolicyEngine's debounce and hysteresis filtering.
        ctx.recovery_mode = self
            .recovery
            .check_recovery(&desired_policy, was_gaming, is_gaming);

        let final_policy = if desired_policy == PolicyState::EmergencyCool {
            // A real emergency (composite/predicted >= temp_critical, or a
            // high score at temp >= temp_hot) must always win over the
            // cooldown/recovery Conservative floor — otherwise EmergencyCool's
            // hard clamp would be silently downgraded to 85% exactly when the
            // SoC needs the most aggressive action.
            PolicyState::EmergencyCool
        } else if ctx.cooldown_active || ctx.recovery_mode {
            PolicyState::Conservative
        } else {
            desired_policy
        };

        // NOTE: no explicit unpin — tids die with the process and
        // cpuset entries are cleaned up by the kernel. Writing to
        // cpuset here would migrate SystemUI tasks and stall the
        // exit animation.

        // 8. Actuation (Governors, Cpuset, Runtime Tuning)
        let policy_str = Self::policy_state_name(&final_policy);

        let policy_changed = match &ctx.current_policy {
            Some(p) => p != policy_str,
            None => true,
        };

        if policy_changed {
            tracing::info!(target: "thermal",
                "Policy transition {} -> {} (score={:.1})",
                ctx.current_policy.as_deref().unwrap_or("None"),
                policy_str, self.policy.last_score());
            self.last_policy_change_at = Some(std::time::Instant::now());
        }

        // If the previous transition tick could not actuate (wake defer,
        // actuation throttle, etc.) the policy label was still committed
        // to ctx.current_policy. Track what we ACTUALLY applied and
        // retry on any subsequent tick where the effective state has
        // drifted from the intended one.
        let needs_apply = policy_changed || self.last_applied_policy.as_deref() != Some(policy_str);

        if policy_changed {
            crate::telemetry::trace_marker::emit(
                ctx.config.profiles.trace_markers_enabled,
                &format!("C|0|thermalai_policy|{}", policy_str),
            );
        }

        let in_hot_gameexit = self.recovery.phase == crate::recovery::RecoveryPhase::GameExit;

        // Check if tweaks are disabled
        let disable_tweaks = ctx.config.profiles.disable_tweaks;

        let hard_immediate = final_policy == PolicyState::EmergencyCool
            || final_policy == PolicyState::Suspend
            || ctx.recovery_mode;
        let can_actuate = self.actuation_allowed(ctx, is_gaming) || hard_immediate;

        // Any wake destination other than Suspend itself is a loosening
        // from Suspend and must bypass the 800 ms wake defer. Leaving
        // Powersave off this list caused the 5-7 s stutter at 09:24
        // when a wake landed directly in Powersave.
        let is_loosening_from_suspend = !matches!(final_policy, PolicyState::Suspend)
            && ctx.current_policy.as_deref() == Some("Suspend");

        let can_actuate = can_actuate
            || (is_loosening_from_suspend && self.actuation_allowed_bypass_wake(ctx, is_gaming));

        if can_actuate && self.pending_wake_nudge {
            self.adaptive_governor.nudge_on_screen_on();
            self.pending_wake_nudge = false;
        }

        if !disable_tweaks {
            // Pin critical render thread — retry for a short window after
            // session start because game engines commonly spawn the
            // RenderThread hundreds of ms to seconds AFTER the main process.
            if is_gaming
                && let Some(pid) = self.gaming.confirmed_pid
            {
                let session_young = ctx
                    .game_session_started_at
                    .and_then(|t| {
                        std::time::SystemTime::now()
                            .duration_since(t)
                            .ok()
                            .map(|d| d.as_secs() < 15)
                    })
                    .unwrap_or(false);
                if !was_gaming || session_young {
                    self.runtime_tuner
                        .pin_critical_render_thread(pid, "top-app");
                }
            }
        } else if needs_apply {
            tracing::info!(target: "tuning", "Tweaks disabled by config, skipping actuation for policy: {}", policy_str);
        }

        // Fallback logic for GPU governor if the requested one is not supported
        let gpu_gov_perf = self.select_gpu_governor(&["performance", "msm-adreno-tz"]);
        let gpu_gov_bal = self.select_gpu_governor(&["msm-adreno-tz", "simple_ondemand"]);
        let gpu_gov_save =
            self.select_gpu_governor(&["powersave", "msm-adreno-tz", "simple_ondemand"]);
        let cpu_gov_perf = self.select_cpu_governor(&["walt", "performance", "schedutil"]);
        // Balanced is the screen-on normal-usage governor. Stock on the
        // peridot (SM8635) WALT kernel is `walt`, whose input-boost and
        // load-tracking are tuned for the 120Hz UI; generic schedutil
        // under-ramps bursty UI workloads and shows as missed frame
        // deadlines. Prefer the stock governor, fall back to schedutil.
        let cpu_gov_bal = self.select_cpu_governor(&["walt", "schedutil"]);

        // Cooldown governor is always schedutil (never conservative)
        // to keep scrolling responsive after game exit.
        let cpu_gov_cons = self.select_cpu_governor(&["schedutil"]);

        // P3: Powersave/EmergencyCool use schedutil, only Suspend uses bare powersave.
        let cpu_gov_save = if final_policy == PolicyState::Suspend {
            self.select_cpu_governor(&["powersave", "schedutil"])
        } else {
            self.select_cpu_governor(&["schedutil"])
        };

        // KGSL power levels: LOWER index = HIGHER performance. The discovery
        // reports the raw bounds (this device: current=10, min=10, max=0) so
        // the "min"/"max" fields are inverted vs. intuition. Derive best/worst
        // from the pair — the old code wrote the raw min (10 = power-save)
        // for Performance/Balanced-gaming and crippled the GPU mid-game.
        let gpu_min = self.hardware.gpu_profile.min_power_level;
        let gpu_max = self.hardware.gpu_profile.max_power_level;
        let gpu_best = gpu_min.map(|m| m.min(gpu_max.unwrap_or(m))).unwrap_or(0);
        let gpu_worst = gpu_min.map(|m| m.max(gpu_max.unwrap_or(m))).unwrap_or(4);

        let gpu_level = match final_policy {
            PolicyState::Performance => gpu_best,
            PolicyState::Balanced if !is_gaming => gpu_worst.saturating_sub(1),
            PolicyState::Balanced => gpu_best,
            PolicyState::Conservative => gpu_worst.saturating_sub(1),
            PolicyState::Powersave => gpu_worst,
            PolicyState::EmergencyCool => gpu_worst,
            PolicyState::Suspend => gpu_worst,
        };

        // Grace period to avoid burst-apply stutter at game launch, tune threshold based on real-device testing.
        let game_grace_elapsed = ctx
            .game_session_started_at
            .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
            .map(|d| d.as_secs() >= 2)
            .unwrap_or(true);

        if !disable_tweaks
            && needs_apply
            && (final_policy != PolicyState::Performance || game_grace_elapsed)
            && can_actuate {
                self.last_actuation_at = Some(std::time::Instant::now());

                if let Err(e) = self.governors.apply_gpu_power_level(gpu_level) {
                    tracing::warn!("GPU power level write failed: {}", e);
                }
                self.last_applied_gpu_level = Some(gpu_level);

                match final_policy {
                    PolicyState::Performance => {
                        if !in_hot_gameexit {
                            if let Some(gov) = &cpu_gov_perf {
                                if let Err(e) = self.governors.apply_cpu_governor(gov) {
                                    tracing::warn!("Failed to apply CPU governor: {}", e);
                                } else {
                                    self.last_applied_cpu_gov = Some(gov.clone());
                                    tracing::debug!(target: "thermal", "Applied CPU governor: {}", gov);
                                }
                            } else {
                                tracing::warn!(
                                    "No common supported CPU governor for Performance policy"
                                );
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Holding CPU governor across GameExit hot phase");
                        }

                        for cluster in &self.hardware.cpu_topology.clusters {
                            if let Some(target) =
                                GovernorManager::max_freq(&cluster.available_frequencies)
                            {
                                let max_freq_path =
                                    format!("{}/scaling_max_freq", cluster.policy_path);
                                if crate::tuning::backend::TuningBackend::try_write_string(
                                    &max_freq_path,
                                    target.to_string(),
                                )
                                .is_ok()
                                {
                                    tracing::debug!(target: "governors", "Applied scaling_max_freq: {} to cluster {} via {}", target, cluster.name, max_freq_path);
                                }
                            }
                        }

                        if let Some(gov) = gpu_gov_perf {
                            if let Err(e) = self.governors.apply_gpu_governor(&gov) {
                                tracing::warn!("Failed to apply GPU governor: {}", e);
                            } else {
                                self.last_applied_gpu_gov = Some(gov.clone());
                                tracing::debug!(target: "thermal", "GPU governor -> {}", gov);
                            }
                        }
                        if !in_hot_gameexit {
                            if let Err(e) = self.cpuset.apply_cpuset("performance", adj_temp, ctx.config.profiles.temp_hot) {
                                tracing::warn!("Failed to apply cpuset: {}", e);
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Deferring cpuset rewrite: still in GameExit hot phase");
                        }
                    }
                    PolicyState::Balanced => {
                        if !in_hot_gameexit {
                            if let Some(gov) = &cpu_gov_bal {
                                if let Err(e) = self.governors.apply_cpu_governor(gov) {
                                    tracing::warn!("Failed to apply CPU governor: {}", e);
                                } else {
                                    self.last_applied_cpu_gov = Some(gov.clone());
                                    tracing::debug!(target: "thermal", "Applied CPU governor: {}", gov);
                                }
                            } else {
                                tracing::warn!(
                                    "No common supported CPU governor for Balanced policy"
                                );
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Holding CPU governor across GameExit hot phase");
                        }
                        if let Some(gov) = gpu_gov_bal {
                            if let Err(e) = self.governors.apply_gpu_governor(&gov) {
                                tracing::warn!("Failed to apply GPU governor: {}", e);
                            } else {
                                self.last_applied_gpu_gov = Some(gov.clone());
                                tracing::debug!(target: "thermal", "GPU governor -> {}", gov);
                            }
                        }
                        if !in_hot_gameexit {
                            if let Err(e) = self.cpuset.apply_cpuset("balanced", adj_temp, ctx.config.profiles.temp_hot) {
                                tracing::warn!("Failed to apply cpuset: {}", e);
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Deferring cpuset rewrite: still in GameExit hot phase");
                        }
                    }
                    PolicyState::Conservative => {
                        if !in_hot_gameexit {
                            if let Some(gov) = &cpu_gov_cons {
                                if let Err(e) = self.governors.apply_cpu_governor(gov) {
                                    tracing::warn!("Failed to apply CPU governor: {}", e);
                                } else {
                                    self.last_applied_cpu_gov = Some(gov.clone());
                                    tracing::debug!(target: "thermal", "Applied CPU governor: {}", gov);
                                }
                            } else {
                                tracing::warn!(
                                    "No common supported CPU governor for Conservative policy"
                                );
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Holding CPU governor across GameExit hot phase");
                        }
                        if let Some(gov) = gpu_gov_bal {
                            if let Err(e) = self.governors.apply_gpu_governor(&gov) {
                                tracing::warn!("Failed to apply GPU governor: {}", e);
                            } else {
                                self.last_applied_gpu_gov = Some(gov.clone());
                                tracing::debug!(target: "thermal", "GPU governor -> {}", gov);
                            }
                        }
                        if !in_hot_gameexit {
                            if let Err(e) = self.cpuset.apply_cpuset("balanced", adj_temp, ctx.config.profiles.temp_hot) {
                                tracing::warn!("Failed to apply cpuset: {}", e);
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Deferring cpuset rewrite: still in GameExit hot phase");
                        }
                    }
                    PolicyState::Powersave | PolicyState::EmergencyCool | PolicyState::Suspend => {
                        if !in_hot_gameexit {
                            if let Some(gov) = &cpu_gov_save {
                                if let Err(e) = self.governors.apply_cpu_governor(gov) {
                                    tracing::warn!("Failed to apply CPU governor: {}", e);
                                } else {
                                    self.last_applied_cpu_gov = Some(gov.clone());
                                    tracing::debug!(target: "thermal", "Applied CPU governor: {}", gov);
                                }
                            } else {
                                tracing::warn!(
                                    "No common supported CPU governor for Powersave policy"
                                );
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Holding CPU governor across GameExit hot phase");
                        }
                        if let Some(gov) = gpu_gov_save {
                            if let Err(e) = self.governors.apply_gpu_governor(&gov) {
                                tracing::warn!("Failed to apply GPU governor: {}", e);
                            } else {
                                self.last_applied_gpu_gov = Some(gov.clone());
                                tracing::debug!(target: "thermal", "GPU governor -> {}", gov);
                            }
                        }
                        if !in_hot_gameexit {
                            if let Err(e) = self.cpuset.apply_cpuset("powersave", adj_temp, ctx.config.profiles.temp_hot) {
                                tracing::warn!("Failed to apply cpuset: {}", e);
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Deferring cpuset rewrite: still in GameExit hot phase");
                        }
                    }
                }
            }

        // Update the background sampler's target package unconditionally based on game state,
        // so it always has the correct package to sample regardless of policy tier or throttle.
        if is_gaming {
            self.background_frame_sampler.set_target_package(confirmed_pkg.clone());
        } else {
            self.background_frame_sampler.set_target_package(None);
        }

        let should_sample = self.adaptive_governor.should_sample();
        tracing::debug!(target: "gaming", "Adaptive gating check: pkg={:?} policy={:?} should_sample={}", confirmed_pkg, final_policy, should_sample);
        if ctx.config.profiles.adaptive_governor_enabled
            && is_gaming
            && !ctx.recovery_mode
            && final_policy == PolicyState::Performance
            && should_sample {
                tracing::debug!(target: "gaming", "Adaptive gating check passed: reading latest stats");
                let frame_stats = self.background_frame_sampler.latest_stats();

                let current_stats = crate::monitor::load_sampler::read_cpu_stat();
                let utilization = if !self.last_load_sample.is_empty() {
                    // Average utilization across all CPU indices present in both samples.
                    let mut total_util = 0.0f32;
                    let mut count = 0;
                    for (idx, curr) in &current_stats {
                        if let Some(prev) = self.last_load_sample.get(idx) {
                            total_util +=
                                crate::monitor::load_sampler::compute_utilization(prev, curr);
                            count += 1;
                        }
                    }
                    if count > 0 {
                        total_util / count as f32
                    } else {
                        0.5
                    } // safe default only if no prior sample overlapped
                } else {
                    0.5 // first-ever sample this daemon run - no previous data to delta against yet
                };
                self.last_load_sample = current_stats;

                let tier = self
                    .adaptive_governor
                    .decide_tier(frame_stats.as_ref(), utilization, gpu_load as f32 / 100.0);

                if can_actuate {
                    self.last_actuation_at = Some(std::time::Instant::now());

                    // R2: When policy is one of the P3-clamped states, apply_cluster_settings
                    // owns scaling_max_freq. Do not fight it from adaptive_governor.
                    let p3_owns_max_freq = matches!(
                        policy_str,
                        "Powersave" | "Conservative" | "EmergencyCool" | "Suspend"
                    );
                    if p3_owns_max_freq {
                        // adaptive_governor still runs its tier/scoring logic for telemetry,
                        // but must not write scaling_max_freq while P3 has clamp authority.
                    } else {
                        for cluster in &self.hardware.cpu_topology.clusters {
                            let target = match tier {
                                crate::scheduler::adaptive_governor::FrequencyTier::Max => {
                                    crate::governors::GovernorManager::max_freq(
                                        &cluster.available_frequencies,
                                    )
                                }
                                crate::scheduler::adaptive_governor::FrequencyTier::High => {
                                    let min = crate::governors::GovernorManager::min_freq(
                                        &cluster.available_frequencies,
                                    )
                                    .unwrap_or(0);
                                    let max = crate::governors::GovernorManager::max_freq(
                                        &cluster.available_frequencies,
                                    )
                                    .unwrap_or(0);
                                    let midpoint = (min + max) / 2;
                                    // Snap to the closest value actually present in this cluster's real
                                    // frequency table, rather than trusting an arithmetic midpoint to be a
                                    // valid step.
                                    cluster
                                        .available_frequencies
                                        .iter()
                                        .copied()
                                        .min_by_key(|&f| (f as i64 - midpoint as i64).abs())
                                }
                                crate::scheduler::adaptive_governor::FrequencyTier::Balanced => {
                                    crate::governors::GovernorManager::mid_freq(
                                        &cluster.available_frequencies,
                                    )
                                }
                                crate::scheduler::adaptive_governor::FrequencyTier::Eco => {
                                    crate::governors::GovernorManager::min_freq(
                                        &cluster.available_frequencies,
                                    )
                                }
                            };

                            if let Some(freq) = target {
                                let path = format!("{}/scaling_max_freq", cluster.policy_path);
                                if crate::tuning::backend::TuningBackend::try_write_string(
                                    &path,
                                    freq.to_string(),
                                )
                                .is_ok()
                                {
                                    tracing::debug!(target: "adaptive_governor", "Tier {:?}: applied {} to cluster {} via {}", tier, freq, cluster.name, path);
                                }
                            }
                        }
                    }
                }
            }

        // Runtime Tuner application on policy transitions
        if !disable_tweaks && needs_apply {
            if can_actuate {
                self.last_actuation_at = Some(std::time::Instant::now());
                if let Err(e) = self.runtime_tuner.apply_network_tweaks(policy_str, is_gaming) {
                    tracing::warn!("Failed to apply network tweaks: {}", e);
                }
                if let Err(e) = self.runtime_tuner.apply_touch_display_tweaks(policy_str) {
                    tracing::warn!("Failed to apply touch display tweaks: {}", e);
                }
                self.runtime_tuner.apply_vm_params(policy_str);
                if !in_hot_gameexit
                    && let Err(e) = self.runtime_tuner.apply_scheduler(policy_str) {
                        tracing::warn!("Failed to apply scheduler: {}", e);
                    }
                // During recovery the final_policy is forced Conservative, but
                // its 85% Fmax clamp starves the exit animation for 20s. Use
                // the gentler Recovery clamp (90% Fmax) instead; every other
                // tuner only branches on is_perf/is_game and behaves identically.
                // Cooldown is routed through the same gentler clamp.
                // During gaming, keep walt governor and avoid schedutil transition
                // stall even if thermal policy is Conservative/Powersave.
                let tuning_policy = if ctx.recovery_mode || ctx.cooldown_active {
                    "Recovery"
                } else if is_gaming && !matches!(policy_str, "Performance" | "performance") {
                    // Gaming with thermal pressure: keep walt governor
                    // but apply a milder frequency clamp (90% vs 85%)
                    "Recovery"
                } else {
                    policy_str
                };
                self.runtime_tuner.apply_universal_cpu_tuning(tuning_policy);
                self.runtime_tuner.apply_universal_gpu_control(policy_str, is_gaming);

                // v3.2.4: advanced tuning pass — schedutil rate limits,
                // CFS/WALT responsiveness, deep-idle enable, zRAM algo,
                // F2FS gc_urgent, msm_performance powerhints. Every write
                // is capability-probed and idempotent, so no-op on kernels
                // that don't expose the knob.
                if ctx.config.profiles.advanced_tuning_enabled {
                    crate::tuning::advanced::apply_all(&self.hardware, policy_str);
                }
            }

            // Stock thermal enable/disable. Keyed on the GAMING STATE, not the
            // policy name: mid-game Performance<->Balanced score flapping must
            // never toggle mi_thermald on/off (each re-arm re-asserts stock
            // frequency caps on a hot SoC and stutters the game). Stock thermal
            // stays off for the whole session and is restored only after game
            // exit settles.
            let want_disabled = is_gaming;
            let currently_disabled = self.stock_thermal_disabled.unwrap_or(false);

            if want_disabled && !currently_disabled {
                self.runtime_tuner.disable_stock_thermal();
                self.stock_thermal_disabled = Some(true);
            } else if !want_disabled && currently_disabled && !in_hot_gameexit {
                // Hand control back to mi_thermald only AFTER the
                // exit animation has settled (>=4 s after game exit).
                self.runtime_tuner.restore_stock_thermal();
                self.stock_thermal_disabled = Some(false);
            } else if !want_disabled && self.stock_thermal_disabled.is_none() {
                // First tick after boot -> declare state = restored without a write.
                self.stock_thermal_disabled = Some(false);
            } else if !want_disabled && currently_disabled && in_hot_gameexit {
                tracing::debug!(target: "thermal", "Deferring restore_stock_thermal: still in GameExit hot phase");
            }

            // Drop cache transition logic
            if policy_str == "EmergencyCool" {
                if let Err(e) = self.runtime_tuner.drop_cache(true) {
                    tracing::warn!("Failed to drop cache: {}", e);
                }
            } else if policy_str == "Powersave" && mem_pressure > 40.0
                && let Err(e) = self.runtime_tuner.drop_cache(false) {
                    tracing::warn!("Failed to drop cache: {}", e);
                }
        }

        if can_actuate && needs_apply {
            self.last_applied_policy = Some(policy_str.to_string());
            crate::telemetry::trace_marker::emit(
                ctx.config.profiles.trace_markers_enabled,
                &format!("I|0|thermalai_apply {}", policy_str),
            );
        }

        // Final tick logging
        tracing::info!(target: "thermal",
            "tick temp cpu={}C gpu={}C bat={}C skin={}C composite={}C pred={}C trend={} policy={:?} cpu_gov={} gpu_gov={} gpu_lvl={} gaming={} screen_off={}",
            cpu_temp, gpu_temp, bat_temp_c, skin_temp, comp_temp, predicted_temp,
            trend_score, final_policy, self.last_applied_cpu_gov.as_deref().unwrap_or("?"), self.last_applied_gpu_gov.as_deref().unwrap_or("?"), self.last_applied_gpu_level.map(|v| v.to_string()).unwrap_or_else(|| "?".to_string()),
            is_gaming, is_screen_off_now);

        if is_gaming {
            let stats = self.background_frame_sampler.latest_stats();
            let (jank_str, p90_str) = match stats {
                Some(s) if s.captured_at.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(12)) => {
                    // Stale stats safety net: older than 12s (sampler cadence is 5s)
                    // means the sampler is frozen/failing
                    ("n/a".to_string(), "n/a".to_string())
                }
                Some(s)
                    if s.p90_frame_ns > 0
                        && s.p90_frame_ns < 500_000_000  // 500 ms sanity cap
                        && s.frame_count() >= 5 =>
                {
                    let jank_pct = s.jank_ratio() as f64 * 100.0;
                    let p90_ms = s.p90_frame_ns as f64 / 1_000_000.0;

                    // Track worst jank/p90 for the session.
                    if jank_pct > ctx.game_session_worst_jank_pct {
                        ctx.game_session_worst_jank_pct = jank_pct;
                    }
                    if p90_ms > ctx.game_session_worst_p90_ms {
                        ctx.game_session_worst_p90_ms = p90_ms;
                    }

                    (
                        format!("{:.2}", jank_pct),
                        format!("{:.1}ms", p90_ms),
                    )
                }
                Some(s) if s.frame_count() < 5 => {
                    // Diagnostic: Parse succeeded, but not enough frames captured
                    // yet in this sampling window to be statistically meaningful.
                    (
                        format!("insufficient_samples({})", s.frame_count()),
                        "n/a".to_string(),
                    )
                }
                _ => ("n/a".to_string(), "n/a".to_string()),
            };
            tracing::info!(target: "gaming",
                "tick pkg={} temp={}C policy={:?} gpu_load={}% jank={}% p90={} comfort={} session_peak={}C",
                confirmed_pkg.as_deref().unwrap_or("?"), comp_temp, final_policy,
                gpu_load, jank_str, p90_str, comfort_weight, ctx.game_session_peak_temp);
        }

        // 9. Charging
        let soc_path = format!("{}/capacity", self.hardware.battery_profile.path);
        let soc = crate::sysfs::read_i64(&soc_path)
            .unwrap_or(50)
            .clamp(0, 100) as u8;

        let (is_plugged, plug_state_reliable) = self.plug_state();

        let c_temp = self
            .read_thermal_source(self.hardware.thermal_profile.charger_zone.as_ref())
            .unwrap_or(bat_temp);

        let u_temp = self
            .read_thermal_source(self.hardware.thermal_profile.usbc_zone.as_ref())
            .unwrap_or(bat_temp);

        let p_temp = self
            .read_thermal_source(self.hardware.thermal_profile.pmic_zone.as_ref())
            .unwrap_or(bat_temp);

        let now = std::time::Instant::now();
        if is_plugged {
            if ctx.plugged_in_at.is_none() {
                ctx.plugged_in_at = Some(now);
                tracing::info!(target: "charging", "Charger connected");
            }
        } else {
            if ctx.plugged_in_at.is_some() {
                tracing::info!(target: "charging", "Charger disconnected");
            }
            ctx.plugged_in_at = None;
        }

        let seconds_since_plugged = ctx
            .plugged_in_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        let current_now_ua = {
            let path = format!("{}/current_now", self.hardware.battery_profile.path);
            crate::sysfs::read_i64(&path).ok().or_else(|| {
                let p2 = "/sys/class/power_supply/battery/current_now";
                crate::sysfs::read_i64(p2).ok()
            })
        };

        let voltage_now_uv = {
            let path = format!("{}/voltage_now", self.hardware.battery_profile.path);
            crate::sysfs::read_i64(&path).ok().or_else(|| {
                let p2 = "/sys/class/power_supply/battery/voltage_now";
                crate::sysfs::read_i64(p2).ok()
            })
        };

        let charge_counter_uah = {
            let path = format!("{}/charge_counter", self.hardware.battery_profile.path);
            crate::sysfs::read_i64(&path).ok().or_else(|| {
                let p2 = "/sys/class/power_supply/battery/charge_counter";
                crate::sysfs::read_i64(p2).ok()
            })
        };

        let charging_inputs = crate::charging::ChargingInputs {
            battery_temp: bat_temp,
            charger_temp: c_temp,
            usb_temp: u_temp,
            pmic_temp: p_temp,
            soc,
            is_plugged,
            plug_state_reliable,
            is_gaming,
            screen_off: is_screen_off_now,
            gpu_load,
            urgent: false,
            seconds_since_plugged,
            charger_id: self.hardware.charging_profile.path.clone(),
            current_now_ua,
            voltage_now_uv,
            charge_counter_uah,
            composite_temp: adj_temp,
        };
        self.charging
            .evaluate(&charging_inputs, &ctx.state_dir, &self.hardware);

        // 10. Adaptive Sleep
        let (sleep_ms, long_idle) = self.calculate_adaptive_sleep(ctx, trend_score, is_screen_off_now, is_gaming, gpu_load);
        ctx.sleep_ms = sleep_ms;

        if !needs_apply {
            // no-op
        } else if !can_actuate {
            tracing::debug!(target: "actuation",
                "policy drift: intended={} applied={:?} - actuation deferred (wake or throttle)",
                policy_str, self.last_applied_policy);
        }

        if just_woke {
            // Cap the pending sleep so the screen-on tick lands immediately.
            ctx.sleep_ms = ctx.sleep_ms.min(400);
        }

        let tick_interval_secs = ctx.sleep_ms / 1000;

        ctx.current_policy = Some(Self::policy_state_name(&final_policy).to_string());

        if ctx.config.profiles.battery_stats_enabled {
            let drain_rate = self.battery_stats.record_sample(
                bat_temp,
                soc,
                current_now_ua,
                !is_screen_off_now,
                is_gaming,
                is_plugged,
                long_idle,
                tick_interval_secs,
            );

            let should_log = self
                .last_battery_log_time
                .map(|t| t.elapsed().as_secs() >= 30)
                .unwrap_or(true);

            if should_log {
                tracing::info!(
                    target: "battery",
                    "batt_temp={}C soc={}% current_ua={} drain={}%/hr screen_on={} gaming={} charging={}",
                    bat_temp, soc,
                    current_now_ua.map(|v| v.to_string()).unwrap_or_else(|| "?".to_string()),
                    drain_rate.map(|d| format!("{:.2}", d.percent_per_hour)).unwrap_or_else(|| "?".to_string()),
                    !is_screen_off_now, is_gaming, is_plugged
                );
                self.last_battery_log_time = Some(std::time::Instant::now());
            }

            // also periodically log summary line, maybe every 10 min
            let should_summary = self
                .last_battery_summary_time
                .map(|t| t.elapsed().as_secs() >= 600)
                .unwrap_or(true);
            if should_summary {
                tracing::info!(target: "battery", "summary: {}", self.battery_stats.summary_line());
                self.last_battery_summary_time = Some(std::time::Instant::now());
            }
        }

        // 11. JSON Telemetry
        let telemetry = serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "ai_temp": adj_temp,
            "predicted_temp": predicted_temp,
            "policy": Self::policy_state_name(&final_policy),
            "gpu_load": gpu_load,
            "gaming": is_gaming,
            "game_pkg": ctx.current_game.clone().unwrap_or_default(),
            "batt_temp": bat_temp,
            "charge_state": format!("{:?}", self.charging.current_state),
            "charge_limit_ma": self.charging.active_limit_ma,
            "trend_score": ctx.trend_score,
            "screen_state": !is_screen_off_now,
            "mem_pressure": mem_pressure,
            "cpu_pressure_some_avg10": cpu_pressure,
            "io_pressure_full_avg10": io_pressure,
            "slow_cooler": is_cooling,
            "session_count": ctx.current_game
                .as_deref()
                .and_then(|pkg| self.game_profiles.get_profile(pkg))
                .map(|p| p.session_count)
                .unwrap_or(0),
            "calibration_offset": self.calibration.active_offset,
            "slow_cooler_persistent": self.calibration.slow_cooler_persistent,
            "sleep_ms": ctx.sleep_ms,
            "session_peak_temp": ctx.game_session_peak_temp,
            "session_started_at": ctx.game_session_started_at.map(|t| {
                std::time::SystemTime::now()
                    .duration_since(t)
                    .ok()
                    .map(|d| chrono::Utc::now().timestamp() - d.as_secs() as i64)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp())
            }),
            // Extra fields consumed by the KernelSU WebUI - always present
            // (null when inactive) so the UI never has to guess a schema.
            "cooldown_active": ctx.cooldown_active,
            "cooldown_source_pkg": ctx.cooldown_source_pkg,
            "plugged_in": ctx.plugged_in_at.is_some(),
            "screen_off": is_screen_off_now,
            "recovery_mode": ctx.recovery_mode,
            "runtime_health": ctx.runtime_health,
            "legacy_write_failures": crate::tuning::backend::TuningBackend::legacy_write_failure_count(),
            "frame_stats_parse_ok": crate::monitor::frame_sampler::last_parse_ok(),
            "frame_p50_us": self.background_frame_sampler.latest_stats().as_ref().map(|s| s.p50_frame_ns / 1000),
            "frame_p90_us": self.background_frame_sampler.latest_stats().as_ref().map(|s| s.p90_frame_ns / 1000),
            "frame_worst_us": self.background_frame_sampler.latest_stats().as_ref().map(|s| s.worst_frame_ns / 1000),
            "frame_max_consecutive_jank": self.background_frame_sampler.latest_stats().as_ref().map(|s| s.max_consecutive_jank),
            "recovery_phase": format!("{:?}", self.recovery.phase),
            "adaptive_tier": format!("{:?}", self.adaptive_governor.current_tier),
            "last_applied_policy": self.last_applied_policy.clone().unwrap_or_else(|| "None".to_string()),
            "gpu_power_level": self.last_applied_gpu_level,
            "charge_control_node": self.charging.limit_nodes.first().cloned(),
            "qcom_voter_count": self.charging.voter_nodes.len(),
            "charge_mode": format!("{:?}", self.charging.charge_mode),
            "restrict_chg_active": self.charging.voter_nodes.iter()
                .any(|n| n.ends_with("/restrict_chg"))
                && self.charging.charge_mode == crate::charging::ChargeMode::BatteryCare,
            "cycle_count": self.hardware.charging_profile.cycle_count,
            "cycle_taper_factor": self.hardware.charging_profile.cycle_count.map(|c| {
                match c {
                    0..=200      => 1.00,
                    201..=400    => 0.97,
                    401..=700    => 0.93,
                    701..=1000   => 0.89,
                    _            => 0.85,
                }
            }).unwrap_or(1.0),
            "game_turbo_active": self.game_turbo.is_active(),
        });

        let policy_now = Self::policy_state_name(&final_policy).to_string();
        let due_time = self
            .last_telemetry_write_at
            .map(|t| t.elapsed().as_millis() >= 2000)
            .unwrap_or(true);
        let policy_changed_for_ui =
            self.last_telemetry_policy.as_deref() != Some(policy_now.as_str());

        if due_time || policy_changed_for_ui || ctx.recovery_mode {
            crate::telemetry::writer::write_telemetry(ctx, &telemetry);
            self.last_telemetry_write_at = Some(std::time::Instant::now());
            self.last_telemetry_policy = Some(policy_now);
        }

        self.watchdog.mark_healthy();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_policy_uses_supported_governor_before_preferred_name() {
        let mut hardware = HardwareProfile::default();
        hardware.gpu_profile.current_governor = "msm-adreno-tz".to_string();
        hardware.gpu_profile.available_governors = vec!["msm-adreno-tz".to_string()];

        let orchestrator = SystemOrchestrator::new_for_test(hardware);
        assert_eq!(
            orchestrator.select_gpu_governor(&["performance", "msm-adreno-tz"]),
            Some("msm-adreno-tz".to_string())
        );
    }

    #[test]
    fn gpu_policy_falls_back_to_current_valid_governor() {
        let mut hardware = HardwareProfile::default();
        hardware.gpu_profile.current_governor = "vendor-safe".to_string();
        hardware.gpu_profile.available_governors = vec!["vendor-safe".to_string()];

        let orchestrator = SystemOrchestrator::new_for_test(hardware);
        assert_eq!(
            orchestrator.select_gpu_governor(&["performance"]),
            Some("vendor-safe".to_string())
        );
    }

    #[test]
    fn cpu_policy_prefers_walt_only_when_all_clusters_support_it() {
        let mut hardware = HardwareProfile::default();
        for id in 0..2 {
            hardware
                .cpu_topology
                .clusters
                .push(crate::hardware::profile::CpuCluster {
                    name: format!("policy{}", id),
                    governor_node: crate::hardware::capability::CapabilityNode {
                        path: format!(
                            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                            id
                        ),
                        valid: true,
                        writable: true,
                        ..Default::default()
                    },
                    available_governors: vec![
                        "walt".to_string(),
                        "performance".to_string(),
                        "schedutil".to_string(),
                    ],
                    ..Default::default()
                });
        }

        let orchestrator = SystemOrchestrator::new_for_test(hardware);
        assert_eq!(
            orchestrator.select_cpu_governor(&["walt", "performance", "schedutil"]),
            Some("walt".to_string())
        );
    }

    #[test]
    fn cpu_policy_falls_back_when_walt_is_partial() {
        let mut hardware = HardwareProfile::default();
        let governor_sets = [
            vec!["walt".to_string(), "performance".to_string()],
            vec!["performance".to_string(), "schedutil".to_string()],
        ];
        for (id, governors) in governor_sets.into_iter().enumerate() {
            hardware
                .cpu_topology
                .clusters
                .push(crate::hardware::profile::CpuCluster {
                    name: format!("policy{}", id),
                    governor_node: crate::hardware::capability::CapabilityNode {
                        path: format!(
                            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                            id
                        ),
                        valid: true,
                        writable: true,
                        ..Default::default()
                    },
                    available_governors: governors,
                    ..Default::default()
                });
        }

        let orchestrator = SystemOrchestrator::new_for_test(hardware);
        assert_eq!(
            orchestrator.select_cpu_governor(&["walt", "performance", "schedutil"]),
            Some("performance".to_string())
        );
    }
}
