use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

static LAST_PARSE_OK: AtomicBool = AtomicBool::new(true);

pub fn last_parse_ok() -> bool {
    LAST_PARSE_OK.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    pub sample_count: usize,
    pub janky_frames: usize, // frames that missed their deadline
    pub p90_frame_ns: u64,   // 90th percentile total frame duration
    pub worst_frame_ns: u64,
    pub captured_at: Option<Instant>,
}

impl FrameStats {
    pub fn jank_ratio(&self) -> f32 {
        if self.sample_count == 0 {
            return 0.0;
        }
        self.janky_frames as f32 / self.sample_count as f32
    }

    pub fn frame_count(&self) -> usize {
        self.sample_count
    }
}

// Target frame budget for jank classification. 16_666_667ns = 60fps budget.
// This should ideally be derived from the display's actual current refresh
// rate (see Step 4's optional refinement) rather than hardcoded, but 60fps is
// a safe, conservative default starting point.
const DEFAULT_FRAME_BUDGET_NS: u64 = 16_666_667;

fn compute_stats_from_durations(mut durations: Vec<u64>, frame_budget_ns: u64) -> Option<FrameStats> {
    if durations.is_empty() {
        return None;
    }

    durations.sort_unstable();
    let sample_count = durations.len();
    let janky_frames = durations.iter().filter(|&&d| d > frame_budget_ns).count();
    let p90_idx = ((sample_count as f32) * 0.9) as usize;
    let p90_frame_ns = durations[p90_idx.min(sample_count - 1)];
    let worst_frame_ns = *durations.last().unwrap();

    Some(FrameStats {
        sample_count,
        janky_frames,
        p90_frame_ns,
        worst_frame_ns,
        captured_at: Some(Instant::now()),
    })
}

fn parse_latency_output(text: &str, frame_budget_ns: u64) -> Option<FrameStats> {
    let mut durations: Vec<u64> = Vec::new();
    let mut lines = text.lines();

    // First line is usually refresh period, skip or use
    lines.next()?;

    for line in lines {
        let fields: Vec<&str> = line.trim().split_whitespace().collect();
        if fields.len() >= 3 {
            // INTENDED_VSYNC (col 0), VSYNC (col 1), FRAME_COMPLETED (col 2)
            if let (Ok(iv), Ok(fc)) = (fields[0].parse::<u64>(), fields[2].parse::<u64>()) {
                if fc > iv && iv > 0 && fc < u64::MAX {
                    durations.push(fc - iv);
                }
            }
        }
    }

    compute_stats_from_durations(durations, frame_budget_ns)
}

fn try_gfxinfo_latency(package: &str) -> Option<FrameStats> {
    let output = Command::new("dumpsys")
        .arg("gfxinfo")
        .arg(package)
        .arg("--latency")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_latency_output(&text, DEFAULT_FRAME_BUDGET_NS)
}

fn try_surfaceflinger_latency(package: &str) -> Option<FrameStats> {
    let output = Command::new("dumpsys")
        .arg("SurfaceFlinger")
        .arg("--latency")
        .arg(format!("SurfaceView[{}]", package))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_latency_output(&text, DEFAULT_FRAME_BUDGET_NS)
}

fn try_surfaceflinger_latency_fallback(package: &str) -> Option<FrameStats> {
    let output = Command::new("dumpsys")
        .arg("SurfaceFlinger")
        .arg("--latency")
        .arg(package)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_latency_output(&text, DEFAULT_FRAME_BUDGET_NS)
}

pub fn sample_frame_stats(package: &str) -> Option<FrameStats> {
    let result = (|| {
        // Try #1: FrameTimeline / gfxinfo latency
        if let Some(stats) = try_gfxinfo_latency(package) {
            return Some(stats);
        }

        // Try #2: SurfaceFlinger latency SurfaceView (usually where game renders)
        if let Some(stats) = try_surfaceflinger_latency(package) {
            return Some(stats);
        }

        // Try #3: SurfaceFlinger latency base package
        if let Some(stats) = try_surfaceflinger_latency_fallback(package) {
            return Some(stats);
        }

        // Try #4: Existing framestats parser (gfxinfo)
        let output = Command::new("dumpsys")
            .arg("gfxinfo")
            .arg(package)
            .arg("framestats")
            .output()
            .ok()?;

        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            return parse_framestats(&text, DEFAULT_FRAME_BUDGET_NS);
        }

        None
    })();

    LAST_PARSE_OK.store(result.is_some(), Ordering::Relaxed);
    result
}

fn parse_framestats(text: &str, frame_budget_ns: u64) -> Option<FrameStats> {
    // framestats CSV rows: each line is one frame's timings. The columns of
    // interest (per Android's documented framestats format) are:
    //   column 1 = INTENDED_VSYNC (ns)
    //   last column commonly used for total duration = FRAME_COMPLETED - INTENDED_VSYNC
    // Only lines that are pure comma-separated numeric data should be parsed;
    // header/section lines should be skipped.
    let mut durations: Vec<u64> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty()
            || !line
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        {
            continue; // skip headers/labels/blank lines
        }
        let fields: Vec<&str> = line.split(',').filter(|f| !f.is_empty()).collect();
        if fields.len() < 14 {
            continue; // not a data row in the expected format
        }
        // Column indices confirmed against 5 live dumpsys gfxinfo
        // framestats captures from this device (com.activision.callofduty.shooter).
        // IntendedVsync and FrameCompleted confirmed via the PROFILEDATA
        // header row itself, not inferred/guessed.
        const INTENDED_VSYNC_COL: usize = 2;
        const FRAME_COMPLETED_COL: usize = 17;
        if fields.len() <= FRAME_COMPLETED_COL {
            continue; // row too short for this layout, skip safely
        }
        let intended_vsync = fields[INTENDED_VSYNC_COL].trim().parse::<u64>().ok();
        let frame_completed = fields[FRAME_COMPLETED_COL].trim().parse::<u64>().ok();

        let (Some(iv), Some(fc)) = (intended_vsync, frame_completed) else { continue; };
        if fc <= iv { continue; }

        durations.push(fc - iv);
    }

    if durations.is_empty() {
        tracing::debug!("framestats parse yielded 0 durations — dumpsys output format may not match expected layout on this Android build");
        return None;
    }

    durations.sort_unstable();
    let sample_count = durations.len();
    let janky_frames = durations.iter().filter(|&&d| d > frame_budget_ns).count();
    let p90_idx = ((sample_count as f32) * 0.9) as usize;
    let p90_frame_ns = durations[p90_idx.min(sample_count - 1)];
    let worst_frame_ns = *durations.last().unwrap();

    Some(FrameStats {
        sample_count,
        janky_frames,
        p90_frame_ns,
        worst_frame_ns,
        captured_at: Some(Instant::now()),
    })
}

pub struct BackgroundFrameSampler {
    latest: Arc<Mutex<Option<FrameStats>>>,
    package: Arc<Mutex<Option<String>>>,
    running: Arc<AtomicBool>,
}

impl Default for BackgroundFrameSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundFrameSampler {
    pub fn new() -> Self {
        let latest = Arc::new(Mutex::new(None));
        let package = Arc::new(Mutex::new(None::<String>));
        let running = Arc::new(AtomicBool::new(true));

        let latest_thread = latest.clone();
        let package_thread = package.clone();
        let running_thread = running.clone();

        std::thread::spawn(move || {
            while running_thread.load(Ordering::SeqCst) {
                let pkg_opt = package_thread.lock().ok().and_then(|p| p.clone());
                let sleep_ms = match pkg_opt {
                    Some(pkg) => {
                        let result = sample_frame_stats(&pkg);
                        if let Ok(mut slot) = latest_thread.lock() {
                            *slot = result;
                        }
                        // Spawning up to 4 `dumpsys` processes per cycle while
                        // a game runs is heavy; every 5s is enough signal for
                        // the adaptive governor without stealing CPU from the
                        // game itself.
                        5000
                    }
                    None => {
                        if let Ok(mut slot) = latest_thread.lock() {
                            *slot = None;
                        }
                        10_000
                    }
                };
                std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            }
        });

        Self {
            latest,
            package,
            running,
        }
    }

    /// Called from the main tick loop (cheap, non-blocking - just updates
    /// which package the background thread should be sampling).
    pub fn set_target_package(&self, pkg: Option<String>) {
        if let Ok(mut slot) = self.package.lock() {
            *slot = pkg;
        }
    }

    /// Called from the main tick loop (cheap, non-blocking - just reads
    /// whatever the background thread most recently produced, if anything).
    /// Note: Because the background thread parses on a fixed 1.5s cadence,
    /// multiple sequential reads from different parts of the orchestrator
    /// during the same tick may see different snapshots (or different cache ages)
    /// if the background thread writes an update in between them. This is expected.
    pub fn latest_stats(&self) -> Option<FrameStats> {
        self.latest.lock().ok().and_then(|s| s.clone())
    }
}

impl Drop for BackgroundFrameSampler {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
