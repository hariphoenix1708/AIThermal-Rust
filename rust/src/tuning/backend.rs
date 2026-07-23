use crate::hardware::capability::CapabilityNode;
use crate::sysfs;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use thiserror::Error;
use tracing::warn;
use std::sync::atomic::{AtomicU64, Ordering};

static LEGACY_WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Consecutive-failure count per sysfs path. When any path reaches
/// `POISON_THRESHOLD` failed writes in a row it is added to the poisoned
/// set and every subsequent write short-circuits. This kills the
/// "reject every 4s for 50+ retries" loops observed on locked cooling
/// nodes and mis-permissioned charging inputs.
const POISON_THRESHOLD: u32 = 5;

fn failure_counts() -> &'static Mutex<HashMap<PathBuf, u32>> {
    static COUNTS: OnceLock<Mutex<HashMap<PathBuf, u32>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn poisoned_nodes() -> &'static Mutex<HashSet<PathBuf>> {
    static POISONED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    POISONED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_poisoned(path: &Path) -> bool {
    poisoned_nodes()
        .lock()
        .map(|s| s.contains(path))
        .unwrap_or(false)
}

fn record_failure(path: &Path) {
    if let Ok(mut counts) = failure_counts().lock() {
        let entry = counts.entry(path.to_path_buf()).or_insert(0);
        *entry = entry.saturating_add(1);
        if *entry == POISON_THRESHOLD {
            tracing::warn!(target: "thermal",
                "Sysfs node {} rejected {} writes in a row, marking unsupported for the rest of this daemon run",
                path.display(),
                POISON_THRESHOLD
            );
            if let Ok(mut poisoned) = poisoned_nodes().lock() {
                poisoned.insert(path.to_path_buf());
            }
        }
    }
}

fn record_success(path: &Path) {
    if let Ok(mut counts) = failure_counts().lock() {
        counts.remove(path);
    }
}

#[derive(Error, Debug)]
pub enum BackendError {
    #[error("Capability missing or invalid: {0}")]
    InvalidCapability(String),
    #[error("Value not supported by capability: {0}")]
    UnsupportedValue(String),
    #[error("Sysfs error: {0}")]
    Sysfs(#[from] sysfs::SysfsError),
    #[error("Node poisoned after repeated failures: {0}")]
    Poisoned(String),
}

pub struct TuningBackend;

impl TuningBackend {
    /// Legacy raw write, transitioning to capability-based writes.
    pub fn write_string<P: AsRef<Path>, S: AsRef<str>>(path: P, value: S) {
        let p = path.as_ref();
        if is_poisoned(p) {
            return;
        }
        match sysfs::write_string(p, value.as_ref()) {
            Ok(()) => {
                record_success(p);
                tracing::trace!(target: "thermal", "sysfs write ok: {} = {}", p.display(), value.as_ref());
            },
            Err(e) => {
                LEGACY_WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
                record_failure(p);
                warn!("TuningBackend: legacy write failed {}: {}", p.display(), e);
            }
        }
    }

    pub fn legacy_write_failure_count() -> u64 {
        LEGACY_WRITE_FAILURES.load(Ordering::Relaxed)
    }

    /// Read the current string value of a sysfs node. Used for idempotent
    /// write suppression (skip writing when the target already matches).
    pub fn read_current<P: AsRef<Path>>(path: P) -> Option<String> {
        sysfs::read_string(path.as_ref()).ok().map(|s| s.trim().to_string())
    }

    pub fn try_write_string<P: AsRef<Path>, S: AsRef<str>>(
        path: P,
        value: S,
    ) -> Result<(), BackendError> {
        let p = path.as_ref();
        if is_poisoned(p) {
            return Err(BackendError::Poisoned(p.to_string_lossy().into_owned()));
        }
        match sysfs::write_string(p, value.as_ref()) {
            Ok(()) => {
                record_success(p);
                tracing::trace!(target: "thermal", "sysfs write ok: {} = {}", p.display(), value.as_ref());
                Ok(())
            }
            Err(e) => {
                record_failure(p);
                Err(BackendError::Sysfs(e))
            }
        }
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
            && node.name.contains("governor")
        {
            return Err(BackendError::UnsupportedValue(val_str.to_string()));
        }

        // Idempotent skip: if the node already holds the target, no write.
        if let Some(cur) = Self::read_current(&node.path) {
            if cur == val_str {
                return Ok(());
            }
        }

        let path = Path::new(&node.path);
        if is_poisoned(path) {
            return Err(BackendError::Poisoned(node.path.clone()));
        }
        match sysfs::write_string(&node.path, val_str) {
            Ok(()) => {
                record_success(path);
                tracing::trace!(target: "thermal", "sysfs write ok: {} = {}", path.display(), val_str);
                Ok(())
            }
            Err(e) => {
                record_failure(path);
                Err(BackendError::Sysfs(e))
            }
        }
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
