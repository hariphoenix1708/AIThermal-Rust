use crate::hardware::HardwareProfile;
use crate::tuning::backend::{BackendError, CpusetBackend};

pub struct CpusetManager {
    hardware: HardwareProfile,
}

impl Default for CpusetManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CpusetManager {
    pub fn new() -> Self {
        Self {
            hardware: Default::default(),
        }
    }

    pub fn discover_hardware(&mut self, hw_profile: &HardwareProfile) {
        self.hardware = hw_profile.clone();
    }

    pub fn apply_profile(&self, mode: &str) -> Result<(), BackendError> {
        if self.hardware.cpuset_profile.root_path.is_empty() {
            return Ok(());
        }

        // Apply background and system-background first
        let bg_val = match mode {
            "powersave" | "emergency_cool" => "0",
            _ => "0-1",
        };

        let sys_bg_val = match mode {
            "powersave" | "emergency_cool" => "0-1",
            _ => "0-2",
        };

        // Apply foreground and top-app
        let fg_val = match mode {
            "powersave" | "emergency_cool" => "0-2",
            _ => "0-6",
        };

        let ta_val = match mode {
            "powersave" | "emergency_cool" => "0-3",
            "performance" => "0-7", // all
            _ => "0-7",
        };

        let restricted_val = match mode {
            "powersave" | "emergency_cool" => "0-3",
            "performance" => "0-6",
            _ => "0-6",
        };

        for node in &self.hardware.cpuset_profile.cpuset_nodes {
            if node.name == "cpuset_top_app" {
                if crate::tuning::backend::TuningBackend::write_capability(node, ta_val).is_ok() {
                    tracing::debug!(target: "cpuset", "Applied cpuset top-app: {} via {}", ta_val, node.path);
                }
            } else if node.name == "cpuset_foreground" {
                if crate::tuning::backend::TuningBackend::write_capability(node, fg_val).is_ok() {
                    tracing::debug!(target: "cpuset", "Applied cpuset foreground: {} via {}", fg_val, node.path);
                }
            } else if node.name == "cpuset_sys_bg" {
                if crate::tuning::backend::TuningBackend::write_capability(node, sys_bg_val).is_ok()
                {
                    tracing::debug!(target: "cpuset", "Applied cpuset sys-bg: {} via {}", sys_bg_val, node.path);
                }
            } else if node.name == "cpuset_background" {
                if crate::tuning::backend::TuningBackend::write_capability(node, bg_val).is_ok() {
                    tracing::debug!(target: "cpuset", "Applied cpuset background: {} via {}", bg_val, node.path);
                }
            } else if node.name == "cpuset_restricted"
                && crate::tuning::backend::TuningBackend::write_capability(node, restricted_val)
                    .is_ok()
            {
                tracing::debug!(target: "cpuset", "Applied cpuset restricted: {} via {}", restricted_val, node.path);
            }
        }

        Ok(())
    }
}

impl CpusetBackend for CpusetManager {
    fn apply_cpuset(&self, mode: &str) -> Result<(), BackendError> {
        self.apply_profile(mode)
    }
}
