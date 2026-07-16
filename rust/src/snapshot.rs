use crate::sysfs;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub values: HashMap<String, String>,
}

pub struct SnapshotManager {
    snapshot_file: PathBuf,
    temp_file: PathBuf,
    hardware: crate::hardware::HardwareProfile,
}

impl SnapshotManager {
    pub fn new(state_dir: &str, hardware: crate::hardware::HardwareProfile) -> Self {
        let base = PathBuf::from(state_dir);
        Self {
            snapshot_file: base.join("snapshot.json"),
            temp_file: base.join("snapshot.json.tmp"),
            hardware,
        }
    }

    pub fn take_snapshot(&self, dynamic_paths: Vec<String>) -> Result<()> {
        if self.snapshot_file.exists() {
            info!("Snapshot already exists, updating...");
        }

        let mut snapshot = Snapshot::default();

        // Base critical paths

        let mut paths_to_save = vec![];
        if !self.hardware.gpu_profile.path.is_empty()
            && !self.hardware.gpu_profile.devfreq_path.is_empty()
        {
            paths_to_save.push(format!(
                "{}/governor",
                self.hardware.gpu_profile.devfreq_path
            ));
        }
        if !self.hardware.gpu_profile.path.is_empty() {
            paths_to_save.push(format!("{}/max_pwrlevel", self.hardware.gpu_profile.path));
            paths_to_save.push(format!("{}/min_pwrlevel", self.hardware.gpu_profile.path));
        }

        // Add some generic VM tunables, could use hardware profile if mapped, but currently they are static strings
        paths_to_save.extend(vec![
            "/proc/sys/vm/swappiness".to_string(),
            "/proc/sys/vm/vfs_cache_pressure".to_string(),
            "/proc/sys/vm/dirty_ratio".to_string(),
            "/proc/sys/vm/dirty_background_ratio".to_string(),
            "/proc/sys/net/ipv4/tcp_keepalive_time".to_string(),
            "/proc/sys/net/ipv4/tcp_syn_retries".to_string(),
            "/proc/sys/net/ipv4/tcp_synack_retries".to_string(),
            "/proc/sys/net/ipv4/tcp_window_scaling".to_string(),
            "/proc/sys/net/ipv4/tcp_timestamps".to_string(),
        ]);

        let root = &self.hardware.cpuset_profile.root_path;
        if !root.is_empty() {
            paths_to_save.push(format!("{}/background/cpus", root));
            paths_to_save.push(format!("{}/system-background/cpus", root));
            paths_to_save.push(format!("{}/top-app/cpus", root));
        }

        paths_to_save.extend(dynamic_paths);

        for path in paths_to_save {
            if Self::is_unsupported_block_path(&path) {
                tracing::debug!("Skipping unsupported block path during snapshot: {}", path);
                continue;
            }
            #[allow(clippy::collapsible_if)]
            if sysfs::exists(&path) {
                if let Ok(val) = sysfs::read_string(&path) {
                    snapshot.values.insert(path, val);
                }
            }
        }

        let content = serde_json::to_string_pretty(&snapshot)?;

        // Atomic write
        fs::write(&self.temp_file, content).context("Failed to write snapshot temp")?;
        fs::rename(&self.temp_file, &self.snapshot_file).context("Failed to commit snapshot")?;

        info!(
            "Taken system snapshot with {} entries",
            snapshot.values.len()
        );

        Ok(())
    }

    pub fn load_snapshot(&self) -> Option<Snapshot> {
        if self.snapshot_file.exists()
            && let Ok(content) = fs::read_to_string(&self.snapshot_file)
            && let Ok(snapshot) = serde_json::from_str::<Snapshot>(&content)
        {
            return Some(snapshot);
        }
        None
    }

    pub fn verify_policy(&self, target_policy: &str) -> bool {
        // Simple verification that we are permitted to change nodes
        //
        if target_policy == "EmergencyCool" {
            // Emergency always verified
            return true;
        }

        if let Some(cluster) = self
            .hardware
            .cpu_topology
            .clusters
            .iter()
            .find(|c| c.governor_node.valid)
        {
            return cluster.governor_node.writable;
        }
        true
    }

    pub fn restore_snapshot(&self) {
        if !self.snapshot_file.exists() {
            warn!("No snapshot found to restore.");
            return;
        }

        #[allow(clippy::collapsible_if)]
        if let Ok(content) = fs::read_to_string(&self.snapshot_file) {
            if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&content) {
                info!("Restoring {} snapshot entries...", snapshot.values.len());
                for (path, val) in snapshot.values {
                    if Self::is_unsupported_block_path(&path) {
                        tracing::debug!("Skipping unsupported block path during restore: {}", path);
                        continue;
                    }
                    crate::tuning::backend::TuningBackend::write_string(&path, &val);
                }
            } else {
                warn!("Snapshot file is corrupted or unreadable. Ignoring.");
            }
        }

        if let Err(e) = fs::remove_file(&self.snapshot_file) {
            tracing::debug!("Failed to remove snapshot file: {}", e);
        }
    }

    fn is_unsupported_block_path(path: &str) -> bool {
        path.contains("/sys/block/dm-")
            || path.contains("/sys/block/loop")
            || path.contains("/sys/block/zram")
    }
}
