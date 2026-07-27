# Changelog

## v3.2.4 (versionCode 324)
### Added
- Advanced tuning pass (`advanced_tuning_enabled = true`), applied once per
  policy transition after the existing tuner. Every write is
  capability-probed and idempotent — safe on non-QCOM / non-Peridot devices.
  - **Schedutil rate limits**: `up_rate_limit_us` = 500 µs (Performance) /
    2000 µs (Balanced), `down_rate_limit_us` = 20 ms. Faster ramp-up
    without idle-oscillation.
  - **WALT hispeed**: `walt/hispeed_freq` pinned to cluster max on
    Performance for lower input latency on Qualcomm kernels.
  - **CFS/WALT scheduler**: `sched_latency_ns`, `sched_min_granularity_ns`,
    `sched_wakeup_granularity_ns`, `sched_migration_cost_ns`, and
    `sched_energy_aware` follow the Pixel-style responsive preset on
    Performance and revert to energy-aware defaults otherwise.
  - **cpuidle**: enables every C-state on every CPU (some vendors ship
    with cluster power-collapse disabled — measurably worse standby).
  - **zRAM**: switches `zram0/comp_algorithm` to `lz4` while gaming for
    lower page-fault latency; standby keeps the vendor default.
  - **F2FS**: `gc_urgent` = 0 during gaming (no GC storms mid-frame),
    `gc_urgent` = 1 idle, `ipu_policy` = 2, `min_hot_blocks` = 16.
  - **msm_performance powerhints**: `touchboost` and `cpus_online`
    pin — prevents cpu0/1 hotplug hitches after standby.
- SELinux (`sepolicy.rule`): Android 17 compatibility — explicit `lseek`
  on sysfs / procfs / power-supply / debugfs-tracing / cpu-devices file
  classes, and write access to `sysfs_devices_system_cpu` for the cpuidle
  and schedutil paths.

### Notes
- The pass is a superset — every knob has a safe no-op fallback if the
  node is absent or the write is rejected. Disable via
  `advanced_tuning_enabled = false` in `profiles.conf` if any regression
  is observed on a new kernel.

## v3.2.3 (versionCode 323)
### Fixed & Improved
- Fixed duplicate "Policy transition" log line for Suspend/EmergencyCool escalations (engine + orchestrator both logged; orchestrator is now the single source).
- (optional) WebUI: gauge gradient, top safe-area inset, larger small-button touch targets.

## v3.2.2 (versionCode 322)
### Fixed
- Policy transitions are now logged exactly once, from the orchestrator on
  the actuated policy change. Removed the duplicate engine-side log for
  EmergencyCool/Suspend and the phantom transitions it produced.
- The transition log now reports the real policy decision score instead of
  the internal context value.
- WebUI "Recent transitions" and `thermalair history` now read
  thermalai_thermal.log, so the transition history is no longer empty.

## v3.2.1 (versionCode 321)
### Fixed
- Idempotent sysfs writes are now consistent across all tuning paths.
  write_if_changed understands the kernel's bracketed scheduler format,
  and write_and_save / restore_or_default no longer rewrite unchanged
  values. Eliminates ~700 redundant queue/scheduler writes and ~100
  redundant vm.swappiness writes per session (reduces actuation overhead
  and log noise).
- Policy transition logs now reflect the ACTUATED policy. Decisions that
  the orchestrator overrides during post-game cooldown / thermal recovery
  no longer emit phantom transition lines that contradict the tick state.

## v3.2.0 (versionCode 320)
### Fixed
- CPU tuning path: wire apply_universal_cpu_tuning into the orchestrator
  actuation cycle so the P3 governor mapping and scaling_max_freq clamps
  (Powersave 70%, Conservative 85%, EmergencyCool 55%) actually take
  effect on transitions.
- adaptive_governor no longer writes scaling_max_freq during
  Powersave/Conservative/EmergencyCool/Suspend — P3 owns the clamp in
  those states; no more competing writes.
- Policy transition INFO log now fires for every X -> Suspend transition
  (Balanced/Conservative/Powersave/Performance/EmergencyCool -> Suspend).
- Removed the duplicate back-to-back "Policy transition" log emission.
- Idempotent sysfs writes: try_write_string now skips when the node
  already holds the target value. Kills repeated no-op writes to
  vm.*, block/*/queue/scheduler, and cpuset nodes across ticks.
- P5 Powersave-arm counter: preserve arm_count across the placeholder
  Conservative step so a sustained two-tick tentative=Powersave actually
  enters Powersave (previously reset by the else-branch on tick N).

## [v3.1.9] (319) - Stable

Fixes user-visible 5-7 s stutters on Conservative -> Powersave transitions at moderate temperatures (v3.1.8 regression).

*   CPU governor for Powersave / Conservative / EmergencyCool is now `schedutil` with a percentage-of-Fmax clamp (70 % / 85 % / 55 %) instead of bare `powersave`. Same cooling effect, no UI cliff. Suspend still uses `powersave` (screen off; no user impact).
*   Powersave cpuset ranges are temperature-gated: at composite < temp_hot (58 C default), Powersave keeps Balanced-shaped ranges so foreground UI keeps big cores.
*   Sustained-heating requirement for Powersave entry: two consecutive ticks above threshold OR composite >= temp_powersave. Prevents single-tick trend spikes from causing UX cliffs.
*   PSI positive amplifier is gated on composite >= temp_warm and reduced from +4.0 to +3.0. Negative relief unchanged.
*   KGSL power-level detection now falls back to `default_pwrlevel` on Adreno 730/735/750 (SM8550/8635/8650) when neither `pwrlevel` nor `current_pwrlevel` are writable. Also records `thermal_pwrlevel` as a floor to avoid fighting the QCOM thermal HAL on HyperOS.
*   Snapshot/restore path restores `default_pwrlevel` on shutdown for the same devices.
*   No behavioural regression on kernels/ROMs without any of the new sysfs nodes - every read/write remains capability-gated with silent no-op on ENOENT/EPERM.
*   Wake-from-Suspend fast path now covers Powersave destinations, not just Balanced/Performance/Conservative. Fixes ~11 s stutter when a screen wake lands directly in Powersave (repro: charging + composite ~46 C at wake).
*   `default_pwrlevel` (Adreno 730/735/750) added to KGSL probe.
*   "Skipping GPU power-level control" log downgraded and gated behind std::sync::Once (was printed twice per tick).


## [v3.1.8] (318) - Stable

*   PSI-aware scoring: CPU (`/proc/pressure/cpu`) and I/O
    (`/proc/pressure/io`) pressure are now folded into the policy
    score alongside memory PSI. On idle-warm devices this holds
    Balanced instead of tightening to Powersave; under real load
    it accelerates tightening.
*   Battery cycle-count-aware charge tapering. Reads
    `/sys/class/power_supply/battery/cycle_count` (or bms/) and
    multiplies the fast-charge current cap by a factor between
    1.00 (fresh) and 0.85 (>1200 cycles).
*   cgroup v2 unified-hierarchy cpuset detection. AOSP A14+ / A16
    devices that ship v2-only cpuset are now supported without
    losing the RenderThread/GLThread pinning behaviour used on v1.
*   Optional atrace/ftrace `trace_marker` hooks for correlating
    policy transitions with Perfetto captures. Off by default;
    enable with `trace_markers_enabled = true` in profiles.conf.
*   sepolicy: added debugfs_tracing / tracing_shell_writable rules
    (needed only when trace markers are enabled).
*   No behavioural regression when PSI, cycle_count, cgroup v2, or
    trace_marker paths are absent - every read is capability-gated.

## [v3.1.7] (317) - First stable release

*   Zero-lag loosening actuation on wake: the first tick after screen
    wake now flips CPU/GPU governors back to their active state in
    the same tick as the Suspend -> Balanced transition (previously
    deferred up to ~4 s by the min_actuation_interval throttle).
*   Removed two panic paths in the daemon tick loop (SystemTime
    unwrap() replaced with unwrap_or(0)).
*   Installer banner now reflects the actual module version instead
    of a hardcoded string.
*   Removed the obsolete/incorrect updater-script; Magisk and KSU
    both use update-binary under SKIPUNZIP=1.
*   sepolicy.rule normalized (trailing semicolons removed), plus
    additional rules for netlink_route and gpu_device access.
*   No behavioural regressions vs v3.1.6-beta. All v40..v42 fixes
    (H1..H9 + I1..I6) carry forward unchanged.

## [v3.1.6-beta] (316)
- Fix 97-second wake lag: dropped actuation on transition ticks is
  now retried until it succeeds (drift-corrected apply).
- Wake defer shortened from 2500 ms to 800 ms.
- Loosening transitions out of Suspend bypass wake defer to keep
  the launcher responsive.
- Telemetry exposes last_applied_policy for drift diagnostics.

## [v3.1.5-beta] (315) - Self-heat + idle-drain reductions

*   Fast-tick threshold hardened; requires sustained hot trend
*   Idle screen-on can no longer pick Performance policy
*   Same-value guards on VM / IO / governor / kgsl tunings
*   Sticky stock-thermal state (no sconfig ping-pong)
*   Frame sampler parks for 10 s when no game is top-app
*   Watchdog counter now advances every healthy tick
*   state.json throttled to <=2 s cadence during steady state
*   Screen-off deep-idle entry brought forward to 30 s

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
