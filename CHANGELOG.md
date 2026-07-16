# Changelog

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
*   **Gaming Intelligence**: Rewrote `scan_oom_score_adj` leveraging `startsWith` and `contains` substring resolution to handle Linux kernel truncation inside `/proc/[pid]/status`. *(Note: This approach was later found to cause false-positive detection and was replaced with exact-match-only comparison in [1.0.2] or later).*
*   **CLI Expansion**: Amplified the standalone `thermalair` console to parse policy triggers via history and support unified daemon `start/restart/stop` cycles cleanly across varied custom ROM layouts.
*   **Runtime Tuning**: Ported I/O scheduler limits, TCP configuration states (BBR, keepalive), and VM swappiness metrics directly into the orchestrator policy transition loops cleanly reversing automatically.
*   **Calibration & Learning**: Enforced a single `calibration.json` source tracking consecutive slow-cool decays cleanly constrained within a safe -6°C to +6°C drift limit dynamically across daemon restarts.
*   **Snapshot & Recovery**: Sequestered true Emergency hardware trips apart from user-triggered game cooldown states cleanly, verifying hardware `cpufreq` policy states concurrently upon initial snapshot restore validations.
*   **Documentation & Build readiness**: Validated Windows 11 `build.ps1` and Linux build systems handling cleanly compiled `x86_64` logic simulations paired securely to final AArch64 targets without runtime warnings or trailing logic duplicates.

## [1.0.1] - Compilation fixes

*   **Compilation**: Fixed a compilation error regarding undefined field `_runtime_tuner` in the SystemOrchestrator by renaming it correctly to `runtime_tuner`.
*   **Documentation**: Updated Magisk repackaging instructions.
