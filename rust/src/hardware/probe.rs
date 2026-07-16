// Intentionally reserved or conditionally compiled across bins

use super::profile::*;
use anyhow::Result;

pub struct HardwareProbe;

impl HardwareProbe {
    pub fn probe() -> Result<HardwareProfile> {
        let mut profile = HardwareProfile {
            metadata: crate::cache::build_cache_metadata(),
            product_device: super::android_prop::get_any_property(
                &["ro.product.device", "ro.product.model"],
                "Unknown",
            ),
            boot_hardware: super::android_prop::get_any_property(&["ro.boot.hardware"], "Unknown"),
            device_identity: super::android_prop::get_any_property(
                &["ro.product.model", "ro.product.device"],
                "Unknown",
            ),
            soc_info: super::android_prop::get_any_property(
                &[
                    "ro.soc.model",
                    "ro.soc.name",
                    "ro.board.platform",
                    "ro.hardware",
                ],
                "Unknown",
            ),
            board_platform: super::android_prop::get_any_property(
                &["ro.board.platform"],
                "Unknown",
            ),
            hardware: super::android_prop::get_any_property(
                &["ro.hardware", "ro.boot.hardware"],
                "Unknown",
            ),
            migt_present: std::path::Path::new("/sys/module/migt/parameters").exists(),
            glk_present: std::path::Path::new("/proc/sys/glk").exists(),
            cpu_topology: super::cpu::probe_cpu(),
            gpu_profile: super::gpu::probe_gpu(),
            thermal_profile: super::thermal::probe_thermal(),
            battery_profile: super::charging::probe_battery(),
            charging_profile: super::charging::probe_charging(),
            cpuset_profile: super::cpuset::probe_cpuset(),
            display_profile: super::display::probe_display(),
            network_profile: super::network::probe_network(),
            memory_profile: super::memory::probe_memory(),
            storage_profile: super::storage::probe_storage(),
            scheduler_profile: super::scheduler::probe_scheduler(),
            services_profile: super::services::probe_services(),
            kernel_profile: super::kernel::probe_kernel(),
            ..Default::default()
        };

        // Probe Qualcomm bus_dcvs for Candidate 4
        for component in ["DDR", "LLCC", "L3"] {
            let base = format!("/sys/devices/system/cpu/bus_dcvs/{}", component);
            if std::path::Path::new(&base).exists() {
                if let Ok(entries) = std::fs::read_dir(&base) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            let path = entry.path();

                            let avail_str = std::fs::read_to_string(path.join("available_frequencies")).unwrap_or_default();
                            let avail_vec: Vec<u64> = avail_str.split_whitespace().filter_map(|s| s.parse().ok()).collect();

                            if !avail_vec.is_empty() {
                                let mut dcvs = DcvsProfile {
                                    component: component.to_string(),
                                    path: path.to_string_lossy().to_string(),
                                    available_frequencies: avail_vec,
                                    ..Default::default()
                                };

                                if std::fs::OpenOptions::new().write(true).open(path.join("hw_max_freq")).is_ok() {
                                    dcvs.hw_max_freq_node = Some(path.join("hw_max_freq").to_string_lossy().to_string());
                                }
                                if std::fs::OpenOptions::new().write(true).open(path.join("hw_min_freq")).is_ok() {
                                    dcvs.hw_min_freq_node = Some(path.join("hw_min_freq").to_string_lossy().to_string());
                                }

                                profile.dcvs_profiles.push(dcvs);
                            }
                        }
                    }
                }
            }
        }

        // Compatibility Summary
        profile.compatibility_report.insert(
            "cpu".to_string(),
            if !profile.cpu_topology.clusters.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "gpu".to_string(),
            if !profile.gpu_profile.path.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "thermal".to_string(),
            if !profile.thermal_profile.all_zones.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "battery".to_string(),
            if !profile.battery_profile.path.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "charging".to_string(),
            if !profile.charging_profile.path.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "cpuset".to_string(),
            if !profile.cpuset_profile.root_path.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );

        profile.compatibility_report.insert(
            "network".to_string(),
            if !profile.network_profile.tcp_congestion_control.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "memory".to_string(),
            if profile.memory_profile.has_psi
                || profile.memory_profile.has_zram
                || profile.memory_profile.has_lmkd
            {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "storage".to_string(),
            if profile.storage_profile.has_ufs || !profile.storage_profile.block_devices.is_empty()
            {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "scheduler".to_string(),
            if profile.scheduler_profile.has_schedutil
                || profile.scheduler_profile.has_uclamp
                || profile.scheduler_profile.has_walt
                || profile.scheduler_profile.has_eas
            {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "services".to_string(),
            if profile.services_profile.thermal_hal
                || profile.services_profile.power_hal
                || profile.services_profile.health_hal
                || profile.services_profile.perf_service
                || profile.services_profile.vendor_thermal_engine
                || profile.services_profile.vendor_performance_daemon
            {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );
        profile.compatibility_report.insert(
            "kernel".to_string(),
            if !profile.kernel_profile.features.is_empty() {
                "PASS".to_string()
            } else {
                "MISSING".to_string()
            },
        );

        Ok(profile)
    }
}
