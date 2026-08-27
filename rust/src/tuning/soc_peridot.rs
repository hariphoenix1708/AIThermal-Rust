// SM8635 (peridot/pineapple) SoC-specific gaming tunings.
// Every knob is capability-probed, idempotent, and routed via TuningBackend.
// All state saved via RuntimeTuner::write_and_save where applicable; this
// module uses the same helper so it can be called from advanced::apply_all
// with minimal duplication.

use crate::tuning::backend::TuningBackend;
use std::path::Path;

fn write(path: &str, val: &str) -> bool {
    if !Path::new(path).exists() {
        return false;
    }
    if let Ok(cur) = std::fs::read_to_string(path)
        && cur.trim() == val.trim()
    {
        return false;
    }
    TuningBackend::try_write_string(path, val).is_ok()
}

// ──────────────────────────────────────────────────────────────────────
// 1. WALT / Scheduler migration block
// ──────────────────────────────────────────────────────────────────────
pub fn apply_walt_gaming(is_gaming: bool) {
    // Up/downmigrate thresholds: widen to keep 4-Thread heavy MT on Big
    // (policy3: 4 cores 480-2803MHz) instead of premature Prime promotion.
    // Stock peridot: upmigrate ~70/down ~60. Gaming: 85/70.
    let up = if is_gaming { "85" } else { "70" };
    let down = if is_gaming { "70" } else { "60" };
    // Both EAS and WALT variants exist across kernels.
    for p in [
        "/proc/sys/kernel/sched_upmigrate",
        "/proc/sys/kernel/sched_walt_upmigrate",
        "/proc/sys/kernel/walt_upmigrate",
    ] {
        write(p, up);
    }
    for p in [
        "/proc/sys/kernel/sched_downmigrate",
        "/proc/sys/kernel/sched_walt_downmigrate",
        "/proc/sys/kernel/walt_downmigrate",
    ] {
        write(p, down);
    }

    // Per-CPU busy hysteresis timers: prevent sudden down-ramp between
    // micro-bursts (e.g., game logic tick gaps 2-4ms).
    for cpu in 0..8 {
        // sched_busy_hysteresis_enable (0/1) + sched_busy_hysteresis_ns
        write(
            &format!("/sys/devices/system/cpu/cpu{cpu}/sched_busy_hysteresis_enable"),
            if is_gaming { "1" } else { "0" },
        );
        // Hysteresis window: 8ms gaming, stock ~0
        write(
            &format!("/sys/devices/system/cpu/cpu{cpu}/sched_busy_hysteresis_ns"),
            if is_gaming { "8000000" } else { "0" },
        );
        // Qualcomm's walt_table busy hysteresis variant
        write(
            &format!(
                "/sys/devices/system/cpu/cpu{cpu}/sched/walt_busy_hysteresis_enable"
            ),
            if is_gaming { "1" } else { "0" },
        );
    }

    // Cpu3 (Big cluster head) / Cpu7 (Prime) walt hispeed_load: load % to
    // jump directly to hispeed_freq. Stock ~85, gaming 95 to avoid premature
    // hispeed spikes on transient UI.
    for (policy, gaming_load, stock_load) in [("policy3", "95", "85"), ("policy7", "95", "90")] {
        let base = format!("/sys/devices/system/cpu/cpufreq/{policy}");
        for suffix in ["walt/hispeed_load", "schedutil/hispeed_load", "hispeed_load"] {
            write(&format!("{base}/{suffix}"), if is_gaming { gaming_load } else { stock_load });
        }
        // Also walt rtg_boost + prefer_idle toggles per cluster
        write(&format!("{base}/walt/rtg_boost"), if is_gaming { "1" } else { "0" });
    }

    // walt_idle_enough: 0 during gaming so scheduler never assumes a core
    // was idle long enough to enter deep sleep between frame deadlines.
    for p in [
        "/proc/sys/kernel/sched_walt_idle_enough",
        "/proc/sys/kernel/walt_idle_enough",
        "/sys/kernel/walt/idle_enough",
    ] {
        write(p, if is_gaming { "0" } else { "1" });
    }
}

// ──────────────────────────────────────────────────────────────────────
// 2. Memory bus & LLC / cache block
// ──────────────────────────────────────────────────────────────────────
pub fn apply_bus_llc_gaming(is_gaming: bool) {
    // Pin DDR & LLCC buses to max bw during gaming. Uses devfreq nodes:
    // /sys/class/devfreq/*bwmon* / *cpubw* / *llccbw* / *l3* etc.
    // All probed via glob-free explicit checks.

    // DDR/L3/LLCC min_freq -> max_freq during gaming
    if let Ok(entries) = std::fs::read_dir("/sys/class/devfreq") {
        for e in entries.flatten() {
            let base = e.path();
            let name = base.file_name().unwrap_or_default().to_string_lossy().to_string();
            // Only bus-related devfreqs: cpubw, llccbw, l3, ddr, memlat
            let is_bus = name.contains("cpubw")
                || name.contains("llccbw")
                || name.contains("l3")
                || name.contains("ddr")
                || name.contains("memlat")
                || name.contains("bus");
            if !is_bus {
                continue;
            }
            let min_path = base.join("min_freq");
            let max_path = base.join("max_freq");
            let avail_path = base.join("available_frequencies");
            if !min_path.exists() || !max_path.exists() {
                continue;
            }
            if is_gaming {
                if let Ok(max) = std::fs::read_to_string(&max_path) {
                    let max = max.trim();
                    // Snap to available_frequencies if present
                    let target = if let Ok(avail) = std::fs::read_to_string(&avail_path) {
                        avail
                            .split_whitespace()
                            .last()
                            .unwrap_or(max)
                            .to_string()
                    } else {
                        max.to_string()
                    };
                    write(min_path.to_string_lossy().as_ref(), &target);
                    // Also vote via userspace governor if present
                    write(base.join("governor").to_string_lossy().as_ref(), "performance");
                }
            } else {
                // Restore: min_freq -> available_frequencies first entry
                if let Ok(avail) = std::fs::read_to_string(&avail_path)
                    && let Some(first) = avail.split_whitespace().next() {
                        write(min_path.to_string_lossy().as_ref(), first);
                    }
                write(base.join("governor").to_string_lossy().as_ref(), "bw_hwmon");
            }

            // hist_memory =0 gaming (react instantly, no averaging), stock 20-30
            write(base.join("bw_hwmon/hist_memory").to_string_lossy().as_ref(), if is_gaming { "0" } else { "20" });
            write(base.join("bw_hwmon/hbm_hist_memory").to_string_lossy().as_ref(), if is_gaming { "0" } else { "20" });

            // DdrBwIoPercent / LlccBwIoPercent: lower threshold to trigger bw votes on I/O
            write(base.join("bw_hwmon/io_percent").to_string_lossy().as_ref(), if is_gaming { "20" } else { "40" });
            write(base.join("bw_hwmon/guard_percent").to_string_lossy().as_ref(), if is_gaming { "10" } else { "30" });
        }
    }

    // Qualcomm L3 / LLCC explicit vote nodes (SoC 865/888+ style)
    for p in [
        "/sys/class/devfreq/soc:qcom,l3-cpu0/max_freq",
        "/sys/class/devfreq/soc:qcom,l3-cpu1/max_freq",
        "/sys/module/l3_vote/parameters/l3_freq",
        "/sys/devices/system/cpu/bus_dcvs/L3/bw_hwmon/hist_memory",
        "/sys/devices/system/cpu/bus_dcvs/DDR/bw_hwmon/hist_memory",
    ] {
        if p.contains("hist_memory") {
            write(p, if is_gaming { "0" } else { "20" });
        }
    }
    // DDR max via msm_bus / bfriend style (legacy)
    for p in [
        "/sys/class/devfreq/soc:qcom,cpubw/min_freq",
        "/sys/class/devfreq/soc:qcom,memlat-cpu0/min_freq",
        "/sys/devices/virtual/devfreq/devfreq0/min_freq",
    ] {
        if is_gaming && Path::new(p).exists()
            && let Ok(max) = std::fs::read_to_string(p.replace("min_freq", "max_freq")) {
                write(p, max.trim());
            }
    }
}

// ──────────────────────────────────────────────────────────────────────
// 3. Uclamp extended (FGMin + latency-sensitive)
// ──────────────────────────────────────────────────────────────────────
pub fn apply_uclamp_extended(is_gaming: bool) {
    // Top-app already handled in advanced::apply_uclamp (40/max).
    // Extend to foreground + latency flag.
    if is_gaming {
        write("/dev/cpuctl/foreground/cpu.uclamp.min", "10");
        write("/dev/cpuctl/foreground/cpu.uclamp.max", "max");
        write("/dev/cpuctl/top-app/cpu.uclamp.latency_sensitive", "1");
        write("/dev/cpuctl/foreground/cpu.uclamp.latency_sensitive", "0");
        // ADPF-adjacent: ensure prefer_idle on top-app cgroup
        write("/dev/cpuctl/top-app/cpu.prefer_idle", "1");
    } else {
        write("/dev/cpuctl/foreground/cpu.uclamp.min", "0");
        write("/dev/cpuctl/foreground/cpu.uclamp.max", "max");
        write("/dev/cpuctl/top-app/cpu.uclamp.latency_sensitive", "0");
        write("/dev/cpuctl/top-app/cpu.prefer_idle", "0");
    }
    // Background already clamped via GameTurbo background lockdown (20% max)
}

// ──────────────────────────────────────────────────────────────────────
// 4. Latency block (CPU DMA latency + GPU idle + sched waking)
// ──────────────────────────────────────────────────────────────────────
pub fn apply_latency_gaming(is_gaming: bool) {
    // CPU DMA latency vote: /dev/cpu_dma_latency open(0) holds pm_qos.
    // We emulate via sysfs pm_qos: /sys/class/power/capabilities or
    // /dev/cpu_dma_latency is a character device voted by open fd.
    // Use the kernel pm_qos interface: /sys/power/pm_qos_resume_latency_us
    // and /proc/sys/kernel/sched_waking_latency_ns.
    write(
        "/proc/sys/kernel/sched_waking_latency_ns",
        if is_gaming { "500000" } else { "1000000" },
    );
    write(
        "/sys/power/pm_qos_resume_latency_us",
        if is_gaming { "0" } else { "100" },
    );
    // Per-CPU pm_qos resume latency (if exposed via cpu device)
    for cpu in 0..8 {
        write(
            &format!("/sys/devices/system/cpu/cpu{cpu}/power/pm_qos_resume_latency_us"),
            if is_gaming { "0" } else { "100" },
        );
        write(
            &format!("/sys/devices/system/cpu/cpu{cpu}/cpuidle/state0/disable"),
            "0",
        ); // keep WFI
    }
    // GPU idle timer handled in game_turbo/gpu_hints.rs (idle_timer 10ms)
    // Ensure KGSL not forced to deep nap during gaming
    write("/sys/class/kgsl/kgsl-3d0/force_no_nap", if is_gaming { "1" } else { "0" });
    write("/sys/class/kgsl/kgsl-3d0/idle_timer", if is_gaming { "10" } else { "80" });
}

// ──────────────────────────────────────────────────────────────────────
// 5. ADPF dynamic tuning (PID loop params + Uclamp High/Low)
// ──────────────────────────────────────────────────────────────────────
pub fn apply_adpf_gaming(is_gaming: bool) {
    // ADPF PowerHAL config nodes vary by OEM:
    // /sys/module/adpf/parameters/*, /sys/kernel/adpf/*, /vendor/etc/powerhint.json override,
    // and per-hint sysfs: /sys/devices/system/cpu/cpufreq/policy*/adpf/*
    // We probe all known PID param nodes and apply vetted peridot values.
    // Values: gaming = aggressive ramp on hotdrop decompression, idle = conservative.

    // Global PID params: used by PowerHAL's PID controller for UCLAMP_MIN dynamic loop
    let pid_on_gaming = "1";
    let pid_on_idle = "0"; // keep disabled when not gaming to save power
    for p in [
        "/sys/module/adpf/parameters/enabled",
        "/sys/kernel/adpf/enabled",
        "/sys/devices/system/cpu/adpf/enabled",
        "/sys/class/power/adpf/enabled",
    ] {
        write(p, if is_gaming { pid_on_gaming } else { pid_on_idle });
    }

    // PID proportional/integral/derivative gains — aggressive on overshoot, ignore micro jitter
    // Stock: Po ~0.15, Pu ~0.04, I ~0.01, Do ~0.005
    // Gaming: Po 0.45 (3x), Pu 0.08 (2x), I 0.02, Do 0.01 — slam max on hotdrop, settle quickly
    let vals_gaming: &[(&str, &str)] = &[
        ("PID_Po", "0.45"),
        ("PID_Pu", "0.08"),
        ("PID_I", "0.02"),
        ("PID_I_Init", "0.1"),
        ("PID_I_High", "0.6"),
        ("PID_I_Low", "-0.3"),
        ("PID_Do", "0.01"),
        ("PID_Du", "0.005"),
    ];
    let vals_idle: &[(&str, &str)] = &[
        ("PID_Po", "0.15"),
        ("PID_Pu", "0.04"),
        ("PID_I", "0.01"),
        ("PID_I_Init", "0.05"),
        ("PID_I_High", "0.3"),
        ("PID_I_Low", "-0.1"),
        ("PID_Do", "0.005"),
        ("PID_Du", "0.002"),
    ];
    let vals = if is_gaming { vals_gaming } else { vals_idle };
    for (key, val) in vals {
        for base in [
            "/sys/module/adpf/parameters",
            "/sys/kernel/adpf",
            "/sys/devices/system/cpu/adpf",
            "/sys/class/power/adpf",
        ] {
            write(&format!("{base}/{key}"), val);
            // lowercase variant some kernels use
            write(&format!("{base}/{}", key.to_ascii_lowercase()), val);
        }
    }

    // UclampMin High/Low dynamic bounds for the PID loop
    // High=hard hardware power cap (85% gaming vs 100 idle), Low=allow downclock (10 idle, 35 gaming for menu/plane)
    let (high, low) = if is_gaming { ("85", "10") } else { ("100", "0") };
    for base in [
        "/sys/module/adpf/parameters",
        "/sys/kernel/adpf",
        "/sys/devices/system/cpu/adpf",
    ] {
        write(&format!("{base}/UclampMin_High"), high);
        write(&format!("{base}/UclampMin_Low"), low);
        write(&format!("{base}/uclamp_min_high"), high);
        write(&format!("{base}/uclamp_min_low"), low);
    }

    // Per-policy ADPF target time / stale time if exposed
    // TargetTime 8ms gaming (120Hz frame budget) vs 16ms idle; Stale 30ms
    for policy in ["policy0", "policy3", "policy7"] {
        let base = format!("/sys/devices/system/cpu/cpufreq/{policy}/adpf");
        write(&format!("{base}/target_time_us"), if is_gaming { "8000" } else { "16000" });
        write(&format!("{base}/stale_time_ms"), if is_gaming { "30" } else { "50" });
        write(&format!("{base}/enabled"), if is_gaming { "1" } else { "0" });
    }

    // Fallback: vendor powerhint ADPF JSON override via setprop (no-op if not present)
    if is_gaming {
        let _ = std::process::Command::new("setprop")
            .args(["vendor.powerhal.adpf.gaming", "1"])
            .output();
    } else {
        let _ = std::process::Command::new("setprop")
            .args(["vendor.powerhal.adpf.gaming", "0"])
            .output();
    }
}
