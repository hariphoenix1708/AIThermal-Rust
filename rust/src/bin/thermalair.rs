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

    let default_state = "/data/local/tmp/AIThermal/state".to_string();
    let default_log = "/data/local/tmp/AIThermal".to_string();

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
                println!("Usage: thermalair charging <adaptive|urgent|maxspeed|batterycare>");
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
            let log_file = find_latest_log(Path::new(&log_dir), "thermalai_thermal.log")
                .unwrap_or_else(|| Path::new(&log_dir).join("thermalai_thermal.log"));
            if let Ok(content) = fs::read_to_string(&log_file) {
                println!("--- Recent Policy Transitions ---");
                let mut count = 0;
                for line in content.lines().rev() {
                    if line.contains("transition")
                        || line.contains("Recovery ->")
                        || line.contains("Recovery cleared")
                        || line.contains("Game detected")
                        || line.contains("Game session ended")
                        || line.contains("Charging session started")
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
        .unwrap_or_else(|_| "/data/local/tmp/AIThermal/state".to_string())
        + "/thermalai_state.json";
    match std::fs::read_to_string(&state_file) {
        Err(_) => println!("Daemon not running (no state file found)"),
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Err(_) => println!("State file present but unreadable"),
            Ok(json) => {
                if json.get("ai_temp").is_none()
                    && json.get("status").and_then(|s| s.as_str()) == Some("starting")
                {
                    println!("Daemon running, waiting for first tick to complete");
                } else {
                    println!(
                        "Temps: {:?}",
                        json.get("ai_temp").unwrap_or(&serde_json::Value::Null)
                    );
                }
            }
        },
    }
}

fn show_policy() {
    let state_file = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/AIThermal/state".to_string())
        + "/thermalai_state.json";
    match std::fs::read_to_string(&state_file) {
        Err(_) => println!("Daemon not running (no state file found)"),
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Err(_) => println!("State file present but unreadable"),
            Ok(json) => {
                if json.get("policy").is_none()
                    && json.get("status").and_then(|s| s.as_str()) == Some("starting")
                {
                    println!("Daemon running, waiting for first tick to complete");
                } else {
                    println!(
                        "Policy: {:?}",
                        json.get("policy").unwrap_or(&serde_json::Value::Null)
                    );
                }
            }
        },
    }
}

fn show_gaming() {
    let state_dir = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/AIThermal/state".to_string());

    let state_file = format!("{}/thermalai_state.json", state_dir);
    let profile_file = format!("{}/game_profiles.json", state_dir);

    // --- GameTurbo status from daemon state ---
    match fs::read_to_string(&state_file) {
        Err(_) => println!("Daemon not running (no state file found)"),
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Err(_) => println!("State file present but unreadable"),
            Ok(json) => {
                let gaming = json.get("gaming");
                let turbo = json.get("game_turbo_active");
                let is_gaming = gaming
                    .and_then(|g| g.get("is_gaming"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let pkg = gaming
                    .and_then(|g| g.get("package"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("—");
                let pid = gaming
                    .and_then(|g| g.get("game_pid"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let jitter = gaming
                    .and_then(|g| g.get("avg_jitter_ms"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let rtt = gaming
                    .and_then(|g| g.get("avg_rtt_ms"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let fps = gaming
                    .and_then(|g| g.get("fps"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let loss = gaming
                    .and_then(|g| g.get("packet_loss"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                let turbo_active = turbo.and_then(|v| v.as_bool()).unwrap_or(false);

                println!("=== Gaming Status ===");
                println!(
                    "  Gaming:   {}",
                    if is_gaming {
                        format!("ACTIVE  ({})", pkg)
                    } else {
                        "inactive".to_string()
                    }
                );
                if is_gaming {
                    println!("  PID:      {}", pid);
                    println!("  FPS:      {:.1}", fps);
                }
                println!(
                    "  GameTurbo: {}",
                    if turbo_active {
                        "ACTIVE (8 features)"
                    } else {
                        "inactive"
                    }
                );
                if is_gaming {
                    println!("  Network:  RTT {:.1}ms | Jitter {:.1}ms | Loss {:.2}%", rtt, jitter, loss);
                }

                if !is_gaming && !turbo_active {
                    // Show last session data
                    let peak = json
                        .get("last_session_peak_temp")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if peak > 0 {
                        println!("  Last session peak: {}C", peak);
                    }
                }
            }
        },
    }

    // --- Per-game profiles ---
    match fs::read_to_string(&profile_file) {
        Err(_) => {
            println!("\n  No game profiles recorded yet.");
        }
        Ok(content) => {
            if let Ok(profiles) =
                serde_json::from_str::<serde_json::Value>(&content)
                && let Some(obj) = profiles.as_object()
            {
                if obj.is_empty() {
                    println!("\n  No game profiles recorded yet.");
                    return;
                }

                // Sort by last_seen descending.
                let mut entries: Vec<_> = obj.iter().collect();
                entries.sort_by(|a, b| {
                    let a_seen = a.1.get("last_seen").and_then(|v| v.as_u64()).unwrap_or(0);
                    let b_seen = b.1.get("last_seen").and_then(|v| v.as_u64()).unwrap_or(0);
                    b_seen.cmp(&a_seen)
                });

                println!("\n=== Game Profiles ===");
                println!(
                    "  {:<35} {:>5} {:>6} {:>5} {:>5} {:>6}",
                    "Package", "Sess", "MaxT", "Hot", "GTur", "AvgT"
                );
                println!("  {}", "-".repeat(65));

                for (pkg, p) in entries.iter().take(15) {
                    let sessions = p.get("session_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let max_t = p.get("max_temp").and_then(|v| v.as_i64()).unwrap_or(0);
                    let hot = if p.get("known_hot").and_then(|v| v.as_bool()).unwrap_or(false) {
                        "Y"
                    } else {
                        " "
                    };
                    let gt_sessions = p
                        .get("game_turbo_sessions")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let avg_t = p
                        .get("avg_peak_temp")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);

                    let short_pkg = if pkg.len() > 34 {
                        format!("{}…", &pkg[..33])
                    } else {
                        pkg.to_string()
                    };

                    println!(
                        "  {:<35} {:>5} {:>5}C {:>5} {:>5} {:>5.1}C",
                        short_pkg, sessions, max_t, hot, gt_sessions, avg_t
                    );
                }

                if entries.len() > 15 {
                    println!("  ... and {} more", entries.len() - 15);
                }
            }
        }
    }
}

fn set_charging_mode(mode: Option<&str>) {
    let state_dir = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/AIThermal/state".to_string());
    let override_file = format!("{}/charging_mode.json", state_dir);
    let tmp_file = format!("{}/charging_mode.json.tmp", state_dir);
    match mode {
        Some("urgent") => {
            // Auto-expire after 30 minutes so an accidental toggle can't leave
            // aggressive charging enabled indefinitely. The daemon clears the
            // override file itself once `expires_at` passes.
            const URGENT_TTL_SECS: u64 = 30 * 60;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let expires_at = now + URGENT_TTL_SECS;
            let payload = format!(r#"{{"urgent": true, "expires_at": {}}}"#, expires_at);
            if std::fs::write(&tmp_file, payload).is_ok() {
                let _ = std::fs::rename(&tmp_file, &override_file);
            }
            println!("Set charging mode to Urgent (auto-expires in 30 min)");
        }
        Some("adaptive") | None => {
            if std::fs::write(&tmp_file, r#"{"urgent": false}"#).is_ok() {
                let _ = std::fs::rename(&tmp_file, &override_file);
            }
            println!("Set charging mode to Adaptive (Default)");
        }
        Some("maxspeed") => {
            if std::fs::write(&tmp_file, r#"{"mode": "MaxSpeed"}"#).is_ok() {
                let _ = std::fs::rename(&tmp_file, &override_file);
            }
            println!("Set charging mode to MaxSpeed");
        }
        Some("batterycare") => {
            if std::fs::write(&tmp_file, r#"{"mode": "BatteryCare"}"#).is_ok() {
                let _ = std::fs::rename(&tmp_file, &override_file);
            }
            println!("Set charging mode to BatteryCare");
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
        std::env::var("THERMALAI_LOG_DIR").unwrap_or_else(|_| "/data/local/tmp/AIThermal".to_string());
    let state_dir = std::env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/AIThermal/state".to_string());
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

    if let Some(root) = module_root {
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
