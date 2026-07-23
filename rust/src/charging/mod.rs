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
    pub session_start_time: Option<std::time::Instant>,
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
}

impl ChargingEngine {
    pub fn new(hw: &crate::hardware::HardwareProfile) -> Self {
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
        }
    }

    fn check_overrides(inputs: &mut ChargingInputs, state_dir: &str) {
        let override_path = format!("{}/charging_mode.json", state_dir);
        if let Ok(content) = fs::read_to_string(&override_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                let urgent = json
                    .get("urgent")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
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

    fn select_charge_mode(inputs: &ChargingInputs) -> ChargeMode {
        if !inputs.is_plugged && inputs.plug_state_reliable {
            ChargeMode::Adaptive
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
        }
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
                    9800
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
                if soc < 20 {
                    18000
                } else if soc < 40 {
                    16000
                } else if soc < 51 {
                    14000
                } else if soc < 60 {
                    12000
                } else if soc < 80 {
                    9000
                } else {
                    5000
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
    pub fn evaluate(&mut self, raw_inputs: &ChargingInputs, state_dir: &str) -> i64 {
        let mut inputs = raw_inputs.clone();
        Self::check_overrides(&mut inputs, state_dir);

        let soc = inputs.soc;
        let bat_temp = inputs.battery_temp;

        if bat_temp > self.session_peak_temp {
            self.session_peak_temp = bat_temp;
        }

        self.charge_mode = Self::select_charge_mode(&inputs);
        let mode_clone = self.charge_mode.clone();
        let next = self.next_state(&inputs, &mode_clone);

        if next == ChargeState::Disconnected {
            if self.current_state != ChargeState::Disconnected {
                // Session finished
                self.finish_session(state_dir, soc);
                self.limit_write_failure_count = 0;
                self.limit_write_disabled = false;
            }
            self.current_state = next;
            return 0;
        }

        if self.current_state == ChargeState::Disconnected {
            self.learned_stable_current = Self::soc_target_ma(soc, &self.charge_mode);
            self.session_start_soc = soc;
            self.session_peak_temp = bat_temp;
            self.session_start_time = Some(std::time::Instant::now());
            self.session_peak_usb_temp = inputs.usb_temp;
            self.session_peak_pmic_temp = inputs.pmic_temp;
            self.thermal_reduction_count = 0;
            self.recovery_count = 0;
            self.total_current_ua_samples = 0;
            self.total_power_uw_samples = 0;
            self.sample_count = 0;
            tracing::info!(target: "charging", "Charging session started at {}% SOC", soc);
            tracing::info!("Charging session started at {}% SOC", soc);

            if let Some(node) = self.limit_nodes.first() {
                tracing::info!(target: "charging",
                    "Charge-limit control node: {} ({} candidates writable)",
                    node, self.limit_nodes.len());
                tracing::info!(
                    "Charge-limit control node: {} ({} candidates writable)",
                    node, self.limit_nodes.len());
            } else {
                tracing::info!(target: "charging",
                    "Charge-limit control: NONE (device controls current itself)");
                tracing::info!(
                    "Charge-limit control: NONE (device controls current itself)");
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

        let final_target = match next {
            ChargeState::Normal => base_target,
            ChargeState::UnderLoad => base_target.min(2500),
            ChargeState::ThermalThrottle => base_target.min(thermal_cap),
            ChargeState::Emergency => 500.min(base_target),
            ChargeState::Disconnected => 0,
        };

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

        if let Some(ceiling) = self.rejected_ceiling {
            if target_ma >= ceiling {
                target_ma = self.last_known_good_ma.unwrap_or(target_ma).min(target_ma);
            }
        }

        if final_target > self.learned_stable_current {
            self.learned_stable_current = self
                .learned_stable_current
                .max(final_target.min(base_target));
        }

        let now = std::time::Instant::now();
        let settled = self
            .session_start_time
            .map(|t| t.elapsed().as_secs() >= 3)
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
            .map(|t| t.elapsed().as_secs())
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
        tracing::info!("Session ended. Started at {}%, Ended at {}%, Duration: {}s, Peak Temp: {}C", self.session_start_soc, final_soc, duration, self.session_peak_temp);
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
            }
            return false;
        }

        if self.limit_write_disabled {
            return false;
        }

        let clamped_ma = ma.clamp(500, 12_000);
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
                self.limit_write_failure_count = self.limit_write_failure_count.saturating_add(1);
                if self.limit_write_failure_count >= 5 {
                    self.limit_write_disabled = true;
                    if let Some(node) = self.limit_nodes.first() {
                        tracing::warn!(target: "charging",
                            "Node {} rejected 5 writes in a row, disabling input_current_limit control for this session",
                            node);
                        crate::logger::blacklist_sysfs_node(node);
                    }
                    self.rejected_ceiling = Some(rounded_ma);
                    return false;
                }

                tracing::debug!(target: "charging", "Failed to apply charge limit {}mA: {}", rounded_ma, e);
                if let Some(node) = self.limit_nodes.first() {
                    match &e {
                        crate::sysfs::SysfsError::PermissionDenied(_)
                        | crate::sysfs::SysfsError::NotFound(_) => {
                            tracing::warn!(target: "charging", "Node {} is unusable (blacklisting): {}", node, e);
                            crate::logger::blacklist_sysfs_node(node);
                        }
                        _ => {
                            tracing::warn!(target: "charging", "Node {} rejected value {}mA, will retry with next computed value: {}", node, rounded_ma, e);
                        }
                    }
                }
                false
            }
        }
    }
}

impl Default for ChargingEngine {
    fn default() -> Self {
        Self::new(&crate::hardware::HardwareProfile::default())
    }
}
