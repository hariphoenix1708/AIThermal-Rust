# ThermalAI - Intelligent Thermal Manager (Rust Edition)

ThermalAI is an advanced, AI-driven thermal and charging orchestrator for Android devices, originally built as a complex shell script and now being rewritten entirely in Rust 2024 for maximum performance, memory safety, and minimal background overhead.

## Architecture

The project is structured as a Magisk/KernelSU module with a native Rust daemon taking over runtime responsibilities.

### Components:
* **`thermalai-daemon` (`daemon.rs`)**: The core background service. Manages safe initialization, PID tracking, lock file crash protection, heartbeat scheduling, and signal handling (`SIGINT`/`SIGTERM`) to guarantee stock thermal logic is restored upon exit.
* **`thermalai-detect` (`discovery.rs`)**: A standalone dynamic hardware discovery utility. It scans sysfs/procfs to detect CPU, GPU, Network, and Storage capabilities, emitting a universal `hardware_profile.json` so the runtime never relies on hardcoded paths.
* **`thermalair` (`thermalair.rs`)**: The Command-Line Interface (CLI). Allows users to query live status, read logs, start/stop the daemon with PID health validation, trigger charging modes (adaptive/urgent), view session histories, and inspect calibration states.
* **Runtime Tuning (`runtime_tuning.rs`)**: Responsibly and reversibly applies network, VM, touch, GPU, and IO scheduler tweaks cleanly. It captures original system states before gaming and restores them after.
* **Charging Modes**: Uses a 17-band SOC curve with dynamic thermal throttling. Users can select `adaptive` for smart battery care, or `urgent` for high-speed charging overrides.
* **Shell Parity Modules**: The original `thermalai_rust/scripts/` directory is now deprecated and kept *only as a reference*. Features like self-calibration, hardware snapshots, charging logic, and gaming heuristics have been fully rewritten into Rust.

### Telemetry & State Variables
All runtime state files are stored securely using atomic swaps to prevent corruption. Paths are dynamically resolved, typically under `/data/local/tmp/thermalai_state` or the module directory:
* **`THERMALAI_STATE_DIR`**: Root for state/history data (`thermalai_state.json`, `charging_session.json`, `charging_mode.json`, `calibration.json`).
* **`THERMALAI_LOG_DIR`**: Root for the logs (`thermalai.log`, `thermalai_verbose.log`, `thermalai_startup.log`).
* **`THERMALAI_MODULE_DIR`**: Automatically resolved by CLI to the Magisk/KernelSU installation path to trigger scripts like `service.sh`.

## Windows 11 Development Setup

If you are developing ThermalAI on Windows 11, follow these instructions carefully to compile the native Android `aarch64` binaries. Even if you've never coded in Rust, these steps will guide you through compiling the application successfully.

### Prerequisites

1. **Git**: Download and install Git from [git-scm.com](https://git-scm.com/).
2. **Rust (The Programming Language)**: Download and run `rustup-init.exe` from [rustup.rs](https://rustup.rs/). Follow the default prompts.
   - Once installed, open PowerShell and run:
     ```powershell
     rustup update
     rustup default stable
     ```
3. **Visual Studio 2022 Build Tools**: Required by Rust on Windows.
   - Download the [Visual Studio Installer](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and install the **"Desktop development with C++"** workload.
4. **Android NDK**: This provides the C/C++ compiler needed to build binaries that Android understands.
   - Download the latest stable NDK (e.g. `r27d`) from [Android Developers](https://developer.android.com/ndk/downloads).
   - Extract the `.zip` file to a simple location, specifically: `C:\Android\android-ndk-r27d`.
5. **Android SDK Platform Tools** (optional): For pushing the compiled binaries to your device using `adb`.

### Environment Setup

1. Add the Android aarch64 target to your Rust toolchain:
   ```powershell
   rustup target add aarch64-linux-android
   ```
2. Set your environment variables to point to the extracted NDK. Open PowerShell as Administrator and run:
   ```powershell
   [System.Environment]::SetEnvironmentVariable('ANDROID_NDK_HOME', 'C:\Android\android-ndk-r27d', 'User')
   [System.Environment]::SetEnvironmentVariable('ANDROID_NDK_ROOT', 'C:\Android\android-ndk-r27d', 'User')
   ```
*(Note: You do **not** need to manually add the NDK toolchain to your PATH or create wrapper symlinks. The `build.ps1` script will automatically discover your NDK installation via `ANDROID_NDK_HOME` and supply the correct `aarch64-linux-android34-clang.cmd` wrapper directly to Cargo. This ensures broad API compatibility from Android 14 up to Android 17.)*

### Build Instructions

From the root of the project, use the provided PowerShell build script to compile the project. It automatically resolves the NDK toolchains and copies the final binaries to the `bin/` directory:

```powershell
.\build.ps1
```

If you wish to run the commands manually:
```powershell
cd rust
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --target aarch64-linux-android
cargo build --release --target aarch64-linux-android
```

*Note: New development continues from `rust/src/`. Legacy shell logic is kept only as a reference for parity checks.*
The compiled binaries are produced under `rust\target\aarch64-linux-android\release\`. The build scripts place `thermalai-daemon`, `thermalai-detect`, and `thermalair` into `system/bin/` inside the packaged module.

## Module Directory Layout

* `META-INF/`: Magisk/KernelSU installer scripts.
* `system/bin/`: Contains the packaged native `thermalai-daemon`, `thermalai-detect`, and `thermalair` binaries.
* `config/`: Contains `profiles.conf` (TOML) and `game_list.conf`.
* `rust/`: The complete Rust 2024 workspace.
* `service.sh`: Android boot script that exports module/config/log/state paths, requests stock thermal service stop, launches the Rust daemon, and validates the daemon PID before reporting success.
* `uninstall.sh`: Android uninstall script that kills the daemon and cleans up state.
* `module.prop`: Standard Magisk metadata.
* `build.sh` / `build.ps1`: Automated build and packaging scripts.

## Changelog

### v2.0.0 (Rust Rewrite Finalization)
- **Architecture**: Replaced the shell-script core with a fully modular, compiled Rust daemon (`thermalai-daemon`).
- **Hardware Discovery**: Implemented a dynamic discovery layer (`thermalai-detect`) that replaces hardcoded sysfs nodes. Supports CPU topology (EAS/WALT), GPU Devfreq, Storage IO schedulers, Memory (PSI/ZRAM), and Network tunables.
- **Adaptive Polling**: The daemon now scales its polling sleep interval dynamically based on prediction trend scores, eliminating unnecessary wakeups.
- **State Management**: Moved all runtime state (snapshots, calibration, game profiles, telemetry) to `/data/local/tmp/thermalai_state` securely using atomic temp-file swaps.
- **Charging**: Full 17-band SOC-aware charging algorithm with strict thermal bounds and session recovery learning.
- **Policy Engine**: Dropped static bands for a multi-factor score-based system mapping to performance/balanced/conservative/powersave/emergency modes.
- **CLI Utilities**: Added `thermalair` for interactive real-time telemetry querying, starting/stopping, calibration/history reporting, and `thermalai-detect` for hardware capability dumping.
- **Runtime State**: All files (`thermalai_state.json`, `charging_session.json`, `charging_mode.json`, `calibration.json`) use atomic swaps.
- **Logging**: Supports distinct `thermalai.log` and `thermalai_verbose.log` outputs.
