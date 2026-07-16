use super::profile::HardwareProfile;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn write_human_report(profile: &HardwareProfile, state_dir: &str) -> Result<()> {
    let path = Path::new(state_dir).join("hardware_report.txt");
    let mut report = String::new();

    report.push_str("=== thermalai_rust Hardware Report ===\n\n");
    report.push_str(&format!("Device Identity: {}\n", profile.device_identity));
    report.push_str(&format!("SoC Platform: {}\n", profile.soc_info));
    report.push_str(&format!("MIGT (MI Game Turbo) module present: {}\n", profile.migt_present));
    report.push_str(&format!("GLK (game low kernel) hooks present: {}\n", profile.glk_present));
    report.push_str(&format!(
        "Build Fingerprint: {}\n",
        profile.metadata.build_fingerprint
    ));
    report.push_str(&format!(
        "Kernel Version: {}\n\n",
        profile.metadata.kernel_version
    ));

    report.push_str("--- CPU ---\n");
    report.push_str(&format!("WALT Support: {}\n", profile.cpu_topology.is_walt));
    report.push_str(&format!("EAS Support: {}\n", profile.cpu_topology.is_eas));
    report.push_str(&format!(
        "UClamp Support: {}\n",
        profile.cpu_topology.has_uclamp
    ));
    for cluster in &profile.cpu_topology.clusters {
        report.push_str(&format!("Cluster [{}]: {:?}\n", cluster.name, cluster.cpus));
        report.push_str(&format!("  Policy Path: {}\n", cluster.policy_path));
        report.push_str(&format!("  Available Frequencies: {:?}\n", cluster.available_frequencies));
    }
    report.push('\n');

    report.push_str("--- GPU ---\n");
    report.push_str(&format!("Path: {}\n", profile.gpu_profile.path));
    report.push_str(&format!(
        "Devfreq Path: {}\n",
        profile.gpu_profile.devfreq_path
    ));
    if let Some(pwr) = &profile.gpu_profile.power_level_path {
        report.push_str(&format!("Power Level Path: {}\n", pwr));
        if let Ok(val) = std::fs::read_to_string(pwr) {
            report.push_str(&format!("Current Power Level: {}\n", val.trim()));
        }
    }
    report.push_str(&format!("KGSL: {}\n", profile.gpu_profile.is_kgsl));
    report.push_str(&format!("Bus Split (kgsl): {}\n", profile.gpu_profile.has_bus_split));
    report.push_str(&format!("Force Clk On (kgsl): {}\n", profile.gpu_profile.has_force_clk_on));
    report.push_str(&format!(
        "Governor: {} (Avail: {:?})\n",
        profile.gpu_profile.current_governor, profile.gpu_profile.available_governors
    ));
    report.push_str(&format!(
        "Freq: {} (Avail: {:?})\n",
        profile.gpu_profile.current_frequency, profile.gpu_profile.available_frequencies
    ));
    report.push_str(&format!(
        "Busy/Total: {:?}/{:?}\n",
        profile.gpu_profile.busy_time, profile.gpu_profile.total_time
    ));
    report.push_str(&format!("Max Freq: {}\n\n", profile.gpu_profile.max_freq));

    report.push_str("--- Thermal ---\n");
    report.push_str(&format!(
        "CPU Zone: {:?}\n",
        profile.thermal_profile.cpu_zone
    ));
    report.push_str(&format!(
        "GPU Zone: {:?}\n",
        profile.thermal_profile.gpu_zone
    ));
    report.push_str(&format!(
        "Battery Zone: {:?}\n",
        profile.thermal_profile.battery_zone
    ));
    report.push_str(&format!(
        "Skin Zone: {:?}\n",
        profile.thermal_profile.skin_zone
    ));
    report.push_str(&format!(
        "Total Discovered Zones: {}\n\n",
        profile.thermal_profile.all_zones.len()
    ));

    report.push_str("--- Charging & Battery ---\n");
    report.push_str(&format!("Battery Path: {}\n", profile.battery_profile.path));
    report.push_str(&format!(
        "Charging Path: {}\n",
        profile.charging_profile.path
    ));
    report.push_str(&format!(
        "Fast Charge Capable: {}\n\n",
        profile.charging_profile.is_fast_charge
    ));

    report.push_str("--- Cpuset ---\n");
    report.push_str(&format!(
        "Root Path: {}\n",
        profile.cpuset_profile.root_path
    ));
    report.push_str(&format!(
        "Top-App Path: {:?}\n",
        profile.cpuset_profile.top_app_path
    ));
    report.push_str(&format!(
        "Foreground Path: {:?}\n",
        profile.cpuset_profile.foreground_path
    ));
    report.push_str(&format!(
        "Background Path: {:?}\n",
        profile.cpuset_profile.background_path
    ));
    report.push_str(&format!(
        "System Background Path: {:?}\n",
        profile.cpuset_profile.system_background_path
    ));
    report.push_str(&format!(
        "Cpuset Nodes: {:?}\n\n",
        profile
            .cpuset_profile
            .cpuset_nodes
            .iter()
            .map(|node| (&node.name, &node.path, node.valid, node.writable))
            .collect::<Vec<_>>()
    ));

    report.push_str("--- Network ---\n");
    report.push_str(&format!(
        "Default qdisc: {}\n",
        profile.network_profile.default_qdisc
    ));
    report.push_str(&format!(
        "TCP Congestion: {}\n\n",
        profile.network_profile.tcp_congestion_control
    ));

    report.push_str("--- Memory & Storage ---\n");
    report.push_str(&format!("PSI: {}\n", profile.memory_profile.has_psi));
    report.push_str(&format!("ZRAM: {}\n", profile.memory_profile.has_zram));
    report.push_str(&format!("LMKD: {}\n", profile.memory_profile.has_lmkd));
    report.push_str(&format!("UFS: {}\n", profile.storage_profile.has_ufs));
    report.push_str(&format!(
        "I/O Schedulers: {:?}\n\n",
        profile.storage_profile.io_schedulers
    ));

    report.push_str("--- Services & Kernel ---\n");
    report.push_str(&format!(
        "Thermal HAL: {}\n",
        profile.services_profile.thermal_hal
    ));
    report.push_str(&format!(
        "Power HAL: {}\n",
        profile.services_profile.power_hal
    ));
    report.push_str(&format!(
        "Vendor Thermal Engine: {}\n",
        profile.services_profile.vendor_thermal_engine
    ));
    report.push_str(&format!(
        "Kernel Features: {:?}\n\n",
        profile.kernel_profile.features
    ));

    if !profile.dcvs_profiles.is_empty() {
        report.push_str("--- DCVS ---\n");
        for dcvs in &profile.dcvs_profiles {
            report.push_str(&format!("Component: {}\n", dcvs.component));
            report.push_str(&format!("  Path: {}\n", dcvs.path));
            report.push_str(&format!("  Available Frequencies: {:?}\n", dcvs.available_frequencies));
            report.push_str(&format!("  hw_max_freq writable: {}\n", dcvs.hw_max_freq_node.is_some()));
            report.push_str(&format!("  hw_min_freq writable: {}\n", dcvs.hw_min_freq_node.is_some()));
        }
        report.push('\n');
    }

    report.push_str("--- Compatibility Summary ---\n");
    for (key, val) in &profile.compatibility_report {
        report.push_str(&format!("{}: {}\n", key, val));
    }

    let is_fully_compatible = !profile.cpu_topology.clusters.is_empty()
        && !profile.gpu_profile.path.is_empty()
        && !profile.thermal_profile.all_zones.is_empty();

    if is_fully_compatible {
        report.push_str("OVERALL: PASS\n");
    } else {
        report.push_str("OVERALL: PARTIAL / MISSING\n");
    }

    fs::write(&path, report).context("Failed to write hardware report")?;
    Ok(())
}
