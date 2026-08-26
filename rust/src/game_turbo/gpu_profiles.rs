//! Per-game GPU frequency profiles — learn optimal GPU power levels
//! for each game from session history.
//!
//! After each game session, records the GPU power level that was active
//! and the resulting performance (jank%, p90). On next game entry,
//! applies the learned optimal level instead of always using gpu_best.
//!
//! This prevents the GPU from being over-provisioned (wasting power
//! and generating heat) or under-provisioned (causing frame drops).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuProfileEntry {
    /// Best GPU power level seen for this game (lowest index = highest perf).
    pub best_level: u32,
    /// Average GPU load during this game (0-100).
    pub avg_gpu_load: f64,
    /// Number of sessions with this profile.
    pub sessions: u32,
    /// Last jank percentage observed.
    pub last_jank_pct: f64,
    /// Whether the game tends to be GPU-bound.
    pub gpu_bound: bool,
}

impl Default for GpuProfileEntry {
    fn default() -> Self {
        Self {
            best_level: 0,
            avg_gpu_load: 0.0,
            sessions: 0,
            last_jank_pct: 0.0,
            gpu_bound: false,
        }
    }
}

pub struct GpuProfileManager {
    path: PathBuf,
    profiles: HashMap<String, GpuProfileEntry>,
}

impl GpuProfileManager {
    pub fn new(state_dir: &str) -> Self {
        let path = Path::new(state_dir).join("gpu_profiles.json");
        let mut manager = Self {
            path,
            profiles: HashMap::new(),
        };
        manager.load();
        manager
    }

    fn load(&mut self) {
        if self.path.exists()
            && let Ok(content) = fs::read_to_string(&self.path)
            && let Ok(profiles) = serde_json::from_str(&content)
        {
            self.profiles = profiles;
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
    pub fn recommend_level(
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
            entry.best_level
        } else {
            // Not GPU-bound: use one level worse than best to save power
            // while still maintaining good performance.
            (entry.best_level + 1).min(gpu_worst)
        };

        // Sanity check: don't go below gpu_best or above gpu_worst.
        Some(recommended.max(gpu_best).min(gpu_worst))
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
                entry.best_level = gpu_level_used;
            }
            // If this level worked well and it's better than our recorded best,
            // update (lower index = higher performance).
            if gpu_level_used < entry.best_level {
                entry.best_level = gpu_level_used;
            }
        } else if jank_pct > 20.0 && entry.best_level > 0 {
            // Poor session — try a better level next time.
            entry.best_level = entry.best_level.saturating_add(1).min(entry.best_level);
            // Don't go below 0 (highest performance).
        }

        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommend_no_profile() {
        let manager = GpuProfileManager {
            path: PathBuf::from("/tmp/test_gpu_profiles.json"),
            profiles: HashMap::new(),
        };
        assert_eq!(manager.recommend_level("com.test.game", 0, 10), None);
    }

    #[test]
    fn test_recommend_insufficient_sessions() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "com.test.game".to_string(),
            GpuProfileEntry {
                best_level: 2,
                sessions: 1,
                ..Default::default()
            },
        );
        let manager = GpuProfileManager {
            path: PathBuf::from("/tmp/test_gpu_profiles.json"),
            profiles,
        };
        // Only 1 session — not enough data.
        assert_eq!(manager.recommend_level("com.test.game", 0, 10), None);
    }

    #[test]
    fn test_recommend_gpu_bound() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "com.test.game".to_string(),
            GpuProfileEntry {
                best_level: 3,
                avg_gpu_load: 85.0,
                sessions: 5,
                gpu_bound: true,
                last_jank_pct: 5.0,
            },
        );
        let manager = GpuProfileManager {
            path: PathBuf::from("/tmp/test_gpu_profiles.json"),
            profiles,
        };
        // GPU-bound: use the learned best level.
        assert_eq!(manager.recommend_level("com.test.game", 0, 10), Some(3));
    }

    #[test]
    fn test_recommend_not_gpu_bound() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "com.test.game".to_string(),
            GpuProfileEntry {
                best_level: 3,
                avg_gpu_load: 40.0,
                sessions: 5,
                gpu_bound: false,
                last_jank_pct: 5.0,
            },
        );
        let manager = GpuProfileManager {
            path: PathBuf::from("/tmp/test_gpu_profiles.json"),
            profiles,
        };
        // Not GPU-bound: use one level worse than best (4).
        assert_eq!(manager.recommend_level("com.test.game", 0, 10), Some(4));
    }
}
