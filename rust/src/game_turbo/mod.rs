//! GameTurbo engine — runtime-only gaming optimizations.
//!
//! Activated when a game is detected, fully reversed on game exit.
//! Every sub-feature is independently gated by config and degrades
//! gracefully on syscall failure.

mod thread_affinity;
mod priority;
mod background;
mod network;
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
}

#[derive(Clone)]
struct GameTurboSnapshot {
    thread_affinity: bool,
    priority_elevator: bool,
    background_lockdown: bool,
    wifi_ps_disable: bool,
    touch_boost: bool,
    big_core_mask: u64,
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
                touch_boost: true,
                big_core_mask: 0xF0,
            },
            affinity: thread_affinity::AffinityState::new(),
            priority: priority::PriorityState::new(),
            background: background::BackgroundState::new(),
            network: network::NetworkState::new(),
            touch: touch::TouchState::new(),
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
            touch_boost: profiles.game_turbo_touch_boost,
            big_core_mask: profiles.game_turbo_big_core_mask,
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

    /// Deactivate all features and restore original state.
    pub fn deactivate(&mut self) {
        if !self.active {
            return;
        }

        tracing::info!(target: "game_turbo", "Deactivating GameTurbo — restoring all state");

        self.touch.deactivate();
        self.network.deactivate_wifi_ps();
        self.background.deactivate();
        self.priority.deactivate();
        self.affinity.deactivate();

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
    }

    #[test]
    fn test_deactivate_when_not_active_is_noop() {
        let mut engine = GameTurboEngine::new();
        engine.deactivate();
        assert!(!engine.is_active());
    }
}
