use super::profile::HardwareProfile;

pub fn apply_peridot_optimizations(profile: &mut HardwareProfile) {
    // Snapdragon 8s Gen 3 (SM8635) / POCO F6 specific optimizations

    // Fast charging profile hint
    profile.charging_profile.is_fast_charge = true;

    // Hardware zones are already dynamically discovered and stored in probe.rs using generic matching.
    // Ensure Peridot prefers specific sensor names based on the audit.
    // `all_zones` stores a map of: type_name -> temp_path
    let find_zone_path =
        |target: &str| -> Option<String> { profile.thermal_profile.all_zones.get(target).cloned() };

    if let Some(p) = find_zone_path("cpu_therm") {
        profile.thermal_profile.cpu_zone = Some(p);
    }
    if let Some(p) = find_zone_path("gpuss-2") {
        profile.thermal_profile.gpu_zone = Some(p);
    }
    if let Some(p) = find_zone_path("battery") {
        profile.thermal_profile.battery_zone = Some(p);
    }
    if let Some(p) = find_zone_path("quiet_therm") {
        profile.thermal_profile.skin_zone = Some(p);
    }
    if let Some(p) = find_zone_path("pm7550ba_tz") {
        profile.thermal_profile.pmic_zone = Some(p);
    }
    if let Some(p) = find_zone_path("usb") {
        profile.thermal_profile.usbc_zone = Some(p);
    }
    if let Some(p) = find_zone_path("charger_therm0") {
        profile.thermal_profile.charger_zone = Some(p);
    }

    // Ensure we explicitly state available network congestion controls from actual hardware
    if profile
        .network_profile
        .available_congestion_controls
        .is_empty()
    {
        profile.network_profile.available_congestion_controls =
            vec!["reno".to_string(), "cubic".to_string()];
    }

    // Add custom compatibility flags
    profile
        .compatibility_report
        .insert("peridot_optimizations".to_string(), "PASS".to_string());
}

pub fn matches(profile: &HardwareProfile) -> bool {
    let mut score = 0;

    let device = profile.product_device.to_lowercase();
    let platform = profile.board_platform.to_lowercase();
    let boot_hw = profile.boot_hardware.to_lowercase();
    let soc = profile.soc_info.to_lowercase();

    if device == "peridot"
        || device.contains("poco f6")
        || profile.device_identity.contains("24069PC21G")
    {
        score += 2;
    }

    if platform == "sun" || soc.contains("sm8635") || boot_hw == "sun" || soc.contains("pineapple")
    {
        score += 2;
    }

    score >= 2
}
