use std::fs;

pub fn is_screen_off() -> bool {
    let candidates = [
        "/sys/class/backlight/panel0-backlight/brightness",
        "/sys/class/backlight/panel1-backlight/brightness",
        "/sys/class/drm/card0-DSI-1/status",
    ];

    for path in candidates {
        if let Ok(content) = fs::read_to_string(path) {
            let val = content.trim();
            if val == "0" || val == "off" || val.contains("sleeping") {
                return true;
            } else if !val.is_empty() {
                return false;
            }
        }
    }

    // Default to false if we can't determine
    false
}

#[allow(clippy::collapsible_if)]
pub fn gpu_load_percent() -> Option<u32> {
    // 1. Check direct percentage if available
    if let Ok(content) = fs::read_to_string("/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage") {
        let trimmed = content.trim();
        if let Ok(v) = trimmed.parse::<u32>() {
            return Some(v.min(100));
        }
    }

    // 2. Check gpubusy pair (busy_time total_time)
    if let Ok(content) = fs::read_to_string("/sys/class/kgsl/kgsl-3d0/gpubusy") {
        let trimmed = content.trim();
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            if let (Ok(busy), Ok(total)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
                if total > 0 {
                    let pct = ((busy as f64 / total as f64) * 100.0).round() as u32;
                    return Some(pct.min(100));
                }
            }
        }
    }

    // 3. Devfreq fallback (requires reading busy_time and total_time separately)
    if let Ok(entries) = glob::glob("/sys/class/devfreq/*") {
        for e in entries.flatten() {
            let busy_path = e.join("busy_time");
            let total_path = e.join("total_time");

            if let (Ok(busy_content), Ok(total_content)) = (
                fs::read_to_string(&busy_path),
                fs::read_to_string(&total_path),
            ) {
                if let (Ok(busy), Ok(total)) = (
                    busy_content.trim().parse::<u64>(),
                    total_content.trim().parse::<u64>(),
                ) {
                    if total > 0 {
                        let pct = ((busy as f64 / total as f64) * 100.0).round() as u32;
                        return Some(pct.min(100));
                    }
                }
            }
        }
    }

    None
}
#[allow(clippy::collapsible_if)]
pub fn read_screen_brightness_percent(
    brightness_path: Option<&str>,
    max_brightness_path: Option<&str>,
) -> i32 {
    let default_brightness = 50;

    // Try provided paths first
    if let (Some(bp), Some(mp)) = (brightness_path, max_brightness_path) {
        if let (Ok(b_str), Ok(m_str)) = (fs::read_to_string(bp), fs::read_to_string(mp)) {
            if let (Ok(b), Ok(m)) = (b_str.trim().parse::<f64>(), m_str.trim().parse::<f64>()) {
                if m > 0.0 {
                    let pct = (b / m * 100.0) as i32;
                    return pct.clamp(0, 100);
                }
            }
        }
    }

    // Fallback to standard paths if not provided or failed
    let fallback_paths = [
        (
            "/sys/class/backlight/panel0-backlight/brightness",
            "/sys/class/backlight/panel0-backlight/max_brightness",
        ),
        (
            "/sys/class/backlight/panel1-backlight/brightness",
            "/sys/class/backlight/panel1-backlight/max_brightness",
        ),
    ];

    for (bp, mp) in fallback_paths {
        if let (Ok(b_str), Ok(m_str)) = (fs::read_to_string(bp), fs::read_to_string(mp)) {
            if let (Ok(b), Ok(m)) = (b_str.trim().parse::<f64>(), m_str.trim().parse::<f64>()) {
                if m > 0.0 {
                    let pct = (b / m * 100.0) as i32;
                    return pct.clamp(0, 100);
                }
            }
        }
    }

    default_brightness
}

use super::profile::DisplayProfile;
use std::path::Path;

fn writable_file(path: &Path) -> bool {
    path.is_file() && fs::OpenOptions::new().write(true).open(path).is_ok()
}

fn is_touch_controller_name(name: &str) -> bool {
    let lower = name.trim().to_lowercase();

    let excluded = [
        "gpio", "key", "pwrkey", "resin", "haptic", "jack", "headset", "button", "snd-card",
        "uinput", "virtual",
    ];

    if excluded.iter().any(|token| lower.contains(token)) {
        return false;
    }

    [
        "goodix_ts",
        "goodix",
        "touch",
        "fts",
        "focal",
        "synaptics",
        "novatek",
        "himax",
    ]
    .iter()
    .any(|token| lower.contains(token))
}

fn known_touch_tuning_attr(name: &str) -> bool {
    matches!(
        name,
        "touch_report_rate"
            | "report_rate"
            | "game_mode"
            | "touch_boost"
            | "sensitivity"
            | "high_touch_rate"
            | "edge_reject"
            | "glove_mode"
    )
}

pub fn probe_display() -> DisplayProfile {
    let mut profile = DisplayProfile::default();

    let fallback_paths = [
        (
            "/sys/class/backlight/panel0-backlight/brightness",
            "/sys/class/backlight/panel0-backlight/max_brightness",
        ),
        (
            "/sys/class/backlight/panel1-backlight/brightness",
            "/sys/class/backlight/panel1-backlight/max_brightness",
        ),
    ];

    for (bp, mp) in fallback_paths {
        if writable_file(Path::new(bp)) && Path::new(mp).is_file() {
            profile.brightness_path = Some(bp.to_string());
            profile.max_brightness_path = Some(mp.to_string());
            break;
        }
    }

    if let Ok(entries) = fs::read_dir("/sys/class/input") {
        let mut candidates: Vec<(String, std::path::PathBuf)> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name_path = path.join("name");
            let Ok(controller_name) = fs::read_to_string(&name_path) else {
                continue;
            };

            let controller_name = controller_name.trim().to_string();
            if !is_touch_controller_name(&controller_name) {
                tracing::debug!("Skipping non-touch input device: {}", controller_name);
                continue;
            }

            candidates.push((controller_name, path));
        }

        let mut selected: Option<(String, std::path::PathBuf)> = None;

        if let Some((name, path)) = candidates.iter().find(|(name, _)| name == "goodix_ts") {
            selected = Some((name.clone(), path.clone()));
        } else if let Some((name, path)) = candidates.into_iter().next() {
            selected = Some((name, path));
        }

        if let Some((controller_name, path)) = selected {
            profile.touch_controller_name = Some(controller_name);

            let mut discovered_nodes = Vec::new();
            for search_root in [path.clone(), path.join("device")] {
                let Ok(attrs) = fs::read_dir(&search_root) else {
                    continue;
                };

                for attr in attrs.flatten() {
                    let attr_path = attr.path();
                    let attr_name = attr.file_name().to_string_lossy().to_string();

                    if known_touch_tuning_attr(&attr_name) && writable_file(&attr_path) {
                        discovered_nodes.push(attr_path.to_string_lossy().to_string());
                    }
                }
            }

            discovered_nodes.sort();
            discovered_nodes.dedup();
            profile.touch_nodes = discovered_nodes;
        } else {
            tracing::debug!("Skipping touch tuning: no verified touch controller discovered");
            profile.touch_nodes = Vec::new();
        }
    }

    profile
}
