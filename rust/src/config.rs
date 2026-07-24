// Intentionally reserved or conditionally compiled across bins

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Structures representing `profiles.conf` (TOML format)
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ProfilesConfig {
    #[serde(default = "default_temp_cool")]
    pub temp_cool: i32,
    #[serde(default = "default_temp_warm")]
    pub temp_warm: i32,
    #[serde(default = "default_temp_hot")]
    pub temp_hot: i32,
    #[serde(default = "default_temp_powersave")]
    pub temp_powersave: i32,
    #[serde(default = "default_temp_critical")]
    pub temp_critical: i32,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
    #[serde(default = "default_temp_history_size")]
    pub temp_history_size: usize,
    #[serde(default = "default_prediction_window")]
    pub prediction_window: usize,
    #[serde(default = "default_policy_debounce_sec")]
    pub policy_debounce_sec: u64,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_gpu_gaming_threshold")]
    pub gpu_gaming_threshold: i32,
    #[serde(default = "default_log_rotate_mb")]
    pub log_rotate_mb: u64,
    #[serde(default = "default_log_retain_count")]
    pub log_retain_count: u32,
    #[serde(default = "default_proc_scan_interval")]
    pub proc_scan_interval: u64,
    #[serde(default = "default_game_latch_sec")]
    pub game_latch_sec: u64,
    #[serde(default = "default_gaming_score_boost")]
    pub gaming_score_boost: i32,
    #[serde(default = "default_disable_tweaks")]
    pub disable_tweaks: bool,
    #[serde(default = "default_adaptive_governor_enabled")]
    pub adaptive_governor_enabled: bool,
    #[serde(default = "default_true")]
    pub battery_stats_enabled: bool,
    // Minimum interval between two consecutive actuation bursts (governor /
    // cpuset / GPU rewrites). Prevents sub-second thrash observed in v3.1.0.
    #[serde(default = "default_min_actuation_interval_ms")]
    pub min_actuation_interval_ms: u64,
    // Congestion control algorithm to install while a game is active.
    // "kernel_default" keeps whatever the kernel already picked (recommended
    // for most carriers; avoids BBR / captive-portal interactions).
    #[serde(default = "default_tcp_cc")]
    pub tcp_congestion_control_gaming: String,
    // Master off-switch for every /proc/sys/net/ipv4/tcp_* write. Off by
    // default because network tweaks in v3.1.0 caused visible connectivity
    // issues on some kernels.
    #[serde(default = "default_false")]
    pub touch_network_stack: bool,
    // Number of consecutive unclean daemon exits that arm safe mode on the
    // next boot (disable_tweaks forced true; telemetry-only).
    #[serde(default = "default_safe_mode_after_crashes")]
    pub safe_mode_after_crashes: u32,
    // Watchdog failure count above which we escalate from
    // DegradedRestoreRecommended to StalledRecoverNow (full snapshot restore).
    #[serde(default = "default_watchdog_stall_threshold")]
    pub watchdog_stall_threshold: u32,

    #[serde(default = "default_false")]
    pub trace_markers_enabled: bool,
}

fn default_false() -> bool { false }
fn default_min_actuation_interval_ms() -> u64 { 2500 }
fn default_tcp_cc() -> String { "kernel_default".to_string() }
fn default_safe_mode_after_crashes() -> u32 { 2 }
fn default_watchdog_stall_threshold() -> u32 { 5 }

fn default_true() -> bool {
    true
}

// Default providers for Serde fallback
fn default_temp_cool() -> i32 {
    42
}
fn default_temp_warm() -> i32 {
    48
}
fn default_temp_hot() -> i32 {
    58
}
fn default_temp_powersave() -> i32 {
    68
}
fn default_temp_critical() -> i32 {
    75
}
fn default_poll_interval() -> u64 {
    2
}

fn default_temp_history_size() -> usize {
    10
}
fn default_prediction_window() -> usize {
    5
}
fn default_policy_debounce_sec() -> u64 {
    10
}
fn default_log_level() -> String {
    "INFO".to_string()
}
fn default_gpu_gaming_threshold() -> i32 {
    20
}

fn default_log_rotate_mb() -> u64 {
    5
}
fn default_log_retain_count() -> u32 {
    1
}
fn default_proc_scan_interval() -> u64 {
    3
}
fn default_game_latch_sec() -> u64 {
    45
}
fn default_gaming_score_boost() -> i32 {
    35
}
fn default_adaptive_governor_enabled() -> bool {
    false
}

fn default_disable_tweaks() -> bool {
    false
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self {
            temp_cool: default_temp_cool(),
            temp_warm: default_temp_warm(),
            temp_hot: default_temp_hot(),
            temp_powersave: default_temp_powersave(),
            temp_critical: default_temp_critical(),
            poll_interval: default_poll_interval(),
            temp_history_size: default_temp_history_size(),
            prediction_window: default_prediction_window(),
            policy_debounce_sec: default_policy_debounce_sec(),
            log_level: default_log_level(),
            gpu_gaming_threshold: default_gpu_gaming_threshold(),
            log_rotate_mb: default_log_rotate_mb(),
            log_retain_count: default_log_retain_count(),
            proc_scan_interval: default_proc_scan_interval(),
            game_latch_sec: default_game_latch_sec(),
            gaming_score_boost: default_gaming_score_boost(),
            adaptive_governor_enabled: default_adaptive_governor_enabled(),
            disable_tweaks: default_disable_tweaks(),
            battery_stats_enabled: default_true(),
            min_actuation_interval_ms: default_min_actuation_interval_ms(),
            tcp_congestion_control_gaming: default_tcp_cc(),
            touch_network_stack: default_false(),
            safe_mode_after_crashes: default_safe_mode_after_crashes(),
            watchdog_stall_threshold: default_watchdog_stall_threshold(),
            trace_markers_enabled: default_false(),
        }
    }
}

impl ProfilesConfig {
    pub fn is_valid(&self) -> Result<(), &'static str> {
        if self.temp_cool >= self.temp_warm {
            return Err("temp_cool >= temp_warm");
        }
        if self.temp_warm >= self.temp_hot {
            return Err("temp_warm >= temp_hot");
        }
        if self.temp_hot >= self.temp_powersave {
            return Err("temp_hot >= temp_powersave");
        }
        if self.temp_powersave >= self.temp_critical {
            return Err("temp_powersave >= temp_critical");
        }
        if self.poll_interval == 0 {
            return Err("poll_interval == 0");
        }
        if self.temp_history_size == 0 {
            return Err("temp_history_size == 0");
        }
        if self.prediction_window == 0 {
            return Err("prediction_window == 0");
        }
        if self.policy_debounce_sec == 0 {
            return Err("policy_debounce_sec == 0");
        }
        if self.log_rotate_mb == 0 {
            return Err("log_rotate_mb == 0");
        }
        if self.log_retain_count == 0 {
            return Err("log_retain_count == 0");
        }
        if self.proc_scan_interval == 0 {
            return Err("proc_scan_interval == 0");
        }
        if self.game_latch_sec == 0 {
            return Err("game_latch_sec == 0");
        }
        Ok(())
    }
}

/// Structures representing `game_list.conf` (List of strings)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameListConfig {
    #[serde(default = "default_game_packages")]
    pub packages: Vec<String>,
}

fn default_game_packages() -> Vec<String> {
    vec![
        "com.miHoYo.GenshinImpact".to_string(),
        "com.miHoYo.hkrpg".to_string(),
        "com.pubg.imobile".to_string(),
        "com.pubg.krmobile".to_string(),
        "com.pubg.newstate".to_string(),
        "com.tencent.ig".to_string(),
        "com.tencent.tmgp.pubgmhd".to_string(),
        "com.krafton.pubgmobile".to_string(),
        "com.activision.callofduty.shooter".to_string(),
        "com.activision.callofduty.warzone".to_string(),
        "com.garena.game.codm".to_string(),
        "com.tencent.tmgp.cod".to_string(),
        "com.tencent.tmgp.kr.codm".to_string(),
        "com.roblox.client".to_string(),
        "com.roblox.client.vnggames".to_string(),
    ]
}

impl Default for GameListConfig {
    fn default() -> Self {
        Self {
            packages: default_game_packages(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub profiles: ProfilesConfig,
    pub games: GameListConfig,
}

impl AppConfig {
    /// Loads configuration files. Does not crash on missing or invalid files;
    /// instead logs errors and uses safe defaults.
    pub fn load_or_default<P: AsRef<Path>>(profiles_path: P, games_path: P) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();

        let profiles = match Self::load_profiles_toml(profiles_path.as_ref()) {
            Ok((prof, mut load_warns)) => {
                warnings.append(&mut load_warns);
                prof
            }
            Err(e) => {
                warnings.push(format!(
                    "Failed to load profiles config ({}), using embedded defaults.",
                    e
                ));
                ProfilesConfig::default()
            }
        };

        let games = match Self::load_game_list_conf(games_path.as_ref()) {
            Ok(g) => g,
            Err(e) => {
                warnings.push(format!(
                    "Failed to load game list config ({}), using embedded defaults.",
                    e
                ));
                GameListConfig::default()
            }
        };

        (Self { profiles, games }, warnings)
    }

    fn load_profiles_toml(path: &Path) -> Result<(ProfilesConfig, Vec<String>)> {
        let mut warnings = Vec::new();
        if !path.exists() {
            anyhow::bail!("File does not exist: {:?}", path);
        }
        let content = fs::read_to_string(path).context("Failed to read profiles configuration")?;
        let config: ProfilesConfig = match toml::from_str(&content) {
            Ok(c) => c,
            Err(e) => anyhow::bail!("Invalid TOML format in profiles config ({})", e),
        };

        // Validation
        if let Err(reason) = config.is_valid() {
            warnings.push(format!(
                "Invalid configuration thresholds or bounds: {}. Falling back to defaults.",
                reason
            ));
            return Ok((ProfilesConfig::default(), warnings));
        }

        Ok((config, warnings))
    }

    fn load_game_list_conf(path: &Path) -> Result<GameListConfig> {
        if !path.exists() {
            anyhow::bail!("File does not exist: {:?}", path);
        }
        let content = fs::read_to_string(path).context("Failed to read game list configuration")?;
        let mut packages_set = HashSet::new();

        for line in content.lines() {
            let mut line = line.trim();
            // strip inline comments if any
            if let Some(idx) = line.find('#') {
                line = line[..idx].trim();
            }
            if line.is_empty() {
                continue;
            }
            packages_set.insert(line.to_string());
        }

        if packages_set.is_empty() {
            Ok(GameListConfig::default())
        } else {
            let mut packages: Vec<String> = packages_set.into_iter().collect();
            packages.sort(); // Optional: sort for predictable output
            Ok(GameListConfig { packages })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_valid_profiles_toml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("profiles.toml");
        let toml_content = r#"
        temp_cool = 35
        temp_warm = 40
        log_level = "DEBUG"
        "#;
        fs::write(&path, toml_content).unwrap();

        let (config, _) = AppConfig::load_profiles_toml(&path).unwrap();
        assert_eq!(config.temp_cool, 35);
        assert_eq!(config.temp_warm, 40);
        assert_eq!(config.log_level, "DEBUG");
        assert_eq!(config.temp_hot, 58); // default
    }

    #[test]
    fn test_invalid_toml_fallback() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("profiles.toml");
        let toml_content = r#"
        temp_cool = "not a number"
        "#;
        fs::write(&path, toml_content).unwrap();

        assert!(AppConfig::load_profiles_toml(&path).is_err());
        let (config, _) = AppConfig::load_or_default(&path, &dir.path().join("fake_games"));
        assert_eq!(config.profiles, ProfilesConfig::default());
    }

    #[test]
    fn test_missing_files() {
        let dir = tempdir().unwrap();
        let (config, _) = AppConfig::load_or_default(
            &dir.path().join("missing.toml"),
            &dir.path().join("missing_games.txt"),
        );
        assert_eq!(config.profiles, ProfilesConfig::default());
        assert_eq!(config.games, GameListConfig::default());
    }

    #[test]
    fn test_threshold_validation_fallback() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("profiles.toml");
        let toml_content = r#"
        temp_cool = 90
        temp_critical = 80
        "#;
        fs::write(&path, toml_content).unwrap();

        let (config, _) = AppConfig::load_profiles_toml(&path).unwrap();
        assert_eq!(config.temp_cool, ProfilesConfig::default().temp_cool);
    }

    #[test]
    fn test_interval_validation_fallback() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("profiles.toml");
        let toml_content = r#"
        poll_interval = 0
        "#;
        fs::write(&path, toml_content).unwrap();

        let (config, _) = AppConfig::load_profiles_toml(&path).unwrap();
        assert_eq!(
            config.poll_interval,
            ProfilesConfig::default().poll_interval
        );
    }

    #[test]
    fn test_game_list_parsing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("game_list.txt");
        let content = r#"
        # comment

        com.test.game1
        com.test.game1 # duplicate
        com.test.game2
        "#;
        fs::write(&path, content).unwrap();

        let config = AppConfig::load_game_list_conf(&path).unwrap();
        assert_eq!(config.packages.len(), 2);
        assert!(config.packages.contains(&"com.test.game1".to_string()));
        assert!(config.packages.contains(&"com.test.game2".to_string()));
    }

    #[test]
    fn test_empty_game_list() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("game_list.txt");
        fs::write(&path, "\n# just comments\n").unwrap();

        let config = AppConfig::load_game_list_conf(&path).unwrap();
        assert_eq!(config.packages, GameListConfig::default().packages);
    }
}
