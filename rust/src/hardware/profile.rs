use super::capability::CapabilityNode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HardwareProfile {
    pub metadata: CacheMetadata,
    pub product_device: String,
    pub boot_hardware: String,
    pub device_identity: String,
    pub soc_info: String,
    pub board_platform: String,
    pub hardware: String,
    pub migt_present: bool,
    pub glk_present: bool,
    pub cpu_topology: CpuTopology,
    pub gpu_profile: GpuProfile,
    pub thermal_profile: ThermalProfile,
    pub battery_profile: BatteryProfile,
    pub charging_profile: ChargingProfile,
    pub cpuset_profile: CpusetProfile,
    pub network_profile: NetworkProfile,
    pub memory_profile: MemoryProfile,
    pub storage_profile: StorageProfile,
    pub scheduler_profile: SchedulerProfile,
    pub services_profile: ServiceProfile,
    pub display_profile: DisplayProfile,
    pub kernel_profile: KernelCapabilityProfile,
    pub dcvs_profiles: Vec<DcvsProfile>,
    pub compatibility_report: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DisplayProfile {
    pub touch_nodes: Vec<String>,
    pub touch_controller_name: Option<String>,
    pub brightness_path: Option<String>,
    pub max_brightness_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkProfile {
    pub default_qdisc: String,
    pub tcp_congestion_control: String,
    pub available_congestion_controls: Vec<String>,
    pub ecn_enabled: Option<bool>,
    pub fast_open: Option<String>,
    pub mtu_probing: Option<u32>,
    pub tcp_keepalive_time: Option<u32>,
    pub tcp_syn_retries: Option<u32>,
    pub tcp_synack_retries: Option<u32>,
    pub tcp_window_scaling: Option<u32>,
    pub tcp_timestamps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryProfile {
    pub has_psi: bool,
    pub has_zram: bool,
    pub has_lmkd: bool,
    pub mem_total_kb: Option<u64>,
    pub mem_free_kb: Option<u64>,
    pub mem_available_kb: Option<u64>,
    pub swap_total_kb: Option<u64>,
    pub swap_free_kb: Option<u64>,
    pub memory_pressure_avg10: Option<f32>,
    pub memory_pressure_avg60: Option<f32>,
    pub memory_pressure_avg300: Option<f32>,
    pub vm_parameters: std::collections::HashMap<String, String>,
    pub zram_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageProfile {
    pub has_ufs: bool,
    pub block_devices: Vec<String>,
    pub io_schedulers: std::collections::HashMap<String, String>,
    pub current_schedulers: std::collections::HashMap<String, String>,
    pub available_schedulers: std::collections::HashMap<String, Vec<String>>,
    pub read_ahead_kb: std::collections::HashMap<String, u64>,
    pub nr_requests: std::collections::HashMap<String, u64>,
    pub rotational: std::collections::HashMap<String, bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchedulerProfile {
    pub has_schedtune: bool,
    pub has_uclamp: bool,
    pub has_schedutil: bool,
    pub has_walt: bool,
    pub has_eas: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceProfile {
    pub thermal_hal: bool,
    pub power_hal: bool,
    pub health_hal: bool,
    pub perf_service: bool,
    pub vendor_thermal_engine: bool,
    pub vendor_performance_daemon: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KernelCapabilityProfile {
    pub features: Vec<String>,
    pub selinux_enforcing: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CacheMetadata {
    pub product_device: String,
    pub boot_hardware: String,
    pub device_identity: String,
    pub board_platform: String,
    pub hardware: String,
    pub schema_version: u32,
    pub timestamp: u64,
    pub android_version: String,
    pub build_fingerprint: String,
    pub kernel_version: String,
    pub vendor_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CpuTopology {
    pub is_walt: bool,
    pub is_eas: bool,
    pub has_uclamp: bool,
    pub clusters: Vec<CpuCluster>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CpuCluster {
    pub name: String,
    pub min_freq: u64,
    pub max_freq: u64,
    pub cpus: Vec<u32>,
    pub policy_path: String,
    pub policy_node: CapabilityNode,
    pub current_governor: String,
    pub governor_node: CapabilityNode,
    pub available_governors: Vec<String>,
    pub current_frequency: u64,
    pub freq_node: CapabilityNode,
    pub min_freq_node: CapabilityNode,
    pub max_freq_node: CapabilityNode,
    pub available_frequencies: Vec<u64>,
    pub cpuinfo_min_freq: u64,
    pub cpuinfo_max_freq: u64,
    pub related_cpus: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuProfile {
    pub path: String,
    pub max_freq: u64,
    pub has_devfreq: bool,
    pub is_kgsl: bool,
    pub devfreq_path: String,
    pub devfreq_governor_node: CapabilityNode,
    pub devfreq_freq_node: CapabilityNode,
    pub current_governor: String,
    pub governor_node: CapabilityNode,
    pub available_governors: Vec<String>,
    pub current_frequency: u64,
    pub freq_node: CapabilityNode,
    pub available_frequencies: Vec<u64>,
    pub busy_time: Option<u64>,
    pub total_time: Option<u64>,
    pub power_level_path: Option<String>,
    pub current_power_level: Option<u32>,
    pub min_power_level: Option<u32>,
    pub max_power_level: Option<u32>,
    pub has_bus_split: bool,
    pub has_force_clk_on: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DcvsProfile {
    pub component: String,
    pub path: String,
    pub hw_max_freq_node: Option<String>,
    pub hw_min_freq_node: Option<String>,
    pub available_frequencies: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CoolingDeviceProfile {
    pub name: String,
    pub device_type: String,
    pub sysfs_path: String,
    pub state_node: CapabilityNode,
    pub current_state: Option<u32>,
    pub max_state: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThermalProfile {
    pub cpu_zone: Option<String>,
    pub gpu_zone: Option<String>,
    pub battery_zone: Option<String>,
    pub skin_zone: Option<String>,
    pub pmic_zone: Option<String>,
    pub usbc_zone: Option<String>,
    pub charger_zone: Option<String>,
    pub all_zones: std::collections::HashMap<String, String>,
    pub cooling_devices: Vec<CoolingDeviceProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BatteryProfile {
    pub path: String,
    pub capability_nodes: Vec<CapabilityNode>,
    pub design_capacity_mah: Option<u64>,
    pub charge_full_design_mah: Option<u64>,
    pub charge_full_mah: Option<u64>,
    pub charge_counter_uah: Option<i64>,
    pub voltage_now_uv: Option<u64>,
    pub current_now_ua: Option<i64>,
    pub temperature_tenths_c: Option<i32>,
    pub health: String,
    pub technology: String,
    pub cycle_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChargingProfile {
    pub path: String,
    pub is_fast_charge: bool,
    pub capability_nodes: Vec<CapabilityNode>,
    pub current_limit_nodes: Vec<String>,
    pub input_current_limit_nodes: Vec<String>,
    pub charge_enable_nodes: Vec<String>,
    pub fast_charge_nodes: Vec<String>,
    pub qcom_battery_root: Option<String>,
    pub voter_nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CpusetProfile {
    pub root_path: String,
    pub cpuset_nodes: Vec<CapabilityNode>,
    pub top_app_path: Option<String>,
    pub background_path: Option<String>,
    pub foreground_path: Option<String>,
    pub system_background_path: Option<String>,
}
