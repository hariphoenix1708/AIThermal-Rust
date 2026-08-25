use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameProfile {
    pub package: String,
    pub known_hot: bool,
    pub max_temp: i32,
    pub session_count: u32,
    pub total_session_seconds: u64,
    pub last_seen: u64,
    pub last_policy: String,
    pub cooldown_sec: u64,
    pub session_started_at: Option<u64>,
    pub last_game_end_at: Option<u64>,
    pub last_game_end_temp: Option<i32>,
    pub last_game_mode: Option<String>,
    pub slow_cooler_flag: bool,
    // v3.3.1: GameTurbo per-game learning.
    #[serde(default)]
    pub game_turbo_sessions: u32,
    #[serde(default)]
    pub thermal_throttle_count: u32,
    #[serde(default)]
    pub avg_peak_temp: f64,
    #[serde(default)]
    pub last_jank_pct: f64,
    #[serde(default)]
    pub last_p90_ms: f64,
    #[serde(default)]
    pub avg_session_peak_temp: f64,
}

impl Default for GameProfile {
    fn default() -> Self {
        Self {
            package: String::new(),
            known_hot: false,
            max_temp: 0,
            session_count: 0,
            total_session_seconds: 0,
            last_seen: 0,
            last_policy: "Balanced".to_string(),
            cooldown_sec: 90,
            session_started_at: None,
            last_game_end_at: None,
            last_game_end_temp: None,
            last_game_mode: None,
            slow_cooler_flag: false,
            game_turbo_sessions: 0,
            thermal_throttle_count: 0,
            avg_peak_temp: 0.0,
            last_jank_pct: 0.0,
            last_p90_ms: 0.0,
            avg_session_peak_temp: 0.0,
        }
    }
}

pub struct GameProfileManager {
    path: PathBuf,
    pub profiles: HashMap<String, GameProfile>,
}

impl GameProfileManager {
    pub fn new(state_dir: &str) -> Self {
        let path = Path::new(state_dir).join("game_profiles.json");
        let mut manager = Self {
            path,
            profiles: HashMap::new(),
        };
        manager.load();
        manager
    }

    pub fn load(&mut self) {
        #[allow(clippy::collapsible_if)]
        if self.path.exists() {
            if let Ok(content) = fs::read_to_string(&self.path) {
                if let Ok(profiles) = serde_json::from_str(&content) {
                    self.profiles = profiles;
                } else {
                    tracing::warn!("Game profiles file is corrupted. Ignoring.");
                }
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let temp_path = self.path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(&self.profiles)?;
        if let Err(e) = fs::write(&temp_path, content) {
            tracing::error!("Failed to write game profile temp file: {}", e);
        }
        fs::rename(&temp_path, &self.path)?;
        Ok(())
    }

    pub fn update_session(
        &mut self,
        package: &str,
        peak_temp: i32,
        last_policy: &str,
        session_seconds: u64,
    ) -> Result<()> {
        let profile = self
            .profiles
            .entry(package.to_string())
            .or_insert(GameProfile {
                package: package.to_string(),
                ..Default::default()
            });

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        profile.session_count += 1;
        profile.total_session_seconds += session_seconds;
        profile.last_seen = now;
        profile.last_policy = last_policy.to_string();
        profile.last_game_end_at = Some(now);
        profile.last_game_end_temp = Some(peak_temp);
        profile.session_started_at = None; // Reset for next session

        if peak_temp > profile.max_temp {
            profile.max_temp = peak_temp;
        }

        // Known hot promotion logic
        if profile.max_temp > 48 {
            profile.known_hot = true;
            profile.cooldown_sec = 120;
        } else if profile.max_temp > 44 {
            profile.cooldown_sec = 90;
        } else {
            profile.cooldown_sec = 60;
        }

        // Secondary promotion condition (stays hot over multiple sessions)
        if profile.session_count > 3 && peak_temp >= 45 {
            profile.known_hot = true;
        }

        if profile.max_temp > 50 {
            profile.slow_cooler_flag = true;
        }

        self.save()
    }

    pub fn get_profile(&self, package: &str) -> Option<&GameProfile> {
        self.profiles.get(package)
    }

    /// Record GameTurbo session stats when a game session ends.
    pub fn record_game_turbo_session(
        &mut self,
        package: &str,
        thermal_throttled: bool,
        peak_temp: i32,
        jank_pct: f64,
        p90_ms: f64,
    ) -> Result<()> {
        let profile = self
            .profiles
            .entry(package.to_string())
            .or_insert(GameProfile {
                package: package.to_string(),
                ..Default::default()
            });

        profile.game_turbo_sessions += 1;
        if thermal_throttled {
            profile.thermal_throttle_count += 1;
        }
        profile.last_jank_pct = jank_pct;
        profile.last_p90_ms = p90_ms;

        // Running average of peak temperature.
        let n = profile.game_turbo_sessions as f64;
        profile.avg_peak_temp =
            (profile.avg_peak_temp * (n - 1.0) + peak_temp as f64) / n;

        self.save()
    }

    /// Get a per-game recommendation based on learned history.
    pub fn recommend(&self, package: &str) -> Option<String> {
        let p = self.profiles.get(package)?;
        if p.session_count < 2 {
            return None; // Not enough data.
        }

        let mut tips = Vec::new();

        if p.known_hot {
            tips.push("Known hot game — thermal-aware constraints will ease early");
        }
        if p.slow_cooler_flag {
            tips.push("Slow cooler — extended cooldown after exit");
        }
        if p.thermal_throttle_count > 0 && p.game_turbo_sessions > 0 {
            let throttle_rate =
                p.thermal_throttle_count as f64 / p.game_turbo_sessions as f64;
            if throttle_rate > 0.5 {
                tips.push("Frequent thermal throttle — consider lowering game graphics");
            }
        }
        if p.avg_peak_temp > 50.0 {
            tips.push("Consistently hot — gaming pad or cooler recommended");
        }
        if p.last_jank_pct > 30.0 {
            tips.push("High jank rate — mostly game-side frame pacing");
        }

        if tips.is_empty() {
            None
        } else {
            Some(tips.join("; "))
        }
    }
}
