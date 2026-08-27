//! Per-game GPU frequency profiles — learn optimal GPU power levels
//! for each game from session history.
//!
//! After each game session, records the GPU power level that was active
//! and the resulting performance (jank%, p90). On next game entry,
//! applies the learned optimal level instead of always using gpu_best.
//!
//! This prevents the GPU from being over-provisioned (wasting power
//! and generating heat) or under-provisioned (causing frame drops).
//!
//! Extended to include: FPS cap, network profile, thermal policy.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameProfileEntry {
    /// Best GPU power level seen for this game (lowest index = highest perf).
    pub best_gpu_level: u32,
    /// Average GPU load during this game (0-100).
    pub avg_gpu_load: f64,
    /// Number of sessions with this profile.
    pub sessions: u32,
    /// Last jank percentage observed.
    pub last_jank_pct: f64,
    /// Whether the game tends to be GPU-bound.
    pub gpu_bound: bool,
    /// Learned optimal FPS cap for this game (0 = unlimited/dynamic).
    pub optimal_fps_cap: u32,
    /// Preferred network profile: 0=auto, 1=wifi, 2=cellular.
    pub preferred_network: u8,
    /// Thermal policy preference: 0=balanced, 1=performance, 2=cool.
    pub thermal_policy: u8,
    /// Last session timestamp (Unix epoch).
    pub last_session_ts: u64,
}

impl Default for GameProfileEntry {
    fn default() -> Self {
        Self {
            best_gpu_level: 0,
            avg_gpu_load: 0.0,
            sessions: 0,
            last_jank_pct: 0.0,
            gpu_bound: false,
            optimal_fps_cap: 0,
            preferred_network: 0,
            thermal_policy: 0,
            last_session_ts: 0,
        }
    }
}

pub struct GameProfileManager {
    path: PathBuf,
    profiles: HashMap<String, GameProfileEntry>,
}

impl GameProfileManager {
    pub fn new(state_dir: &str) -> Self {
        let path = Path::new(state_dir).join("game_turbo_profiles.json");
        let mut manager = Self {
            path,
            profiles: HashMap::new(),
        };
        manager.load();
        manager
    }

    fn load(&mut self) {
        // Primary: game_turbo_profiles.json
        if self.path.exists() {
            match fs::read_to_string(&self.path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(profiles) => {
                        self.profiles = profiles;
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(target: "game_turbo", "game_turbo_profiles.json corrupted ({}), starting fresh", e);
                        return;
                    }
                },
                Err(e) => {
                    tracing::warn!(target: "game_turbo", "Failed to read game_turbo_profiles.json: {}", e);
                    return;
                }
            }
        }
        // Compat: migrate game_profiles.json if it contains turbo schema (v3.6.0 collision)
        let unified_path = Path::new(self.path.parent().unwrap_or(Path::new("."))).join("game_profiles.json");
        if unified_path.exists()
            && let Ok(content) = fs::read_to_string(&unified_path)
            && let Ok(map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&content) {
                let looks_like_turbo = map.values().any(|v| v.get("best_gpu_level").is_some() || v.get("optimal_fps_cap").is_some());
                if looks_like_turbo && let Ok(profiles) = serde_json::from_str::<HashMap<String, GameProfileEntry>>(&content) {
                    self.profiles = profiles;
                    self.save();
                    tracing::info!(target: "game_turbo", "Migrated game_profiles.json → game_turbo_profiles.json ({} entries, turbo schema)", self.profiles.len());
                    return;
                }
            }
        // Compat: migrate old gpu_profiles.json if present (v3.4.0 → v3.5.0 rename)
        let legacy_path = Path::new(self.path.parent().unwrap_or(Path::new("."))).join("gpu_profiles.json");
        if legacy_path.exists()
            && let Ok(content) = fs::read_to_string(&legacy_path)
        {
            // Try new format first
            if let Ok(profiles) = serde_json::from_str::<HashMap<String, GameProfileEntry>>(&content) {
                self.profiles = profiles;
                self.save();
                let _ = fs::remove_file(&legacy_path);
                tracing::info!(target: "game_turbo", "Migrated gpu_profiles.json → game_profiles.json ({} entries)", self.profiles.len());
                return;
            }
            // Fallback: legacy GpuProfileEntry format (best_level → best_gpu_level)
            if let Ok(legacy) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&content) {
                let mut migrated = 0;
                for (pkg, v) in legacy {
                    let best = v.get("best_level").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                    let best_gpu = v.get("best_gpu_level").and_then(|x| x.as_u64()).unwrap_or(best as u64) as u32;
                    let entry = GameProfileEntry {
                        best_gpu_level: best_gpu,
                        avg_gpu_load: v.get("avg_gpu_load").and_then(|x| x.as_f64()).unwrap_or(0.0),
                        sessions: v.get("sessions").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                        last_jank_pct: v.get("last_jank_pct").and_then(|x| x.as_f64()).unwrap_or(0.0),
                        gpu_bound: v.get("gpu_bound").and_then(|x| x.as_bool()).unwrap_or(false),
                        optimal_fps_cap: v.get("optimal_fps_cap").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                        preferred_network: v.get("preferred_network").and_then(|x| x.as_u64()).unwrap_or(0) as u8,
                        thermal_policy: v.get("thermal_policy").and_then(|x| x.as_u64()).unwrap_or(0) as u8,
                        last_session_ts: v.get("last_session_ts").and_then(|x| x.as_u64()).unwrap_or(0),
                    };
                    if entry.sessions > 0 {
                        self.profiles.insert(pkg, entry);
                        migrated += 1;
                    }
                }
                if migrated > 0 {
                    self.save();
                    let _ = fs::remove_file(&legacy_path);
                    tracing::info!(target: "game_turbo", "Migrated legacy gpu_profiles.json → game_profiles.json ({} entries)", migrated);
                }
            }
        }
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.profiles) {
            let temp_path = self.path.with_extension("json.tmp");
            let _ = fs::write(&temp_path, json);
            let _ = fs::rename(&temp_path, &self.path);
        }
    }

    /// Get the recommended GPU power level for a game.
    /// Returns None if no profile exists or not enough data.
    pub fn recommend_gpu_level(
        &self,
        package: &str,
        gpu_best: u32,
        gpu_worst: u32,
    ) -> Option<u32> {
        let entry = self.profiles.get(package)?;
        if entry.sessions < 2 {
            return None; // Not enough data to trust the profile.
        }

        // If the game is GPU-bound, prefer the learned best level.
        // If not GPU-bound, we can use a slightly worse level to save power.
        let recommended = if entry.gpu_bound {
            entry.best_gpu_level
        } else {
            // Not GPU-bound: use one level worse than best to save power
            // while still maintaining good performance.
            (entry.best_gpu_level + 1).min(gpu_worst)
        };

        // Sanity check: don't go below gpu_best or above gpu_worst.
        Some(recommended.max(gpu_best).min(gpu_worst))
    }

    /// Get the recommended FPS cap for a game.
    /// Returns None if no profile exists or not enough data.
    pub fn recommend_fps_cap(&self, package: &str) -> Option<u32> {
        let entry = self.profiles.get(package)?;
        if entry.sessions < 2 {
            return None;
        }
        if entry.optimal_fps_cap > 0 {
            Some(entry.optimal_fps_cap)
        } else {
            None
        }
    }

    /// Get the recommended network profile for a game.
    /// Returns None if no profile exists or not enough data.
    pub fn recommend_network_profile(&self, package: &str) -> Option<u8> {
        let entry = self.profiles.get(package)?;
        if entry.sessions < 2 {
            return None;
        }
        if entry.preferred_network > 0 {
            Some(entry.preferred_network)
        } else {
            None
        }
    }

    /// Get the recommended thermal policy for a game.
    /// Returns None if no profile exists or not enough data.
    pub fn recommend_thermal_policy(&self, package: &str) -> Option<u8> {
        let entry = self.profiles.get(package)?;
        if entry.sessions < 2 {
            return None;
        }
        if entry.thermal_policy > 0 {
            Some(entry.thermal_policy)
        } else {
            None
        }
    }

    /// Record session results for a game.
    pub fn record_session(
        &mut self,
        package: &str,
        gpu_level_used: u32,
        gpu_load_avg: f64,
        jank_pct: f64,
    ) {
        let entry = self
            .profiles
            .entry(package.to_string())
            .or_default();

        entry.sessions += 1;
        entry.last_jank_pct = jank_pct;
        entry.last_session_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Update running average of GPU load.
        let n = entry.sessions as f64;
        entry.avg_gpu_load = (entry.avg_gpu_load * (n - 1.0) + gpu_load_avg) / n;

        // Determine if this game is GPU-bound (>70% GPU load).
        entry.gpu_bound = entry.avg_gpu_load > 70.0;

        // If jank was low (<10%), the current GPU level was good.
        // If jank was high (>20%), we might need a better level.
        if jank_pct < 10.0 {
            // Good session — current level is optimal.
            // If we haven't recorded a best yet, use current.
            if entry.sessions <= 1 {
                entry.best_gpu_level = gpu_level_used;
            }
            // If this level worked well and it's better than our recorded best,
            // update (lower index = higher performance).
            if gpu_level_used < entry.best_gpu_level {
                entry.best_gpu_level = gpu_level_used;
            }
        } else if jank_pct > 20.0 && entry.best_gpu_level > 0 {
            // Poor session — try a better level next time.
            entry.best_gpu_level = entry.best_gpu_level.saturating_sub(1);
            // Don't go below 0 (highest performance).
        }

        self.save();
    }

    /// Record FPS cap feedback for a game.
    pub fn record_fps_cap(&mut self, package: &str, fps_cap: u32, jank_pct: f64) {
        let entry = self
            .profiles
            .entry(package.to_string())
            .or_default();

        // If jank is low with this cap, record it as optimal.
        if jank_pct < 10.0 && fps_cap > 0
            && (entry.optimal_fps_cap == 0 || fps_cap < entry.optimal_fps_cap) {
                entry.optimal_fps_cap = fps_cap;
            }
        self.save();
    }

    /// Record network profile preference for a game.
    pub fn record_network_profile(&mut self, package: &str, network_type: u8) {
        let entry = self
            .profiles
            .entry(package.to_string())
            .or_default();
        entry.preferred_network = network_type;
        self.save();
    }

    /// Record thermal policy preference for a game.
    pub fn record_thermal_policy(&mut self, package: &str, policy: u8) {
        let entry = self
            .profiles
            .entry(package.to_string())
            .or_default();
        entry.thermal_policy = policy;
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommend_no_profile() {
        let manager = GameProfileManager {
            path: PathBuf::from("/tmp/test_game_profiles.json"),
            profiles: HashMap::new(),
        };
        assert_eq!(manager.recommend_gpu_level("com.test.game", 0, 10), None);
    }

    #[test]
    fn test_recommend_insufficient_sessions() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "com.test.game".to_string(),
            GameProfileEntry {
                best_gpu_level: 2,
                sessions: 1,
                ..Default::default()
            },
        );
        let manager = GameProfileManager {
            path: PathBuf::from("/tmp/test_game_profiles.json"),
            profiles,
        };
        // Only 1 session — not enough data.
        assert_eq!(manager.recommend_gpu_level("com.test.game", 0, 10), None);
    }

    #[test]
    fn test_recommend_gpu_bound() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "com.test.game".to_string(),
            GameProfileEntry {
                best_gpu_level: 3,
                avg_gpu_load: 85.0,
                sessions: 5,
                gpu_bound: true,
                last_jank_pct: 5.0,
                ..Default::default()
            },
        );
        let manager = GameProfileManager {
            path: PathBuf::from("/tmp/test_game_profiles.json"),
            profiles,
        };
        // GPU-bound: use the learned best level.
        assert_eq!(manager.recommend_gpu_level("com.test.game", 0, 10), Some(3));
    }

    #[test]
    fn test_recommend_not_gpu_bound() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "com.test.game".to_string(),
            GameProfileEntry {
                best_gpu_level: 3,
                avg_gpu_load: 40.0,
                sessions: 5,
                gpu_bound: false,
                last_jank_pct: 5.0,
                ..Default::default()
            },
        );
        let manager = GameProfileManager {
            path: PathBuf::from("/tmp/test_game_profiles.json"),
            profiles,
        };
        // Not GPU-bound: use one level worse than best (4).
        assert_eq!(manager.recommend_gpu_level("com.test.game", 0, 10), Some(4));
    }
}