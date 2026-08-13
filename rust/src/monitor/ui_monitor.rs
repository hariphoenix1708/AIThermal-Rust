use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::daemon::RuntimeTask;
use crate::runtime_context::RuntimeContext;
use anyhow::Result;

const SAMPLE_EVERY: Duration = Duration::from_secs(5);
const REFRESH_CACHE_EVERY: Duration = Duration::from_secs(30);

const TRACKED_PROCS: &[&str] = &[
    "surfaceflinger",
    "system_server",
    "com.android.systemui",
    "com.xiaomi.home",
    "com.miui.home",
    "com.xiaomi.joyose",
    "perfd",
    "mi_thermald",
    "thermalai-daemon",
];

#[derive(Clone, Copy, Default)]
struct ProcCpu {
    ticks: u64,
    system_total: u64,
}

struct GfxSummary {
    frames: u64,
    janky: u64,
    p50_ms: f32,
    p90_ms: f32,
    missed_vsync: u64,
    slow_ui: u64,
}

impl GfxSummary {
    fn jank_pct(&self) -> f32 {
        if self.frames == 0 {
            return 0.0;
        }
        self.janky as f32 * 100.0 / self.frames as f32
    }

    fn to_compact(&self) -> String {
        format!(
            "frames={} jank={}({:.2}%) p50={:.1}ms p90={:.1}ms missVsync={} slowUI={}",
            self.frames,
            self.janky,
            self.jank_pct(),
            self.p50_ms,
            self.p90_ms,
            self.missed_vsync,
            self.slow_ui
        )
    }
}

#[derive(Default)]
pub struct UiMonitor {
    last_sample: Option<Instant>,
    last_refresh_sample: Option<Instant>,
    cached_refresh_hz: Option<f32>,
    proc_prev: HashMap<String, ProcCpu>,
}

impl UiMonitor {
    fn sample(&mut self, ctx: &RuntimeContext) {
        let policy = ctx.current_policy.as_deref().unwrap_or("none");

        let refresh_hz = self.refresh_hz_cached();
        let top = read_top_window();
        let gfx = top.as_deref().and_then(read_gfxinfo_summary);
        let procs = self.proc_cpu_percent();
        let anim = read_anim_scales();
        let scaling = read_scaling_state();

        let refresh_str = refresh_hz
            .map(|h| format!("{:.0}", h))
            .unwrap_or_else(|| "?".to_string());
        let top_str = top
            .as_deref()
            .map(|p| p.split('/').next().unwrap_or(p))
            .unwrap_or("none");
        let gfx_str = gfx
            .as_ref()
            .map(|g| g.to_compact())
            .unwrap_or_else(|| "n/a".to_string());
        let proc_str = procs.join(" ");
        let anim_str = format!("w={} t={} a={}", anim[0], anim[1], anim[2]);
        let scaling_str = scaling.join(" ");

        tracing::info!(
            target: "ui",
            "policy={} refresh={}Hz top={} anim[{}] cpu[{}] freq[{}] gfx[{}]",
            policy,
            refresh_str,
            top_str,
            anim_str,
            proc_str,
            scaling_str,
            gfx_str
        );

        if let Some(g) = &gfx
            && (g.jank_pct() > 10.0 || g.p90_ms > 16.7 || g.slow_ui > 0)
        {
            tracing::warn!(
                target: "ui",
                "UI jank detected policy={} top={} {}",
                policy,
                top_str,
                g.to_compact()
            );
        }

        for a in &anim {
            if a.trim() == "0.0" {
                tracing::warn!(target: "ui", "animation scale is 0.0 (scaled-off animations can read as stutter)");
            }
        }
    }

    fn refresh_hz_cached(&mut self) -> Option<f32> {
        let now = Instant::now();
        let stale = self
            .last_refresh_sample
            .map(|t| now.duration_since(t) >= REFRESH_CACHE_EVERY)
            .unwrap_or(true);
        if stale {
            let hz = read_display_refresh_hz();
            if hz.is_some() {
                self.cached_refresh_hz = hz;
            }
            self.last_refresh_sample = Some(now);
        }
        self.cached_refresh_hz
    }

    fn proc_cpu_percent(&mut self) -> Vec<String> {
        let sys_total = system_total_ticks();
        let mut out = Vec::new();
        let mut next: HashMap<String, ProcCpu> = HashMap::new();

        for name in TRACKED_PROCS {
            let ticks = resolve_proc_ticks(name);
            let curr = ProcCpu { ticks, system_total: sys_total };
            if let Some(prev) = self.proc_prev.get(*name) {
                let d_ticks = curr.ticks.saturating_sub(prev.ticks);
                let d_total = curr.system_total.saturating_sub(prev.system_total);
                let pct = if d_total > 0 {
                    d_ticks as f32 * 100.0 / d_total as f32
                } else {
                    0.0
                };
                out.push(format!("{:.1}:{}", pct, name));
            }
            next.insert((*name).to_string(), curr);
        }

        self.proc_prev = next;
        out
    }
}

impl RuntimeTask for UiMonitor {
    fn execute(&mut self, ctx: &mut RuntimeContext) -> Result<()> {
        if crate::hardware::display::is_screen_off() {
            self.last_sample = None;
            self.last_refresh_sample = None;
            self.proc_prev.clear();
            return Ok(());
        }

        let now = Instant::now();
        if self
            .last_sample
            .is_none_or(|t| now.duration_since(t) < SAMPLE_EVERY)
        {
            return Ok(());
        }
        self.last_sample = Some(now);
        self.sample(ctx);
        Ok(())
    }
}

fn resolve_proc_ticks(name: &str) -> u64 {
    let mut total: u64 = 0;
    if let Ok(out) = Command::new("pidof").arg(name).output()
        && out.status.success()
    {
        let pid_str = String::from_utf8_lossy(&out.stdout);
        for tok in pid_str.split_whitespace() {
            if let Ok(pid) = tok.parse::<u32>() {
                total = total.saturating_add(read_proc_ticks(pid));
            }
        }
    }
    total
}

fn read_proc_ticks(pid: u32) -> u64 {
    let s = match std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let Some((_, after)) = s.rsplit_once(')') else {
        return 0;
    };
    let fields: Vec<&str> = after.split_whitespace().collect();
    let utime = fields.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let stime = fields.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    utime.saturating_add(stime)
}

fn system_total_ticks() -> u64 {
    crate::monitor::load_sampler::read_cpu_stat()
        .values()
        .map(|l| l.total)
        .sum()
}

fn read_display_refresh_hz() -> Option<f32> {
    let out = Command::new("dumpsys").arg("display").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if !line.contains("refreshRate") {
            continue;
        }
        if let Some(hz) = extract_hz(line) {
            return Some(hz);
        }
    }
    None
}

fn extract_hz(line: &str) -> Option<f32> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        if t.contains("refreshRate") {
            for next in toks.iter().skip(i + 1) {
                let digits: String = next
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                if !digits.is_empty() {
                    return digits.parse().ok();
                }
            }
        }
    }
    None
}

fn read_top_window() -> Option<String> {
    let out = Command::new("dumpsys").arg("window").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().find_map(|line| {
        if !line.contains("mCurrentFocus") {
            return None;
        }
        let idx = line.find("u0 ")?;
        let rest = &line[idx + 3..];
        let comp: String = rest.chars().take_while(|c| *c != '}').collect();
        let comp = comp.trim();
        if comp.is_empty() {
            None
        } else {
            Some(comp.to_string())
        }
    })
}

fn read_gfxinfo_summary(pkg: &str) -> Option<GfxSummary> {
    let out = Command::new("dumpsys")
        .arg("gfxinfo")
        .arg(pkg)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);

    let mut frames: u64 = 0;
    let mut janky: u64 = 0;
    let mut p50_ms: f32 = 0.0;
    let mut p90_ms: f32 = 0.0;
    let mut missed_vsync: u64 = 0;
    let mut slow_ui: u64 = 0;

    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Total frames rendered:") {
            frames = rest.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(rest) = t.strip_prefix("Janky frames:") {
            janky = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0);
        } else if let Some(rest) = t.strip_prefix("50th percentile:") {
            p50_ms = parse_ms(rest).unwrap_or(0.0);
        } else if let Some(rest) = t.strip_prefix("90th percentile:") {
            p90_ms = parse_ms(rest).unwrap_or(0.0);
        } else if let Some(rest) = t.strip_prefix("Number Missed Vsync:") {
            missed_vsync = rest.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(rest) = t.strip_prefix("Number Slow UI thread:") {
            slow_ui = rest.trim().parse::<u64>().unwrap_or(0);
        }
    }

    if frames == 0 && janky == 0 && p50_ms == 0.0 && p90_ms == 0.0 {
        return None;
    }

    Some(GfxSummary {
        frames,
        janky,
        p50_ms,
        p90_ms,
        missed_vsync,
        slow_ui,
    })
}

fn parse_ms(rest: &str) -> Option<f32> {
    let digits: String = rest
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn read_anim_scales() -> [String; 3] {
    let names = [
        "window_animation_scale",
        "transition_animation_scale",
        "animator_duration_scale",
    ];
    let mut out = [String::new(), String::new(), String::new()];
    for (i, name) in names.iter().enumerate() {
        let v = Command::new("settings")
            .arg("get")
            .arg("global")
            .arg(name)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        out[i] = v;
    }
    out
}

fn read_scaling_state() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
        let mut policies: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("policy"))
            .map(|e| e.path().to_string_lossy().to_string())
            .collect();
        policies.sort();
        for p in policies {
            let gov = std::fs::read_to_string(format!("{}/scaling_governor", p))
                .unwrap_or_default();
            let cur = std::fs::read_to_string(format!("{}/scaling_cur_freq", p))
                .unwrap_or_default();
            let mhz = cur
                .trim()
                .parse::<u64>()
                .ok()
                .map(|h| h / 1000)
                .map(|m| m.to_string())
                .unwrap_or_default();
            let name = std::path::Path::new(&p)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            out.push(format!("{}:{}:{}MHz", name, gov.trim(), mhz));
        }
    }
    out
}
