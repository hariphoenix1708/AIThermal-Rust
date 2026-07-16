use crate::hardware::HardwareProfile;
use crate::sysfs;
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct SensorNode {
    pub name: String,
    pub path: String,
    pub kind: String,
}

pub struct SensorManager {
    pub nodes: HashMap<String, SensorNode>,
    pub ambient_sensor_path: Option<String>,
}

impl Default for SensorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorManager {
    pub fn read_ambient_temp_c(&mut self) -> i32 {
        if let Some(path) = &self.ambient_sensor_path
            && let Ok(content) = std::fs::read_to_string(path)
            && let Ok(val) = content.trim().parse::<i32>()
        {
            return val / 1000;
        }

        // Scan for ambient
        if let Ok(entries) = std::fs::read_dir("/sys/bus/iio/devices") {
            for entry in entries.flatten() {
                let path = entry.path().join("in_temp_input");
                if path.exists() {
                    self.ambient_sensor_path = Some(path.to_string_lossy().to_string());
                    if let Ok(content) = std::fs::read_to_string(&path)
                        && let Ok(val) = content.trim().parse::<i32>()
                    {
                        return val / 1000;
                    }
                }
            }
        }

        25 // Default ambient
    }

    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            ambient_sensor_path: None,
        }
    }

    pub fn discover_hardware(&mut self, hw_profile: &HardwareProfile) {
        self.nodes.clear();
        // Use thermal zones from hardware profile
        // The hardware profile stores: all_zones: HashMap<type_name, temp_path>
        for (type_name, temp_path) in &hw_profile.thermal_profile.all_zones {
            self.nodes.insert(
                type_name.clone(),
                SensorNode {
                    name: type_name.clone(),
                    path: temp_path.clone(),
                    kind: type_name.clone(), // or we can just leave kind as the type_name
                },
            );
        }
        debug!("Loaded {} sensors from hardware profile.", self.nodes.len());
    }

    pub fn read_sensor(&self, name: &str) -> Option<i32> {
        if let Some(node) = self.nodes.get(name) {
            // Path was already formatted as /sys/class/thermal/{}/temp during discovery
            let temp_path = &node.path;
            if let Some(temp) = crate::hardware::thermal::read_valid_temp_c(temp_path) {
                return Some(temp);
            } else if let Err(e) = sysfs::read_string(temp_path) {
                tracing::debug!(
                    "Skipping unreadable sensor {} at {}: {}",
                    name,
                    temp_path,
                    e
                );
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensor_discovery() {
        let mut profile = HardwareProfile::default();
        profile
            .thermal_profile
            .all_zones
            .insert("test_zone".to_string(), "cpu".to_string());

        profile
            .thermal_profile
            .all_zones
            .insert("test_zone2".to_string(), "cpu2".to_string());

        let mut manager = SensorManager::new();
        manager.discover_hardware(&profile);

        assert_eq!(manager.nodes.len(), 2);
    }
}
