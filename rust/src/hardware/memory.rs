use super::profile::MemoryProfile;
use std::path::Path;

#[allow(clippy::collapsible_if)]
pub fn probe_memory() -> MemoryProfile {
    let mut profile = MemoryProfile {
        mem_total_kb: None,
        mem_free_kb: None,
        mem_available_kb: None,
        has_psi: Path::new("/proc/pressure/memory").exists(),
        has_zram: Path::new("/sys/block/zram0").exists(),
        has_lmkd: Path::new("/sys/module/lowmemorykiller").exists()
            || Path::new("/dev/lmkd").exists(),
        swap_total_kb: None,
        swap_free_kb: None,
        memory_pressure_avg10: None,
        memory_pressure_avg60: None,
        memory_pressure_avg300: None,
        vm_parameters: std::collections::HashMap::new(),
        zram_devices: Vec::new(),
    };

    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            let mut parts = line.split_whitespace();
            if let Some(key) = parts.next() {
                if let Some(val_str) = parts.next() {
                    if let Ok(val) = val_str.parse::<u64>() {
                        match key {
                            "MemTotal:" => profile.mem_total_kb = Some(val),
                            "MemFree:" => profile.mem_free_kb = Some(val),
                            "MemAvailable:" => profile.mem_available_kb = Some(val),
                            "SwapTotal:" => profile.swap_total_kb = Some(val),
                            "SwapFree:" => profile.swap_free_kb = Some(val),
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    if profile.has_psi {
        if let Ok(psi) = std::fs::read_to_string("/proc/pressure/memory") {
            for line in psi.lines() {
                if line.starts_with("some ") {
                    // some avg10=0.00 avg60=0.00 avg300=0.00 total=0
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    for part in parts {
                        if let Some(val_str) = part.strip_prefix("avg10=") {
                            profile.memory_pressure_avg10 = val_str.parse::<f32>().ok();
                        } else if let Some(val_str) = part.strip_prefix("avg60=") {
                            profile.memory_pressure_avg60 = val_str.parse::<f32>().ok();
                        } else if let Some(val_str) = part.strip_prefix("avg300=") {
                            profile.memory_pressure_avg300 = val_str.parse::<f32>().ok();
                        }
                    }
                }
            }
        }
    }

    profile.vm_parameters = std::fs::read_dir("/proc/sys/vm")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    if e.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                        if let Ok(content) = std::fs::read_to_string(e.path()) {
                            Some((
                                e.file_name().to_string_lossy().into_owned(),
                                content.trim().to_string(),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    profile.zram_devices = std::fs::read_dir("/sys/block")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with("zram") {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    profile
}
