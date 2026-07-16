use super::capability::CapabilityNode;
use super::profile::{CoolingDeviceProfile, ThermalProfile};
use std::fs;
use std::path::Path;

pub fn valid_thermal_temp_raw(temp: i32) -> bool {
    temp > -50000 && temp != 0 && temp < 200000
}

pub fn read_valid_temp_c(path: &str) -> Option<i32> {
    let temp = fs::read_to_string(path).ok()?.trim().parse::<i32>().ok()?;
    if !valid_thermal_temp_raw(temp) {
        return None;
    }
    Some(if temp > 1000 { temp / 1000 } else { temp })
}

pub fn verified_temp_path(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let path_str = path.to_string_lossy().to_string();
    read_valid_temp_c(&path_str)?;
    Some(path_str)
}

pub fn probe_thermal() -> ThermalProfile {
    let mut profile = ThermalProfile::default();

    if let Ok(entries) = fs::read_dir("/sys/class/thermal") {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();

            if file_name.starts_with("thermal_zone") {
                if let Ok(type_name) = fs::read_to_string(path.join("type")) {
                    let type_name = type_name.trim().to_string();
                    let Some(temp_path) = verified_temp_path(&path.join("temp")) else {
                        tracing::debug!("Skipping invalid thermal zone {}", path.display());
                        continue;
                    };

                    profile
                        .all_zones
                        .entry(type_name.clone())
                        .or_insert_with(|| temp_path.clone());

                    if profile.cpu_zone.is_none()
                        && (type_name.contains("cpu") || type_name.contains("soc"))
                    {
                        profile.cpu_zone = Some(temp_path.clone());
                    }
                    if profile.gpu_zone.is_none() && type_name.contains("gpu") {
                        profile.gpu_zone = Some(temp_path.clone());
                    }
                    if profile.battery_zone.is_none()
                        && (type_name.contains("battery") || type_name.contains("bms"))
                    {
                        profile.battery_zone = Some(temp_path.clone());
                    }
                    if profile.skin_zone.is_none() && type_name.contains("skin") {
                        profile.skin_zone = Some(temp_path.clone());
                    }
                    if profile.pmic_zone.is_none() && type_name.contains("pmic") {
                        profile.pmic_zone = Some(temp_path.clone());
                    }
                    if profile.charger_zone.is_none() && type_name.contains("chg") {
                        profile.charger_zone = Some(temp_path.clone());
                    }
                    if profile.usbc_zone.is_none() && type_name.contains("usb") {
                        profile.usbc_zone = Some(temp_path.clone());
                    }
                }
            } else if file_name.starts_with("cooling_device") {
                if let Ok(type_name) = fs::read_to_string(path.join("type")) {
                    let type_name = type_name.trim().to_string();
                    let sysfs_path = path.to_string_lossy().to_string();

                    let mut max_state = None;
                    if let Ok(max) = fs::read_to_string(path.join("max_state")) {
                        max_state = max.trim().parse().ok();
                    }

                    profile.cooling_devices.push(CoolingDeviceProfile {
                        name: file_name.clone(),
                        device_type: type_name,
                        sysfs_path: sysfs_path.clone(),
                        current_state: None, // Discovered dynamically
                        max_state,
                        state_node: CapabilityNode::new(
                            &format!("{}/cur_state", sysfs_path),
                            "cooling_cur_state",
                        ),
                    });
                }
            }
        }
    }
    profile
}
