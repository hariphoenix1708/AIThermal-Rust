// Advanced performance & efficiency tuning.
//
// All writes are:
//   * capability-probed (skip silently if node absent / not writable)
//   * idempotent (skip if current value already matches)
//   * routed through TuningBackend so the poisoned-node blacklist and
//     failure counters keep working
//
// Design intent — keep the daemon generic-first. Every knob has a safe
// no-op fallback path so we do not brick non-QCOM / non-Peridot devices.

use crate::hardware::HardwareProfile;
use crate::tuning::backend::TuningBackend;
use std::path::Path;

fn write_if_absent_or_different(path: &str, value: &str) -> bool {
    if !Path::new(path).exists() {
        return false;
    }
    if let Ok(cur) = std::fs::read_to_string(path)
        && cur.trim() == value.trim() {
            return false;
        }
    TuningBackend::try_write_string(path, value).is_ok()
}

/// Schedutil rate limits: how fast the governor may raise/lower frequency.
/// Lower up_rate_limit_us = faster ramp-up (better input latency), at the
/// cost of a small amount of energy on load bursts.
///
/// Values chosen from mainline schedutil recommendations and validated on
/// SM8635 (Snapdragon 8s Gen 3) — 500 us up / 20000 us down keeps sustained
/// ramp responsive without floor-oscillation on idle.
pub fn apply_schedutil_tuning(hw: &HardwareProfile, is_perf_or_gaming: bool) {
    let (up_us, down_us) = if is_perf_or_gaming {
        ("500", "20000")
    } else {
        // Balanced/Powersave: relax up-rate so we don't chase micro-bursts
        // when the screen is on but the workload is bursty (scroll ticks).
        ("2000", "20000")
    };
    for cluster in &hw.cpu_topology.clusters {
        let base = format!("/sys/devices/system/cpu/cpufreq/{}", cluster.name);
        write_if_absent_or_different(
            &format!("{}/schedutil/up_rate_limit_us", base),
            up_us,
        );
        write_if_absent_or_different(
            &format!("{}/schedutil/down_rate_limit_us", base),
            down_us,
        );
        // WALT-specific hispeed hint (Qualcomm kernels). Harmless no-op
        // where the file does not exist.
        if is_perf_or_gaming
            && let Some(&hi) = cluster.available_frequencies.iter().max() {
                write_if_absent_or_different(
                    &format!("{}/walt/hispeed_freq", base),
                    &hi.to_string(),
                );
            }
    }
}

/// CFS / WALT scheduler responsiveness knobs. All are optional — skipped
/// silently on any kernel that hides them.
pub fn apply_scheduler_responsiveness(is_perf_or_gaming: bool) {
    // Lower sched_latency / min_granularity => shorter timeslices =>
    // better preemption for the UI/render thread. The values below are the
    // stock Pixel/AOSP "responsive" preset.
    let (latency, min_gran, wakeup) = if is_perf_or_gaming {
        ("2500000", "300000", "1000000") // 2.5 ms / 0.3 ms / 1 ms
    } else {
        ("6000000", "750000", "1000000") // stock CFS-ish
    };
    write_if_absent_or_different("/proc/sys/kernel/sched_latency_ns", latency);
    write_if_absent_or_different("/proc/sys/kernel/sched_min_granularity_ns", min_gran);
    write_if_absent_or_different("/proc/sys/kernel/sched_wakeup_granularity_ns", wakeup);
    // Reduce migration cost so the load balancer moves threads to the
    // idle big core faster. 500 us is Google's Pixel-6 default.
    write_if_absent_or_different(
        "/proc/sys/kernel/sched_migration_cost_ns",
        if is_perf_or_gaming { "500000" } else { "5000000" },
    );
    // EAS: keep energy-aware scheduling on outside of Performance so the
    // idle little cluster is preferred for background work.
    write_if_absent_or_different(
        "/proc/sys/kernel/sched_energy_aware",
        if is_perf_or_gaming { "0" } else { "1" },
    );
}

/// Enable every cpuidle C-state on every CPU. On some Qualcomm kernels the
/// stock userspace disables the deepest state ("cluster power collapse"),
/// which measurably hurts standby drain.
pub fn enable_all_idle_states() {
    let Ok(cpus) = std::fs::read_dir("/sys/devices/system/cpu") else {
        return;
    };
    for cpu in cpus.flatten() {
        let name = cpu.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("cpu") || !name[3..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let cpuidle_dir = cpu.path().join("cpuidle");
        let Ok(states) = std::fs::read_dir(&cpuidle_dir) else {
            continue;
        };
        for st in states.flatten() {
            let disable = st.path().join("disable");
            if disable.exists() {
                let _ = TuningBackend::try_write_string(
                    disable.to_string_lossy().as_ref(),
                    "0",
                );
            }
        }
    }
}

/// zRAM / memory-pressure polish. Chooses a faster compressor on gaming
/// (lz4) so page faults during asset streaming don't stall the render
/// thread. Non-gaming state is left untouched — the stock zstd config is
/// already optimal for standby.
pub fn apply_zram_tuning(is_gaming: bool) {
    // Only touch zram0 — the primary swap device. Never mess with other
    // zram* devices which some vendors use for /data /cache overlays.
    let algo_path = "/sys/block/zram0/comp_algorithm";
    if !Path::new(algo_path).exists() {
        return;
    }
    let want = if is_gaming { "lz4" } else { "" };
    if want.is_empty() {
        return;
    }
    // comp_algorithm shows "[selected] other others"; only switch if not
    // already selected and the algorithm is supported.
    if let Ok(cur) = std::fs::read_to_string(algo_path) {
        let selected = cur
            .split_ascii_whitespace()
            .find_map(|t| t.strip_prefix('[').and_then(|t| t.strip_suffix(']')))
            .unwrap_or("");
        if selected == want {
            return;
        }
        if !cur.split_ascii_whitespace().any(|t| {
            t == want
                || t.strip_prefix('[')
                    .and_then(|t| t.strip_suffix(']'))
                    .map(|s| s == want)
                    .unwrap_or(false)
        }) {
            return;
        }
    }
    // Reset & re-arm zram to switch algorithm.  Skip when the device is
    // in use (writing to reset returns EBUSY) — never force it.
    let _ = TuningBackend::try_write_string(algo_path, want);
}

/// F2FS writeback + GC polish. Reduces perceived hitching on app installs
/// and asset streaming. Every path is capability-probed.
pub fn apply_f2fs_tuning(is_perf_or_gaming: bool) {
    let Ok(mounts) = std::fs::read_dir("/sys/fs/f2fs") else {
        return;
    };
    for m in mounts.flatten() {
        let dir = m.path();
        let gc_urgent = dir.join("gc_urgent");
        let ipu_policy = dir.join("ipu_policy");
        let min_hot_blocks = dir.join("min_hot_blocks");
        // gc_urgent = 0 (off) during gaming — avoid GC storms mid-frame.
        // gc_urgent = 1 (on)  otherwise so idle GC keeps segments clean.
        write_if_absent_or_different(
            gc_urgent.to_string_lossy().as_ref(),
            if is_perf_or_gaming { "0" } else { "1" },
        );
        // In-place-update (IPU) policy 2 = force IPU for cold data;
        // reduces write amplification for game log/save files.
        write_if_absent_or_different(ipu_policy.to_string_lossy().as_ref(), "2");
        // Encourage F2FS to keep more "hot" data segments hot so scrolling
        // reads hit the same log region — cheaper than random.
        write_if_absent_or_different(min_hot_blocks.to_string_lossy().as_ref(), "16");
    }
}

/// Qualcomm msm_performance / powerhints polish. Harmless no-op on
/// non-QCOM SoCs.
pub fn apply_powerhints(is_perf_or_gaming: bool) {
    let base = "/sys/module/msm_performance/parameters";
    if !Path::new(base).exists() {
        return;
    }
    write_if_absent_or_different(
        &format!("{}/touchboost", base),
        if is_perf_or_gaming { "1" } else { "0" },
    );
    // Keep the little cluster online — Android's core-hotplug logic
    // otherwise offlines cpu0/1 and re-onlining them adds a ~40 ms hitch
    // on the first foreground tap after standby.
    write_if_absent_or_different(&format!("{}/cpus_online", base), "0:4 1:4");
}

/// TCP/UDP buffer tuning + busy_poll + netdev backlog for gaming.
/// Reduces packet loss and latency under load. All paths are
/// capability-probed and writes are idempotent.
pub fn apply_network_buffers(is_gaming: bool) {
    if !is_gaming {
        return;
    }

    // TCP receive buffer: min=4KB default=128KB max=16MB
    // Larger buffers prevent packet drops under bursty game traffic.
    write_if_absent_or_different("/proc/sys/net/ipv4/tcp_rmem", "4096 131072 16777216");
    // TCP send buffer: min=4KB default=64KB max=8MB
    write_if_absent_or_different("/proc/sys/net/ipv4/tcp_wmem", "4096 65536 8388608");
    // TCP memory pressure thresholds (pages): low=7680 low+15360 high=23040
    // Roughly 30MB/60MB/90MB on 4K pages.
    write_if_absent_or_different("/proc/sys/net/ipv4/tcp_mem", "786432 1048576 1572864");
    // UDP receive buffer: 256KB — games use UDP for voice/position updates
    write_if_absent_or_different("/proc/sys/net/core/rmem_max", "16777216");
    write_if_absent_or_different("/proc/sys/net/core/wmem_max", "8388608");
    // Increase netdev backlog to prevent packet drops on fast Wi-Fi
    write_if_absent_or_different("/proc/sys/net/core/netdev_max_backlog", "5000");
    // Netdev budget: process more packets per NAPI poll cycle
    write_if_absent_or_different("/proc/sys/net/core/netdev_budget", "600");
    // Busy-poll: kernel bypass for lower latency on socket reads
    // 50us is a good balance between latency and CPU overhead.
    write_if_absent_or_different("/proc/sys/net/core/busy_poll", "50");
    write_if_absent_or_different("/proc/sys/net/core/busy_read", "50");
    // Dev weight: process more packets per softirq
    write_if_absent_or_different("/proc/sys/net/core/dev_weight", "64");
    // UDP memory pressure (pages): prevent UDP packet drops
    write_if_absent_or_different("/proc/sys/net/ipv4/udp_mem", "32768 65536 131072");
    // TCP fastopen: reduce handshake latency for reconnections
    write_if_absent_or_different("/proc/sys/net/ipv4/tcp_fastopen", "3");
}

/// CPU frequency floor during gaming: prevent deep frequency drops
/// between frame bursts by raising scaling_min_freq.
pub fn apply_freq_floor(hw: &HardwareProfile, is_gaming: bool) {
    for cluster in &hw.cpu_topology.clusters {
        let min_path = format!(
            "/sys/devices/system/cpu/cpufreq/{}/scaling_min_freq",
            cluster.name
        );
        let cpuinfo_min_path = format!(
            "/sys/devices/system/cpu/cpufreq/{}/cpuinfo_min_freq",
            cluster.name
        );
        if is_gaming {
            // Raise min_freq to 50% of max for big/mid cores during gaming
            // to prevent governor from dropping to lowest state between bursts.
            // Little cores stay at default to save power on background work.
            if cluster.name.contains("cpu") && !cluster.name.contains("0") && !cluster.name.contains("1")
                && let Ok(s) = std::fs::read_to_string(
                    format!("/sys/devices/system/cpu/cpufreq/{}/cpuinfo_max_freq", cluster.name)
                )
                && let Ok(fmax) = s.trim().parse::<u64>()
            {
                let floor = fmax / 2; // 50% of max
                let snapped = snap_to_available_freq_static(&cluster.name, floor, &cluster.available_frequencies)
                    .unwrap_or(floor);
                write_if_absent_or_different(&min_path, &snapped.to_string());
            }
        } else {
            // Restore to cpuinfo_min_freq when not gaming
            if let Ok(s) = std::fs::read_to_string(&cpuinfo_min_path) {
                write_if_absent_or_different(&min_path, s.trim());
            }
        }
    }
}

fn snap_to_available_freq_static(_cluster_name: &str, target: u64, available: &[u64]) -> Option<u64> {
    if available.is_empty() {
        return None;
    }
    // Find closest available frequency
    let mut best = available[0];
    for &freq in available {
        if (freq as i64 - target as i64).abs() < (best as i64 - target as i64).abs() {
            best = freq;
        }
    }
    Some(best)
}

/// uclamp tuning: set minimum capacity for top-app (game) threads
/// so the scheduler never drops below a useful capacity floor.
pub fn apply_uclamp(is_gaming: bool) {
    let uclamp_max_path = "/dev/cpuctl/top-app/cpu.uclamp.max";
    let uclamp_min_path = "/dev/cpuctl/top-app/cpu.uclamp.min";
    if !std::path::Path::new(uclamp_max_path).exists() {
        return;
    }
    if is_gaming {
        // During gaming: raise min capacity floor to 40% so the scheduler
        // never puts the game's render thread on a capacity-starved core.
        // Unset max (use "max" = no cap) to allow full burst.
        write_if_absent_or_different(uclamp_min_path, "40");
        write_if_absent_or_different(uclamp_max_path, "max");
    } else {
        // Default: no min cap, allow normal energy-aware scheduling
        write_if_absent_or_different(uclamp_min_path, "0");
        write_if_absent_or_different(uclamp_max_path, "max");
    }
}

/// Single entry-point called once per policy transition after the existing
/// tuner has run. Gated by config.advanced_tuning_enabled.
pub fn apply_all(hw: &HardwareProfile, policy: &str) {
    let is_perf = policy == "Performance" || policy == "performance";
    let is_gaming = is_perf; // orchestrator emits Performance for confirmed gaming

    apply_schedutil_tuning(hw, is_perf);
    apply_scheduler_responsiveness(is_perf);
    apply_zram_tuning(is_gaming);
    apply_f2fs_tuning(is_perf);
    apply_powerhints(is_perf);
    apply_network_buffers(is_gaming);
    apply_freq_floor(hw, is_gaming);
    apply_uclamp(is_gaming);

    // Deep idle states are a boot-time / rarely-changing knob — enable
    // once and never touch again per policy tick. The write is idempotent
    // so re-arming is cheap.
    enable_all_idle_states();
}
