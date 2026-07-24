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
}

impl SystemOrchestrator {
    fn actuation_allowed(&self, ctx: &RuntimeContext, is_gaming: bool) -> bool {
        if let Some(defer) = self.wake_defer_until {
            if std::time::Instant::now() < defer { return false; }
        }
        let base_ms = ctx.config.profiles.min_actuation_interval_ms;
        if base_ms == 0 {
            return true;
        }
        // While a game is running, hold the floor at 8 s. Burst-
        // rewriting governors mid-frame is worse than a slightly
        // stale policy.
        let min_ms = if is_gaming { base_ms.max(8_000) } else { base_ms };
        match self.last_actuation_at {
            None => true,
            Some(t) => t.elapsed().as_millis() as u64 >= min_ms,
        }
    }

    fn actuation_allowed_bypass_wake(&self, ctx: &RuntimeContext, is_gaming: bool) -> bool {
        // Same throttle as actuation_allowed but ignores wake_defer_until.
        let base_ms = ctx.config.profiles.min_actuation_interval_ms;
        if base_ms == 0 { return true; }
        let min_ms = if is_gaming { base_ms.max(8_000) } else { base_ms };
        match self.last_actuation_at {
            None => true,
            Some(t) => t.elapsed().as_millis() as u64 >= min_ms,
        }
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
        let mut base = 10.0;

        //
        if skin_temp >= 42 {
            base += 15.0;
        } else if skin_temp >= 40 {
            base += 8.0;
        }

        if bat_temp >= 45 {
            base += 15.0;
        } else if bat_temp >= 42 {
            base += 8.0;
        }

        if is_cooling_slowly {
            base += 5.0;
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
        let _ = governors.discover_hardware(&hardware);

        let mut cpuset = CpusetManager::new();
        cpuset.discover_hardware(&hardware);

        let gaming = GameDetector::new(
            ctx.config.games.packages.clone(),
            ctx.config.profiles.game_latch_sec,
            ctx.config.profiles.proc_scan_interval,
        );

        let thermal = ThermalEngine::new(ctx.config.profiles.temp_history_size);
        let policy = PolicyEngine::new(
            ctx.config.profiles.policy_debounce_sec,
            ctx.config.profiles.poll_interval,
        );
        let prediction = PredictionEngine::new(ctx.config.profiles.prediction_window, 3); // 3 steps ahead

        let charging = ChargingEngine::new(&hardware);
        let watchdog = Watchdog::new(ctx.config.profiles.poll_interval);
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
        }
    }

    fn compute_game_modifier(
        &self,
        pkg: Option<&str>,
        ctx: &crate::runtime_context::RuntimeContext,
    ) -> f64 {
        if let Some(p) = pkg.and_then(|name| self.game_profiles.get_profile(name)) {
            let mut modifier = if p.known_hot { -12.0 } else { 0.0 };

            // Active foreground gaming priority influence
            let is_screen_off = crate::hardware::display::is_screen_off();
            let fg_priority = self.gaming.foreground_priority(p.known_hot, is_screen_off) as f64;
            modifier += fg_priority / 10.0;

            // Frame stutter penalty mitigation
            if self
                .gaming
                .detect_frame_stutter(ctx.game_session_started_at)
            {
                modifier += 15.0; // Boost performance score to mitigate stutter
            }

            let active_secs = ctx
                .game_session_started_at
                .map(|t: std::time::Instant| t.elapsed().as_secs())
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
            modifier
        } else {
            0.0
        }
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
            last_gaming_state: false,
            plugged_in_at: None,
            screen_off_since: None,
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
            charging: ChargingEngine::new(&hardware),
            gaming: GameDetector::new(Vec::new(), 0, 1),
            watchdog: Watchdog::new(ctx.config.profiles.poll_interval),
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
}

impl RuntimeTask for SystemOrchestrator {
    fn cleanup(&mut self) {
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
            self.wake_defer_until = Some(std::time::Instant::now()
                + std::time::Duration::from_millis(800));
            self.pending_wake_nudge = true;
            tracing::info!(target: "wake", "Screen wake detected; deferring actuation for 800ms");
        }

        if just_woke
            && ctx.screen_off_since
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
            ctx.game_session_started_at = Some(now);
            tracing::info!(
                target: "gaming",
                "Game detected: {}",
                confirmed_pkg.as_deref().unwrap_or("unknown")
            );
            // Peak temp will be set below after sensors
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

        let gpu_load = crate::hardware::display::gpu_load_percent().unwrap_or(50);
        let comp_temp =
            ThermalEngine::composite_temp(cpu_temp, gpu_temp, bat_temp, skin_temp, gpu_load);

        // Apply calibration
        let adj_temp = comp_temp + self.calibration.active_offset;

        self.thermal.update(adj_temp);

        if is_gaming {
            if !was_gaming {
                ctx.game_session_peak_temp = adj_temp;
            } else {
                ctx.game_session_peak_temp = ctx.game_session_peak_temp.max(adj_temp);
            }
        }
        let is_cooling = self.thermal.is_cooling();
        self.calibration.apply_calibration(!is_cooling);

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

        let game_modifier = self.compute_game_modifier(confirmed_pkg.as_deref(), ctx);
        let mem_pressure = self
            .hardware
            .memory_profile
            .memory_pressure_avg10
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
            &ctx.config.profiles,
        );

        // 6. Recovery overrides
        // 6. Post-game cooldown and session updates
        if was_gaming && !is_gaming {
            tracing::info!(
                target: "gaming",
                "Game session ended: {} (peak {}C)",
                ctx.cooldown_source_pkg.as_deref().unwrap_or("unknown"),
                ctx.game_session_peak_temp
            );
            let pkg = ctx.cooldown_source_pkg.clone().unwrap_or_default();
            let cd_sec = self
                .game_profiles
                .get_profile(&pkg)
                .map(|p| p.cooldown_sec)
                .unwrap_or(90);
            ctx.cooldown_until = Some(now + std::time::Duration::from_secs(cd_sec));

            let session_secs = ctx
                .game_session_started_at
                .map(|t: std::time::Instant| t.elapsed().as_secs())
                .unwrap_or(0);
            if let Err(e) = self.game_profiles.update_session(
                &pkg,
                ctx.game_session_peak_temp,
                Self::policy_state_name(&desired_policy), // Using desired before overrides
                session_secs,
            ) {
                tracing::warn!("Failed to save game profile for {}: {}", pkg, e);
            }

            ctx.game_session_peak_temp = 0;
            ctx.game_session_started_at = None;
            ctx.current_game = None;
        }

        let is_cooldown = ctx.cooldown_until.is_some_and(|t| t > now);

        // Evaluate post-game cooling when cooldown expires
        if !is_cooldown && ctx.cooldown_until.is_some() {
            self.calibration
                .evaluate_post_game_cooling(ctx.game_session_peak_temp, bat_temp);
            ctx.cooldown_until = None;
            ctx.cooldown_source_pkg = None;
        }

        ctx.cooldown_active = is_cooldown && !is_gaming;

        // 7. Recovery overrides & Final Policy Computation
        ctx.recovery_mode = self
            .recovery
            .check_recovery(&desired_policy, was_gaming, is_gaming);

        let final_policy = if ctx.cooldown_active || ctx.recovery_mode {
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

        // If the previous transition tick could not actuate (wake defer,
        // actuation throttle, etc.) the policy label was still committed
        // to ctx.current_policy. Track what we ACTUALLY applied and
        // retry on any subsequent tick where the effective state has
        // drifted from the intended one.
        let needs_apply = policy_changed
            || self.last_applied_policy.as_deref() != Some(policy_str);

        let in_hot_gameexit =
            self.recovery.phase == crate::recovery::RecoveryPhase::GameExit;

        // Check if tweaks are disabled
        let disable_tweaks = ctx.config.profiles.disable_tweaks;

        let hard_immediate = final_policy == PolicyState::EmergencyCool || final_policy == PolicyState::Suspend || ctx.recovery_mode;
        let can_actuate = self.actuation_allowed(ctx, is_gaming) || hard_immediate;

        // Loosening transitions (any policy that is NOT Suspend/Powersave)
        // coming out of Suspend must actuate immediately, or the CPU stays
        // pinned to powersave until the next transition and the user sees
        // lag on every screen wake.
        let is_loosening_from_suspend = matches!(
            final_policy,
            PolicyState::Balanced | PolicyState::Performance | PolicyState::Conservative
        ) && ctx.current_policy.as_deref() == Some("Suspend");

        let can_actuate = can_actuate
            || (is_loosening_from_suspend
                && self.actuation_allowed_bypass_wake(ctx, is_gaming));

        if can_actuate && self.pending_wake_nudge {
            self.adaptive_governor.nudge_on_screen_on();
            self.pending_wake_nudge = false;
        }

        if !disable_tweaks {
            // If game was just detected and confirmed, try pinning critical render thread
            if is_gaming && !was_gaming {
                if let Some(pid) = self.gaming.confirmed_pid {
                    self.runtime_tuner.pin_critical_render_thread(pid, "top-app");
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
        let cpu_gov_bal = self.select_cpu_governor(&["schedutil", "walt"]);

        // Cooldown governor is always schedutil (never conservative)
        // to keep scrolling responsive after game exit.
        let cpu_gov_cons = self.select_cpu_governor(&["schedutil"]);

        let cpu_gov_save = self.select_cpu_governor(&["powersave", "schedutil"]);


        let gpu_level = match final_policy {
            PolicyState::Performance             => self.hardware.gpu_profile.min_power_level.unwrap_or(0),
            PolicyState::Balanced if !is_gaming  => self.hardware.gpu_profile.max_power_level.unwrap_or(4).saturating_sub(1),
            PolicyState::Balanced                => self.hardware.gpu_profile.min_power_level.unwrap_or(0),
            PolicyState::Conservative            => self.hardware.gpu_profile.max_power_level.unwrap_or(4).saturating_sub(1),
            PolicyState::Powersave               => self.hardware.gpu_profile.max_power_level.unwrap_or(4),
            PolicyState::EmergencyCool           => self.hardware.gpu_profile.max_power_level.unwrap_or(4),
            PolicyState::Suspend                 => self.hardware.gpu_profile.max_power_level.unwrap_or(4),
        };

        // Grace period to avoid burst-apply stutter at game launch, tune threshold based on real-device testing.
        let game_grace_elapsed = ctx
            .game_session_started_at
            .map(|t| t.elapsed().as_secs() >= 2)
            .unwrap_or(true);

        if !disable_tweaks && needs_apply && (final_policy != PolicyState::Performance || game_grace_elapsed) {
            if can_actuate {
                self.last_actuation_at = Some(std::time::Instant::now());

                let _ = self.governors.apply_gpu_power_level(gpu_level);
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
                            tracing::warn!("No common supported CPU governor for Performance policy");
                        }
                    } else {
                        tracing::debug!(target: "thermal", "Holding CPU governor across GameExit hot phase");
                    }

                    for cluster in &self.hardware.cpu_topology.clusters {
                        if let Some(target) = GovernorManager::max_freq(&cluster.available_frequencies) {
                            let max_freq_path = format!("{}/scaling_max_freq", cluster.policy_path);
                            if crate::tuning::backend::TuningBackend::try_write_string(
                                &max_freq_path, &target.to_string()
                            ).is_ok() {
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
                        if let Err(e) = self.cpuset.apply_cpuset("performance") {
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
                            tracing::warn!("No common supported CPU governor for Balanced policy");
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
                        if let Err(e) = self.cpuset.apply_cpuset("balanced") {
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
                            tracing::warn!("No common supported CPU governor for Conservative policy");
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
                        if let Err(e) = self.cpuset.apply_cpuset("balanced") {
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
                            tracing::warn!("No common supported CPU governor for Powersave policy");
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
                            if let Err(e) = self.cpuset.apply_cpuset("powersave") {
                                tracing::warn!("Failed to apply cpuset: {}", e);
                            }
                        } else {
                            tracing::debug!(target: "thermal", "Deferring cpuset rewrite: still in GameExit hot phase");
                        }
                    }
                }
            }
        }

        if ctx.config.profiles.adaptive_governor_enabled
            && is_gaming
            && !ctx.recovery_mode
            && final_policy == PolicyState::Performance
        {
            if self.adaptive_governor.should_sample() {
                self.background_frame_sampler.set_target_package(confirmed_pkg.clone());
                let frame_stats = self.background_frame_sampler.latest_stats();

                let current_stats = crate::monitor::load_sampler::read_cpu_stat();
                let utilization = if !self.last_load_sample.is_empty() {
                    // Average utilization across all CPU indices present in both samples.
                    let mut total_util = 0.0f32;
                    let mut count = 0;
                    for (idx, curr) in &current_stats {
                        if let Some(prev) = self.last_load_sample.get(idx) {
                            total_util += crate::monitor::load_sampler::compute_utilization(prev, curr);
                            count += 1;
                        }
                    }
                    if count > 0 { total_util / count as f32 } else { 0.5 } // safe default only if no prior sample overlapped
                } else {
                    0.5 // first-ever sample this daemon run - no previous data to delta against yet
                };
                self.last_load_sample = current_stats;

                let tier = self.adaptive_governor.decide_tier(frame_stats.as_ref(), utilization);

                if can_actuate {
                    self.last_actuation_at = Some(std::time::Instant::now());
                    for cluster in &self.hardware.cpu_topology.clusters {
                        let target = match tier {
                        crate::scheduler::adaptive_governor::FrequencyTier::Max => crate::governors::GovernorManager::max_freq(&cluster.available_frequencies),
                        crate::scheduler::adaptive_governor::FrequencyTier::High => {
                            let min = crate::governors::GovernorManager::min_freq(&cluster.available_frequencies).unwrap_or(0);
                            let max = crate::governors::GovernorManager::max_freq(&cluster.available_frequencies).unwrap_or(0);
                            let midpoint = (min + max) / 2;
                            // Snap to the closest value actually present in this cluster's real
                            // frequency table, rather than trusting an arithmetic midpoint to be a
                            // valid step.
                            cluster.available_frequencies
                                .iter()
                                .copied()
                                .min_by_key(|&f| (f as i64 - midpoint as i64).abs())
                        },
                        crate::scheduler::adaptive_governor::FrequencyTier::Balanced => crate::governors::GovernorManager::mid_freq(&cluster.available_frequencies),
                        crate::scheduler::adaptive_governor::FrequencyTier::Eco => crate::governors::GovernorManager::min_freq(&cluster.available_frequencies),
                    };

                        if let Some(freq) = target {
                            let path = format!("{}/scaling_max_freq", cluster.policy_path);
                            if crate::tuning::backend::TuningBackend::try_write_string(&path, &freq.to_string()).is_ok() {
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
                if let Err(e) = self.runtime_tuner.apply_network_tweaks(policy_str) {
                    tracing::warn!("Failed to apply network tweaks: {}", e);
                }
                if let Err(e) = self.runtime_tuner.apply_touch_display_tweaks(policy_str) {
                    tracing::warn!("Failed to apply touch display tweaks: {}", e);
                }
                self.runtime_tuner.apply_vm_params(policy_str);
                if !in_hot_gameexit {
                    if let Err(e) = self.runtime_tuner.apply_scheduler(policy_str) {
                        tracing::warn!("Failed to apply scheduler: {}", e);
                    }
                }
                self.runtime_tuner.apply_universal_gpu_control(policy_str);
            }

            // Stock thermal enable/disable based on gaming/perf
            let want_disabled = policy_str == "Performance" || policy_str == "Gaming";
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
            } else if policy_str == "Powersave" && mem_pressure > 40.0 {
                if let Err(e) = self.runtime_tuner.drop_cache(false) {
                    tracing::warn!("Failed to drop cache: {}", e);
                }
            }
        }

        if can_actuate && needs_apply {
            self.last_applied_policy = Some(policy_str.to_string());
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
                Some(s) if s.p90_frame_ns > 0
                        && s.p90_frame_ns < 500_000_000  // 500 ms sanity cap
                        && s.frame_count() >= 5 => (
                    format!("{:.2}", s.jank_ratio() * 100.0),
                    format!("{:.1}", s.p90_frame_ns as f64 / 1_000_000.0),
                ),
                _ => ("n/a".to_string(), "n/a".to_string()),
            };
            tracing::info!(target: "gaming",
                "tick pkg={} temp={}C policy={:?} gpu_load={}% jank={}% p90={}ms comfort={} session_peak={}C",
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
            voltage_now_uv: None,
            charge_counter_uah: None,
        };
        self.charging.evaluate(&charging_inputs, &ctx.state_dir);

        // 10. Adaptive Sleep
        let clamped_trend = (trend_score * 50).clamp(-50, 50);
        ctx.trend_score = clamped_trend;

        let long_idle = is_screen_off_now
            && !is_gaming
            && !ctx.plugged_in_at.is_some()
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

        ctx.sleep_ms = if sustained_hot_trend {
            750
        } else if clamped_trend > 15 {
            1500
        } else if long_idle {
            30_000
        } else if is_screen_off_now && !is_gaming && (-2..=2).contains(&clamped_trend) {
            ctx.config.profiles.poll_interval.saturating_mul(4000)
        } else {
            ctx.config.profiles.poll_interval.saturating_mul(1000)
        };
        tracing::trace!(
            "adaptive sleep: base={}ms chosen={}ms trend={} sustained={} long_idle={} screen_off={} gaming={}",
            ctx.config.profiles.poll_interval.saturating_mul(1000),
            ctx.sleep_ms, clamped_trend, sustained_hot_trend, long_idle, is_screen_off_now, is_gaming);

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

            let should_log = self.last_battery_log_time
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
            let should_summary = self.last_battery_summary_time.map(|t| t.elapsed().as_secs() >= 600).unwrap_or(true);
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
            "session_started_at": ctx.game_session_started_at.map(|t| chrono::Utc::now().timestamp() - t.elapsed().as_secs() as i64),
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
        });

        let policy_now = Self::policy_state_name(&final_policy).to_string();
        let due_time = self.last_telemetry_write_at
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
