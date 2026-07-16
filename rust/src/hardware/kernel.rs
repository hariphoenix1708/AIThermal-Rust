use super::profile::KernelCapabilityProfile;
use std::path::Path;

#[allow(clippy::collapsible_if)]
pub fn probe_kernel() -> KernelCapabilityProfile {
    let mut profile = KernelCapabilityProfile::default();

    use flate2::read::GzDecoder;
    use std::io::Read;

    let target_features = [
        "CONFIG_SCHED_WALT",
        "CONFIG_UCLAMP_TASK",
        "CONFIG_UCLAMP_BUCKETS_COUNT",
        "CONFIG_ENERGY_MODEL",
        "CONFIG_CPU_FREQ",
        "CONFIG_CPU_IDLE",
        "CONFIG_CPUSETS",
        "CONFIG_CGROUPS",
        "CONFIG_CGROUP_SCHED",
        "CONFIG_PSI",
        "CONFIG_PSI_DEFAULT_DISABLED",
        "CONFIG_THERMAL",
        "CONFIG_THERMAL_GOV_STEP_WISE",
        "CONFIG_THERMAL_GOV_POWER_ALLOCATOR",
        "CONFIG_DEVFREQ_GOV_SIMPLE_ONDEMAND",
        "CONFIG_QCOM_CPUFREQ_HW",
        "CONFIG_ARM64",
        "CONFIG_HZ",
        "CONFIG_PREEMPT",
        "CONFIG_PREEMPT_DYNAMIC",
    ];

    let mut has_config = false;
    if let Ok(file) = std::fs::File::open("/proc/config.gz") {
        let mut gz = GzDecoder::new(file);
        let mut content = String::new();
        if gz.read_to_string(&mut content).is_ok() {
            has_config = true;
            for line in content.lines() {
                if !line.starts_with("CONFIG_") {
                    continue;
                }

                let is_match = target_features.iter().any(|&tf| line.starts_with(tf));

                if is_match && (line.contains("=y") || line.contains("=m") || line.contains('=')) {
                    if let Some(_idx) = line.find('=') {
                        // Special handling to allow reading values for CONFIG_HZ or CONFIG_UCLAMP_BUCKETS_COUNT if needed,
                        // but storing just the flag or the whole line is fine. We will store feature=value.
                        profile.features.push(line.to_string());
                    }
                }
            }
        }
    }

    if !has_config {
        if Path::new("/sys/module/devfreq").exists() {
            profile
                .features
                .push("CONFIG_DEVFREQ_GOV_SIMPLE_ONDEMAND=y".to_string());
        }
        if Path::new("/sys/kernel/debug/energy_model").exists() {
            profile.features.push("CONFIG_ENERGY_MODEL=y".to_string());
        }
        if Path::new("/proc/pressure/memory").exists() {
            profile.features.push("CONFIG_PSI=y".to_string());
        }
    }

    profile
}
