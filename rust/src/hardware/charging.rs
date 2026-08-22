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

    // Xiaomi/QCOM voter framework (peridot / SM8635 and family).
    // These control CHARGE current in ways the generic power_supply
    // nodes cannot on Xiaomi HyperOS kernels.
    let qcom_root = "/sys/class/qcom-battery";
    if Path::new(qcom_root).exists() {
        profile.qcom_battery_root = Some(qcom_root.to_string());
        for name in [
            "restrict_cur",
            "input_suspend",
            "night_charging",
        ] {
            let p = format!("{}/{}", qcom_root, name);
            if !Path::new(&p).exists() {
                continue;
            }
            let writable = match std::fs::read_to_string(&p) {
                Ok(cur) => crate::sysfs::write_string(&p, cur.trim()).is_ok(),
                Err(_) => false,
            };
            if writable {
                profile.voter_nodes.push(p);
            } else if name == "restrict_cur" {
                // Some Xiaomi kernels reject idempotent writes (read→write
                // same value) on restrict_cur to reduce SPMI bus traffic,
                // falsely marking it read-only. Writing a DIFFERENT value
                // ("0" = no cap) usually succeeds. Test this fallback and
                // add to voter_nodes if it works so apply_voters_for_mode()
                // can maintain it in MaxSpeed/Urgent.
                if crate::sysfs::write_string(&p, "0").is_ok() {
                    tracing::info!(
                        target: "charging",
                        "QCOM voter {} accepted write of0 (idempotent test failed, non-idempotent succeeded); adding to voters",
                        p
                    );
                    profile.voter_nodes.push(p);
                } else {
                    tracing::info!(
                        target: "charging",
                        "QCOM voter {} exists but is truly read-only; skipping", p
                    );
                }
            } else {
                tracing::info!(
                    target: "charging",
                    "QCOM voter {} exists but is read-only; skipping", p
                );
            }
        }
    }

    // Legacy fallback for older Xiaomi kernels that expose the
    // same voters under /sys/class/power_supply/battery/*
    for name in ["input_suspend", "night_charging"] {
        let p = format!("/sys/class/power_supply/battery/{}", name);
        if Path::new(&p).exists() && !profile.voter_nodes.iter().any(|x| x.ends_with(name))
            && let Ok(cur) = std::fs::read_to_string(&p)
                && crate::sysfs::write_string(&p, cur.trim()).is_ok() {
                    profile.voter_nodes.push(p);
                }
    }

    // AOSP paths (Android 14+): /sys/class/power_supply/battery/cycle_count
    // HyperOS/QC also uses:      /sys/class/power_supply/bms/cycle_count
    for path in [
        "/sys/class/power_supply/battery/cycle_count",
        "/sys/class/power_supply/bms/cycle_count",
    ] {
        if let Ok(s) = std::fs::read_to_string(path)
            && let Ok(n) = s.trim().parse::<u64>() {
                profile.cycle_count = Some(n);
                profile.cycle_count_path = Some(path.to_string());
                break;
            }
    }

    // Phase 1: keep only nodes that exist AND accept a probe write of a
    // small, safe value (500 mA in microamps = "500000"). Nodes that reject
    // EINVAL on this probe are dropped up front so we never poison them at
    // runtime.
    let ordered = [
        "/sys/class/power_supply/battery/constant_charge_current_max",
        "/sys/class/power_supply/bms/constant_charge_current_max",
        "/sys/class/power_supply/main/constant_charge_current_max",
        "/sys/class/power_supply/battery/current_max",
        "/sys/class/power_supply/main/current_max",
        // usb/dc/ac input_current_limit are last-resort only
        "/sys/class/power_supply/usb/current_max",
        "/sys/class/power_supply/dc/current_max",
        "/sys/class/power_supply/ac/current_max",
        "/sys/class/power_supply/usb/input_current_limit",
        "/sys/class/power_supply/dc/input_current_limit",
        "/sys/class/power_supply/ac/input_current_limit",
    ];
    for node in ordered {
        if !Path::new(node).exists() {
            continue;
        }
        // Try a probe write of the current value (read-then-write same value)
        // so we don’t perturb hardware but do verify EINVAL doesn’t fire.
        let probe_ok = match std::fs::read_to_string(node) {
            Ok(current) => crate::sysfs::write_string(node, current.trim()).is_ok(),
            Err(_) => false,
        };
        if probe_ok {
            profile.current_limit_nodes.push(node.to_string());
            profile
                .capability_nodes
                .push(CapabilityNode::new(node, "charge_limit"));
        } else {
            tracing::info!(
                target: "charging",
                "Charge-limit node {} exists but rejected probe write; skipping",
                node
            );
        }
    }

    profile
}
