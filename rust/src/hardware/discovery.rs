// Intentionally reserved or conditionally compiled across bins

use super::probe::HardwareProbe;
use super::profile::HardwareProfile;
use super::report::write_human_report;
use crate::cache::{load_profile, save_profile};
use anyhow::{Context, Result};
use std::fs;

pub fn discover_or_load(state_dir: &str) -> Result<HardwareProfile> {
    fs::create_dir_all(state_dir).context("Failed to create state directory")?;

    if let Ok(profile) = load_profile(state_dir) {
        tracing::info!(target: "lifecycle", "Successfully loaded hardware profile from cache.");
        write_human_report(&profile, state_dir)?;
        return Ok(profile);
    }

    tracing::info!(target: "lifecycle", "Hardware profile not found or invalid in cache. Running full discovery...");
    let mut profile = HardwareProbe::probe()?;

    if crate::hardware::peridot::matches(&profile) {
        crate::hardware::peridot::apply_peridot_optimizations(&mut profile);
    }

    save_profile(&profile, state_dir)?;
    write_human_report(&profile, state_dir)?;

    Ok(profile)
}

pub fn discover_force_rescan(state_dir: &str) -> Result<HardwareProfile> {
    fs::create_dir_all(state_dir).context("Failed to create state directory")?;

    tracing::info!(target: "lifecycle", "Forcing hardware discovery rescan...");
    let mut profile = HardwareProbe::probe()?;

    if crate::hardware::peridot::matches(&profile) {
        crate::hardware::peridot::apply_peridot_optimizations(&mut profile);
    }

    save_profile(&profile, state_dir)?;
    write_human_report(&profile, state_dir)?;

    Ok(profile)
}
