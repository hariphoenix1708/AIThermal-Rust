//! GameTurbo engine — runtime-only gaming optimizations.
//!
//! Activated when a game is detected, fully reversed on game exit.
//! Every sub-feature is independently gated by config and degrades
//! gracefully on syscall failure.
//!
//! ## Thermal-aware mode
//! When composite temperature exceeds `temp_hot` the engine eases
//! aggressive constraints (background lockdown, big-core affinity) to
//! help the SoC shed heat. Priority and touch boosts remain active —
//! they add negligible thermal load.

mod background;
mod combat_boost;
mod combat_detector;
mod cpu_idle;
mod fps_cap;
mod gpu_freq;
mod gpu_hints;
mod game_profiles;
mod sched_deadline;
mod io_scheduler;
mod memory;
mod network;
mod network_qos;
mod perf_hint;
mod priority;
mod thread_affinity;
mod touch;

use crate::config::ProfilesConfig;

pub struct GameTurboEngine {
    active: bool,
    config_snapshot: GameTurboSnapshot,
    affinity: thread_affinity::AffinityState,
    priority: priority::PriorityState,
    background: background::BackgroundState,
    network: network::NetworkState,
    touch: touch::TouchState,
    io_scheduler: io_scheduler::IoSchedulerState,
    gpu_freq: gpu_freq::GpuFreqState,
    perf_hint: perf_hint::PerfHintState,
    cpu_idle: cpu_idle::CpuIdleControl,
    gpu_hints: gpu_hints::GpuBusyHints,
    memory: memory::MemoryManager,
    network_qos: network_qos::NetworkQoS,
    fps_cap: fps_cap::FpsCapManager,
    gpu_profiles: game_profiles::GameProfileManager,
    sched_deadline: sched_deadline::SchedDeadlineManager,
    combat_detector: combat_detector::CombatDetector,
    combat_boost: combat_boost::CombatBoost,
    /// Whether we've entered thermal-throttle mode (eased constraints).
    thermal_throttled: bool,
    /// Pending GPU power info (set by orchestrator before activate).
    pending_gpu_power_level_path: Option<String>,
    pending_gpu_current_level: Option<u32>,
    pending_gpu_best_level: u32,
    /// GPU load accumulator for session average.
    gpu_load_sum: u64,
    gpu_load_samples: u32,
}

#[derive(Clone)]
struct GameTurboSnapshot {
    thread_affinity: bool,
    priority_elevator: bool,
    background_lockdown: bool,
    wifi_ps_disable: bool,
    network_buffers: bool,
    touch_boost: bool,
    io_scheduler_boost: bool,
    gpu_freq_boost: bool,
    thermal_throttle_enabled: bool,
    big_core_mask: u64,
    /// Fallback big-core mask when thermal-throttled (allows some small cores).
    thermal_throttle_mask: u64,
}

impl Default for GameTurboEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GameTurboEngine {
    pub fn new() -> Self {
        Self {
            active: false,
            config_snapshot: GameTurboSnapshot {
                thread_affinity: true,
                priority_elevator: true,
                background_lockdown: true,
                wifi_ps_disable: true,
                network_buffers: true,
                touch_boost: true,
                io_scheduler_boost: true,
                gpu_freq_boost: true,
                thermal_throttle_enabled: true,
                big_core_mask: 0xF0,
                thermal_throttle_mask: 0xFF,
            },
            affinity: thread_affinity::AffinityState::new(),
            priority: priority::PriorityState::new(),
            background: background::BackgroundState::new(),
            network: network::NetworkState::new(),
            touch: touch::TouchState::new(),
            io_scheduler: io_scheduler::IoSchedulerState::new(),
            gpu_freq: gpu_freq::GpuFreqState::new(),
            perf_hint: perf_hint::PerfHintState::new(),
            cpu_idle: cpu_idle::CpuIdleControl::new(),
            gpu_hints: gpu_hints::GpuBusyHints::new(),
            memory: memory::MemoryManager::new(),
            network_qos: network_qos::NetworkQoS::new(),
            fps_cap: fps_cap::FpsCapManager::new(),
            gpu_profiles: game_profiles::GameProfileManager::new(""),
            sched_deadline: sched_deadline::SchedDeadlineManager::new(),
            combat_detector: combat_detector::CombatDetector::new(),
            combat_boost: combat_boost::CombatBoost::new(),
            thermal_throttled: false,
            pending_gpu_power_level_path: None,
            pending_gpu_current_level: None,
            pending_gpu_best_level: 0,
            gpu_load_sum: 0,
            gpu_load_samples: 0,
        }
    }

    /// Provide GPU power info before activation (called by orchestrator).
    pub fn set_gpu_power_info(
        &mut self,
        power_level_path: Option<String>,
        current_level: Option<u32>,
        best_level: u32,
    ) {
        self.pending_gpu_power_level_path = power_level_path;
        self.pending_gpu_current_level = current_level;
        self.pending_gpu_best_level = best_level;
    }

    /// Initialize the GPU profile manager with the state directory.
    pub fn init_profiles(&mut self, state_dir: &str) {
        self.gpu_profiles = game_profiles::GameProfileManager::new(state_dir);
    }

    /// Get recommended GPU level for a game from learned profiles.
    pub fn recommended_gpu_level(&self, package: &str, gpu_best: u32, gpu_worst: u32) -> Option<u32> {
        self.gpu_profiles.recommend_gpu_level(package, gpu_best, gpu_worst)
    }

    /// Get recommended FPS cap for a game from learned profiles.
    pub fn recommended_fps_cap(&self, package: &str) -> Option<u32> {
        self.gpu_profiles.recommend_fps_cap(package)
    }

    /// Get recommended network profile for a game from learned profiles.
    pub fn recommended_network_profile(&self, package: &str) -> Option<u8> {
        self.gpu_profiles.recommend_network_profile(package)
    }

    /// Get recommended thermal policy for a game from learned profiles.
    pub fn recommended_thermal_policy(&self, package: &str) -> Option<u8> {
        self.gpu_profiles.recommend_thermal_policy(package)
    }

    pub fn record_fps_cap(&mut self, package: &str, fps_cap: u32, jank_pct: f64) {
        self.gpu_profiles.record_fps_cap(package, fps_cap, jank_pct);
    }

    pub fn record_network_profile(&mut self, package: &str, net_type: u8) {
        self.gpu_profiles.record_network_profile(package, net_type);
    }

    pub fn record_thermal_policy(&mut self, package: &str, policy: u8) {
        self.gpu_profiles.record_thermal_policy(package, policy);
    }

    pub fn current_fps_cap(&self) -> Option<u32> {
        self.fps_cap.current_cap()
    }

    pub fn combat_update(&mut self, gpu_load: u32) -> bool {
        let is_burst = self.combat_detector.update(gpu_load);
        if is_burst {
            self.combat_boost.trigger(gpu_load, "");
        }
        // Tick boost state machine (handles hold/decay re-enter)
        self.combat_boost.tick(is_burst, gpu_load);
        self.combat_boost.is_active()
    }

    pub fn combat_is_active(&self) -> bool {
        self.combat_boost.is_active()
    }

    pub fn combat_reset(&mut self) {
        self.combat_detector.reset();
        self.combat_boost.reset();
    }

    /// Record GPU load sample for session average.
    pub fn record_gpu_load(&mut self, gpu_load: u32) {
        if self.active {
            self.gpu_load_sum += gpu_load as u64;
            self.gpu_load_samples += 1;
        }
    }

    /// Record session results when game exits.
    pub fn record_session(&mut self, package: &str, gpu_level_used: u32, jank_pct: f64) {
        let avg_load = if self.gpu_load_samples > 0 {
            self.gpu_load_sum as f64 / self.gpu_load_samples as f64
        } else {
            50.0 // Assume moderate load if no data.
        };
        self.gpu_profiles.record_session(package, gpu_level_used, avg_load, jank_pct);
        self.gpu_load_sum = 0;
        self.gpu_load_samples = 0;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns true if the engine entered thermal-throttle mode during this session.
    pub fn was_thermally_throttled(&self) -> bool {
        self.thermal_throttled
    }

    /// Activate all enabled GameTurbo features for the given game PID.
    pub fn activate(&mut self, game_pid: u32, profiles: &ProfilesConfig) {
        if self.active {
            return;
        }

        self.config_snapshot = GameTurboSnapshot {
            thread_affinity: profiles.game_turbo_thread_affinity,
            priority_elevator: profiles.game_turbo_priority_elevator,
            background_lockdown: profiles.game_turbo_background_lockdown,
            wifi_ps_disable: profiles.game_turbo_wifi_ps_disable,
            network_buffers: profiles.game_turbo_network_buffers,
            touch_boost: profiles.game_turbo_touch_boost,
            io_scheduler_boost: profiles.game_turbo_io_scheduler,
            gpu_freq_boost: profiles.game_turbo_gpu_freq_boost,
            thermal_throttle_enabled: profiles.game_turbo_thermal_throttle,
            big_core_mask: profiles.game_turbo_big_core_mask,
            // When thermally throttled, expand to all 8 cores (0xFF)
            // so the scheduler can shift work to efficiency cores.
            thermal_throttle_mask: 0xFF,
        };

        tracing::info!(
            target: "game_turbo",
            "Activating GameTurbo for pid={} big_cores={:#x}",
            game_pid, self.config_snapshot.big_core_mask,
        );

        if self.config_snapshot.thread_affinity {
            self.affinity
                .activate(game_pid, self.config_snapshot.big_core_mask);
        }
        if self.config_snapshot.priority_elevator {
            self.priority.activate(game_pid);
        }
        if self.config_snapshot.background_lockdown {
            self.background.activate(game_pid);
        }
        if self.config_snapshot.wifi_ps_disable {
            self.network.activate_wifi_ps();
        }
        self.network.activate_rps();
        if self.config_snapshot.network_buffers {
            self.network.activate_buffers();
        }
        if self.config_snapshot.io_scheduler_boost {
            self.io_scheduler.activate();
        }
        if self.config_snapshot.touch_boost {
            self.touch.activate();
        }
        if self.config_snapshot.gpu_freq_boost {
            self.gpu_freq.activate(
                self.pending_gpu_power_level_path.as_deref(),
                self.pending_gpu_current_level,
                self.pending_gpu_best_level,
            );
        }

        // Activate Performance Hint — set top-app uclamp_min for gaming.
        self.perf_hint.activate();

        // Activate CPU Idle Control — disable deep C-states for low wake latency.
        self.cpu_idle.activate();

        // Activate GPU Busy Hints — keep GPU clocks responsive.
        self.gpu_hints.activate();

        // Activate Memory Manager — compact ZRAM, protect game from OOM.
        self.memory.activate(game_pid);

        // Activate Network QoS — prioritize gaming traffic on active interface.
        // Use the network interface detected by the network module.
        if let Some(ref iface) = self.network.active_interface() {
            self.network_qos.activate(iface);
        }

        // Activate FPS Cap — battery/thermal-aware max FPS limit.
        // FPS cap reads thermal temp from sysfs internally.
        self.fps_cap.activate();

        // Activate SCHED_DEADLINE — guaranteed CPU time for render threads.
        self.sched_deadline.activate(game_pid);

        // Reset combat detector for fresh Ranked session
        self.combat_detector.reset();
        self.combat_boost.reset();

        self.active = true;
    }

    /// Per-tick refresh — re-scan threads that may have spawned after
    /// the initial activate (game engines commonly defer thread creation).
    pub fn tick(&mut self, game_pid: u32, gpu_load: u32, thermal_temp: u32) {
        if !self.active {
            return;
        }

        if self.config_snapshot.thread_affinity {
            self.affinity
                .tick(game_pid, self.config_snapshot.big_core_mask);
        }
        if self.config_snapshot.priority_elevator {
            self.priority.tick(game_pid);
        }
        if self.config_snapshot.touch_boost {
            self.touch.tick();
        }
        self.network
            .refresh_for_network_handoff(self.config_snapshot.wifi_ps_disable);
        // Re-boost newly spawned game threads with uclamp.
        self.perf_hint.tick(gpu_load, thermal_temp);

        // Adjust GPU idle timer based on GPU load.
        self.gpu_hints.tick(gpu_load);

        // Memory manager tick (no-op currently).
        self.memory.tick();

        // Network QoS tick (no-op currently).
        self.network_qos.tick();

        // FPS Cap tick — adjust based on battery/thermal.
        self.fps_cap.tick();

        // SCHED_DEADLINE tick — re-scan for new render threads.
        self.sched_deadline.tick(game_pid);

        // Combat heuristic — burst before frame miss (side effects only; state read via combat_is_active())
        let _ = self.combat_update(gpu_load);
    }

    /// Thermal-aware adjustment — called each tick with the current
    /// composite temperature. When above `temp_hot` the engine eases
    /// aggressive constraints to help the SoC cool down.
    pub fn thermal_adjust(&mut self, composite_temp: i32, temp_hot: i32, game_pid: u32) {
        if !self.active || !self.config_snapshot.thermal_throttle_enabled {
            return;
        }

        const THERMAL_RELEASE_MARGIN: i32 = 3;
        let was_throttled = self.thermal_throttled;
        self.thermal_throttled = if was_throttled {
            composite_temp >= (temp_hot - THERMAL_RELEASE_MARGIN)
        } else {
            composite_temp >= temp_hot
        };

        if self.thermal_throttled && !was_throttled {
            tracing::info!(
                target: "game_turbo",
                "Thermal throttle ON: temp={}C >= temp_hot={}C — easing affinity + background lockdown",
                composite_temp, temp_hot
            );

            // Expand affinity mask to all cores so the scheduler can
            // move work to efficiency cores.
            if self.config_snapshot.thread_affinity {
                self.affinity
                    .update_mask(game_pid, self.config_snapshot.thermal_throttle_mask);
            }

            // Release background cgroup clamp to reduce contention.
            if self.config_snapshot.background_lockdown {
                self.background.deactivate();
            }
        } else if !self.thermal_throttled && was_throttled {
            tracing::info!(
                target: "game_turbo",
                "Thermal throttle OFF: temp={}C < release_threshold={}C (temp_hot {} - margin {}) — re-applying full boost",
                composite_temp,
                temp_hot - THERMAL_RELEASE_MARGIN,
                temp_hot,
                THERMAL_RELEASE_MARGIN
            );

            // Re-pin to big cores.
            if self.config_snapshot.thread_affinity {
                self.affinity
                    .update_mask(game_pid, self.config_snapshot.big_core_mask);
            }

            // Re-clamp background cgroups.
            if self.config_snapshot.background_lockdown {
                self.background.activate(game_pid);
            }
        }
    }

    /// Deactivate all features and restore original state.
    pub fn deactivate(&mut self) {
        if !self.active {
            return;
        }

        tracing::info!(target: "game_turbo", "Deactivating GameTurbo — restoring all state");

        self.gpu_freq.deactivate();
        self.touch.deactivate();
        self.io_scheduler.deactivate();
        self.network.deactivate_buffers();
        self.network.deactivate_rps();
        self.network.deactivate_wifi_ps();
        self.background.deactivate();
        self.priority.deactivate();
        self.affinity.deactivate();
        self.perf_hint.deactivate();
        self.cpu_idle.deactivate();
        self.gpu_hints.deactivate();
        self.memory.deactivate();
        self.network_qos.deactivate();
        self.fps_cap.deactivate();
        self.sched_deadline.deactivate();
        self.combat_reset();

        self.thermal_throttled = false;
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_turbo_new_not_active() {
        let engine = GameTurboEngine::new();
        assert!(!engine.is_active());
        assert!(!engine.thermal_throttled);
    }

    #[test]
    fn test_deactivate_when_not_active_is_noop() {
        let mut engine = GameTurboEngine::new();
        engine.deactivate();
        assert!(!engine.is_active());
    }

    #[test]
    fn test_thermal_adjust_when_not_active_is_noop() {
        let mut engine = GameTurboEngine::new();
        engine.thermal_adjust(80, 58, 1234);
        assert!(!engine.thermal_throttled);
    }
}
