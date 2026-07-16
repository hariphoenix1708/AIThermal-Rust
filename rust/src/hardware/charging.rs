use super::capability::CapabilityNode;
use super::profile::{BatteryProfile, ChargingProfile};
use std::path::Path;

pub fn probe_battery() -> BatteryProfile {
    let candidates = [
        "/sys/class/power_supply/battery",
        "/sys/class/power_supply/bms",
        "/sys/class/power_supply/main",
    ];
    for path in candidates {
        if Path::new(path).exists() {
            return BatteryProfile {
                path: path.to_string(),
                capability_nodes: vec![CapabilityNode::new(
                    &format!("{}/capacity", path),
                    "battery_capacity",
                )],
                ..Default::default()
            };
        }
    }
    BatteryProfile::default()
}

pub fn probe_charging() -> ChargingProfile {
    let mut profile = ChargingProfile::default();
    let candidates = [
        "/sys/class/power_supply/usb",
        "/sys/class/power_supply/dc",
        "/sys/class/power_supply/ac",
        "/sys/class/power_supply/pc_port",
    ];
    for path in candidates {
        if Path::new(path).exists() {
            profile.path = path.to_string();
            break;
        }
    }

    let limit_nodes = [
        "/sys/class/power_supply/battery/constant_charge_current_max",
        "/sys/class/power_supply/bms/constant_charge_current_max",
        "/sys/class/power_supply/battery/current_max",
        "/sys/class/power_supply/main/constant_charge_current_max",
        "/sys/class/power_supply/main/current_max",
        "/sys/class/power_supply/usb/current_max",
        "/sys/class/power_supply/usb/input_current_limit",
        "/sys/class/power_supply/dc/current_max",
        "/sys/class/power_supply/dc/input_current_limit",
        "/sys/class/power_supply/ac/current_max",
        "/sys/class/power_supply/ac/input_current_limit",
    ];

    for node in limit_nodes {
        if Path::new(node).exists() {
            profile.current_limit_nodes.push(node.to_string());
            profile
                .capability_nodes
                .push(CapabilityNode::new(node, "charge_limit"));
        }
    }

    profile
}
