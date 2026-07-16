// Intentionally reserved or conditionally compiled across bins

use crate::hardware::android_prop::get_any_property;
use crate::hardware::profile::{CacheMetadata, HardwareProfile};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const HARDWARE_PROFILE_SCHEMA_VERSION: u32 = 4;

pub fn save_profile(profile: &HardwareProfile, state_dir: &str) -> Result<()> {
    fs::create_dir_all(state_dir).context("Failed to create state dir")?;
    let path = Path::new(state_dir).join("hardware_profile.json");
    let json = serde_json::to_string_pretty(profile)?;

    let temp_path = Path::new(state_dir).join("hardware_profile.json.tmp");
    fs::write(&temp_path, json).context("Failed to write hardware profile temp file")?;
    fs::rename(&temp_path, &path).context("Failed to commit hardware profile cache")?;

    Ok(())
}

pub fn load_profile(state_dir: &str) -> Result<HardwareProfile> {
    fs::create_dir_all(state_dir).context("Failed to create state dir")?;
    let path = Path::new(state_dir).join("hardware_profile.json");
    let json = fs::read_to_string(&path).context("Failed to read hardware profile cache")?;
    let profile: HardwareProfile = serde_json::from_str(&json)?;

    validate_profile_cache(&profile)?;

    Ok(profile)
}

fn validate_profile_cache(profile: &HardwareProfile) -> Result<()> {
    if profile.metadata.schema_version != HARDWARE_PROFILE_SCHEMA_VERSION {
        anyhow::bail!("Hardware profile cache invalidated: schema version mismatch");
    }

    let current = build_cache_metadata();

    if profile.metadata.build_fingerprint != current.build_fingerprint {
        anyhow::bail!("Hardware profile cache invalidated: build fingerprint mismatch");
    }
    if profile.metadata.vendor_fingerprint != current.vendor_fingerprint {
        anyhow::bail!("Hardware profile cache invalidated: vendor fingerprint mismatch");
    }
    if profile.metadata.android_version != current.android_version {
        anyhow::bail!("Hardware profile cache invalidated: Android version mismatch");
    }
    if profile.metadata.kernel_version != current.kernel_version {
        anyhow::bail!("Hardware profile cache invalidated: kernel version mismatch");
    }
    if profile.metadata.product_device != current.product_device {
        anyhow::bail!("Hardware profile cache invalidated: product device mismatch");
    }
    if profile.metadata.boot_hardware != current.boot_hardware {
        anyhow::bail!("Hardware profile cache invalidated: boot hardware mismatch");
    }
    if profile.metadata.device_identity != current.device_identity {
        anyhow::bail!("Hardware profile cache invalidated: device identity mismatch");
    }
    if profile.metadata.board_platform != current.board_platform {
        anyhow::bail!("Hardware profile cache invalidated: board platform mismatch");
    }
    if profile.metadata.hardware != current.hardware {
        anyhow::bail!("Hardware profile cache invalidated: hardware mismatch");
    }

    validate_required_paths(profile)?;
    validate_storage_devices(profile)?;
    validate_thermal_map(profile)?;

    Ok(())
}

fn validate_required_paths(profile: &HardwareProfile) -> Result<()> {
    for cluster in &profile.cpu_topology.clusters {
        if !cluster.policy_path.is_empty() && !Path::new(&cluster.policy_path).is_dir() {
            anyhow::bail!(
                "Hardware profile cache invalidated: CPU policy disappeared: {}",
                cluster.policy_path
            );
        }
        for node in [
            &cluster.governor_node,
            &cluster.min_freq_node,
            &cluster.max_freq_node,
        ] {
            if !node.path.is_empty() && !Path::new(&node.path).is_file() {
                anyhow::bail!(
                    "Hardware profile cache invalidated: CPU node disappeared: {}",
                    node.path
                );
            }
        }
    }

    for node in [
        &profile.gpu_profile.devfreq_governor_node,
        &profile.gpu_profile.devfreq_freq_node,
        &profile.gpu_profile.governor_node,
        &profile.gpu_profile.freq_node,
    ] {
        if !node.path.is_empty() && !Path::new(&node.path).is_file() {
            anyhow::bail!(
                "Hardware profile cache invalidated: GPU node disappeared: {}",
                node.path
            );
        }
    }

    if !profile.battery_profile.path.is_empty()
        && !Path::new(&profile.battery_profile.path).is_dir()
    {
        anyhow::bail!(
            "Hardware profile cache invalidated: battery path disappeared: {}",
            profile.battery_profile.path
        );
    }

    Ok(())
}

fn unsupported_block_device(dev: &str) -> bool {
    dev.starts_with("dm-") || dev.starts_with("loop") || dev.starts_with("zram")
}

fn validate_storage_devices(profile: &HardwareProfile) -> Result<()> {
    for dev in &profile.storage_profile.block_devices {
        if unsupported_block_device(dev) {
            anyhow::bail!(
                "Hardware profile cache invalidated: unsupported block device cached: {}",
                dev
            );
        }
    }
    Ok(())
}

fn validate_thermal_map(profile: &HardwareProfile) -> Result<()> {
    for (zone_type, path) in &profile.thermal_profile.all_zones {
        if !path.starts_with("/sys/class/thermal/thermal_zone") || !path.ends_with("/temp") {
            anyhow::bail!(
                "Hardware profile cache invalidated: malformed thermal mapping {} -> {}",
                zone_type,
                path
            );
        }
        if !Path::new(path).is_file() {
            anyhow::bail!(
                "Hardware profile cache invalidated: thermal temp node disappeared: {}",
                path
            );
        }
    }
    Ok(())
}

pub fn build_cache_metadata() -> CacheMetadata {
    CacheMetadata {
        schema_version: HARDWARE_PROFILE_SCHEMA_VERSION,
        timestamp: match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(_) => 0,
        },
        product_device: get_any_property(&["ro.product.device"], "Unknown"),
        boot_hardware: get_any_property(&["ro.boot.hardware"], "Unknown"),
        device_identity: get_any_property(&["ro.product.model", "ro.product.device"], "Unknown"),
        board_platform: get_any_property(&["ro.board.platform"], "Unknown"),
        hardware: get_any_property(&["ro.hardware"], "Unknown"),
        android_version: get_any_property(&["ro.build.version.release"], "Unknown"),
        build_fingerprint: get_any_property(&["ro.build.fingerprint"], "Unknown"),
        kernel_version: fs::read_to_string("/proc/version")
            .unwrap_or_default()
            .trim()
            .to_string(),
        vendor_fingerprint: get_any_property(&["ro.vendor.build.fingerprint"], "Unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::profile::HardwareProfile;

    fn profile_with_current_metadata() -> HardwareProfile {
        HardwareProfile {
            metadata: build_cache_metadata(),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_old_schema_cache() {
        let mut profile = profile_with_current_metadata();
        profile.metadata.schema_version = HARDWARE_PROFILE_SCHEMA_VERSION - 1;
        assert!(validate_profile_cache(&profile).is_err());
    }

    #[test]
    fn rejects_unsupported_cached_block_devices() {
        let mut profile = profile_with_current_metadata();
        profile
            .storage_profile
            .block_devices
            .push("dm-0".to_string());
        assert!(validate_profile_cache(&profile).is_err());
    }

    #[test]
    fn rejects_reversed_thermal_cache_map() {
        let mut profile = profile_with_current_metadata();
        profile
            .thermal_profile
            .all_zones
            .insert("thermal_zone60".to_string(), "battery".to_string());
        assert!(validate_profile_cache(&profile).is_err());
    }
}
