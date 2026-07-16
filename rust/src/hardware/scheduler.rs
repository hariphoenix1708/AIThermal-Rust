use super::profile::SchedulerProfile;
use std::path::Path;

pub fn probe_scheduler() -> SchedulerProfile {
    let mut has_schedutil = false;

    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("policy")
                && let Ok(content) =
                    std::fs::read_to_string(entry.path().join("scaling_available_governors"))
                && content.contains("schedutil")
            {
                has_schedutil = true;
                break;
            }
        }
    }

    SchedulerProfile {
        has_schedtune: Path::new("/dev/stune").exists(),
        has_uclamp: Path::new("/dev/cpuctl/top-app/cpu.uclamp.max").exists(),
        has_eas: Path::new("/proc/sys/kernel/sched_energy_aware").exists()
            || Path::new("/sys/devices/system/cpu/cpu0/cpufreq/energy_model").exists(),
        has_schedutil,
        has_walt: Path::new("/proc/sys/kernel/sched_walt_rotate_capacity").exists(),
    }
}
