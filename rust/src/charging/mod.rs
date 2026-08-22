use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChargeState {
    Disconnected,
    Normal,
    UnderLoad,
    ThermalThrottle,
    Emergency,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChargeMode {
    Adaptive,
    Urgent,
    BatteryCare,
    UnderLoad,
    MaxSpeed,
}

#[derive(Clone)]
pub struct ChargingInputs {
    pub battery_temp: i32,
    pub charger_temp: i32,
    pub usb_temp: i32,
    pub pmic_temp: i32,
    pub soc: u8,
    pub is_plugged: bool,
    pub plug_state_reliable: bool,
    pub is_gaming: bool,
    pub screen_off: bool,
    pub gpu_load: u32,
    pub urgent: bool,
    pub seconds_since_plugged: u64,
    pub charger_id: String,
    pub current_now_ua: Option<i64>,
    pub voltage_now_uv: Option<i64>,
    pub charge_counter_uah: Option<i64>,
    pub composite_temp: i32,
}

pub struct ChargingEngine {
    pub limit_nodes: Vec<String>,
    pub active_limit_ma: i64,
    pub previous_target: i64,
    pub current_state: ChargeState,
    pub learned_stable_current: i64,
    pub session_start_soc: u8,
    pub taper_started_at: Option<std::time::Instant>,
    pub re_enforce_at: std::time::Instant,
    pub charge_mode: ChargeMode,
    pub session_peak_temp: i32,
    pub session_start_time: Option<std::time::SystemTime>,
    pub session_peak_usb_temp: i32,
    pub session_peak_pmic_temp: i32,
    pub thermal_reduction_count: u32,
    pub recovery_count: u32,
    pub total_current_ua_samples: i64,
    pub total_power_uw_samples: i64,
    pub sample_count: u32,
    pub consecutive_failures: u32,
    pub last_known_good_ma: Option<i64>,
    pub rejected_ceiling: Option<i64>,
    pub last_apply_attempt: Option<std::time::Instant>,
    pub limit_write_failure_count: u32,
    pub limit_write_disabled: bool,
    pub no_nodes_warned: bool,
    pub voter_nodes: Vec<String>,
    pub voter_dump_done: bool,
    pub forced_mode: Option<ChargeMode>,
    pub thermal_temp_warm: i32,
    pub thermal_temp_hot: i32,
}

impl ChargingEngine {
    /// Returns a multiplier in [0.85, 1.00] applied to the fast-charge
    /// current cap based on battery age. 0 cycles -> 1.00 (no change).
    /// 300 cycles -> 0.97. 600 -> 0.93. 900 -> 0.89. >=1200 -> 0.85.
    fn cycle_taper_factor(cycle_count: u64) -> f32 {
        match cycle_count {
            0..=200 => 1.00,
            201..=400 => 0.97,
            401..=700 => 0.93,
            701..=1000 => 0.89,
            _ => 0.85,
        }
    }

    pub fn new(hw: &crate::hardware::HardwareProfile, thermal_temp_warm: i32, thermal_temp_hot: i32) -> Self {
        let limit_nodes = hw.charging_profile.current_limit_nodes.clone();

        Self {
            limit_nodes,
            active_limit_ma: 0,
            previous_target: 3000,
            current_state: ChargeState::Disconnected,
            learned_stable_current: 3000,
            session_start_soc: 0,
            taper_started_at: None,
            re_enforce_at: std::time::Instant::now(),
            charge_mode: ChargeMode::Adaptive,
            session_peak_temp: 0,
            session_start_time: None,
            session_peak_usb_temp: 0,
            session_peak_pmic_temp: 0,
            thermal_reduction_count: 0,
            recovery_count: 0,
            total_current_ua_samples: 0,
            total_power_uw_samples: 0,
            sample_count: 0,
            consecutive_failures: 0,
            last_known_good_ma: None,
            rejected_ceiling: None,
            last_apply_attempt: None,
            limit_write_failure_count: 0,
            limit_write_disabled: false,
            no_nodes_warned: false,
            voter_nodes: hw.charging_profile.voter_nodes.clone(),
            voter_dump_done: false,
            forced_mode: None,
            thermal_temp_warm,
            thermal_temp_hot,
        }
    }

    /// Reads (does NOT write) every discovered voter node plus a
    /// small set of read-only charger-state nodes and emits them
    /// to thermalai_charging.log. Called once per session on the
    /// Disconnected -> Normal transition.
    fn dump_charger_diagnostics(&mut self) {
        if self.voter_dump_done {
            return;
        }
        self.voter_dump_done = true;

        let read_only_probes = [
            "/sys/class/power_supply/usb/pd_active",
            "/sys/class/power_supply/usb/real_type",
            "/sys/class/power_supply/usb/typec_mode",
            "/sys/class/power_supply/usb/voltage_max",
            "/sys/class/power_supply/usb/voltage_now",
            "/sys/class/power_supply/usb/current_max",
            "/sys/class/power_supply/usb/input_current_now",
            "/sys/class/power_supply/battery/charge_type",
            "/sys/class/power_supply/battery/constant_charge_current_max",
            "/sys/class/power_supply/battery/voltage_now",
            "/sys/class/power_supply/battery/current_now",
            "/sys/class/power_supply/battery/status",
            "/sys/class/power_supply/battery/health",
            "/sys/class/qcom-battery/restrict_chg",
            "/sys/class/qcom-battery/restrict_cur",
            "/sys/class/qcom-battery/charging_enabled",
            "/sys/class/power_supply/battery/system_temp_level",
        ];

        tracing::info!(target: "charging", "----- charger diagnostic dump -----");
        for path in read_only_probes {
            if let Ok(v) = std::fs::read_to_string(path) {
                tracing::info!(target: "charging", "  {} = {}", path, v.trim());
            }
        }
        for node in &self.voter_nodes.clone() {
            if let Ok(v) = std::fs::read_to_string(node) {
                tracing::info!(target: "charging", "  {} = {}  (writable)", node, v.trim());
            }
        }
        for node in &self.voter_nodes.clone() {
            if node.ends_with("/restrict_cur")
                && let Ok(v) = std::fs::read_to_string(node)
                && let Ok(ua) = v.trim().parse::<i64>()
                && ua > 0
            {
                tracing::warn!(target: "charging",
                    "restrict_cur={}mA is set: this caps charge current regardless of the charger's negotiated contract. \
                     MaxSpeed/Urgent charging mode clears it to restore full speed.",
                    ua / 1000);
            }
        }
        // --- restrict_cur / restrict_chg analysis (even if not in voter_nodes) ---
        // These nodes control charge current on Qualcomm PMIC. restrict_chg=1
        // enforces restrict_cur; restrict_chg=0 disables the cap entirely.
        // They may not be in voter_nodes (detected as read-only by idempotent
        // probe), but writing a DIFFERENT value (like "0") can still succeed.
        if let Ok(v) = std::fs::read_to_string("/sys/class/qcom-battery/restrict_chg") {
            let val = v.trim();
            if val == "1" {
                // restrict_chg=1 means restrict_cur is being enforced
                if let Ok(cv) = std::fs::read_to_string("/sys/class/qcom-battery/restrict_cur")
                    && let Ok(ua) = cv.trim().parse::<i64>()
                    && ua > 0
                {
                    tracing::warn!(target: "charging",
                        "restrict_chg=1 ENFORCES restrict_cur={}mA — this is the root cause of slow charging. \
                         One-shot clear will attempt restrict_chg=0 + restrict_cur=0 at session start.",
                        ua / 1000);
                } else {
                    tracing::warn!(target: "charging",
                        "restrict_chg=1 but restrict_cur unreadable — current may be capped. \
                         One-shot clear will attempt restrict_chg=0 at session start.");
                }
            } else {
                tracing::info!(target: "charging",
                    "restrict_chg={} — current restriction is {}.",
                    val, if val == "0" { "disabled (good)" } else { "unknown state" });
            }
        }
        // Also check restrict_cur directly in case it's not in voter_nodes
        if !self.voter_nodes.iter().any(|n| n.ends_with("/restrict_cur"))
            && let Ok(v) = std::fs::read_to_string("/sys/class/qcom-battery/restrict_cur")
            && let Ok(ua) = v.trim().parse::<i64>()
            && ua > 0
        {
            tracing::warn!(target: "charging",
                "restrict_cur={}mA (read-only to idempotent probe): caps charge current. \
                 Will attempt one-shot write of 0 at session start.",
                ua / 1000);
        }

        // --- Voltage analysis ---
        if let Ok(v) = std::fs::read_to_string("/sys/class/power_supply/usb/voltage_max")
            && let Ok(v_max) = v.trim().parse::<i64>()
        {
            if v_max < 9_000_000 {
                tracing::warn!(target: "charging",
                    "Charger voltage_max={}mV — charger is NOT negotiating fast charge. \
                     Hardware limitation: charger/cable does not support QC/PD. \
                     Charging speed limited to ~900mA at 5V.",
                    v_max / 1000);
            } else {
                tracing::info!(target: "charging",
                    "Charger voltage_max={}mV — fast charge (QC/PD) is active.",
                    v_max / 1000);
            }
        }
        tracing::info!(target: "charging", "----- end diagnostic dump -----");
    }

    /// One-shot: clear restrict_chg and restrict_cur at session start.
    ///
    /// These nodes control the Qualcomm PMIC charge current limit.
    /// - `restrict_chg=1` + `restrict_cur=X` → current capped at X μA
    /// - `restrict_chg=0` → restriction disabled entirely
    ///
    /// Writing `restrict_chg` every tick causes SPMI bus contention with
    /// the display controller on SM8635, so we only do this once per
    /// charging session (at the Disconnected→Normal transition).
    ///
    /// The idempotent probe in `probe_charging()` reads the current value
    /// and writes it back. Some Xiaomi kernels reject idempotent writes
    /// (to reduce unnecessary SPMI traffic), falsely marking the node as
    /// read-only. Writing a DIFFERENT value ("0") usually succeeds.
    fn one_shot_clear_restrict(&self) {
        let restrict_chg_path = "/sys/class/qcom-battery/restrict_chg";
        let restrict_cur_path = "/sys/class/qcom-battery/restrict_cur";

        // Step 1: Clear restrict_chg first (disables enforcement)
        if Path::new(restrict_chg_path).exists()
            && let Ok(current) = fs::read_to_string(restrict_chg_path)
        {
            let val = current.trim();
            if val == "1" {
                match crate::sysfs::write_string(restrict_chg_path, "0") {
                    Ok(()) => tracing::info!(target: "charging",
                        "One-shot: restrict_chg 1 → 0 (disabled current restriction enforcement)"),
                    Err(e) => tracing::warn!(target: "charging",
                        "One-shot: restrict_chg write failed: {} — current may remain limited. \
                         If this persists, the SPMI bus may be contended; try a reboot.",
                        e),
                }
            } else if val == "0" {
                tracing::debug!(target: "charging", "restrict_chg already 0 (no enforcement)");
            } else {
                tracing::info!(target: "charging", "restrict_chg={} (unexpected value, attempting clear)", val);
                let _ = crate::sysfs::write_string(restrict_chg_path, "0");
            }
        }

        // Step 2: Clear restrict_cur (removes any residual cap)
        if Path::new(restrict_cur_path).exists()
            && let Ok(current) = fs::read_to_string(restrict_cur_path)
        {
            let val = current.trim();
            if let Ok(ua) = val.parse::<i64>() {
                if ua > 0 {
                    match crate::sysfs::write_string(restrict_cur_path, "0") {
                        Ok(()) => tracing::info!(target: "charging",
                            "One-shot: restrict_cur {} → 0 (cleared {}mA current cap)",
                            val, ua / 1000),
                        Err(e) => tracing::warn!(target: "charging",
                            "One-shot: restrict_cur write failed: {} — {}mA cap may remain. \
                             The node's idempotent probe failed but a direct write of 0 \
                             should work on most Qualcomm PMICs.",
                            e, ua / 1000),
                    }
                } else {
                    tracing::debug!(target: "charging", "restrict_cur already 0 (no cap)");
                }
            } else {
                tracing::info!(target: "charging",
                    "restrict_cur='{}' (non-numeric, attempting clear)", val);
                let _ = crate::sysfs::write_string(restrict_cur_path, "0");
            }
        }
    }

    /// Writes the correct voter state for the current ChargeMode.
    /// Idempotent — safe to call every tick; only writes when the
    /// desired value differs from the currently-read value.
    fn apply_voters_for_mode(&self, mode: &ChargeMode, target_ma: i64, composite_temp: i32) {
        // Desired state per mode.
        // (restrict_chg, restrict_cur_ua, input_suspend, night_charging)
        let (restrict, cur_ua, suspend, night) = match mode {
            ChargeMode::MaxSpeed | ChargeMode::Urgent => {
                // Clear any current restriction (restrict_cur=0 = no cap).
                // Xiaomi's qcom-battery can leave restrict_cur at 1 A even
                // with restrict_chg=0, silently capping a 3 A charger at
                // ~900 mA - exactly the "charging is slow" symptom seen on
                // the POCO F6 (peridot). MaxSpeed/Urgent must lift it.
                (Some("0"), Some(0), Some("0"), Some("0"))
            }
            ChargeMode::BatteryCare => {
                // Cap current at target_ma; but never below 500 mA and
                // never above 3000 mA in BatteryCare.
                let cap_ma = target_ma.clamp(500, 3000);
                // We leak the string via to_string() into a local; the
                // helper below only borrows for the write() call.
                (
                    Some("1"),
                    Some(cap_ma * 1000  ),
                    Some("0"),
                    Some("0"),
                )
            }
            ChargeMode::UnderLoad => {
                // Proactive thermal-aware charging during gaming:
                // reduce PMIC heat BEFORE the thermal policy escalates
                // to Powersave. This keeps composite below temp_hot and
                // prevents the cpuset-tight frame drops.
                //
                // restrict_cur acts as a hard PMIC current cap. The
                // input_current_limit (via limit_nodes) is the softer lever.
                // Together they reduce total PMIC heat dissipation.
                let restrict_cur_ua = if composite_temp >= self.thermal_temp_hot {
                    1_500_000  // 1.5A: aggressive thermal relief at temp_hot
                } else if composite_temp >= self.thermal_temp_warm {
                    2_500_000  // 2.5A: moderate reduction approaching temp_hot
                } else {
                    0          // no cap: full speed when cool
                };
                (Some("0"), Some(restrict_cur_ua), Some("0"), Some("0"))
            }
            ChargeMode::Adaptive => {
                // Neutral: don't fight HyperOS, just make sure charge
                // isn't accidentally suspended.
                (None, None, Some("0"), None)
            }
        };

        for node in &self.voter_nodes {
            let want: Option<String> = if node.ends_with("/restrict_chg") {
                restrict.map(str::to_string)
            } else if node.ends_with("/restrict_cur") {
                cur_ua.map(|v| v.to_string())
            } else if node.ends_with("/input_suspend") {
                suspend.map(str::to_string)
            } else if node.ends_with("/night_charging") {
                night.map(str::to_string)
            } else {
                None
            };

            let Some(want) = want else {
                continue;
            };
            let current = std::fs::read_to_string(node).unwrap_or_default();
            if current.trim() == want {
                continue;
            }

            match crate::sysfs::write_string(node, &want) {
                Ok(()) => tracing::info!(target: "charging",
                    "voter {} : {} -> {}", node, current.trim(), want),
                Err(e) => tracing::warn!(target: "charging",
                    "voter {} write to {} failed: {}", node, want, e),
            }
        }
    }

    pub fn release_voters_on_shutdown(&self) {
        for node in &self.voter_nodes {
            let default = if node.ends_with("/restrict_chg") {
                "0"
            } else if node.ends_with("/input_suspend") {
                "0"
            } else if node.ends_with("/night_charging") {
                "0"
            } else {
                continue;
            };
            let _ = crate::sysfs::write_string(node, default);
        }
        // Clear restrict nodes that may not be in voter_nodes (detected as
        // read-only by idempotent probe). Writing "0" removes any current
        // cap so HyperOS can manage charging normally after AIThermal exits.
        for path in [
            "/sys/class/qcom-battery/restrict_chg",
            "/sys/class/qcom-battery/restrict_cur",
        ] {
            if Path::new(path).exists() {
                let _ = crate::sysfs::write_string(path, "0");
            }
        }
    }

    fn check_overrides(
        inputs: &mut ChargingInputs,
        state_dir: &str,
        forced_mode: &mut Option<ChargeMode>,
    ) {
        let override_path = format!("{}/charging_mode.json", state_dir);
        *forced_mode = None;
        if let Ok(content) = fs::read_to_string(&override_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                let urgent = json
                    .get("urgent")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mode_str = json.get("mode").and_then(|v| v.as_str());
                if let Some(m) = mode_str {
                    if m == "MaxSpeed" {
                        *forced_mode = Some(ChargeMode::MaxSpeed);
                    } else if m == "BatteryCare" {
                        *forced_mode = Some(ChargeMode::BatteryCare);
                    }
                }
                let expires_at = json.get("expires_at").and_then(|v| v.as_u64());

                if urgent {
                    if let Some(exp) = expires_at {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        if now > exp {
                            let _ = fs::remove_file(&override_path);
                            inputs.urgent = false;
                            *forced_mode = None;
                        } else {
                            inputs.urgent = true;
                        }
                    } else {
                        inputs.urgent = true;
                    }
                } else {
                    inputs.urgent = false;
                }
            } else {
                inputs.urgent = content.contains("\"urgent\": true") || content.contains("Urgent");
            }
        }
    }

    fn select_charge_mode(inputs: &ChargingInputs, forced_mode: &Option<ChargeMode>) -> ChargeMode {
        // Never let MaxSpeed / Urgent stand if the battery is already hot.
        // 42°C is the point where Xiaomi's own kernel starts
        // negotiating down to 5V·3A anyway.
        let hot = inputs.battery_temp >= 42;

        let chosen = if !inputs.is_plugged && inputs.plug_state_reliable {
            ChargeMode::Adaptive
        } else if let Some(fm) = forced_mode {
            fm.clone()
        } else if inputs.urgent {
            ChargeMode::Urgent
        } else if inputs.is_gaming {
            ChargeMode::UnderLoad
        } else if inputs.screen_off && inputs.soc > 80 {
            ChargeMode::BatteryCare
        } else if inputs.screen_off && inputs.soc < 50 && inputs.battery_temp < 40 {
            ChargeMode::MaxSpeed
        } else {
            ChargeMode::Adaptive
        };

        if hot && matches!(chosen, ChargeMode::MaxSpeed | ChargeMode::Urgent) {
            return if inputs.is_gaming {
                ChargeMode::UnderLoad
            } else {
                ChargeMode::Adaptive
            };
        }
        chosen
    }

    // NOTE: The specific mA target values below (4500, 2500, 5000, etc.) were empirically
    // tuned against the POCO F6 (peridot) and its specific charger IC behavior. They should be
    // treated as a starting point rather than universal constants if this code is ever adapted
    // for a different device.
    // See the TODO at the bottom of this file (line ~515) regarding manual probing if EINVAL persists.
    fn soc_target_ma(soc: u8, mode: &ChargeMode) -> i64 {
        match mode {
            ChargeMode::UnderLoad => {
                if soc < 20 {
                    9000
                } else if soc < 40 {
                    8750
                } else if soc < 51 {
                    8400
                } else if soc < 55 {
                    8000
                } else if soc < 60 {
                    7000
                } else if soc < 65 {
                    6600
                } else if soc < 73 {
                    6300
                } else if soc < 76 {
                    5600
                } else if soc < 80 {
                    4900
                } else if soc < 83 {
                    4500
                } else if soc < 86 {
                    3800
                } else if soc < 89 {
                    3100
                } else if soc < 91 {
                    2800
                } else if soc < 93 {
                    2500
                } else if soc < 95 {
                    2100
                } else if soc < 97 {
                    1500
                } else {
                    1000
                }
            }
            ChargeMode::MaxSpeed | ChargeMode::Urgent => {
                if soc < 40 {
                    9000
                } else if soc < 51 {
                    8500
                } else if soc < 60 {
                    7500
                } else if soc < 80 {
                    6000
                } else {
                    3500
                }
            }
            ChargeMode::BatteryCare => {
                if soc < 50 {
                    5000
                } else if soc < 80 {
                    3000
                } else {
                    1000
                }
            }
            ChargeMode::Adaptive => {
                if soc < 20 {
                    14000
                } else if soc < 40 {
                    12500
                } else if soc < 51 {
                    12000
                } else if soc < 55 {
                    11500
                } else if soc < 60 {
                    10000
                } else if soc < 65 {
                    9500
                } else if soc < 73 {
                    9000
                } else if soc < 76 {
                    8000
                } else if soc < 80 {
                    7000
                } else if soc < 83 {
                    6500
                } else if soc < 86 {
                    5500
                } else if soc < 89 {
                    4500
                } else if soc < 91 {
                    4000
                } else if soc < 93 {
                    3600
                } else if soc < 95 {
                    3000
                } else if soc < 97 {
                    2200
                } else {
                    1500
                }
            }
        }
    }

    fn next_state(&mut self, inputs: &ChargingInputs, mode: &ChargeMode) -> ChargeState {
        if inputs.plug_state_reliable {
            if !inputs.is_plugged {
                return ChargeState::Disconnected;
            }
        } else if inputs.soc == 0 {
            tracing::warn!(
                target: "charging",
                "Charging plug state unavailable; falling back to SOC-based disconnect heuristic"
            );
            return ChargeState::Disconnected;
        }

        if inputs.battery_temp >= 50
            || inputs.charger_temp >= 70
            || inputs.usb_temp >= 65
            || inputs.pmic_temp >= 70
        {
            ChargeState::Emergency
        } else if (inputs.battery_temp >= 44 && *mode != ChargeMode::Urgent)
            || (inputs.battery_temp >= 48 && *mode == ChargeMode::Urgent)
            || inputs.charger_temp >= 60
            || inputs.usb_temp >= 55
            || inputs.pmic_temp >= 60
        {
            ChargeState::ThermalThrottle
        } else if *mode == ChargeMode::UnderLoad {
            ChargeState::UnderLoad
        } else {
            ChargeState::Normal
        }
    }
    pub fn evaluate(
        &mut self,
        raw_inputs: &ChargingInputs,
        state_dir: &str,
        hw_profile: &crate::hardware::HardwareProfile,
    ) -> i64 {
        let mut inputs = raw_inputs.clone();
        Self::check_overrides(&mut inputs, state_dir, &mut self.forced_mode);

        let soc = inputs.soc;
        let bat_temp = inputs.battery_temp;

        if bat_temp > self.session_peak_temp {
            self.session_peak_temp = bat_temp;
        }

        self.charge_mode = Self::select_charge_mode(&inputs, &self.forced_mode);
        self.apply_voters_for_mode(&self.charge_mode, self.learned_stable_current, inputs.composite_temp);
        let mode_clone = self.charge_mode.clone();
        let next = self.next_state(&inputs, &mode_clone);

        if next == ChargeState::Disconnected {
            if self.current_state != ChargeState::Disconnected {
                // Session finished
                self.finish_session(state_dir, soc);
                self.limit_write_failure_count = 0;
                self.limit_write_disabled = false;
                // Release all voter nodes on disconnect to prevent latching.
                // Without this, input_suspend=1 from a prior state can remain
                // stuck across reconnect cycles (observed: 8-minute latch).
                self.release_voters_on_shutdown();
            }
            self.current_state = next;
            return 0;
        }

        if self.current_state == ChargeState::Disconnected {
            self.learned_stable_current = Self::soc_target_ma(soc, &self.charge_mode);
            self.session_start_soc = soc;
            self.session_peak_temp = bat_temp;
            self.session_start_time = Some(std::time::SystemTime::now());
            self.session_peak_usb_temp = inputs.usb_temp;
            self.session_peak_pmic_temp = inputs.pmic_temp;
            self.thermal_reduction_count = 0;
            self.recovery_count = 0;
            self.total_current_ua_samples = 0;
            self.total_power_uw_samples = 0;
            self.sample_count = 0;
            self.voter_dump_done = false;
            tracing::info!(target: "charging", "Charging session started at {}% SOC", soc);

            self.dump_charger_diagnostics();

            // --- One-shot restrict clearance ---
            // The Qualcomm PMIC charging driver uses two sysfs nodes to
            // control charge current:
            //   restrict_chg=1 + restrict_cur=X → cap current at X μA
            //   restrict_chg=0                  → ignore restrict_cur, full speed
            //
            // restrict_cur's idempotent probe (read→write same value) often
            // fails on Xiaomi kernels, falsely marking it read-only. Writing
            // a DIFFERENT value ("0") usually succeeds.
            //
            // restrict_chg was removed from voter_nodes in v3.2.18 because
            // writing it every tick caused SPMI bus contention with the
            // display controller. Writing it ONCE at session start is safe
            // because: (a) the display is freshly on and idle, (b) the
            // write completes in microseconds, (c) it's not repeated.
            self.one_shot_clear_restrict();

            if let Some(node) = self.limit_nodes.first() {
                tracing::info!(target: "charging",
                    "Charge-limit control node: {} ({} candidates writable)",
                    node, self.limit_nodes.len());
            } else {
                tracing::info!(target: "charging",
                    "Charge-limit control: NONE (device controls current itself)");
            }

            // --- Charger voltage check ---
            // Log the charger contract voltage for diagnostics. If < 9V,
            // the charger is NOT negotiating QC/PD fast charge.
            // WARNING: We do NOT write restrict_chg here — toggling it
            // on the SPMI bus can freeze the display controller on
            // Xiaomi devices (SM8635 shares SPMI between charger PMIC
            // and display PMIC).
            let voltage_max = std::fs::read_to_string(
                "/sys/class/power_supply/usb/voltage_max")
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok());
            if let Some(v_max) = voltage_max {
                tracing::info!(target: "charging", "USB voltage_max={}mV (charger contract)", v_max / 1000);
                if v_max < 9_000_000 {
                    tracing::warn!(target: "charging",
                        "Slow charger detected ({}mV < 9000mV): charger is NOT negotiating QC/PD. \
                         This is a hardware limitation — the charger or cable does not support \
                         Quick Charge. No software workaround is possible. Charging speed is \
                         limited to ~900mA at 5V (~4.5W).",
                        v_max / 1000);
                }
            }
        }

        // Tracking peaks and samples
        self.session_peak_usb_temp = self.session_peak_usb_temp.max(inputs.usb_temp);
        self.session_peak_pmic_temp = self.session_peak_pmic_temp.max(inputs.pmic_temp);

        if let Some(current) = inputs.current_now_ua {
            self.total_current_ua_samples += current.abs();
            if let Some(voltage) = inputs.voltage_now_uv {
                let power_uw = (current.abs() as f64 / 1_000_000.0 * voltage as f64) as i64;
                self.total_power_uw_samples += power_uw;
            }
            self.sample_count += 1;
        }

        if next == ChargeState::ThermalThrottle
            && self.current_state != ChargeState::ThermalThrottle
        {
            self.thermal_reduction_count += 1;
            tracing::info!(target: "charging", "Thermal throttle engaged (Reduction count: {})", self.thermal_reduction_count);
        } else if self.current_state == ChargeState::ThermalThrottle && next == ChargeState::Normal
        {
            self.recovery_count += 1;
            tracing::info!(target: "charging", "Recovered from thermal throttle (Recovery count: {})", self.recovery_count);
        }

        if self.current_state != next {
            tracing::info!(target: "charging", "State changed: {:?} -> {:?}", self.current_state, next);
        }
        self.current_state = next.clone();

        let base_target = Self::soc_target_ma(soc, &self.charge_mode);

        let thermal_cap = if bat_temp >= 50 {
            2000
        } else if bat_temp >= 48 {
            4000
        } else if bat_temp >= 46 {
            7000
        } else if bat_temp >= 44 {
            9000
        } else {
            base_target
        };

        let mut final_target = match next {
            ChargeState::Normal => base_target,
            ChargeState::UnderLoad => {
                // Proactive composite-based thermal cap: reduce PMIC heat
                // before thermal policy escalates to Powersave. When composite
                // is cool, allow fast charging (POCO F6 supports 90W). As it
                // warms, back off proactively to keep gameplay smooth.
                let composite_cap = if inputs.composite_temp >= self.thermal_temp_hot {
                    1_500  // 1.5A: aggressive thermal relief at temp_hot
                } else if inputs.composite_temp >= self.thermal_temp_warm {
                    2_500  // 2.5A: moderate reduction approaching temp_hot
                } else {
                    5_000  // 5A: allow fast charging when cool
                };
                base_target.min(composite_cap)
            }
            ChargeState::ThermalThrottle => base_target.min(thermal_cap),
            ChargeState::Emergency => 500.min(base_target),
            ChargeState::Disconnected => 0,
        };

        let live_cycles = hw_profile
            .charging_profile
            .cycle_count_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s: String| s.trim().parse::<u64>().ok())
            .or(hw_profile.charging_profile.cycle_count);
        if let Some(cycles) = live_cycles {
            let f = Self::cycle_taper_factor(cycles);
            if (f - 1.0).abs() > f32::EPSILON {
                final_target = ((final_target as f32) * f) as i64;
                tracing::debug!(
                    target: "charging",
                    "cycle taper: cycles={} factor={:.2} cap_after={}",
                    cycles, f, final_target
                );
            }
        }

        let mut target_ma = self.previous_target;
        let step = 200;

        if final_target < target_ma {
            target_ma = final_target;
        } else if final_target > target_ma {
            target_ma += step;
            if target_ma > final_target {
                target_ma = final_target;
            }
        }

        if let Some(ceiling) = self.rejected_ceiling
            && target_ma >= ceiling {
                target_ma = self.last_known_good_ma.unwrap_or(target_ma).min(target_ma);
            }

        if final_target > self.learned_stable_current {
            self.learned_stable_current = self
                .learned_stable_current
                .max(final_target.min(base_target));
        }

        let now = std::time::Instant::now();
        let settled = self
            .session_start_time
            .map(|t| t.elapsed().unwrap_or_default().as_secs() >= 3)
            .unwrap_or(true);

        let ready_for_next_attempt = settled
            && match self.last_apply_attempt {
                Some(last) => now.duration_since(last).as_millis() >= 2500,
                None => true,
            };

        if ready_for_next_attempt
            && (self.active_limit_ma != target_ma
                || now.duration_since(self.re_enforce_at).as_secs() > 30)
        {
            self.last_apply_attempt = Some(now);
            match self.apply_limit(target_ma) {
                true => {
                    self.consecutive_failures = 0;
                    self.last_known_good_ma = Some(target_ma);
                }
                false => {
                    self.consecutive_failures += 1;
                    if self.consecutive_failures >= 5 {
                        self.rejected_ceiling = Some(target_ma);
                        if let Some(good) = self.last_known_good_ma {
                            if self.apply_limit(good) {
                                target_ma = good;
                                self.consecutive_failures = 0;
                            } else {
                                self.last_known_good_ma = None; // fallback failed, clear it to avoid loop
                            }
                        }
                    }
                }
            }
            self.re_enforce_at = now;
        }

        self.previous_target = target_ma;

        target_ma
    }

    fn finish_session(&self, state_dir: &str, final_soc: u8) {
        let duration = self
            .session_start_time
            .map(|t| t.elapsed().unwrap_or_default().as_secs())
            .unwrap_or(0);

        let mut avg_current = 0;
        let mut avg_power_uw = 0;
        if self.sample_count > 0 {
            avg_current = self.total_current_ua_samples / self.sample_count as i64;
            avg_power_uw = self.total_power_uw_samples / self.sample_count as i64;
        }

        let summary = serde_json::json!({
            "start_soc": self.session_start_soc,
            "end_soc": final_soc,
            "duration_sec": duration,
            "peak_batt_temp": self.session_peak_temp,
            "peak_usb_temp": self.session_peak_usb_temp,
            "peak_pmic_temp": self.session_peak_pmic_temp,
            "thermal_reductions": self.thermal_reduction_count,
            "thermal_recoveries": self.recovery_count,
            "avg_current_ua": avg_current,
            "avg_power_uw": avg_power_uw,
            "samples": self.sample_count,
            "end_time": chrono::Utc::now().timestamp(),
        });

        let file_path = Path::new(state_dir).join("charging_session.json");
        let temp_path = Path::new(state_dir).join("charging_session.json.tmp");

        if let Ok(json_str) = serde_json::to_string_pretty(&summary) {
            if let Err(e) = fs::write(&temp_path, json_str) {
                tracing::error!("Failed to write charging session state: {}", e);
            }
            let _ = fs::rename(&temp_path, &file_path);
        }

        tracing::info!(target: "charging", "Session ended. Started at {}%, Ended at {}%, Duration: {}s, Peak Temp: {}C", self.session_start_soc, final_soc, duration, self.session_peak_temp);
    }

    fn apply_limit(&mut self, ma: i64) -> bool {
        self.limit_nodes
            .retain(|node| !crate::logger::is_sysfs_blacklisted(node));

        if self.limit_nodes.is_empty() {
            if !self.no_nodes_warned {
                self.no_nodes_warned = true;
                tracing::info!(target: "charging",
                    "AIThermal has no writable current-limit node on this device; \
                     observed charge current is set entirely by kernel/PMIC (typically \
                     ~900 mA on USB SDP, or the negotiated USB-PD/QC contract).");
                // If a writable restrict_cur cap exists (voter node), the
                // observed ~900 mA is almost certainly that 1 A cap biting,
                // not the source. Surface it so the user knows MaxSpeed/Urgent
                // will clear it.
                for node in &self.voter_nodes {
                    if node.ends_with("/restrict_cur")
                        && let Ok(v) = std::fs::read_to_string(node)
                        && let Ok(ua) = v.trim().parse::<i64>()
                        && ua > 0
                    {
                        tracing::warn!(target: "charging",
                            "Detected a {}mA current cap on {} — slow charging is caused by \
                             this cap, not by AIThermal. MaxSpeed/Urgent mode clears it.",
                            ua / 1000, node);
                    }
                }
            }
            return false;
        }

        if self.limit_write_disabled {
            return false;
        }

        let clamped_ma = ma.clamp(500, 9_000);
        // Round to nearest 100mA as a first attempt at hitting an accepted step;
        // if EINVAL persists even after this, the device may need a hardcoded
        // accepted-value table instead.
        // TODO(device-specific): confirm accepted current steps for this node via manual probing if EINVAL persists
        let rounded_ma = ((clamped_ma + 50) / 100) * 100;
        let micro_amps = (rounded_ma * 1000).to_string();

        match crate::sysfs::write_first_available(&self.limit_nodes, &micro_amps) {
            Ok(()) => {
                self.limit_write_failure_count = 0;
                self.active_limit_ma = rounded_ma;
                tracing::debug!(target: "charging", "Applied charge limit: {}mA via {}", rounded_ma, self.limit_nodes.first().map(|s| s.as_str()).unwrap_or("?"));
                true
            }
            Err(e) => {
                let hard_reject = matches!(
                    e,
                    crate::sysfs::SysfsError::PermissionDenied(_) | crate::sysfs::SysfsError::NotFound(_)
                );

                if hard_reject {
                    self.limit_write_failure_count = 5;
                } else {
                    self.limit_write_failure_count = self.limit_write_failure_count.saturating_add(1);
                }

                if self.limit_write_failure_count >= 5 {
                    self.limit_write_disabled = true;
                    if let Some(node) = self.limit_nodes.first() {
                        tracing::warn!(target: "charging",
                            "Node {} repeatedly rejected writes or is unusable, disabling input_current_limit control for this session",
                            node);
                        crate::logger::blacklist_sysfs_node(node);
                    }
                    self.rejected_ceiling = Some(rounded_ma);
                    return false;
                }

                tracing::debug!(target: "charging", "Failed to apply charge limit {}mA: {}", rounded_ma, e);
                if let Some(node) = self.limit_nodes.first() {
                    tracing::warn!(target: "charging", "Node {} rejected value {}mA, will retry with next computed value: {}", node, rounded_ma, e);
                }
                false
            }
        }
    }
}

impl Default for ChargingEngine {
    fn default() -> Self {
        Self::new(&crate::hardware::HardwareProfile::default(), 48, 58)
    }
}
