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

mod thread_affinity;
mod priority;
mod background;
mod network;
mod touch;
mod io_scheduler;

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
    /// Whether we've entered thermal-throttle mode (eased constraints).
    thermal_throttled: bool,
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
            thermal_throttled: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
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
            self.affinity.activate(game_pid, self.config_snapshot.big_core_mask);
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
        if self.config_snapshot.network_buffers {
            self.network.activate_buffers();
        }
        if self.config_snapshot.io_scheduler_boost {
            self.io_scheduler.activate();
        }
        if self.config_snapshot.touch_boost {
            self.touch.activate();
        }

        self.active = true;
    }

    /// Per-tick refresh — re-scan threads that may have spawned after
    /// the initial activate (game engines commonly defer thread creation).
    pub fn tick(&mut self, game_pid: u32) {
        if !self.active {
            return;
        }

        if self.config_snapshot.thread_affinity {
            self.affinity.tick(game_pid, self.config_snapshot.big_core_mask);
        }
        if self.config_snapshot.priority_elevator {
            self.priority.tick(game_pid);
        }
        if self.config_snapshot.touch_boost {
            self.touch.tick();
        }
    }

    /// Thermal-aware adjustment — called each tick with the current
    /// composite temperature. When above `temp_hot` the engine eases
    /// aggressive constraints to help the SoC cool down.
    pub fn thermal_adjust(&mut self, composite_temp: i32, temp_hot: i32, game_pid: u32) {
        if !self.active || !self.config_snapshot.thermal_throttle_enabled {
            return;
        }

        let was_throttled = self.thermal_throttled;
        self.thermal_throttled = composite_temp >= temp_hot;

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
                "Thermal throttle OFF: temp={}C < temp_hot={}C — re-applying full boost",
                composite_temp, temp_hot
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

        self.touch.deactivate();
        self.io_scheduler.deactivate();
        self.network.deactivate_buffers();
        self.network.deactivate_wifi_ps();
        self.background.deactivate();
        self.priority.deactivate();
        self.affinity.deactivate();

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
