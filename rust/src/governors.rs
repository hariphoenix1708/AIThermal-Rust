use crate::hardware::HardwareProfile;
use crate::tuning::backend::{BackendError, CpuBackend, GpuBackend};
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct GovernorManager {
    hardware: HardwareProfile,
}

impl Default for GovernorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GovernorManager {
    pub fn max_freq(available: &[u64]) -> Option<u64> {
        available.iter().copied().max()
    }

    pub fn min_freq(available: &[u64]) -> Option<u64> {
        available.iter().copied().min()
    }

    pub fn mid_freq(available: &[u64]) -> Option<u64> {
        if available.is_empty() {
            return None;
        }
        let mut sorted: Vec<u64> = available.to_vec();
        sorted.sort_unstable();
        Some(sorted[sorted.len() / 2])
    }

    pub fn new() -> Self {
        Self {
            hardware: Default::default(),
        }
    }

    pub fn discover_hardware(&mut self, hw_profile: &HardwareProfile) -> Result<()> {
        self.hardware = hw_profile.clone();
        Ok(())
    }

    pub fn apply_cpu_governor(&self, governor: &str) -> Result<(), BackendError> {
        for cluster in &self.hardware.cpu_topology.clusters {
            if cluster.governor_node.path.is_empty() || !cluster.governor_node.valid {
                tracing::warn!(
                    "CPU governor node unavailable for cluster {}, skipping",
                    cluster.name
                );
                continue;
            }
            if !cluster.available_governors.contains(&governor.to_string()) {
                tracing::warn!(
                    "Governor {} not supported on cluster {}",
                    governor,
                    cluster.name
                );
                continue;
            }
            crate::tuning::backend::TuningBackend::write_capability(
                &cluster.governor_node,
                governor,
            )?;
            tracing::debug!(target: "governor", "Applied CPU governor: {} to cluster {} via {}", governor, cluster.name, cluster.governor_node.path);
        }
        Ok(())
    }

    pub fn apply_gpu_governor(&self, governor: &str) -> Result<(), BackendError> {
        if !self
            .hardware
            .gpu_profile
            .available_governors
            .contains(&governor.to_string())
        {
            tracing::warn!("GPU Governor {} not supported, skipping", governor);
            return Ok(());
        }

        if self.hardware.gpu_profile.has_devfreq {
            crate::tuning::backend::TuningBackend::write_capability(
                &self.hardware.gpu_profile.devfreq_governor_node,
                governor,
            )?;
            tracing::debug!(target: "governor", "Applied GPU devfreq governor: {} via {}", governor, self.hardware.gpu_profile.devfreq_governor_node.path);
        } else if !self.hardware.gpu_profile.path.is_empty() {
            crate::tuning::backend::TuningBackend::write_capability(
                &self.hardware.gpu_profile.governor_node,
                governor,
            )?;
            tracing::debug!(target: "governor", "Applied GPU governor: {} via {}", governor, self.hardware.gpu_profile.governor_node.path);
        }
        Ok(())
    }

    pub fn apply_gpu_power_level(&self, level: u32) -> Result<(), BackendError> {
        if let Some(ref path) = self.hardware.gpu_profile.power_level_path {
            let Some(min_level) = self.hardware.gpu_profile.min_power_level else {
                tracing::debug!("Skipping GPU power level: missing discovered min bound");
                return Ok(());
            };
            let Some(max_level) = self.hardware.gpu_profile.max_power_level else {
                tracing::debug!("Skipping GPU power level: missing discovered max bound");
                return Ok(());
            };
            let low = min_level.min(max_level);
            let high = min_level.max(max_level);
            if !(low..=high).contains(&level) {
                tracing::warn!(
                    "Skipping GPU power level {} outside discovered bounds {}..={}",
                    level,
                    low,
                    high
                );
                return Ok(());
            }
            crate::tuning::backend::TuningBackend::write_string(path, level.to_string());
            tracing::debug!(target: "governor", "Applied GPU power level: {} via {}", level, path);
        } else {
            tracing::debug!("Skipping GPU power level: no writable power-level path discovered");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_discovery() {
        let mut profile = HardwareProfile::default();
        profile
            .cpu_topology
            .clusters
            .push(crate::hardware::profile::CpuCluster {
                name: "test_cluster".to_string(),
                policy_path: "/test/policy0".to_string(),
                ..Default::default()
            });

        profile.gpu_profile.path = "/test/gpu".to_string();
        profile.gpu_profile.is_kgsl = true;

        let mut manager = GovernorManager::new();
        manager.discover_hardware(&profile).unwrap();
        assert_eq!(manager.hardware.cpu_topology.clusters.len(), 1);
        assert_eq!(
            manager.hardware.cpu_topology.clusters[0].policy_path,
            "/test/policy0"
        );
        assert!(!manager.hardware.gpu_profile.path.is_empty());
    }
}
impl CpuBackend for GovernorManager {
    fn apply_cpu_governor(&self, governor: &str) -> Result<(), BackendError> {
        self.apply_cpu_governor(governor)
    }
}

impl GpuBackend for GovernorManager {
    fn apply_gpu_governor(&self, governor: &str) -> Result<(), BackendError> {
        self.apply_gpu_governor(governor)
    }

    fn apply_gpu_power_level(&self, level: u32) -> Result<(), BackendError> {
        self.apply_gpu_power_level(level)
    }
}
