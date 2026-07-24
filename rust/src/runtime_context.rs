use crate::config::AppConfig;

pub struct RuntimeContext {
    pub config: AppConfig,
    pub state_dir: String,
    pub snapshot_taken: bool,
    pub recovery_mode: bool,
    pub initialized: bool,
    pub runtime_health: bool,
    // Tick-level ownership:
    pub battery_temp_c: i32,
    pub trend_score: i32,
    pub prev_hot_trend: bool,
    pub sleep_ms: u64,
    pub current_policy: Option<String>,
    pub current_game: Option<String>,
    pub cooldown_active: bool,
    pub cooldown_until: Option<std::time::Instant>,
    pub cooldown_source_pkg: Option<String>,
    pub game_session_started_at: Option<std::time::Instant>,
    pub game_session_peak_temp: i32,
    pub last_gaming_state: bool,
    pub plugged_in_at: Option<std::time::Instant>,
    pub screen_off_since: Option<std::time::Instant>,
}
