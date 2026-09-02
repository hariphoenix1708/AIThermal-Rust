use super::profile::ServiceProfile;

/// Returns true if a phone call is active (offhook/ringing) via
/// `dumpsys telephony.registry` (`mCallState` 1=ringing, 2=offhook).
/// Polled at low frequency (once every 5-10 ticks) — cheap enough
/// alongside other dumpsys checks, no extra permission.
pub fn is_call_active() -> bool {
    std::process::Command::new("dumpsys")
        .args(["telephony.registry"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.contains("mCallState=2") || s.contains("mCallState=1"))
        .unwrap_or(false)
}

pub fn probe_services() -> ServiceProfile {
    ServiceProfile {
        thermal_hal: super::android_prop::get_property("init.svc.vendor.thermal-hal-2-0", "")
            == "running"
            || super::android_prop::get_property("init.svc.thermal-engine", "") == "running",
        vendor_thermal_engine: super::android_prop::get_property("init.svc.thermal-engine", "")
            == "running"
            || super::android_prop::get_property("init.svc.mi_thermald", "") == "running",
        power_hal: super::android_prop::get_property("init.svc.vendor.power-hal-1-0", "")
            == "running",
        health_hal: super::android_prop::get_property("init.svc.vendor.health-hal-2-1", "")
            == "running",
        perf_service: super::android_prop::get_property("init.svc.perfservice", "") == "running"
            || super::android_prop::get_property("init.svc.vendor.perfservice", "") == "running"
            || super::android_prop::get_property("init.svc.perf-daemon", "") == "running"
            || super::android_prop::get_property("init.svc.perf-service", "") == "running",
        vendor_performance_daemon: super::android_prop::get_property(
            "init.svc.vendor.performance-daemon",
            "",
        ) == "running"
            || super::android_prop::get_property("init.svc.vendor.perf-hal-1-0", "") == "running",
    }
}
