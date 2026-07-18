use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    pub sample_count: usize,
    pub janky_frames: usize,       // frames that missed their deadline
    pub p90_frame_ns: u64,         // 90th percentile total frame duration
    pub worst_frame_ns: u64,
    pub sampled_at: Option<Instant>,
}

impl FrameStats {
    pub fn jank_ratio(&self) -> f32 {
        if self.sample_count == 0 { return 0.0; }
        self.janky_frames as f32 / self.sample_count as f32
    }
}

// Target frame budget for jank classification. 16_666_667ns = 60fps budget.
// This should ideally be derived from the display's actual current refresh
// rate (see Step 4's optional refinement) rather than hardcoded, but 60fps is
// a safe, conservative default starting point.
const DEFAULT_FRAME_BUDGET_NS: u64 = 16_666_667;

pub fn sample_frame_stats(package: &str) -> Option<FrameStats> {
    let output = Command::new("dumpsys")
        .arg("gfxinfo")
        .arg(package)
        .arg("framestats")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_framestats(&text, DEFAULT_FRAME_BUDGET_NS)
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
        if line.is_empty() || !line.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            continue; // skip headers/labels/blank lines
        }
        let fields: Vec<&str> = line.split(',').filter(|f| !f.is_empty()).collect();
        if fields.len() < 14 {
            continue; // not a data row in the expected format
        }
        // fields[1] = INTENDED_VSYNC, fields[13] (or last) = FRAME_COMPLETED in
        // most Android versions' framestats layout - VERIFY against a real
        // captured sample from this device/Android version before trusting
        // fixed column indices, since Android has changed this format across
        // versions. A safer, version-tolerant approach: parse the first and
        // last numeric fields on the line as intended_vsync and frame_completed
        // respectively, since framestats rows are always ordered chronologically
        // within each row's own timestamp fields regardless of exact column
        // count differences between versions.
        let nums: Vec<u64> = fields.iter().filter_map(|f| f.trim().parse::<u64>().ok()).collect();
        if nums.len() < 2 { continue; }
        let intended_vsync = nums[1.min(nums.len() - 1)];
        let frame_completed = *nums.last().unwrap();
        if frame_completed > intended_vsync {
            durations.push(frame_completed - intended_vsync);
        }
    }

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
        sampled_at: Some(Instant::now()),
    })
}
