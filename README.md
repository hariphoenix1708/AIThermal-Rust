# AIThermal-Rust

A highly optimized, adaptive thermal and performance orchestrator for Android devices, natively written in Rust for safety, minimal overhead, and rigorous stability. It scales device responsiveness intelligently across dynamic system states (Idle, Gaming, Charging, Emergency).

## ⚠️ Disclaimer

This is an independent project, provided as-is with no warranty. **Use at your
own risk.** This module was built and tested specifically on a **POCO F6
(peridot)** running HyperOS 3 - it may or may not work correctly on other
devices. See [DISCLAIMER.md](./DISCLAIMER.md) for the full disclaimer, warranty,
and device-compatibility statement before installing.

*Please see [CHANGELOG.md](./CHANGELOG.md) for release notes and [LICENSE](./LICENSE) for the project license.*

## Project Overview & Architecture

AIThermal-Rust replaces legacy shell-based orchestration with a memory-safe, deterministic, and highly concurrent Rust daemon.

### Key Features
*   **Adaptive Governor**: An opt-in, frame-timing-and-load-aware CPU frequency governor (`adaptive_governor_enabled`) that dynamically adjusts frequencies during active gaming based on real per-frame data (via `dumpsys`) with a CPU-load-based fallback.
*   **Netlink Screen Detection**: Implements low-latency `uevent` screen-state detection to quickly trigger idle policies when the screen turns off. Includes a broadened-match mode for compatibility across diverse kernel uevent behaviors alongside a reliable polling fallback.
*   **Advanced Game Detection**: Hardened game detection relying on exact full-string matching of process names, supplemented by a secondary `top-app` cgroup-based confirmation to prevent false positives from background processes that share package names.
*   **Battery Telemetry**: Tracks detailed battery and power statistics—including temperature, charge current, drain rate (%/hr), and screen-on/off/deep-sleep times—logged to an isolated `thermalai_battery.log` file.
*   **Dynamic Policy Stability**: Incorporates policy engine hysteresis to prevent rapid governor flapping near threshold boundaries, and a startup grace period to stabilize initial daemon evaluation.
*   **Intelligent Tuning**: Reversibly applies network, VM, touch, and IO scheduler tweaks dynamically; extracts game PIDs to pin rendering threads into the `top-app` cpuset for extreme rendering latency optimization.

### Runtime Architecture
The system operates on a tick-based orchestrator model:
1. **`main.rs` & `daemon.rs`**: Initialize the environment, acquire process locks, load static caches, and maintain the isolated `RuntimeContext` which acts as the single source of truth for all tick-level state (e.g. cooldowns, active policy, hardware limits).
2. **`orchestrator.rs`**: Coordinates the runtime sequence. It queries hardware sensors, validates gaming latches, reads dynamic battery states, computes environmental context penalties, predicts thermal trajectories, and delegates state changes to respective engines.
3. **Subsystems**:
    * **Policy Engine**: Dynamically shifts the device between `Performance`, `Balanced`, `Conservative`, `Powersave`, `Suspend`, and `EmergencyCool` based on calculated composite scores.
    * **Prediction Engine**: Evaluates linear thermal velocity to produce `trend_score` penalties ahead of actual throttling events.
    * **Charging Framework**: Intelligently throttles charge current (mA) dynamically referencing live SOC curves, peak temps, and `urgent` UNIX timestamp overrides.
    * **Runtime Tuning**: Mutates I/O schedulers, TCP tunables (BBR, keepalive), swappiness, and display touch-rates dynamically; reverting cleanly when exiting gaming/performance modes.
    * **Calibration Engine**: Actively learns from game session exit curves. Slow cooling environments persist via `calibration.json` dynamically altering overall base thermal weights smoothly.
    * **Gaming Intelligence**: Leverages `OOM_SCORE_ADJ` scanning paired sequentially against `/proc/<pid>/status` package truncations to lock onto active game loads robustly.

### Hardware Discovery Framework
The system implements a generic-first capability detector caching expensive static features on boot (`HardwareProfile`):
*   **Generic First**: Reads generic nodes (e.g. standard `thermal_zone`, `power_supply`, I/O schedulers, `/proc/config.gz` for WALT/EAS/PSI capabilities).
*   **Qualcomm / Snapdragon Optimization**: Dynamically recognizes `kgsl` devfreq instances, CPU topology bounds, and QCOM thermal engines.
*   **POCO F6 (Peridot) Quirks**: Engages tailored thresholds exclusively when multiple corroborating system properties (`ro.product.device`, `ro.board.platform`, `ro.boot.hardware`) confirm the device matches the Snapdragon 8s Gen 3 "SM8635" Peridot identity.

Reference audits from Uperf Game Turbo and Encore are folded in only where they can remain capability-driven: discovered cpuset groups are tuned through validated nodes, and common CODM/Roblox/PUBG variants are included in the default game list. Static SoC frequency, bus, and scheduler hardlocks are intentionally not copied unless the current ROM/kernel exposes verified writable nodes.

## Compatibility

- **Android Versions**: Android 14 – 17
- **ROMs**: Supports stock HyperOS and AOSP-based custom ROMs natively.
- **Kernel Support**: Linux Kernel 4.14 – 6.1+
- **Architectures**: AArch64 (64-bit ARM)

## Directory Layout & Binaries

- `/rust/src/bin/`: Contains the entry binaries:
    - `thermalair`: The primary CLI wrapper exposing state reads, log tailing, and charging overrides.
    - `thermalai-detect`: A standalone hardware discovery diagnostic dumping the raw `HardwareProfile`.
- `/rust/src/`: Contains the core `thermalai-daemon` library modules.
- `/scripts/`: *[Legacy]* Shell reference material strictly preserved for logic parity checking.

### Runtime State Directory Layout
By default, the active runtime writes JSON files safely via atomic renames inside `THERMALAI_STATE_DIR` (default: `/data/local/tmp/thermalai_state`):
- `calibration.json`: Persists learned thermal behaviors, offset clamps (-6 to +6), and `slow_cooler` flags across reboots.
- `charging_session.json`: Written atomically post-charge summarizing durations, max temperatures, and average loads.
- `thermalai_state.json`: Live telemetry dump continuously reflecting the tick-by-tick daemon memory.
- `game_profiles.json`: Accumulates historical sessions, peak thresholds, and triggers per-app hot modifiers.

## Build Setup

Cross-compiling requires the Android NDK to be natively installed and available.

### Prerequisites

- **Git**: [git-scm.com](https://git-scm.com/)
- **Rust**: Install via `rustup-init` from [rustup.rs](https://rustup.rs/). Then run:
  ```bash
  rustup update
  rustup default stable
  ```
- **Android NDK**: Download the latest stable NDK (e.g., `r27d`) from [Android Developers](https://developer.android.com/ndk/downloads).

### Windows 11
1. **Visual Studio 2022 Build Tools**: Required by Rust on Windows. Install the **"Desktop development with C++"** workload via the [Visual Studio Installer](https://visualstudio.microsoft.com/visual-cpp-build-tools/).
2. Extract the NDK `.zip` to `C:\Android\android-ndk-r27d`.
3. Add the Android aarch64 target to your Rust toolchain:
   ```powershell
   rustup target add aarch64-linux-android
   ```
4. Set your environment variables to point to the extracted NDK. Open PowerShell as Administrator and run:
   ```powershell
   [System.Environment]::SetEnvironmentVariable('ANDROID_NDK_HOME', 'C:\Android\android-ndk-r27d', 'User')
   [System.Environment]::SetEnvironmentVariable('ANDROID_NDK_ROOT', 'C:\Android\android-ndk-r27d', 'User')
   ```
5. Use the included Powershell script to compile and package the module ZIP (it automatically uses 7-Zip if available in PATH or `C:\Program Files\7-Zip`):
   ```powershell
   .\build.ps1
   ```

### Linux / macOS
1. Extract the NDK `.zip` to a path like `~/android-ndk`.
2. Ensure the `ANDROID_NDK_HOME` environment variable is defined (e.g., in your `~/.bashrc` or `~/.zshrc`):
   ```bash
   export ANDROID_NDK_HOME=~/android-ndk
   export ANDROID_NDK_ROOT=~/android-ndk
   ```
3. Add the Android aarch64 target to your Rust toolchain:
   ```bash
   rustup target add aarch64-linux-android
   ```
4. Run the shell equivalent compiler and packager. You will need the `zip` utility installed (e.g., `sudo apt install zip` on Debian/Ubuntu):
   ```bash
   ./build.sh
   ```

## Installation & Module Usage
Install the packaged ZIP file directly through Magisk or KernelSU. The module uses `service.sh` to scaffold the module, config, log, and state environment before launching `thermalai-daemon`; startup only succeeds after the daemon PID is alive.

Install-time setup is handled by `customize.sh`, which runs on-device during module installation to set executable modes and SELinux contexts for the packaged `system/bin` daemons. `sepolicy.rule` is included for Magisk/KernelSU policy loading, and the Windows build path uses 7-Zip while preserving Android ARM64 ELF binaries.

## CLI Usage (`thermalair`)

The `thermalair` executable allows granular read/write access to the daemon gracefully:

- `thermalair start`: Boot the daemon through `service.sh` when available, and report success only after PID health validation.
- `thermalair restart`: Stop running daemons, await exit, and reinitialize.
- `thermalair status / temps / policy / gaming`: Directly inspect realtime telemetry JSON state blocks.
- `thermalair charging <adaptive|urgent>`: Switch between battery care cycles and maximum throughput.
- `thermalair history`: Chronologically parse active policy transitions directly out of `thermalai.log` combined with session summaries.
- `thermalair verbose`: Tail the verbose runtime logs (use `thermalair verbose clear` to truncate).
- `thermalair calibrate`: Dump the actively persisted `calibration.json` offsets.

## Logging & Troubleshooting
Logs are generated locally under `THERMALAI_LOG_DIR` (default: `/data/local/tmp/`).
- `thermalai.log`: Standard info-level lifecycle and policy transitions.
- `thermalai_startup.log`: Launcher contract, resolved paths, stale PID cleanup, and daemon validation.
- `thermalai_verbose.log`: Granular trace-level tick telemetry.
- `thermalai_battery.log`: Detailed battery/power statistics (temperature, drain rate, screen-on/off/deep-sleep).

Runtime logs are truncated in place every 2 hours to keep `/data/local/tmp` bounded without moving log paths away from the Android-side diagnostics workflow.

**Known Limitations**:
- `/proc/config.gz` parsing handles disabled PSI bounds cleanly but depends on kernel visibility.
- `/proc/<pid>/status` gaming extraction requires standard namespace visibility which may be obfuscated by severe zygote isolation environments in bleeding-edge AOSP forks.

## Repacking as a Magisk / KernelSU Module

Once compiled, you can easily package AIThermal-Rust as a flashable `.zip` for Magisk or KernelSU.

### Directory Structure

Ensure your file tree looks like this before compressing:

```
thermalai_rust_module/
│
├── META-INF/
│   └── com/
│       └── google/
│           └── android/
│               ├── update-binary    # Core installer logic (shell)
│               └── updater-script   # Legacy updater wrapper
│
├── system/
│   └── bin/
│       ├── thermalai-daemon             # The compiled Rust background service
│       ├── thermalai-detect             # Hardware discovery CLI tool
│       └── thermalair                   # The main CLI for controls & telemetry
│
├── config/                          # Optional default configs mapped here
│
├── module.prop                      # Module metadata (name, version, code)
├── service.sh                       # Late-start boot script to run the daemon
└── uninstall.sh                     # Cleanup logic on module removal
```

### Packaging Instructions

1. **Verify Binaries**: Run `build.ps1` (Windows) or `build.sh` (Linux). The packaging step places `thermalai-daemon`, `thermalai-detect`, and `thermalair` under `system/bin/` inside the ZIP.
2. **Zip the contents**: Use the build scripts so `customize.sh`, `sepolicy.rule`, LF line endings, and Android ARM64 ELF validation are applied consistently.

   Using the terminal:
   ```bash
   7z a -tzip AIThermal-Rust-v3.0.1.zip META-INF system config module.prop service.sh customize.sh sepolicy.rule uninstall.sh
   ```
3. **Flash**: Move the `.zip` to your Android device and install it through the Magisk or KernelSU manager app.
4. **Verify**: Upon reboot, you can verify the module is active by opening a root shell (e.g., via Termux or ADB shell) and running `su -c thermalair status`.
