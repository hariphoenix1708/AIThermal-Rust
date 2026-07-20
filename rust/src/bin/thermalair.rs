use anyhow::Result;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn find_latest_log(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with(prefix))
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    candidates.sort_by_key(|(t, _)| *t);
    candidates.pop().map(|(_, p)| p)
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Usage: thermalair <command> [options]");
        println!(
            "Commands: status, logs, gaming, temps, stop, start, restart, policy, calibrate, history, verbose, charging adaptive, charging urgent"
        );
        return Ok(());
    }

    let command = args[1].as_str();

    let magisk_path = Path::new("/data/adb/modules/thermalai_rust");
    let apatch_path = Path::new("/data/adb/ap/modules/thermalai_rust");

    let mut resolved_module_dir = None;
    if magisk_path.exists() {
        resolved_module_dir = Some(magisk_path);
    } else if apatch_path.exists() {
        resolved_module_dir = Some(apatch_path);
    }

    let default_state = "/data/local/tmp/thermalai_state".to_string();
    let default_log = "/data/local/tmp".to_string();

    let state_dir = env::var("THERMALAI_STATE_DIR").unwrap_or(default_state);
    let log_dir = env::var("THERMALAI_LOG_DIR").unwrap_or(default_log);

    // Store module root in env to be accessed by start_daemon
    if let Some(p) = resolved_module_dir {
        unsafe { env::set_var("THERMALAI_MODULE_DIR", p.to_string_lossy().to_string()) };
    }

    match command {
        "status" => {
            let state_file = Path::new(&state_dir).join("thermalai_state.json");
            if let Ok(content) = fs::read_to_string(&state_file) {
                println!("thermalai_rust Daemon Status:\n{}", content);
            } else {
                println!("Failed to read daemon state. Is the daemon running?");
            }
        }
        "logs" => {
            let log_file = find_latest_log(Path::new(&log_dir), "thermalai.log")
                .unwrap_or_else(|| Path::new(&log_dir).join("thermalai.log"));
            if let Ok(content) = fs::read_to_string(&log_file) {
                println!("{}", content);
            } else {
                println!("Failed to read logs at {:?}", log_file);
            }
        }
        "stop" => {
            let pid_file = Path::new(&log_dir).join("thermalai.pid");
            if let Ok(pid_str) = fs::read_to_string(&pid_file) {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    println!("Sending SIGTERM to daemon PID {}...", pid);
                    unsafe { nix::libc::kill(pid, nix::libc::SIGTERM) };
                    for _ in 0..10 {
                        if !pid_alive(pid) {
                            let _ = fs::remove_file(&pid_file);
                            println!("Daemon stopped.");
                            return Ok(());
                        }
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    println!("Daemon did not exit within timeout.");
                }
            } else {
                println!("PID file not found. Daemon may not be running.");
            }
        }
        "start" => {
            start_daemon()?;
        }
        "restart" => {
            println!("Stopping daemon...");
            let pid_file = Path::new(&log_dir).join("thermalai.pid");
            if let Ok(pid_str) = fs::read_to_string(&pid_file)
                && let Ok(pid) = pid_str.trim().parse::<i32>()
            {
                unsafe { nix::libc::kill(pid, nix::libc::SIGTERM) };
                for _ in 0..10 {
                    if !pid_alive(pid) {
                        let _ = fs::remove_file(&pid_file);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
            start_daemon()?;
        }
        "temps" => show_temps(),
        "policy" => show_policy(),
        "gaming" => show_gaming(),
        "charging" => {
            if args.len() < 3 {
                println!("Usage: thermalair charging <adaptive|urgent>");
                return Ok(());
            }
            set_charging_mode(Some(args[2].as_str()));
        }
        "calibrate" => {
            let cal_file = Path::new(&state_dir).join("calibration.json");
            if let Ok(content) = fs::read_to_string(&cal_file) {
                println!("Calibration State:\n{}", content);
            } else {
                println!("No calibration state found or unable to read.");
            }
        }
        "history" => {
            let log_file = find_latest_log(Path::new(&log_dir), "thermalai.log")
                .unwrap_or_else(|| Path::new(&log_dir).join("thermalai.log"));
            if let Ok(content) = fs::read_to_string(&log_file) {
                println!("--- Recent Policy Transitions ---");
                let mut count = 0;
                for line in content.lines().rev() {
                    if line.contains("transition")
                        || line.contains("Policy changed")
                        || line.contains("Applying policy")
                        || line.contains("Evaluating policy")
                        || line.contains("Starting session")
                    {
                        println!("{}", line);
                        count += 1;
                        if count >= 10 {
                            break;
                        }
                    }
                }
                println!();
            }

            let session_file = Path::new(&state_dir).join("charging_session.json");
            if let Ok(content) = fs::read_to_string(&session_file) {
                println!("--- Last Charging Session ---");
                println!("{}", content);
            } else {
                println!("No charging session history found.");
            }

            let games_file = Path::new(&state_dir).join("game_profiles.json");
            if let Ok(content) = fs::read_to_string(&games_file) {
                println!("\n--- Game Profiles History ---");
                println!("{}", content);
            }
        }
        "verbose" => {
            let verbose_file = find_latest_log(Path::new(&log_dir), "thermalai_verbose.log")
                .unwrap_or_else(|| Path::new(&log_dir).join("thermalai_verbose.log"));
            if let Some(arg) = args.get(2)
                && arg == "clear"
            {
                let _ = fs::write(&verbose_file, "");
                println!("Verbose log cleared.");
                return Ok(());
            }

            if let Ok(content) = fs::read_to_string(&verbose_file) {
                let lines: Vec<&str> = content.lines().collect();
                let limit = args
                    .get(2)
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(100);
                for line in lines.iter().rev().take(limit).rev() {
                    println!("{}", line);
                }
            } else {
                println!("Verbose log not found at {:?}", verbose_file);
            }
        }
        _ => {
            println!("Unknown command: {}", command);
            println!(
                "Commands: status, logs, gaming, temps, stop, start, restart, policy, calibrate, history, verbose, charging adaptive, charging urgent"
            );
        }
    }

    Ok(())
}

fn show_temps() {
    let state_file = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string())
        + "/thermalai_state.json";
    match std::fs::read_to_string(&state_file) {
        Err(_) => println!("Daemon not running (no state file found)"),
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Err(_) => println!("State file present but unreadable"),
            Ok(json) => {
                if json.get("ai_temp").is_none() && json.get("status").and_then(|s| s.as_str()) == Some("starting") {
                    println!("Daemon running, waiting for first tick to complete");
                } else {
                    println!(
                        "Temps: {:?}",
                        json.get("ai_temp").unwrap_or(&serde_json::Value::Null)
                    );
                }
            }
        }
    }
}

fn show_policy() {
    let state_file = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string())
        + "/thermalai_state.json";
    match std::fs::read_to_string(&state_file) {
        Err(_) => println!("Daemon not running (no state file found)"),
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Err(_) => println!("State file present but unreadable"),
            Ok(json) => {
                if json.get("policy").is_none() && json.get("status").and_then(|s| s.as_str()) == Some("starting") {
                    println!("Daemon running, waiting for first tick to complete");
                } else {
                    println!(
                        "Policy: {:?}",
                        json.get("policy").unwrap_or(&serde_json::Value::Null)
                    );
                }
            }
        }
    }
}

fn show_gaming() {
    let state_file = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string())
        + "/thermalai_state.json";
    match std::fs::read_to_string(&state_file) {
        Err(_) => println!("Daemon not running (no state file found)"),
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Err(_) => println!("State file present but unreadable"),
            Ok(json) => {
                if json.get("gaming").is_none() && json.get("status").and_then(|s| s.as_str()) == Some("starting") {
                    println!("Daemon running, waiting for first tick to complete");
                } else {
                    println!(
                        "Gaming: {:?}",
                        json.get("gaming").unwrap_or(&serde_json::Value::Null)
                    );
                }
            }
        }
    }
}

fn set_charging_mode(mode: Option<&str>) {
    let state_dir = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string());
    let override_file = format!("{}/charging_mode.json", state_dir);
    let tmp_file = format!("{}/charging_mode.json.tmp", state_dir);
    match mode {
        Some("urgent") => {
            if std::fs::write(&tmp_file, r#"{"urgent": true}"#).is_ok() {
                let _ = std::fs::rename(&tmp_file, &override_file);
            }
            println!("Set charging mode to Urgent");
        }
        Some("adaptive") | None => {
            if std::fs::write(&tmp_file, r#"{"urgent": false}"#).is_ok() {
                let _ = std::fs::rename(&tmp_file, &override_file);
            }
            println!("Set charging mode to Adaptive (Default)");
        }
        Some(other) => println!("Unknown charging mode: {}", other),
    }
}

fn start_daemon() -> Result<()> {
    println!("Starting thermalai_rust daemon...");
    let magisk_path = Path::new("/data/adb/modules/thermalai_rust");
    let apatch_path = Path::new("/data/adb/ap/modules/thermalai_rust");

    let mut module_root = None;
    if magisk_path.exists() {
        module_root = Some(magisk_path);
    } else if apatch_path.exists() {
        module_root = Some(apatch_path);
    }

    let log_dir =
        std::env::var("THERMALAI_LOG_DIR").unwrap_or_else(|_| "/data/local/tmp".to_string());
    let state_dir = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string());
    let pid_file = Path::new(&log_dir).join("thermalai.pid");

    if let Ok(pid_str) = std::fs::read_to_string(&pid_file)
        && let Ok(pid) = pid_str.trim().parse::<i32>()
    {
        if pid_alive(pid) {
            println!("Daemon already running (PID: {}).", pid);
            std::process::exit(0);
        }
        let _ = std::fs::remove_file(&pid_file);
        let _ = std::fs::remove_file(PathBuf::from(format!(
            "{}.lock",
            pid_file.to_string_lossy()
        )));
    }

    let _ = std::fs::create_dir_all(&log_dir);
    let _ = std::fs::create_dir_all(&state_dir);

    let mut spawned = false;

    if let Some(root) = module_root.clone() {
        let service_sh = root.join("service.sh");
        if service_sh.exists() {
            println!("Executing service.sh from module root...");
            match std::process::Command::new("sh")
                .arg(service_sh)
                .env("THERMALAI_LOG_DIR", &log_dir)
                .env("THERMALAI_STATE_DIR", &state_dir)
                .status()
            {
                Ok(status) if status.success() => {
                    spawned = true;
                }
                Ok(status) => anyhow::bail!("service.sh exited with status {}", status),
                Err(e) => anyhow::bail!("failed to execute service.sh: {}", e),
            }
        } else {
            let daemon_bin = root.join("system").join("bin").join("thermalai-daemon");
            if daemon_bin.exists() {
                println!("Executing thermalai-daemon directly from module root...");
                if std::process::Command::new(daemon_bin)
                    .env("THERMALAI_MODULE_DIR", root)
                    .env("THERMALAI_LOG_DIR", &log_dir)
                    .env("THERMALAI_STATE_DIR", &state_dir)
                    .spawn()
                    .is_ok()
                {
                    spawned = true;
                }
            }
        }
    }

    if !spawned {
        // Fallback: execute thermalai-daemon directly
        println!("service.sh not found. Attempting to start thermalai-daemon directly...");
        if std::process::Command::new("thermalai-daemon")
            .env("THERMALAI_LOG_DIR", &log_dir)
            .env("THERMALAI_STATE_DIR", &state_dir)
            .spawn()
            .is_ok()
        {
            spawned = true;
        } else {
            anyhow::bail!("failed to start thermalai-daemon directly");
        }
    }

    if spawned {
        let pid = wait_for_validated_daemon(&pid_file)?;
        println!("Daemon started successfully (PID: {}).", pid);
        return Ok(());
    }

    anyhow::bail!("daemon launcher did not spawn a process")
}

fn pid_alive(pid: i32) -> bool {
    (unsafe { nix::libc::kill(pid, 0) }) == 0
}

fn validated_daemon_pid(pid_file: &Path) -> Option<i32> {
    let pid = std::fs::read_to_string(pid_file)
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()?;
    pid_alive(pid).then_some(pid)
}

fn wait_for_validated_daemon(pid_file: &Path) -> Result<i32> {
    let mut last_reason = format!("PID file not found at {}", pid_file.display());
    for _ in 0..12 {
        if let Some(pid) = validated_daemon_pid(pid_file) {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if pid_alive(pid) {
                return Ok(pid);
            }
            last_reason = format!("daemon PID {} died during validation delay", pid);
        } else if pid_file.exists() {
            match std::fs::read_to_string(pid_file) {
                Ok(pid_str) => {
                    last_reason =
                        format!("PID file contains invalid or dead PID: {}", pid_str.trim());
                }
                Err(e) => {
                    last_reason = format!("PID file exists but could not be read: {}", e);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    anyhow::bail!("daemon startup validation failed: {}", last_reason)
}
