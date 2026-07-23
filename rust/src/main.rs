use thermalai_daemon::*;

use anyhow::Result;
use config::AppConfig;
use daemon::Daemon;
use std::env;
use std::path::Path;
use std::path::PathBuf;

fn main() -> Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        tracing::error!("PANIC: {}", panic_info);
        // Best-effort attempt to restore hardware to its snapshotted
        // state before the process goes down - this cannot guarantee
        // success (the panic may have occurred inside the restore path
        // itself, or state may be inconsistent), but gives a real chance
        // of avoiding a stuck sysfs state that would otherwise persist
        // until the next clean daemon restart.
        let state_dir = std::env::var("THERMALAI_STATE_DIR")
            .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string());
        if let Ok(hw) = thermalai_daemon::cache::load_profile(&state_dir) {
            let snapshot = thermalai_daemon::snapshot::SnapshotManager::new(&state_dir, hw);
            snapshot.restore_snapshot();
        }
        default_hook(panic_info);
    }));

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

    tracing::info!(target: "lifecycle", "════════════════════════════════════════");
    tracing::info!(target: "lifecycle", " ThermalAI daemon starting up...");
    tracing::info!(target: "lifecycle", " Version                   : {}", version);
    tracing::info!(target: "lifecycle", " PID                       : {}", current_pid);
    tracing::info!(target: "lifecycle", " Executable Path           : {}", current_exe);
    tracing::info!(target: "lifecycle", " Resolved Module Directory : {}", module_dir);
    tracing::info!(target: "lifecycle", " Resolved Config Directory : {}", config_dir);
    tracing::info!(target: "lifecycle", " Resolved Log Directory    : {}", log_dir);
    tracing::info!(target: "lifecycle", " Resolved State Directory  : {}", state_dir);
    tracing::info!(target: "lifecycle", " Resolved PID File Path    : {}", pid_file);
    tracing::info!(target: "lifecycle", " Android Version           : {}", android_version);
    tracing::info!(target: "lifecycle", " Kernel Version            : {}", kernel_version);
    tracing::info!(target: "lifecycle", " Startup Timestamp         : {}", startup_timestamp);
    tracing::info!(target: "lifecycle", " Config loaded: {:?}", config.profiles);
    tracing::info!(target: "lifecycle", "════════════════════════════════════════");

    let crash_marker = std::path::Path::new(&state_dir).join("crash_marker.json");
    let crash_count: u32 = std::fs::read_to_string(&crash_marker)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("count").and_then(|c| c.as_u64()))
        .map(|c| c as u32)
        .unwrap_or(0);

    let mut config = config; // shadow so we can force safe mode
    if crash_count >= config.profiles.safe_mode_after_crashes {
        tracing::warn!(
            "Entering SAFE MODE: {} consecutive crashes detected — forcing disable_tweaks=true",
            crash_count
        );
        config.profiles.disable_tweaks = true;
    }

    // Write a fresh crash_marker BEFORE we start, so if we die before
    // clean shutdown the next boot sees +1.
    let _ = std::fs::write(
        &crash_marker,
        serde_json::json!({ "count": crash_count + 1 }).to_string(),
    );

    let mut daemon = Daemon::new(
        &pid_file,
        config,
        &state_dir,
        profiles_path.clone(),
        games_path.clone(),
    );

    // Hardware Discovery & Profile Loading
    tracing::info!(target: "lifecycle", "Starting hardware discovery...");
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
            tracing::info!(target: "lifecycle", " Cache Status              : {}", cache_status);
            tracing::info!(target: "lifecycle", " Detected Device/SOC       : {}", profile.device_identity);
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

    tracing::info!(target: "lifecycle", "Daemon initialized successfully.");

    daemon.register_task(Box::new(orchestrator));

    let start_result = daemon.start();
    let _ = std::fs::remove_file(&crash_marker);
    start_result?;

    tracing::info!(target: "lifecycle", "ThermalAI daemon shutdown complete.");

    Ok(())
}
