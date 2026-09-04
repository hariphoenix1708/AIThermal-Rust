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
    pub p50_frame_ns: u64,   // median total frame duration
    pub p90_frame_ns: u64,   // 90th percentile total frame duration
    pub worst_frame_ns: u64,
    pub max_consecutive_jank: usize, // longest streak of consecutive janky frames
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

// Target frame budget for jank classification. Dynamically detected from
// the display's actual current refresh rate. Falls back to 60fps if
// detection fails.
const DEFAULT_FRAME_BUDGET_NS: u64 = 16_666_667;

/// Detect the display's actual refresh rate from dumpsys.
/// Returns the frame budget in nanoseconds.
fn detect_frame_budget_ns() -> u64 {
    // Try to read from dumpsys display
    if let Ok(output) = std::process::Command::new("dumpsys")
        .arg("display")
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        // Look for "mRefreshRate=" or "refreshRate=" pattern
        for line in text.lines() {
            let line = line.trim();
            if let Some(idx) = line.find("mRefreshRate=") {
                let val_str = &line[idx + 13..];
                if let Some(end) = val_str.find(|c: char| !c.is_ascii_digit() && c != '.') {
                    let val_str = &val_str[..end];
                    if let Ok(hz) = val_str.parse::<f64>()
                        && hz > 0.0 && hz <= 240.0
                    {
                        let ns = (1_000_000_000.0 / hz) as u64;
                        tracing::debug!("Detected display refresh rate: {}Hz (budget={}ns)", hz, ns);
                        return ns;
                    }
                }
            }
            if let Some(idx) = line.find("refreshRate=") {
                let val_str = &line[idx + 12..];
                if let Some(end) = val_str.find(|c: char| !c.is_ascii_digit() && c != '.') {
                    let val_str = &val_str[..end];
                    if let Ok(hz) = val_str.parse::<f64>()
                        && hz > 0.0 && hz <= 240.0
                    {
                        let ns = (1_000_000_000.0 / hz) as u64;
                        tracing::debug!("Detected display refresh rate: {}Hz (budget={}ns)", hz, ns);
                        return ns;
                    }
                }
            }
        }
    }

    // Fallback: try to read from SurfaceFlinger
    if let Ok(output) = std::process::Command::new("dumpsys")
        .arg("SurfaceFlinger")
        .arg("--display-id")
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if line.contains("refresh") && line.contains("Hz") {
                // Try to extract number before "Hz"
                if let Some(idx) = line.rfind(|c: char| c.is_ascii_digit()) {
                    let start = line[..=idx].rfind(|c: char| !c.is_ascii_digit() && c != '.')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let val_str = &line[start..=idx];
                    if let Ok(hz) = val_str.parse::<f64>()
                        && hz > 0.0 && hz <= 240.0
                    {
                        let ns = (1_000_000_000.0 / hz) as u64;
                        tracing::debug!("Detected display refresh rate via SF: {}Hz (budget={}ns)", hz, ns);
                        return ns;
                    }
                }
            }
        }
    }

    tracing::debug!("Could not detect display refresh rate, using 60fps default");
    DEFAULT_FRAME_BUDGET_NS
}

fn compute_stats_from_durations(durations: Vec<u64>, frame_budget_ns: u64) -> Option<FrameStats> {
    if durations.is_empty() {
        return None;
    }

    // Keep original order for consecutive-jank calculation.
    let mut sorted = durations.clone();
    sorted.sort_unstable();
    let sample_count = sorted.len();
    let janky_frames = sorted.iter().filter(|&&d| d > frame_budget_ns).count();
    let p50_idx = sample_count / 2;
    let p50_frame_ns = sorted[p50_idx];
    let p90_idx = ((sample_count as f32) * 0.9) as usize;
    let p90_frame_ns = sorted[p90_idx.min(sample_count - 1)];
    let worst_frame_ns = *sorted.last().unwrap();

    // Max consecutive jank: scan in original presentation order.
    let mut max_streak = 0usize;
    let mut cur_streak = 0usize;
    for &d in &durations {
        if d > frame_budget_ns {
            cur_streak += 1;
            max_streak = max_streak.max(cur_streak);
        } else {
            cur_streak = 0;
        }
    }

    Some(FrameStats {
        sample_count,
        janky_frames,
        p50_frame_ns,
        p90_frame_ns,
        worst_frame_ns,
        max_consecutive_jank: max_streak,
        captured_at: Some(Instant::now()),
    })
}

fn parse_latency_output(text: &str, frame_budget_ns: u64) -> Option<FrameStats> {
    let mut durations: Vec<u64> = Vec::new();
    let mut lines = text.lines();

    // First line is usually refresh period, skip or use
    lines.next()?;

    for line in lines {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 3 {
            // INTENDED_VSYNC (col 0), VSYNC (col 1), FRAME_COMPLETED (col 2)
            if let (Ok(iv), Ok(fc)) = (fields[0].parse::<u64>(), fields[2].parse::<u64>())
                && fc > iv && iv > 0 && fc < u64::MAX
            {
                durations.push(fc - iv);
            }
        }
    }

    compute_stats_from_durations(durations, frame_budget_ns)
}

fn try_gfxinfo_latency(package: &str, frame_budget_ns: u64) -> Option<FrameStats> {
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
    parse_latency_output(&text, frame_budget_ns)
}

/// Discover the actual game rendering layer from SurfaceFlinger --list.
/// CoD Mobile and other games may use layer names that don't match the
/// standard SurfaceView[<pkg>] format. This scans the layer list for
/// the best candidate.
fn discover_game_layer(package: &str) -> Option<String> {
    let output = Command::new("dumpsys")
        .arg("SurfaceFlinger")
        .arg("--list")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let pkg_short = package.rsplit('.').next().unwrap_or(package);

    // Priority 1: Exact SurfaceView match
    let sv_exact = format!("SurfaceView[{}]", package);
    for line in text.lines() {
        let line = line.trim();
        if line == sv_exact {
            return Some(line.to_string());
        }
    }

    // Priority 2: SurfaceView containing the package name (partial match)
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("SurfaceView[") && line.contains(pkg_short) {
            return Some(line.to_string());
        }
    }

    // Priority 3: Any layer containing the package name
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() && line.contains(package) {
            return Some(line.to_string());
        }
    }

    // Priority 4: Layer containing the short package name (e.g. "callofduty")
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() && line.contains(pkg_short) && !line.contains("Animation") {
            return Some(line.to_string());
        }
    }

    None
}

fn try_surfaceflinger_latency(package: &str, frame_budget_ns: u64) -> Option<FrameStats> {
    // First try the standard SurfaceView[<pkg>] format
    let std_layer = format!("SurfaceView[{}]", package);
    let output = Command::new("dumpsys")
        .arg("SurfaceFlinger")
        .arg("--latency")
        .arg(&std_layer)
        .output()
        .ok()?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        // Check if we got more than just the budget line (1 line = no real data)
        let line_count = text.lines().filter(|l| !l.trim().is_empty()).count();
        if line_count > 1 {
            if let Some(stats) = parse_latency_output(&text, frame_budget_ns) {
                return Some(stats);
            }
        }
    }

    // Standard format failed — discover the actual game layer
    if let Some(layer_name) = discover_game_layer(package) {
        let output = Command::new("dumpsys")
            .arg("SurfaceFlinger")
            .arg("--latency")
            .arg(&layer_name)
            .output()
            .ok()?;

        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let line_count = text.lines().filter(|l| !l.trim().is_empty()).count();
            if line_count > 1 {
                tracing::debug!("Using discovered SF layer: {} ({} lines)", layer_name, line_count);
                return parse_latency_output(&text, frame_budget_ns);
            }
        }
    }

    None
}

fn try_surfaceflinger_latency_fallback(package: &str, frame_budget_ns: u64) -> Option<FrameStats> {
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
    // Check for budget-only output (1 line = no real frame data)
    let line_count = text.lines().filter(|l| !l.trim().is_empty()).count();
    if line_count > 1 {
        parse_latency_output(&text, frame_budget_ns)
    } else {
        None
    }
}

pub fn sample_frame_stats(package: &str) -> Option<FrameStats> {
    // Detect display refresh rate each sample cycle to handle
    // dynamic refresh rate changes (e.g., 60Hz idle -> 120Hz gaming).
    let frame_budget_ns = detect_frame_budget_ns();

    let result = (|| {
        // Try #1: FrameTimeline / gfxinfo latency
        if let Some(stats) = try_gfxinfo_latency(package, frame_budget_ns) {
            return Some(stats);
        }

        // Try #2: SurfaceFlinger latency SurfaceView (usually where game renders)
        if let Some(stats) = try_surfaceflinger_latency(package, frame_budget_ns) {
            return Some(stats);
        }

        // Try #3: SurfaceFlinger latency base package
        if let Some(stats) = try_surfaceflinger_latency_fallback(package, frame_budget_ns) {
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
            return parse_framestats(&text, frame_budget_ns);
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

    // Keep original order for consecutive-jank calculation.
    let original_order = durations.clone();
    durations.sort_unstable();
    let sample_count = durations.len();
    let janky_frames = durations.iter().filter(|&&d| d > frame_budget_ns).count();
    let p50_idx = sample_count / 2;
    let p50_frame_ns = durations[p50_idx];
    let p90_idx = ((sample_count as f32) * 0.9) as usize;
    let p90_frame_ns = durations[p90_idx.min(sample_count - 1)];
    let worst_frame_ns = *durations.last().unwrap();

    // Max consecutive jank in original presentation order.
    let mut max_streak = 0usize;
    let mut cur_streak = 0usize;
    for &d in &original_order {
        if d > frame_budget_ns {
            cur_streak += 1;
            max_streak = max_streak.max(cur_streak);
        } else {
            cur_streak = 0;
        }
    }

    Some(FrameStats {
        sample_count,
        janky_frames,
        p50_frame_ns,
        p90_frame_ns,
        worst_frame_ns,
        max_consecutive_jank: max_streak,
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
        let latest = Arc::new(Mutex::new(None::<FrameStats>));
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
                            // Only refresh the slot when values actually
                            // changed.  If dumpsys returns stale / frozen
                            // counters we leave the OLD slot in place so
                            // its original captured_at ages normally and
                            // the existing 12s staleness guard fires.
                            let unchanged = matches!(
                                (&*slot, &result),
                                (Some(prev), Some(new))
                                    if prev.sample_count == new.sample_count
                                        && prev.janky_frames == new.janky_frames
                                        && prev.p90_frame_ns == new.p90_frame_ns
                                        && prev.worst_frame_ns == new.worst_frame_ns
                                        && prev.max_consecutive_jank == new.max_consecutive_jank
                            );
                            // Bypass unchanged guard when frame count is suspiciously low.
                            // At 120Hz any game produces 100+ frames per 5s sample.
                            // A tiny batch (e.g. 5 frames) means we captured a cold-start
                            // or loading snapshot — always overwrite with fresh data.
                            let is_tiny_batch = result.as_ref().map_or(false, |r| r.sample_count < 15);
                            if !unchanged || is_tiny_batch {
                                *slot = result;
                            }
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
