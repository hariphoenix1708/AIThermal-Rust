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

    pub fn load_game_profile(&mut self, package: &str) -> GameProfile {
        self.profiles
            .get(package)
            .cloned()
            .unwrap_or_else(|| GameProfile {
                package: package.to_string(),
                ..Default::default()
            })
    }

    pub fn get_game_profile_modifier(&self, package: &str, session_seconds: u64) -> f64 {
        if let Some(profile) = self.profiles.get(package) {
            let p_fac = match profile.last_policy.as_str() {
                "Performance" | "performance" => 1.5,
                "Balanced" | "balanced" => 1.0,
                _ => 0.8,
            };

            let mut modifier = 0.0;

            if profile.known_hot {
                modifier -= 25.0 * p_fac;
            } else {
                modifier += 5.0 * p_fac;
            }

            if profile.slow_cooler_flag {
                modifier -= 10.0;
            }

            if session_seconds > 3600 {
                modifier -= 20.0;
            } else if session_seconds > 1800 {
                modifier -= 10.0;
            }

            // Residual heat check based on last game end
            if let Some(last_end) = profile.last_game_end_at {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let time_since_last_game = now.saturating_sub(last_end);

                // If started gaming again within 30 mins, add penalty
                if time_since_last_game < 1800 && profile.known_hot {
                    modifier -= 5.0;
                }
            }

            modifier
        } else {
            0.0
        }
    }

    pub fn get_profile(&self, package: &str) -> Option<&GameProfile> {
        self.profiles.get(package)
    }
}
