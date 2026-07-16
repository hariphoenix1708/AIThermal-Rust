use super::capability::CapabilityNode;
use super::profile::GpuProfile;
use std::fs;
use std::path::Path;

fn read_u32(path: &str) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse::<u32>().ok()
}

fn writable_file(path: &str) -> bool {
    Path::new(path).is_file() && fs::OpenOptions::new().write(true).open(path).is_ok()
}

pub fn probe_gpu() -> GpuProfile {
    let mut profile = GpuProfile::default();

    // Prioritize KGSL path
    if Path::new("/sys/class/kgsl/kgsl-3d0").exists() {
        profile.path = "/sys/class/kgsl/kgsl-3d0".to_string();
        profile.is_kgsl = true;
        profile.has_devfreq = Path::new(&format!("{}/devfreq", profile.path)).exists();

        let min_pwr_path = format!("{}/min_pwrlevel", profile.path);
        let max_pwr_path = format!("{}/max_pwrlevel", profile.path);

        profile.min_power_level = read_u32(&min_pwr_path);
        profile.max_power_level = read_u32(&max_pwr_path);

        for current_name in ["pwrlevel", "current_pwrlevel"] {
            let current_path = format!("{}/{}", profile.path, current_name);

            if let Some(level) = read_u32(&current_path) {
                profile.current_power_level = Some(level);
            }

            if writable_file(&current_path) {
                profile.power_level_path = Some(current_path);
                break;
            }
        }

        let bus_split_path = format!("{}/bus_split", profile.path);
        let force_clk_on_path = format!("{}/force_clk_on", profile.path);

        if Path::new(&bus_split_path).exists() {
            profile.has_bus_split = true;
        }
        if Path::new(&force_clk_on_path).exists() {
            profile.has_force_clk_on = true;
        }

        if profile.has_devfreq {
            profile.devfreq_path = format!("{}/devfreq", profile.path);
            let mut gov_node = CapabilityNode::new(
                &format!("{}/governor", profile.devfreq_path),
                "gpu_devfreq_governor",
            );
            let mut freq_node = CapabilityNode::new(
                &format!("{}/userspace/set_freq", profile.devfreq_path),
                "gpu_devfreq_freq",
            );
            if let Ok(govs) =
                fs::read_to_string(format!("{}/available_governors", profile.devfreq_path))
            {
                profile.available_governors =
                    govs.split_whitespace().map(|s| s.to_string()).collect();
                gov_node.supported_values = profile.available_governors.clone();
            }
            if let Ok(cur) = fs::read_to_string(format!("{}/governor", profile.devfreq_path)) {
                profile.current_governor = cur.trim().to_string();
            }
            if let Ok(freqs) =
                fs::read_to_string(format!("{}/available_frequencies", profile.devfreq_path))
            {
                profile.available_frequencies = freqs
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                freq_node.supported_values = profile
                    .available_frequencies
                    .iter()
                    .map(|f| f.to_string())
                    .collect();
            }
            profile.devfreq_governor_node = gov_node;
            profile.devfreq_freq_node = freq_node;
        }
    } else {
        // Fallback to devfreq directly
        if let Ok(entries) = fs::read_dir("/sys/class/devfreq") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("kgsl") || name.contains("mali") || name.contains("gpu") {
                    profile.path = format!("/sys/class/devfreq/{}", name);
                    profile.has_devfreq = true;
                    profile.devfreq_path = profile.path.clone();
                    let mut gov_node = CapabilityNode::new(
                        &format!("{}/governor", profile.path),
                        "gpu_devfreq_governor",
                    );
                    let mut freq_node = CapabilityNode::new(
                        &format!("{}/userspace/set_freq", profile.path),
                        "gpu_devfreq_freq",
                    );

                    if let Ok(govs) =
                        fs::read_to_string(format!("{}/available_governors", profile.path))
                    {
                        profile.available_governors =
                            govs.split_whitespace().map(|s| s.to_string()).collect();
                        gov_node.supported_values = profile.available_governors.clone();
                    }
                    if let Ok(cur) = fs::read_to_string(format!("{}/governor", profile.path)) {
                        profile.current_governor = cur.trim().to_string();
                    }
                    if let Ok(freqs) =
                        fs::read_to_string(format!("{}/available_frequencies", profile.path))
                    {
                        profile.available_frequencies = freqs
                            .split_whitespace()
                            .filter_map(|s| s.parse().ok())
                            .collect();
                        freq_node.supported_values = profile
                            .available_frequencies
                            .iter()
                            .map(|f| f.to_string())
                            .collect();
                    }
                    profile.devfreq_governor_node = gov_node;
                    profile.devfreq_freq_node = freq_node;
                    break;
                }
            }
        }
    }

    profile
}
