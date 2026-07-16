use crate::hardware::capability::CapabilityNode;
use crate::sysfs;
use std::path::Path;
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug)]
pub enum BackendError {
    #[error("Capability missing or invalid: {0}")]
    InvalidCapability(String),
    #[error("Value not supported by capability: {0}")]
    UnsupportedValue(String),
    #[error("Sysfs error: {0}")]
    Sysfs(#[from] sysfs::SysfsError),
}

pub struct TuningBackend;

impl TuningBackend {
    /// Legacy raw write, transitioning to capability-based writes.
    pub fn write_string<P: AsRef<Path>, S: AsRef<str>>(path: P, value: S) {
        let p = path.as_ref();
        if let Err(e) = sysfs::write_string(p, value.as_ref()) {
            warn!("TuningBackend: legacy write failed {}: {}", p.display(), e);
        }
    }

    pub fn try_write_string<P: AsRef<Path>, S: AsRef<str>>(
        path: P,
        value: S,
    ) -> Result<(), BackendError> {
        let p = path.as_ref();
        sysfs::write_string(p, value.as_ref()).map_err(BackendError::Sysfs)
    }

    /// Write via CapabilityNode with validation
    pub fn write_capability<S: AsRef<str>>(
        node: &CapabilityNode,
        value: S,
    ) -> Result<(), BackendError> {
        if !node.valid || !node.writable {
            return Err(BackendError::InvalidCapability(node.name.clone()));
        }

        let val_str = value.as_ref();
        if !node.supported_values.is_empty()
            && !node.supported_values.contains(&val_str.to_string())
        {
            if node.name.contains("governor")
                && !node.supported_values.contains(&val_str.to_string())
            {
                return Err(BackendError::UnsupportedValue(val_str.to_string()));
            }
        }

        sysfs::write_string(&node.path, val_str)?;
        Ok(())
    }
}

pub trait CpuBackend {
    fn apply_cpu_governor(&self, governor: &str) -> Result<(), BackendError>;
}

pub trait GpuBackend {
    fn apply_gpu_governor(&self, governor: &str) -> Result<(), BackendError>;
    fn apply_gpu_power_level(&self, level: u32) -> Result<(), BackendError>;
}

pub trait ThermalBackend {
    fn read_zone(&self, zone_type: &str) -> Option<i32>;
}

pub trait ChargingBackend {
    fn set_charge_limit(&self, ma: u32) -> Result<(), BackendError>;
}

pub trait StorageBackend {
    fn apply_scheduler(&self, sched: &str) -> Result<(), BackendError>;
}

pub trait CpusetBackend {
    fn apply_cpuset(&self, mode: &str) -> Result<(), BackendError>;
}

pub trait VmBackend {
    fn drop_caches(&self) -> Result<(), BackendError>;
}

pub trait NetworkBackend {
    fn apply_congestion_control(&self, algo: &str) -> Result<(), BackendError>;
}

pub trait DisplayBackend {
    fn read_brightness(&self) -> i32;
}

pub trait TouchBackend {
    fn apply_touch_tweaks(&self, gaming: bool) -> Result<(), BackendError>;
}
