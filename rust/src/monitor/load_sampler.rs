pub struct LoadSample { pub idle: u64, pub total: u64 }

pub fn read_cpu_stat() -> std::collections::HashMap<usize, LoadSample> {
    let mut result = std::collections::HashMap::new();
    if let Ok(content) = std::fs::read_to_string("/proc/stat") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("cpu") {
                if let Some((idx_str, fields)) = rest.split_once(' ') {
                    if let Ok(idx) = idx_str.trim().parse::<usize>() {
                        let nums: Vec<u64> = fields.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                        if nums.len() >= 4 {
                            let idle = nums[3];
                            let total: u64 = nums.iter().sum();
                            result.insert(idx, LoadSample { idle, total });
                        }
                    }
                }
            }
        }
    }
    result
}

pub fn compute_utilization(prev: &LoadSample, curr: &LoadSample) -> f32 {
    let total_delta = curr.total.saturating_sub(prev.total);
    let idle_delta = curr.idle.saturating_sub(prev.idle);
    if total_delta == 0 { return 0.0; }
    1.0 - (idle_delta as f32 / total_delta as f32)
}
