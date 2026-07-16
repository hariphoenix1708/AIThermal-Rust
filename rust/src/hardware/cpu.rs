use super::capability::CapabilityNode;
use super::profile::{CpuCluster, CpuTopology};
use std::fs;
use walkdir::WalkDir;

pub fn probe_cpu() -> CpuTopology {
    let mut topo = CpuTopology {
        is_walt: std::path::Path::new("/proc/sys/kernel/sched_walt_rotate_capacity").exists(),
        has_uclamp: std::path::Path::new("/dev/cpuctl/top-app/cpu.uclamp.max").exists(),
        is_eas: std::path::Path::new("/proc/sys/kernel/sched_energy_aware").exists()
            || std::path::Path::new("/sys/devices/system/cpu/cpu0/cpufreq/energy_model").exists(),
        ..Default::default()
    };

    for entry in WalkDir::new("/sys/devices/system/cpu/cpufreq")
        .min_depth(1)
        .max_depth(1)
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_name().to_string_lossy().starts_with("policy") {
            let policy_path = entry.path().to_string_lossy().to_string();

            let mut governor_node =
                CapabilityNode::new(&format!("{}/scaling_governor", policy_path), "cpu_governor");
            let mut available_governors = Vec::new();
            if let Ok(avail_gov) =
                fs::read_to_string(entry.path().join("scaling_available_governors"))
            {
                available_governors = avail_gov
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                governor_node.supported_values = available_governors.clone();
            }

            let freq_node =
                CapabilityNode::new(&format!("{}/scaling_cur_freq", policy_path), "cpu_cur_freq");
            let mut min_freq_node =
                CapabilityNode::new(&format!("{}/scaling_min_freq", policy_path), "cpu_min_freq");
            let mut max_freq_node =
                CapabilityNode::new(&format!("{}/scaling_max_freq", policy_path), "cpu_max_freq");
            let mut available_frequencies = Vec::new();
            if let Ok(avail_freq) =
                fs::read_to_string(entry.path().join("scaling_available_frequencies"))
            {
                let avail_freq_vec: Vec<u64> = avail_freq
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                available_frequencies = avail_freq_vec;
                min_freq_node.supported_values = available_frequencies
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                max_freq_node.supported_values = min_freq_node.supported_values.clone();
            }

            let mut cluster = CpuCluster {
                policy_node: CapabilityNode::new(&policy_path, "cpu_policy"),
                policy_path: policy_path.clone(),
                governor_node,
                freq_node,
                min_freq_node,
                max_freq_node,
                available_governors,
                available_frequencies,
                ..Default::default()
            };

            cluster.name = entry.file_name().to_string_lossy().to_string();

            if let Ok(cur_gov) = fs::read_to_string(entry.path().join("scaling_governor")) {
                cluster.current_governor = cur_gov.trim().to_string();
            }

            if let Ok(cur_freq) = fs::read_to_string(entry.path().join("scaling_cur_freq")) {
                if let Ok(freq) = cur_freq.trim().parse() {
                    cluster.current_frequency = freq;
                }
            }

            if let Ok(cpus) = fs::read_to_string(entry.path().join("affected_cpus")) {
                for cpu_str in cpus.split_whitespace() {
                    if let Ok(cpu) = cpu_str.parse() {
                        cluster.cpus.push(cpu);
                    }
                }
            }

            if let Ok(related) = fs::read_to_string(entry.path().join("related_cpus")) {
                for cpu_str in related.split_whitespace() {
                    if let Ok(cpu) = cpu_str.parse() {
                        cluster.related_cpus.push(cpu);
                    }
                }
            } else if let Ok(affected) = fs::read_to_string(entry.path().join("affected_cpus")) {
                for cpu_str in affected.split_whitespace() {
                    if let Ok(cpu) = cpu_str.parse() {
                        cluster.related_cpus.push(cpu);
                    }
                }
            } else {
                cluster.related_cpus = cluster.cpus.clone();
            }

            if let Ok(max) = fs::read_to_string(entry.path().join("cpuinfo_max_freq")) {
                if let Ok(max_val) = max.trim().parse() {
                    cluster.cpuinfo_max_freq = max_val;
                    cluster.max_freq = max_val;
                }
            }
            if let Ok(min) = fs::read_to_string(entry.path().join("cpuinfo_min_freq")) {
                if let Ok(min_val) = min.trim().parse() {
                    cluster.cpuinfo_min_freq = min_val;
                    cluster.min_freq = min_val;
                }
            }

            topo.clusters.push(cluster);
        }
    }

    // Sort clusters by max frequency, then by cpu id
    topo.clusters.sort_by(|a, b| {
        a.cpuinfo_max_freq.cmp(&b.cpuinfo_max_freq).then_with(|| {
            a.cpus
                .first()
                .unwrap_or(&0)
                .cmp(b.cpus.first().unwrap_or(&0))
        })
    });

    topo
}
