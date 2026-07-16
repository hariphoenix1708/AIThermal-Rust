use notify::{Config, Event, EventKind, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;

pub fn spawn_config_watcher(
    config_path: String,
    game_list_path: String,
    reload_flag: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let (tx, rx) = channel();
        let mut watcher = match notify::RecommendedWatcher::new(tx, Config::default()) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("Failed to create config file watcher: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(Path::new(&config_path), RecursiveMode::NonRecursive) {
            tracing::warn!("Failed to watch config file {}: {}", config_path, e);
        }
        if let Err(e) = watcher.watch(Path::new(&game_list_path), RecursiveMode::NonRecursive) {
            tracing::warn!("Failed to watch game list file {}: {}", game_list_path, e);
        }

        for res in rx {
            if let Ok(Event { kind, .. }) = res {
                if matches!(kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    tracing::info!("Config or game list file changed on disk, flagging for reload");
                    reload_flag.store(true, Ordering::SeqCst);
                }
            }
        }
    });
}
