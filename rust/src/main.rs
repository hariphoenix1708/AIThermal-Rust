use thermalai_daemon::*;

use anyhow::Result;
use config::AppConfig;
use daemon::Daemon;
use std::env;
use std::path::Path;
use std::path::PathBuf;

fn main() -> Result<()> {
    let module_dir = env::var("THERMALAI_MODULE_DIR").unwrap_or_else(|_| {
        std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("."))
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .to_string()
    });

    let config_dir =
        env::var("THERMALAI_CONFIG_DIR").unwrap_or_else(|_| format!("{}/config", module_dir));

    let profiles_path = format!("{}/profiles.conf", config_dir);
    let games_path = format!("{}/game_list.conf", config_dir);

    let log_dir = env::var("THERMALAI_LOG_DIR").unwrap_or_else(|_| "/data/local/tmp".to_string());
    let state_dir = env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string());

    std::fs::create_dir_all(&state_dir)?;

    let pid_file = format!("{}/thermalai.pid", log_dir);

    let (config, warnings) = AppConfig::load_or_default(&profiles_path, &games_path);

    let _logger_guards = logger::init_logger(
        &config.profiles.log_level,
        &log_dir,
        config.profiles.log_rotate_mb,
        config.profiles.log_retain_count,
    )?;

    for warning in warnings.iter() {
        tracing::warn!("Config Warning: {}", warning);
    }

    let version = env!("CARGO_PKG_VERSION");
    let current_exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("unknown"))
        .to_string_lossy()
        .to_string();
    let current_pid = std::process::id();
    let startup_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let android_version = std::fs::read_to_string("/system/build.prop")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("ro.build.version.release="))
                .map(|l| l.replace("ro.build.version.release=", ""))
        })
        .unwrap_or_else(|| "Unknown".to_string());

    let kernel_version =
        std::fs::read_to_string("/proc/version").unwrap_or_else(|_| "Unknown".to_string());

    tracing::info!("════════════════════════════════════════");
    tracing::info!(" ThermalAI daemon starting up...");
    tracing::info!(" Version                   : {}", version);
    tracing::info!(" PID                       : {}", current_pid);
    tracing::info!(" Executable Path           : {}", current_exe);
    tracing::info!(" Resolved Module Directory : {}", module_dir);
    tracing::info!(" Resolved Config Directory : {}", config_dir);
    tracing::info!(" Resolved Log Directory    : {}", log_dir);
    tracing::info!(" Resolved State Directory  : {}", state_dir);
    tracing::info!(" Resolved PID File Path    : {}", pid_file);
    tracing::info!(" Android Version           : {}", android_version);
    tracing::info!(" Kernel Version            : {}", kernel_version);
    tracing::info!(" Startup Timestamp         : {}", startup_timestamp);
    tracing::info!(" Config loaded: {:?}", config.profiles);
    tracing::info!("════════════════════════════════════════");

    let mut daemon = Daemon::new(
        &pid_file,
        config,
        &state_dir,
        profiles_path.clone(),
        games_path.clone(),
    );

    // Hardware Discovery & Profile Loading
    tracing::info!("Starting hardware discovery...");
    let hardware_cache_path = std::path::Path::new(&state_dir).join("hardware_profile.json");
    let cache_existed_before_discovery = hardware_cache_path.exists();
    let hw_profile_result = hardware::discovery::discover_or_load(&state_dir);

    let hw_profile = match hw_profile_result {
        Ok(profile) => {
            let cache_status = if cache_existed_before_discovery {
                "LOADED"
            } else {
                "GENERATED"
            };
            tracing::info!(" Cache Status              : {}", cache_status);
            tracing::info!(" Detected Device/SOC       : {}", profile.device_identity);
            profile
        }
        Err(e) => {
            tracing::error!("Failed during hardware discovery: {}", e);
            return Err(e);
        }
    };

    let mut orchestrator = orchestrator::SystemOrchestrator::new(daemon.get_ctx(), hw_profile);
    if let Err(e) = orchestrator.bootstrap() {
        tracing::error!("Failed to bootstrap orchestrator: {}", e);
        return Err(e);
    }

    tracing::info!("Daemon initialized successfully.");

    daemon.register_task(Box::new(orchestrator));

    daemon.start()?;

    tracing::info!("ThermalAI daemon shutdown complete.");

    Ok(())
}
