use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::{info, warn};

use std::sync::atomic::AtomicU64;

pub fn spawn_screen_state_watcher(screen_on: Arc<AtomicBool>, last_update: Arc<AtomicU64>) {
    std::thread::spawn(move || loop {
        match watch_uevent_for_screen_state(&screen_on, &last_update) {
            Ok(()) => {}
            Err(e) => {
                warn!("Screen netlink watcher error, falling back to polling: {}", e);
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        }
    });
}

fn watch_uevent_for_screen_state(screen_on: &Arc<AtomicBool>, last_update: &Arc<AtomicU64>) -> Result<(), Box<dyn std::error::Error>> {
    use netlink_sys::{protocols::NETLINK_KOBJECT_UEVENT, Socket, SocketAddr};

    let mut socket = Socket::new(NETLINK_KOBJECT_UEVENT)?;
    let addr = SocketAddr::new(std::process::id(), 1); // Multicast group 1 for uevents
    socket.bind(&addr)?;

    let mut buf = vec![0; 8192];

    loop {
        let (len, _) = socket.recv_from(&mut buf, 0)?;
        if len == 0 {
            continue;
        }

        // Parse null-separated variables in the uevent payload
        let payload = &buf[..len];

        let mut is_power_subsystem = false;
        let mut is_backlight_subsystem = false;
        let mut action = "";
        let mut power_action = "";

        for part in payload.split(|&b| b == 0) {
            if let Ok(s) = std::str::from_utf8(part) {
                if s.starts_with("SUBSYSTEM=") {
                    let sub = &s["SUBSYSTEM=".len()..];
                    if sub == "power" {
                        is_power_subsystem = true;
                    } else if sub == "backlight" {
                        is_backlight_subsystem = true;
                    }
                } else if s.starts_with("ACTION=") {
                    action = &s["ACTION=".len()..];
                } else if s.starts_with("POWER_ACTION=") {
                    power_action = &s["POWER_ACTION=".len()..];
                }
            }
        }

        if is_power_subsystem {
            if power_action == "early_suspend" {
                screen_on.store(false, Ordering::SeqCst);
                info!("Screen state changed via netlink: OFF");
            } else if power_action == "late_resume" {
                screen_on.store(true, Ordering::SeqCst);
                info!("Screen state changed via netlink: ON");
            }
        } else if is_backlight_subsystem && action == "change" {
            let is_off = super::display::is_screen_off();
            screen_on.store(!is_off, Ordering::SeqCst);
            info!(
                "Screen state changed via netlink (backlight change): {}",
                if is_off { "OFF" } else { "ON" }
            );
        }

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        last_update.store(now, Ordering::SeqCst);
    }
}
