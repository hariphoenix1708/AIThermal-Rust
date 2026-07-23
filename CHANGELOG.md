# Changelog

## [v3.1.4-beta] - Smooth game-exit

*   **Game-exit hot phase**: for the first ~4 s after a
    fullscreen game exits, the daemon holds cpuset, CPU
    governor, mi_thermald hand-off, and I/O scheduler in
    their in-game configuration. This eliminates the
    rare screen-blank / auto-lock that could happen when
    SurfaceFlinger's exit animation collided with the
    policy-transition write burst.
*   **Telemetry**: WebUI Overview now shows the current
    recovery phase.

## [v3.1.3-beta] - IST timestamps, qcom-battery voters, clean uninstall

*   **IST Timestamps**: Every daemon-emitted log stream and the
    `service.sh` startup log now print wall-clock time in
    Asia/Kolkata (`YYYY-MM-DD HH:MM:SS.mmm+05:30`), independent
    of the process TZ.
*   **QCOM Battery Voter Awareness (peridot / Xiaomi)**: On the
    first charger-connect of each session the daemon now dumps
    every readable `qcom-battery` and `power_supply/usb` node
    into `thermalai_charging.log`, so the actual cause of a
    slow-charge event is visible in one place. Discovered
    writable voter nodes (`restrict_chg`, `restrict_cur`,
    `input_suspend`, `night_charging`) are now driven from
    `ChargeMode`:
      - MaxSpeed / Urgent : `restrict_chg=0`, `input_suspend=0`,
        `night_charging=0`  → releases the ~1000 mA HyperOS cap.
      - BatteryCare       : `restrict_chg=1`, `restrict_cur`
        set to the SoC-target current.
      - Adaptive          : neutral (only clears `input_suspend`
        if it was asserted).
    A 42°C thermal guard downgrades MaxSpeed to Adaptive /
    UnderLoad automatically. Voters are restored to defaults on
    clean daemon shutdown and on uninstall.
*   **CLI**: `thermalair charging` now accepts `maxspeed` and
    `batterycare` in addition to `adaptive` / `urgent`.
*   **WebUI**: Charging tab now surfaces the current
    `charge_mode`, discovered voter count, and whether the
    BatteryCare cap is currently active.
*   **Uninstall Hygiene**: `uninstall.sh` now removes all six
    log streams (main / verbose / startup / battery / thermal /
    charging / gaming) plus any `.1` / `.gz` rotation siblings,
    and force-resets `restrict_chg`, `input_suspend`, and
    `night_charging` to `0` before removing the module.

## [v3.1.2-beta] - Charge-node probe, wake defer, split logging

*   **Charging Node Discovery**: Added a probe-write phase to
    hardware discovery that drops sysfs current-limit nodes which
    reject `EINVAL` at runtime. On peridot this eliminates the
    repeated `input_current_limit` rejections observed in v35 and
    logs a single explicit "Charge-limit control: NONE" line when
    the device manages charge current itself.
*   **Screen-Wake Actuation Defer**: On screen-on, actuator writes
    (governors, cpuset, GPU) are deferred for 2500 ms and the
    thermal EMA/history is reset after long deep-sleep. Eliminates
    the wake-burst stutter previously observed on POCO F6.
*   **Adaptive Governor Streaks**: Promotion and demotion between
    Eco and Balanced tiers now require two consecutive samples,
    with the Eco cutoff raised from 35% to 55% cluster utilization
    to stop idle browsing from tripping walt.
*   **Per-Policy GPU Power Level**: Balanced (non-gaming),
    Conservative, and Powersave now pin the GPU to its deepest
    idle power level; Performance and gaming keep the shallowest.
*   **Split Logging**: Added `thermalai_thermal.log`,
    `thermalai_charging.log`, and `thermalai_gaming.log`; the main
    `thermalai.log` is now a curated high-signal stream, and
    `thermalai_verbose.log` remains TRACE-level for debugging.
*   **WebUI**: Logs tab exposes all five streams; dashboard and
    charging views surface adaptive tier, GPU power level, and the
    active charge-limit control node.

## [v3.1.1-beta] - Wall-clock hysteresis and telemetry cleanup

*   **Policy & Recovery Stability**: Converted internal PolicyEngine debounce and RecoveryManager thermal threshold limits from cycle tick counts to robust `std::time::Instant` wall-clock seconds. This solves a prominent stutter/judder issue where dynamic sleep-tick changes (adaptive polling) during gaming or screen-wake were improperly accelerating cooldown evaluation logic and thrashing CPU governors.
*   **Snapshot Cleanup**: Eliminated TCP state paths from being unnecessarily cached within the system snapshot since they are actively being ignored by `touch_network_stack` flags in `profiles.conf`.
*   **Frame Sampler Guard**: Bolstered the Android frame parsing logic for Adaptive Governor to require at least three timestamps per row, dropping faulty sparse samples (zero-duration frame logs).
*   **Sensor Hardware Handling**: Secured `ambient_temp_c` sensor parsing to guarantee a faulty/unreadable fallback probe won't clobber the valid path inside the hardware profile cache.

## [v3.1.0-beta] - Major Features and Stability Update

*   **Adaptive Governor**: Added an opt-in, frame-timing-and-load-aware CPU frequency governor (`adaptive_governor_enabled`) during active gaming, using real per-frame data via `dumpsys` where available, with a CPU-load-based fallback.
*   **Policy Stability**: Introduced policy engine hysteresis to prevent rapid governor flapping near threshold boundaries, and a 30-second startup grace period to stabilize initial daemon evaluation.
*   **Netlink Screen Detection**: Implemented low-latency `uevent` screen-state detection as a complement to polling, including a broadened-match mode for compatibility across diverse kernel uevent behaviors.
*   **Game Detection Hardening**: Implemented `top-app` cgroup-based confirmation for game detection, reducing false positives from background processes sharing package names. Corrected previous substring matching to exact full-string matching.
*   **Battery Telemetry**: Added new dedicated battery/power statistics logging (`thermalai_battery.log`) to track temperature, charge current, drain rate, and screen-on/off/deep-sleep time.
*   **Thermal Engine Management**: Expanded stock-thermal-engine disablement to clear per-core thermal limits (`thermal_message/cpu_limits`).
*   **GPU & Daemon Coordination**: Added KGSL GPU `bus_split`/`force_clk_on` tuning during active gameplay. Updated `service.sh` to explicitly coordinate and stop conflicting Xiaomi/HyperOS performance daemons.
*   **Reliability Improvements**: Improved charging current-limit application reliability, enhanced uninstall/reinstall cleanup processes, and fixed log rotation edge cases.

## [v3.0.3-beta] - Maintenance release

*   **Version**: Bumped `module.prop` to `v3.0.3-beta` (versionCode `304`) for redistribution.
*   **No functional changes**: Daemon behavior, hardware discovery, policy engine, charging engine, and CLI surface are unchanged from `v3.0.2-beta`.


## [1.0.2] - Runtime packaging and gaming smoothness hardening

*   **Packaging Contract**: Standardized packaged Rust executables under `system/bin`, added install-time `customize.sh` permission/context setup, included `sepolicy.rule`, and kept Windows ZIP creation on 7-Zip with Android ARM64 ELF validation.
*   **Startup Reliability**: Kept daemon startup validation strict with `sys.boot_completed`, PID liveness checks, and logs under `/data/local/tmp`.
*   **Logging**: Increased in-place runtime log truncation from 1 hour to 2 hours while keeping log files in `/data/local/tmp`.
*   **Gaming Smoothness**: Added capability-selected CPU governor preference so game/performance mode uses WALT only when every discovered CPU policy exposes a writable WALT governor, then falls back to performance or schedutil safely.
*   **Game Detection Defaults**: Added CODM Garena and Roblox to embedded/default game coverage so fallback configuration still recognizes the requested games.
*   **Thermal Coordination**: Blacklists cooling-device `cur_state` nodes after a kernel write rejection to stop repeated invalid writes while still logging the first failure.
*   **Reference Project Audit**: Ported safe ideas from Uperf/Encore by adding discovered `background` and `restricted` cpuset group handling, expanding CODM/Roblox/PUBG package variants, and reporting verified cpuset nodes in the hardware audit.

## [1.0.0] - AIThermal-Rust Rewrite Complete

*   **Build Recovery**: Addressed logger type mismatches, variable scope boundaries, and resolved duplicated profile definitions, bringing the entire workspace to a clean compiling state targeting AArch64.
*   **State Atomicity**: Pushed total tick-level ownership out of scattered subsystem structs and unified it under `RuntimeContext` utilizing atomic `fs::rename` operations for all local caches.
*   **Policy Engine**: Dropped arbitrary runtime scaling multipliers (`* 10.0`) globally, calibrating variables organically internally to align explicitly with legacy scoring equations without relying on magic numbering.
*   **Charging Framework**: Corrected real `SOC` consumption logic and bounded hardware thermal reduction limits securely to `500mA`, guarding against `urgent` config drift by expiring invalid UNIX timestamps gracefully.
*   **Hardware Discovery Expansion**: Upgraded the generic probe sequences. Safely maps TCP metrics, memory PSI 10/60/300s diagnostic stalls, block storage I/O parameters, explicit CPUSet mappings, and extracts valid features dynamically out of `/proc/config.gz`.
*   **Peridot Match Validations**: Hardened POCO F6 matching to require rigorous corroboration spanning `ro.product.device`, `ro.boot.hardware`, and `ro.board.platform` before applying SD8sGen3 capabilities.
*   **Gaming Intelligence**: Rewrote `scan_oom_score_adj` leveraging `startsWith` and `contains` substring resolution to handle Linux kernel truncation inside `/proc/[pid]/status`. *(Note: This approach was later found to cause false-positive detection and was replaced with exact-match-only comparison in [v3.1.0-beta]).*
*   **CLI Expansion**: Amplified the standalone `thermalair` console to parse policy triggers via history and support unified daemon `start/restart/stop` cycles cleanly across varied custom ROM layouts.
*   **Runtime Tuning**: Ported I/O scheduler limits, TCP configuration states (BBR, keepalive), and VM swappiness metrics directly into the orchestrator policy transition loops cleanly reversing automatically.
*   **Calibration & Learning**: Enforced a single `calibration.json` source tracking consecutive slow-cool decays cleanly constrained within a safe -6°C to +6°C drift limit dynamically across daemon restarts.
*   **Snapshot & Recovery**: Sequestered true Emergency hardware trips apart from user-triggered game cooldown states cleanly, verifying hardware `cpufreq` policy states concurrently upon initial snapshot restore validations.
*   **Documentation & Build readiness**: Validated Windows 11 `build.ps1` and Linux build systems handling cleanly compiled `x86_64` logic simulations paired securely to final AArch64 targets without runtime warnings or trailing logic duplicates.

## [1.0.1] - Compilation fixes

*   **Compilation**: Fixed a compilation error regarding undefined field `_runtime_tuner` in the SystemOrchestrator by renaming it correctly to `runtime_tuner`.
*   **Documentation**: Updated Magisk repackaging instructions.
