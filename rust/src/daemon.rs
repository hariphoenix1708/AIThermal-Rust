use crate::config::AppConfig;
use anyhow::{Context, Result};

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

use crate::runtime_context::RuntimeContext;

pub trait RuntimeTask: Send + Sync {
    fn execute(&mut self, ctx: &mut RuntimeContext) -> Result<()>;
    fn cleanup(&mut self) {}
}

#[derive(Default)]
pub struct RuntimeState {
    pub poll_count: u64,
}

pub struct Daemon {
    tasks: Vec<Box<dyn RuntimeTask>>,
    ctx: RuntimeContext,
    running: Arc<AtomicBool>,
    pid_file: String,
    lock_file: String,
    config_path: String,
    game_list_path: String,
    reload_flag: Arc<AtomicBool>,
    screen_on: Arc<AtomicBool>,
    last_screen_netlink_update: Arc<AtomicU64>,
}

impl Daemon {
    pub fn new(pid_file: &str, config: AppConfig, state_dir: &str, config_path: String, game_list_path: String) -> Self {
        let initial_screen_off = crate::hardware::display::is_screen_off();
        let lock_file = format!("{}.lock", pid_file);
        let ctx = RuntimeContext {
            config: config.clone(),
            state_dir: state_dir.to_string(),
            snapshot_taken: false,
            recovery_mode: false,
            initialized: false,
            runtime_health: true,
            battery_temp_c: 0,
            trend_score: 0,
            sleep_ms: config.profiles.poll_interval.saturating_mul(1000),
            current_policy: None,
            current_game: None,
            cooldown_active: false,
            cooldown_until: None,
            cooldown_source_pkg: None,
            game_session_started_at: None,
            game_session_peak_temp: 0,
            last_gaming_state: false,
            plugged_in_at: None,
            screen_off_since: None,
        };

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        Self {
            pid_file: pid_file.to_string(),
            lock_file,
            tasks: Vec::new(),
            ctx,
            running: Arc::new(AtomicBool::new(false)),
            config_path,
            game_list_path,
            reload_flag: Arc::new(AtomicBool::new(false)),
            screen_on: Arc::new(AtomicBool::new(!initial_screen_off)),
            last_screen_netlink_update: Arc::new(AtomicU64::new(now)),
        }
    }

    fn check_screen_off(&self) -> bool {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let last_update = self.last_screen_netlink_update.load(Ordering::SeqCst);
        let netlink_off = !self.screen_on.load(Ordering::SeqCst);

        if now >= last_update && (now - last_update) < 60 {
            netlink_off
        } else {
            crate::hardware::display::is_screen_off()
        }
    }

    pub fn get_ctx(&self) -> &RuntimeContext {
        &self.ctx
    }

    pub fn register_task(&mut self, task: Box<dyn RuntimeTask>) {
        self.tasks.push(task);
    }

    fn check_lock_file(&self) -> Result<()> {
        if Path::new(&self.lock_file).exists() {
            if let Ok(content) = fs::read_to_string(&self.pid_file) {
                // allow(clippy::collapsible_if) to avoid requiring nightly let-chains
                #[allow(clippy::collapsible_if)]
                if let Ok(pid) = content.trim().parse::<i32>() {
                    let ret = unsafe { nix::libc::kill(pid, 0) };
                    if ret == 0 {
                        anyhow::bail!("Daemon is already running with PID {}", pid);
                    } else if ret == -1 {
                        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        if err == nix::libc::EPERM {
                            anyhow::bail!("Daemon is already running with PID {}", pid);
                        }
                    }
                }
            }
            tracing::warn!("Found stale lock file. Cleaning up...");
            let _ = fs::remove_file(&self.lock_file);
        }
        Ok(())
    }

    pub fn start(&mut self) -> Result<()> {
        self.check_lock_file()?;

        if let Err(e) = self.write_pid_file() {
            self.cleanup();
            return Err(e);
        }

        // Scope block to catch errors and trigger rollback
        let mut startup = || -> Result<()> {
            self.write_lock_file()?;
            self.setup_signal_handlers()?;
            self.ctx.initialized = true;

            crate::watcher::spawn_config_watcher(
                self.config_path.clone(),
                self.game_list_path.clone(),
                self.reload_flag.clone(),
            );

            crate::hardware::screen_netlink::spawn_screen_state_watcher(
                self.screen_on.clone(),
                self.last_screen_netlink_update.clone()
            );

            Ok(())
        };

        if let Err(e) = startup() {
            self.cleanup();
            return Err(e);
        }

        self.running.store(true, Ordering::SeqCst);

        while self.running.load(Ordering::SeqCst) {
            if self.reload_flag.load(Ordering::SeqCst) {
                self.reload_flag.store(false, Ordering::SeqCst);
                tracing::info!("Reloading config due to file change");
                let (new_config, _) = crate::config::AppConfig::load_or_default(&self.config_path, &self.game_list_path);
                self.ctx.config = new_config;
            }

            if let Err(e) = self.tick() {
                error!("Error in daemon tick: {}", e);
            }

            let sleep_ms = self.ctx.sleep_ms.max(1);
            let was_screen_off = self.check_screen_off();
            // Tune segment_ms: smaller reacts faster but polls screen state more often during idle;
            // larger reduces file reads but increases worst-case wake latency.
            let segment_ms: u64 = 500;
            let mut elapsed_ms: u64 = 0;

            while elapsed_ms < sleep_ms {
                let this_segment = segment_ms.min(sleep_ms - elapsed_ms);
                thread::sleep(Duration::from_millis(this_segment));
                elapsed_ms += this_segment;

                // Only bother re-checking screen state early if we're in a long
                // (idle-tier) sleep to begin with - short sleeps don't need this.
                if sleep_ms > 2000 {
                    let now_screen_off = self.check_screen_off();
                    if was_screen_off && !now_screen_off {
                        tracing::debug!("Screen turned on during idle sleep, waking daemon early");
                        break;
                    }
                }
            }
        }

        self.cleanup();
        Ok(())
    }

    fn tick(&mut self) -> Result<()> {
        let mut healthy = true;
        for task in self.tasks.iter_mut() {
            if let Err(e) = task.execute(&mut self.ctx) {
                error!("Task execution error: {}", e);
                healthy = false;
            }
        }
        self.ctx.runtime_health = healthy;
        Ok(())
    }

    pub fn write_pid_file(&self) -> Result<()> {
        let pid = std::process::id();
        fs::write(&self.pid_file, pid.to_string())
            .with_context(|| format!("Failed to write PID file: {}", self.pid_file))?;
        Ok(())
    }

    pub fn write_lock_file(&self) -> Result<()> {
        fs::write(&self.lock_file, "")
            .with_context(|| format!("Failed to write lock file: {}", self.lock_file))?;
        Ok(())
    }

    pub fn cleanup(&mut self) {
        info!("Restoring original system state and cleaning up files...");

        for task in self.tasks.iter_mut() {
            task.cleanup();
        }

        // Snapshot restore
        let hw = crate::cache::load_profile(&self.ctx.state_dir).unwrap_or_default();
        let snapshot_manager = crate::snapshot::SnapshotManager::new(&self.ctx.state_dir, hw);
        snapshot_manager.restore_snapshot();

        if Path::new(&self.pid_file).exists() {
            #[allow(clippy::collapsible_if)]
            if let Err(e) = fs::remove_file(&self.pid_file) {
                warn!("Failed to remove PID file: {}", e);
            }
        }

        if Path::new(&self.lock_file).exists() {
            #[allow(clippy::collapsible_if)]
            if let Err(e) = fs::remove_file(&self.lock_file) {
                warn!("Failed to remove lock file: {}", e);
            }
        }
    }

    fn setup_signal_handlers(&self) -> Result<()> {
        let running = Arc::clone(&self.running);
        let running_term = Arc::clone(&self.running);

        // SIGINT (Ctrl-C)
        ctrlc::set_handler(move || {
            warn!("Received SIGINT. Initiating graceful shutdown...");
            running.store(false, Ordering::SeqCst);
        })
        .context("Error setting Ctrl-C handler")?;

        // SIGTERM (systemctl stop / kill)
        #[cfg(unix)]
        {
            use nix::sys::signal::{SigHandler, Signal, signal};
            use std::sync::atomic::AtomicBool;
            use std::thread;

            // To safely use nix signal we create a simple static atomic bool.
            // Since Daemon uses an Arc<AtomicBool>, we'll use a thread to monitor a static bool
            // or just use sigaction. For simplicity, we'll spawn a signal thread using signal_hook if we had it,
            // but we can just use a static atomic for SIGTERM.

            static TERMINATE: AtomicBool = AtomicBool::new(false);

            extern "C" fn handle_sigterm(_: nix::libc::c_int) {
                TERMINATE.store(true, Ordering::SeqCst);
            }

            unsafe {
                let _ = signal(Signal::SIGTERM, SigHandler::Handler(handle_sigterm));
            }

            // Spawn a monitor thread
            thread::spawn(move || {
                while !TERMINATE.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(500));
                }
                warn!("Received SIGTERM. Initiating graceful shutdown...");
                running_term.store(false, Ordering::SeqCst);
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use tempfile::tempdir;

    #[test]
    fn test_daemon_lifecycle() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        let pid_str = pid_path.to_string_lossy().to_string();

        let (config, _) = AppConfig::load_or_default("missing", "missing");
        let mut daemon = Daemon::new(&pid_str, config, dir.path().to_str().unwrap(), "".to_string(), "".to_string());

        let lock_path = Path::new(&daemon.lock_file).to_path_buf();

        assert!(daemon.write_pid_file().is_ok());
        assert!(pid_path.exists());

        assert!(daemon.write_lock_file().is_ok());
        assert!(lock_path.exists());

        daemon.ctx.initialized = true;

        daemon.tick().unwrap();

        daemon.cleanup();
        assert!(!pid_path.exists());
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_stale_lock() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("stale.pid");
        let pid_str = pid_path.to_string_lossy().to_string();

        let (config, _) = AppConfig::load_or_default("missing", "missing");
        let daemon = Daemon::new(&pid_str, config, dir.path().to_str().unwrap(), "".to_string(), "".to_string());

        let lock_path = Path::new(&daemon.lock_file).to_path_buf();

        fs::write(&pid_path, "99999999").unwrap(); // Non existent PID
        fs::write(&lock_path, "").unwrap();

        // Should succeed because PID doesn't exist
        assert!(daemon.check_lock_file().is_ok());
    }

    #[test]
    fn test_active_lock_fails() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("active.pid");
        let pid_str = pid_path.to_string_lossy().to_string();

        let (config, _) = AppConfig::load_or_default("missing", "missing");
        let daemon = Daemon::new(&pid_str, config, dir.path().to_str().unwrap(), "".to_string(), "".to_string());
        let lock_path = Path::new(&daemon.lock_file).to_path_buf();

        fs::write(&pid_path, std::process::id().to_string()).unwrap(); // Using our own PID
        fs::write(&lock_path, "").unwrap();

        // Should fail because PID exists and belongs to us
        assert!(daemon.check_lock_file().is_err());
    }
}
