# Changelog

## v3.3.11 (versionCode 371)
### Fixed — WiFi/mobile-data handoff stability
- **Handoff-aware RPS refresh**: GameTurbo now rechecks active WiFi and rmnet
  RX queues during an active game. A switch after game entry no longer leaves
  the new transport without RPS, avoiding CPU0 softirq-driven ping spikes.
- **WiFi low-latency follows the transport**: The framework WiFi low-latency
  mode is now acquired only while WiFi is active and released on cellular,
  avoiding unnecessary scan/roam suppression during a mobile-data session.
- **Removed unsafe shell network mutations**: The legacy wrapper no longer
  changes Android-managed DNS properties, persistent radio fast-dormancy flags,
  IRQ affinity, TX queue length, congestion control, or global TCP/core sysctls.
  Those writes could race ConnectivityService/netd during a transport handoff.
  It retains a backup-based one-time migration restore for settings written by
  v3.3.10 and earlier.
- **Preserve kernel socket buffer defaults**: GameTurbo no longer overrides
  global `rmem_default` / `wmem_default`; per-network socket policy remains
  with the kernel and Android network stack.
- **Accurate diagnostics**: ICMP probes now reject delayed replies from a prior
  sequence and calculate jitter in packet-arrival order instead of sorted RTT
  order, so handoff instability is no longer hidden in telemetry.

## v3.3.10 (versionCode 370)
### Fixed — Network tuning backup race condition and deconfliction
- **Removed `apply_network_buffers()` from `advanced.rs`**: Was writing
  tcp_rmem/tcp_wmem values WORSE than kernel defaults (512K/1M vs 2M/6M),
  causing TCP window scaling degradation during gaming. Network buffer tuning
  is now handled exclusively by the shell script with proper backup/restore.
- **Simplified `tweak_network_buffers()` in shell script**: Reduced from 13
  sysctl writes to 3 — only `netdev_budget` (300→600), `busy_poll` (0→50),
  and `busy_read` (0→50). All other values (tcp_rmem, tcp_wmem, rmem_max,
  wmem_max, netdev_max_backlog, dev_weight, tcp_fastopen) were redundant
  or downgrades from kernel defaults.
- **Made `tweak_wifi_power_save()` and `tweak_rps()` shell no-ops**: These
  were duplicate-writes conflicting with Rust GameTurbo's
  `activate_wifi_ps()`/`activate_rps()`, which has proper save/restore.
  Eliminates dual-backup race conditions where the second saver captured
  already-modified values.
- **Added network sysctl restoration to `uninstall.sh`**: Belt-and-suspenders
  fallback to restore kernel defaults (netdev_budget, busy_poll, busy_read,
  tcp_low_latency, rps) if daemon doesn't restore cleanly on uninstall.
- **Comprehensive Rust + shell script audit**: Full audit of all network
  sysctl/sysfs writes across the codebase. Identified 4 LOW-severity
  remaining dual-write paths (WiFi PS, RPS, tcp_low_latency,
  tcp_congestion_control) — all idempotent and restore-safe due to correct
  boot-ID tracking and execution ordering.

## v3.3.9 (versionCode 369)
### Fixed — rmnet interface detection and stale backup cleanup
- **Shell RPS detection for rmnet**: `tweak_rps()` in `tweak_network_gaming.sh`
  checked `operstate=="up"` but rmnet interfaces on SM8635 report
  `operstate="unknown"` even when active. RPS was never applied to mobile
  data interfaces. Now falls back to `carrier==1` check.
- **Shell interface detection in `detect_network_quality.sh`**:
  `detect_active_interface()` returned "none" when gaming on mobile data
  because rmnet operstate is "unknown". Now checks `carrier==1` as fallback.
- **Shell network type in `detect_codm_servers.sh`**:
  `detect_network_type()` returned "none" for rmnet. Now checks `carrier==1`.
- **Rust `check_network_quality()`**: Only checked `operstate=="up"`, returning
  false for rmnet interfaces. Now checks all active interfaces (wlan0,
  rmnet_data0-3) with operstate OR carrier fallback.
- **Rust `read_wifi_active()`**: Same operstate-only issue. Now uses carrier
  fallback via shared `iface_is_active()` helper.
- **Rust `activate_rps()` in `game_turbo/network.rs`**: Was using
  `Path::exists()` on operstate file (worked but semantically inconsistent).
  Now uses the same `iface_is_active()` helper (operstate OR carrier).
- **Stale backup files**: Backup directory at `$STATE_DIR/network_backup/`
  could retain values from a previous boot where RuntimeTuner or advanced.rs
  had already modified sysctl values before the shell captured originals.
  Now tracks boot ID (`/proc/sys/kernel/random/boot_id`) and clears backups
  on each new boot, ensuring fresh captures of true kernel defaults.

## v3.3.8 (versionCode 368)
### Fixed — Comprehensive audit: critical bugs in network tuning, RPS, and shell scripts
- **`advanced.rs` buffer values restored**: The Rust advanced tuner still had
  pre-v3.3.6 downgraded `tcp_rmem` (4K min vs 512K), `tcp_wmem` (4K min vs
  256K), and `udp_mem` (8x downgrade). Now preserves system defaults, matching
  the v3.3.6 shell fix.
- **RPS on mobile data interfaces**: RPS (Receive Packet Steering) was only
  applied to `wlan0`, leaving mobile data (rmnet_data0-3) unprotected from
  softirq storms on CPU0. Now applies RPS to all active network interfaces
  (WiFi + mobile data) in both Rust and shell code.
- **IRQ affinity restore on disable**: `tweak_irq_affinity` in the shell script
  never restored IRQ affinities when gaming ended — WiFi and modem IRQs
  remained permanently pinned to big cores. Now properly restores from backup.
- **100% packet loss rated "excellent"**: `detect_network_quality.sh` would
  report quality_score=70 ("excellent") when all pings failed (avg=0, loss=100).
  Now correctly returns "unreachable" / score=0.
- **Quality rating considers RTT**: Previous versions only used jitter for
  quality rating. A connection with 800ms avg but 2ms jitter was rated
  "excellent". Now applies penalty for avg >150ms (-10) and >200ms (-20).
- **WiFi QoS restore**: `tweak_wifi_qos` disable path only restored ath11k
  aggregation but left roaming, APF, scan, and WMM settings permanently
  modified. Now restores all modified values.
- **WiFi PS dumpsys fallback**: `detect_network_quality.sh` returned raw
  dumpsys line instead of clean on/off value.
- **Interface detection**: Both Rust and shell now check rmnet_data0-3 for
  dual-SIM mobile data gaming.

## v3.3.7 (versionCode 367)
### Fixed — Ping stability: WiFi power save and RPS for Qualcomm WCN6750
- **WiFi PS disable via `cmd wifi`**: Previous versions targeted
  `/sys/module/iwlmvm/parameters/power_save` (Intel) and
  `/sys/class/net/wlan0/power_save` — neither exists on Qualcomm WCN6750.
  WiFi PS was never actually disabled during gaming. Now uses
  `cmd wifi force-low-latency-mode enabled` which is the correct Android
  framework API for Qualcomm WiFi chipsets. Tested: reduces avg ping from
  23ms to 18ms, mdev from 11.3ms to 6.7ms.
- **RPS (Receive Packet Steering) during gaming**: All WLAN interrupts on
  SM8635 land on CPU0 (NET_RX: 147K on CPU0 vs 3K on CPU1). Without RPS,
  thermal/GameTurbo processing on CPU0 causes softirq storms and ping
  spikes. Now enables RPS (rps_cpus=ff) + flow steering (32768 entries)
  during gaming. Tested: combined with low-latency mode, reduces mdev from
  11.3ms to 2.5ms (78% jitter reduction), max from 64.6ms to 24.8ms.

## v3.3.6 (versionCode 366)
### Fixed — Network connectivity during game opening and in-match
- **Removed `tcp_timestamps=0`** (shell + Rust): This was disabling TCP window
  scaling (limited to 64KB windows), breaking RTTM accuracy, and causing game
  server connections to fail/stall during game opening. TCP timestamps are
  required for window scaling on Linux — the 12-byte savings per packet was
  negligible compared to the throughput loss.
- **Removed `tcp_delack_min=0`** (shell + Rust): This doubled the TCP packet
  rate on WiFi (every packet gets immediate ACK), causing airtime contention
  and increased latency. On SM8635 this sysctl is `__UNWRITABLE__` anyway.
- **Removed `tcp_init_cwnd=10`** (shell + Rust): Default is already 10 on
  modern kernels. Setting cwnd>3 during SYN can break some game servers.
- **Fixed `tcp_rmem` downgrade**: Gaming value was `4096 131072 16777216`
  (min=4K, default=128K) — 32x smaller than system default (512K/1MB). Now
  preserves system defaults: `524288 1048576 16777216`.
- **Fixed `tcp_wmem` downgrade**: Gaming value was `4096 65536 8388608`
  (default=64K, max=8MB) —8x smaller default, max halved. Now preserves
  system defaults: `262144 524288 16777216`.
- **Fixed `udp_mem` 8x downgrade**: Gaming value was `32768 65536 131072`
  (~512MB) vs system default `268212 357617 536424` (~2GB). CODM uses UDP
  for real-time game data — reduced memory caused packet drops during matches.
  Now preserves system defaults.

## v3.3.5 (versionCode 365)
### Added — GameTurbo Phase 3: Display & GPU optimizations
- **GPU frequency floor during gaming**: New `gpu_freq` submodule in
  `game_turbo/` saves current GPU power level on game entry and sets to
  best (lowest latency) level. Restored on game exit. Configured via
  `game_turbo_gpu_freq_boost` (default: true).
- **GPU load-aware adaptive governor**: `decide_tier()` now accepts GPU
  load as input. When GPU load > 90%, forces Max tier. When GPU load >
  80%, blocks Eco/Balanced demotion — prevents CPU frequency scaling from
  starving a GPU-bound game.
- **Refresh-rate-aware jank threshold**: `UiMonitor` jank warning now
  uses dynamic frame budget (1000/refresh_hz) instead of hardcoded 16.7ms,
  correctly detecting jank on 60/90/120Hz displays.
- **Frame time histogram in telemetry**: New fields `frame_p50_us`,
  `frame_p90_us`, `frame_worst_us`, `frame_max_consecutive_jank` in JSON
  telemetry output for richer frame pacing visibility.
- **Frame pacing metrics in FrameStats**: Added `p50_frame_ns` and
  `max_consecutive_jank` to `FrameStats` struct. `compute_stats_from_durations`
  now tracks consecutive jank in presentation order.

## v3.3.4 (versionCode 364)
### Fixed — Silent error drops and incomplete error handling from codebase audit
- **GPU power level writes silently discarded**: `apply_gpu_power_level()` return
  was discarded via `let _ =` in two critical paths (game exit heat-shed and main
  policy actuation). Now logs `WARN` on failure so GPU power state issues are visible.
- **WiFi PS state inconsistency**: If the sysfs write to disable WiFi power-save
  failed, the state still recorded it as modified. On deactivate, a "restore" would
  write the original value to a setting that was never changed. Now only tracks the
  path/original when the write succeeds.
- **I/O scheduler writes silently ignored**: `write_str()` return values in
  `io_scheduler.rs` were discarded during activation. Now logs `WARN` on failure
  so blocked scheduler boosts are visible.
- **Touch IRQ restore ignoring failure**: `restore_scheduler()` called
  `set_scheduler()` but ignored the bool return. If restoring an IRQ thread's
  scheduling policy fails, it stayed at SCHED_FIFO permanently with no log. Now
  logs `DEBUG` on failure.
- **PID race skipping entire GameTurbo session**: If `confirmed_pid` was `None`
  at game detection (common with Zygote-forked games), GameTurbo activation was
  skipped for the entire session. Now retries activation on subsequent ticks when
  the PID becomes available.
- **Unnecessary `Vec::clone()` in charging voter iteration**: Three
  `for node in &self.voter_nodes.clone()` patterns cloned the entire voter list
  just to iterate. Changed to `for node in &self.voter_nodes`.
- **Magic battery thermal thresholds**: Hardcoded temperature values (42, 44, 46,
  48, 50°C) replaced with named constants (`BATTERY_TEMP_HOT_THRESHOLD`,
  `BATTERY_TEMP_THERMAL_STEP_1` through `_4`).
- **GovernorManager::discover_hardware**: Changed return type from `Result<()>`
  (which could never fail) to `()` to avoid misleading error handling at call site.

## v3.3.3 (versionCode 363)
### Fixed — Log noise and uclamp diagnostics from v3.3.2 on-device logs
- **uclamp.max ERANGE logging**: Both `"512"` and `"50"` fail on
  `/dev/cpuctl/background/cpu.uclamp.max` on SM8635. Now reads the current
  value before writing and logs a `WARN` with the current value and attempted
  values when the kernel rejects both. Cgroups where the write fails are
  excluded from the "clamped N cgroups" count (tracked separately as
  "skipped").
- **Thread affinity restore log spam**: Game exit tried to restore affinity
  for ~29 threads that had already been killed by the game engine, producing
  29 individual `DEBUG` "No such process" messages. Now batches failures
  into a single summary line: `"restored 0/29 threads, 29 dead (skipped)"`.

## v3.3.2 (versionCode 362)
### Fixed — Critical bugs from v3.3.1 on-device log analysis
- **Network buffer downgrade (CRITICAL)**: GameTurbo's `GAMING_NET_TUNABLES`
  wrote `rmem_max=256KB` and `wmem_max=256KB`, overwriting the shell script's
  optimal values (16MB and 8MB respectively). Removed all entries from
  GameTurbo that conflict with `tweak_network_gaming.sh` — the shell script
  handles network buffer tuning correctly with full backup/restore.
- **I/O scheduler tuning ram devices**: `ram0`-`ram15` (RAM disk block
  devices) were being set to `none` I/O scheduler. Added `"ram"` to
  `SKIP_PREFIXES` so only real storage is tuned.
- **`uclamp.max` ERANGE on SM8635**: Writing `"512"` to
  `/dev/cpuctl/background/cpu.uclamp.max` failed with errno 34 (ERANGE) on
  some Qualcomm kernels that use 0-100 percentage range instead of 0-1024.
  Added automatic fallback: tries `"512"` first, then `"50"` if ERANGE.
- **Battery drain stuck at -360%/hr at 100% SOC**: When battery reached 100%
  and stopped charging, the cached drain rate from the charge-up phase
  persisted indefinitely. Now returns `None` when SOC=100% and not charging.
- **Game profile stats always zero**: `avg_session_peak_temp`,
  `last_jank_pct`, and `last_p90_ms` in per-game profiles were never updated
  because the orchestrator passed hardcoded 0.0 values. Added session-level
  worst-case tracking via `RuntimeContext` (`game_session_worst_jank_pct`,
  `game_session_worst_p90_ms`) and wired it to `record_game_turbo_session`.
  `avg_session_peak_temp` now computed from all sessions (not just GameTurbo).

## v3.2.34 (versionCode 354)
### Fixed — Network probe targets and battery drain calculation
- **RTT regression fix**: Reordered ICMP ping targets — DNS resolvers
  (8.8.8.8, 1.1.1.1) first, CODM Activision CDN IPs after. DNS servers
  are anycast and return nearest PoP, giving true network RTT from user's
  location. Activision IPs are region-specific (EU-West, NA-Central) and
  gave ~200ms RTT from India — now DNS servers give ~10-30ms.
- **Early exit threshold**: Changed from `targets_tried >= 3` (hard cap)
  to `avg_rtt < 80ms && targets_tried >= 2`. Only exits early on genuinely
  good RTT, allowing more targets to be tried on slow connections.
- **Battery drain always showing `?%/hr`**: Added `last_drain_sample` field
  to `BatteryStatsTracker`. Drain is now computed between samples where
  SOC actually changed (every few minutes), not between consecutive 1-second
  ticks where SOC almost never changes. Cached drain rate is displayed
  between SOC changes.
- **Quality score now factors in jitter tier**: Score formula now adds
  bonus for excellent jitter (+30 for S+, +20 for S, +10 for A) and
  penalizes very high RTT (>200ms: -20, >150ms: -10). A connection with
  SPlus jitter (0.5ms) but 195ms RTT now scores 70 instead of 40.
- Fixed stale doc comment on `probe_quality`.

## v3.2.33 (versionCode 353)
### Fixed — Critical bugs from v3.2.32 device log analysis
- **Tick loop stall (32s) caused by ICMP ping blocking**: Reduced ICMP
  timeout from 2s to 500ms per packet, reduced ping count from 10 to 5,
  and added early termination (stops after first working target or 3
  targets tried). Worst-case probe time: ~7.5s (was ~60s+). Typical: <2s.
- **Empty interface name in probe**: Interface detection now also checks
  `/sys/class/net/*/carrier` (value=1) as fallback when `operstate`
  is not "up". Some kernels show "unknown" during WiFi association
  even though the interface is functionally usable.
- **WiFi reads used hardcoded wlan0**: Now uses the detected interface
  name for RSSI, frequency, and power_save sysfs reads.

## v3.2.32 (versionCode 352)
### Improved — CODM Bullet Registration & Low-Latency Gaming
- **TCP low-latency optimizations (Rust + shell)**: During gaming, disables
  TCP timestamps (saves 12 bytes/pkt overhead), disables delayed ACK
  (eliminates 40-200ms Nagle delay for game packets), enables
  `tcp_low_latency` hint, and sets initial congestion window to 10
  for fast connection ramp-up. All restored on game exit.
- **TCP_NODELAY equivalent via sysfs**: `tcp_delack_min=0` effectively
  disables Nagle's algorithm for new connections — critical for CODM
  where position updates and shot packets are small and time-sensitive.
- **WiFi QoS/WMM tweaks**: Disables AMSDU/AMPDU aggregation during
  gaming (reduces jitter from frame batching), enables WMM priority,
  sets roaming aggressiveness to maximum, disables APF (Android Packet
  Filter) which adds latency on some chipsets.
- **Congestion control auto-selection**: Auto-detects and enables
  BBR > Westwood+ > HTCP (in priority order) if available on the
  kernel. These algorithms handle WiFi/LTE latency better than cubic.
- **CODM game server ping targets**: ICMP ping now targets Activision
  CDN infrastructure (EU, NA, Asia) in addition to DNS fallbacks,
  giving more relevant latency data for bullet registration scoring.
- **Faster network probes**: Probe interval reduced from 30s to 10s
  during gaming for quicker detection of network quality changes.
  Ping count reduced to 10 packets at 100ms interval (1s total).
- **touch_network_stack enabled by default**: The Rust-side TCP
  optimizations are now on by default (was off since v3.1.0 connectivity
  regression). All writes are idempotent and capability-probed.
- **tcp_keepalive_time**: Lowered from 1200 to 600 for gaming — faster
  dead-connection detection prevents stale sockets blocking game state.

## v3.2.31 (versionCode 351)
### Fixed
- **ICMP ping RTT=0 bug**: `icmp_ping_one` was using the send-time variable
  (`now_us`) instead of capturing the receive time, making RTT always ~0.
  Now uses `Instant`-based receive timestamp for accurate measurement.
- **Battery drain always 0.00%/hr**: `BatteryStatsTracker` switched from
  `chrono::Utc` (coarse clock resolution on Android) to `Instant` for
  inter-sample timing. Also returns `None` when SOC delta is 0 instead of
  reporting 0.00% (cleaner, avoids polluting logs with zero-noise).
- **ROM detection mismatch**: Shell scripts used `grep -qi "xiaomi\|poco\|redmi"`
  which fails on Android's ToyBox (no alternation support). Replaced with
  `case` statements in all 3 scripts — now consistently detects HyperOS.
- **Network tweak false "rejected"**: `write_if_different` now normalizes
  whitespace (tabs→spaces) before comparing, so proc multi-value entries
  with tabs are not falsely flagged as rejected.
- **Network tweak IRQ failures logged as success**: `tweak_irq_affinity`
  now checks `backup_and_write` return value and logs actual failure
  with the real affinity value.
- **Network tweak targeting dummy interfaces**: `tweak_txqueuelen` now only
  tunes `wlan0` and `rmnet_data*` interfaces instead of all netdevs
  (dummy0, erspan0, gre0, etc. were being incorrectly tuned).
- **Network tweak DNS not restored on disable**: `tweak_dns` now saves
  original DNS values on enable and restores them on disable.
- **Network tweak backlog/keepalive direction**: Gaming `netdev_max_backlog`
  kept at device default (was being lowered to 5000); gaming
  `tcp_keepalive_time` set to 600 (was being raised to 1200).

## v3.2.30 (versionCode 350)
### Changed
- **Active network probing rewritten in pure Rust**: ICMP ping via raw sockets
  (`SOCK_RAW` + `IPPROTO_ICMP`), CRC-16 checksum, jitter calculation
  (RFC 3550 mean absolute difference), packet loss measurement. Pings both
  8.8.8.8 and 1.1.1.1, picks the lower-latency target. DNS resolution time
  measured via `getaddrinfo`. No shell delegation for the runtime probe path.
- **Orchestrator no longer shells out for network probing**: `probe_network_quality()`
  now calls `network_diag::probe_quality()` directly instead of spawning
  `detect_network_quality.sh`.
- **Boot-time script removed from service.sh**: `detect_network_quality.sh` is no
  longer called at boot — the daemon runs its own probe on startup.
- **Uninstall cleanup extended**: Network log files (`network_diag.log`,
  `network_tweak.log`, `codm_network_diag.log`) now removed on module uninstall.
- **Clippy clean**: 0 warnings (down from 7). Fixed `manual_abs_diff`,
  `collapsible_if`, `manual_strip`, `if_same_then_else`, `unnecessary_map_or`,
  `too_many_arguments` (suppressed).

### Preserved
- Shell scripts (`tweak_network_gaming.sh`, `detect_network_quality.sh`,
  `detect_codm_servers.sh`) retained for manual diagnostics and boot-time
  network tweaks. `tweak_network_gaming.sh` is still called by the orchestrator
  for game-start/end network tuning (sysctl writes, interface settings).

## v3.2.29 (versionCode 349)
### Added
- **Network quality detection**: Active RTT/jitter/packet-loss measurement
  via `detect_network_quality.sh`. Pings Google DNS (8.8.8.8) and Cloudflare
  (1.1.1.1) with 20-packet samples, computes jitter and loss. Outputs JSON
  report with quality score for daemon consumption. Runs at boot and on
  gaming session start.
- **CODM network diagnostics**: `detect_codm_servers.sh` provides CODM-specific
  network analysis — server ping measurement, bullet registration quality
  assessment based on jitter vs CODM's 30Hz tick rate (33ms interval).
  Categorizes quality as S+/S/A/B/C/D tiers.
- **Gaming network tweaks**: `tweak_network_gaming.sh` applies ROM-conditional
  network optimizations for online gaming. WiFi power-save disable, TCP/UDP
  buffer tuning (256KB UDP receive, 16MB TCP max), DNS fast resolution
  (Cloudflare+Google), NIC IRQ affinity, fast dormancy disable, TX queue
  length 3000. Full backup/restore semantics — every original value is
  saved before modification and restored on game exit.
- **HyperOS vs AOSP ROM detection**: All scripts detect ROM type via
  `getprop ro.mi.os.version.incremental` and `ro.product.brand`. ROM-conditional
  logic for fast dormancy (AOSP sets persist property, HyperOS hints only),
  DNS tuning, and WiFi power-save handling.
- **Rust network diagnostics module** (`network_diag.rs`): Passive sysfs probing
  (interface state, buffer sizes, power-save, RSSI, TX queue length), quality
  scoring with bullet registration assessment (Excellent/Good/Fair/Poor/Bad),
  jitter tiering (S+/S/A/B/C/D), cached quality for orchestrator consumption.
- **Orchestrator integration**: Network quality probe on gaming session start,
  periodic re-probe during gaming (configurable interval, default 30s), gaming
  network tweaks applied on session start and restored on exit.
- **Config options**: `network_diagnostics_enabled`, `gaming_network_tweaks_enabled`,
  `network_probe_interval_sec` (default 30s).
### Compatibility
- All network tweaks are capability-probed and no-op on missing nodes.
- ROM detection works across HyperOS, MIUI, and AOSP custom ROMs.
- WiFi power-save disable uses both `iw` and sysfs fallback.
- IRQ affinity adapts to CPU core count (big-core mask for SM8635: 0xf0).

## v3.2.28 (versionCode 348)
### Fixed
- **Voter node write-failure circuit breaker**: On AOSP custom ROMs
  (where `mi_thermald` is absent and `thermal_fcc_ua` stays at 0),
  the kernel's `qti_battery_charger.c` driver rejects non-zero
  `restrict_cur` writes with EINVAL. BatteryCare and UnderLoad modes
  were writing non-zero values every tick, causing WARN log spam
  (every 2 seconds). Now tracks per-voter consecutive write failures;
  after 3 failures the node is disabled for the rest of the session.
  One-shot clears at session start are unaffected. On HyperOS (where
  `mi_thermald` sets `thermal_fcc_ua` properly), non-zero writes
  succeed and the counter never reaches the threshold — BatteryCare
  mode works as intended.
### Compatibility
- Verified compatible with both HyperOS (Xiaomi stock) and AOSP-based
  custom ROMs. Runtime capability detection adapts to each ROM's
  kernel driver behavior without ROM-type checks.

## v3.2.27 (versionCode 347)
### Fixed
- **Safety: Emergency thermal state protection**: Periodic battery cooling
  device enforcement now skips during `Emergency` state (battery ≥50°C,
  charger ≥70°C, USB ≥65°C, PMIC ≥70°C). Previously, the enforcement
  would zero the kernel's emergency battery thermal mitigation while
  AIThermal was also trying to throttle — suppressing a safety backstop.
- **Safety: Hot battery at plug-in**: `one_shot_clear_restrict()` now
  skips the battery cooling device clear when battery temperature is
  already ≥44°C at plug-in. The kernel's thermal mitigation should remain
  active to protect the battery.
- **Session carryover state**: `rejected_ceiling`, `last_known_good_ma`,
  `consecutive_failures`, `active_limit_ma`, and `previous_target` are
  now reset at session start. Previously, a ceiling learned with one
  charger/cable could throttle the next session with different hardware.
- **Battery cooling timer reset**: `last_battery_cooling_clear` is reset
  to `None` at session start so the first periodic re-check runs
  immediately after the one-shot clear (was inheriting stale timer from
  prior session).
- **Voltage_max false positive**: The "Slow charger detected" warning
  (which fired on every session where voltage_max showed 5V before QC/PD
  renegotiation completed) is now an informational note explaining the
  value may be stale after the battery cooling device clear.
### Changed
- Updated stale comments that framed `restrict_chg`/`restrict_cur` as the
  root cause of slow charging — now correctly described as secondary
  contributors (primary cause is the battery thermal cooling device).
### Removed
- Dead code: `taper_started_at` field (never read), `Default` impl for
  `ChargingEngine` (no callers), unreachable `Disconnected` match arm.
- Reduced clippy warnings from 9 to 7 (removed 2 dead-code warnings).

## v3.2.26 (versionCode 346)
### Fixed
- **Primary slow-charging fix**: On Xiaomi SM8635 (peridot), the kernel
  thermal framework's battery cooling device (`cooling_device41/cur_state`)
  is the actual throttle mechanism — NOT `restrict_chg`/`restrict_cur`
  (which are already 0). The battery cooling device forces
  `voltage_max=5V`, blocks QC/PD voltage negotiation, and caps charge
  current to ~900mA even with a 40W charger. Confirmed by RedFox Kernel
  Manager's "Bypass Thermal Limit" (clears `cur_state` → 0) which
  instantly restores 40W charging (9516mA/40.6W peak).
- Added `battery_cooling_path` field to `ChargingEngine` — discovered at
  init by scanning cooling devices for `type=battery`. At session start,
  AIThermal now clears the battery cooling device to 0 (one-shot).
- Added periodic enforcement: every 15 seconds, re-checks
  `cur_state`; if the stock thermal engine has re-set it, clears it
  back to 0. This prevents mi_thermald/thermal-engine from re-throttling
  charging during the session. AIThermal's own thermal management
  (Emergency at 50°C, ThermalThrottle at 44-48°C) replaces the kernel
  battery cooling device.
- Added battery cooling device state to diagnostic dump — logs
  `cur_state` value and warns when it's non-zero (root cause of slow
  charging).

## v3.2.25 (versionCode 345)
### Fixed
- Slow charging root cause: On Xiaomi SM8635 (peridot), the kernel
  thermal framework sets `restrict_chg=1` + `restrict_cur=<limit>` to
  cap charge current, but the idempotent probe (read→write same value)
  falsely marks `restrict_cur` as read-only. Added non-idempotent probe
  fallback (write "0") in `probe_charging()` so the node is added to
  `voter_nodes` when it actually accepts different values.
- One-shot restrict clearance: At each session start
  (Disconnected→Normal), AIThermal now writes `restrict_chg=0` (disables
  enforcement) and `restrict_cur=0` (clears any residual cap) ONCE.
  Writing `restrict_chg` every tick caused SPMI bus contention with the
  display controller on SM8635; writing it once at session start is safe.
- Diagnostic coverage: `dump_charger_diagnostics()` now reads and logs
  `restrict_chg`, `restrict_cur`, `charging_enabled`, and
  `system_temp_level` values (even if read-only) so the user can see the
  kernel's charging state. Warns when `restrict_chg=1` is the root cause
  of slow charging.
- Shutdown cleanup: `release_voters_on_shutdown()` now also clears
  `restrict_chg` and `restrict_cur` to "0" even if they are not in
  `voter_nodes`, preventing stale caps from persisting across daemon
  restarts.

## v3.2.24 (versionCode 344)
### Fixed
- Battery stats screen-time drift: screen_on/deep_sleep/awake accumulators
  now use real wall-clock elapsed time between samples instead of the
  daemon's intended sleep duration, which can diverge during Doze.
- UI jank false alarms: jank warning now compares delta frame counts
  between samples instead of cumulative process-lifetime gfxinfo counters.
  A single slow frame no longer triggers warnings for the rest of the
  app's foreground lifetime.
- Charging tier consistency: MaxSpeed/Urgent and UnderLoad 9800mA tier at
  SoC<20% lowered to 9000mA to match the apply_limit() clamp ceiling.

## v3.2.23 (versionCode 343)
### Fixed
- Watchdog blind spot: `write_capability()` (cpuset, GPU governor/power-level
  writes) now feeds `LEGACY_WRITE_FAILURES` on error, so the watchdog can
  detect sysfs write floods from the newer capability-validated path.
- Post-game cooling diagnostic: `evaluate_post_game_cooling()` was always
  called with peak_temp=0 because the value was zeroed before the call. Added
  `last_session_peak_temp` to RuntimeContext to preserve the peak across the
  zeroing boundary.
- Frame sampler freeze: `BackgroundFrameSampler` now only refreshes its
  slot when actual values (sample_count, janky_frames, p90, worst) change.
  Stale dumpsys output no longer re-stamps captured_at, so the existing 12s
  staleness guard can correctly fall back to utilization-only governor.
- EmergencyCool fast-exit: Leaving EmergencyCool no longer rides out the
  generic debounce window — once the thermal ladder determines the emergency
  has passed, max-throttle is released immediately.
- Game session timing: `game_session_started_at` and daemon stall timer
  (`last_tick_completed`) converted from `Instant` (CLOCK_MONOTONIC, freezes
  during Doze) to `SystemTime` for accurate wall-clock durations across
  suspend. Game session duration, stutter grace period, and stall detection
  now correctly account for screen-off gaps.
- Render thread pin retry: `pin_critical_render_thread()` now retries for
  15s after game detection instead of a single one-shot attempt, closing
  the gap where game engines spawn RenderThread after the detection tick.
- `drop_cache(false)` rate-limited to once per 30s to prevent forced
  re-faulting on every Powersave tick during sustained memory pressure.
### Changed
- Charging tier values: MaxSpeed/Urgent tiers capped at 9800mA (was 18000)
  to match the `apply_limit()` clamp ceiling, and the hard clamp tightened
  from 12A to 9A for a safer single-cell 5000mAh pack rating.

## v3.2.17 (versionCode 337)
### Fixed
- Mid-game policy flip-flop (7 drops in a single 20-min session on the
  reference device) persisted through v3.2.16: the gaming drop-latch released
  to Balanced whenever the score held above 25 for 3 ticks, but legitimate
  mid-game heat (composite ~50C) routinely scored 25-33, so a rendering game
  was still softened and churned back up repeatedly. The latch threshold now
  sits at the Conservative boundary (40): a Balanced-band gaming score is held
  at Performance, and only a score that genuinely crosses into Conservative
  territory can soften the game (clamped to Balanced by the gaming floor while
  composite stays below `temp_hot`). Thermal protection is preserved — drops
  still happen right below the hot cliff (~56C on the reference device).
- Version banner mismatch: the binary logged "3.2.15" while the module
  reported v3.2.16 because `rust/Cargo.toml` was never bumped when the module
  version advanced. `Cargo.toml` now matches `module.prop` (v3.2.17/337).

## v3.2.16 (versionCode 336)
### Fixed
- Mid-game policy flip-flop (Performance <-> Balanced every ~15-30s) as
  temperatures climbed. The per-game `game_modifier` was gated behind the
  `game_profiles.json` lookup, and profiles are only written at session END, so
  a game's first session ran with `game_modifier=0.0` — no `known_hot` relief
  and, critically, no frame-stutter mitigation. On this device the COD session
  hovered right on the `Balanced` score boundary, so the policy churned between
  `performance` and `walt` governors mid-match (visible CPU drops to ~1.1 GHz on
  the big cluster in the UI log). Fixes:
  - Frame-stutter mitigation now applies to EVERY game, profiled or not. When
    `detect_frame_stutter()` confirms heavy rendering (KGSL busy >95%), the
    modifier pulls the score DOWN (-15) instead of pushing it up, keeping a
    visibly-rendering game in `Performance` rather than softening it under its
    own heat load. The old `+15` was inverted — it actively pushed the score
    toward `Balanced`/`Conservative` exactly when the game needed the headroom.
  - Added a symmetric gaming return-latch: once softened to `Balanced`
    mid-game, the engine requires two consecutive confirmed-cool ticks before
    flipping back to `performance`, so a single cool dip cannot yank the
    governor up only to reheat and drop again seconds later.
- `compute_game_modifier` still applies `known_hot` (-12), foreground-priority
  and long-session relief only to profiled games, as before.

## v3.2.15 (versionCode 335)
### Fixed
- In-game stutter from the adaptive governor capping the CPU for entire game
  sessions. The governor was still called with the utilization-only fallback
  whenever the framestats capture yielded fewer than `MIN_JANK_SAMPLES`
  durations (this device regularly returns only 3-4 in lobbies/menus), so it
  sat on the `Balanced` mid-frequency cap (1228/1593/1651 MHz) from the moment
  COD launched and never moved — the jank signal could not fire. When the
  frame signal is too thin to prove the game is running smoothly, the
  governor now holds `Max` (full `scaling_max_freq`) instead of trusting CPU
  utilization; only jank==0 over a real sample count is allowed to step the
  tier down. Thermal safety during gaming is unchanged — the policy engine
  still escalates by temperature and the P3 clamps still own `scaling_max_freq`.
- Charging locked at ~900 mA despite a 3 A-capable source. The device exposes
  `/sys/class/qcom-battery/restrict_cur` as a writable voter node and it was
  left at `1000000` (1 A), which caps charge current regardless of the
  negotiated USB-PD/QC contract. `MaxSpeed`/`Urgent` (and gaming `UnderLoad`)
  now write `restrict_cur=0` to clear the cap; the charging log warns at
  session start whenever a positive `restrict_cur` cap is present and explains
  how to clear it. `Adaptive` mode still does not fight HyperOS by design.
- Charging session duration under-reported (e.g. 179 s recorded for a 22-minute
  charge). The session clock used `Instant` (CLOCK_MONOTONIC), which pauses
  while the device dozes during screen-off charging. Switched to wall-clock
  timing so `duration_sec`, samples-per-second and drain rates are correct.

## v3.2.14 (versionCode 334)
### Fixed
- CPU was throttled to the mid-frequency cap for entire game sessions. The
  adaptive governor only trusts the jank signal after `MIN_JANK_SAMPLES` (10)
  parsed frames, but the Android 16 framestats windows on this device yield
  only ~5-9 durations per capture (see the earlier noise note in the code), so
  the threshold was effectively never reached. `decide_tier` fell back to the
  utilization-only branch and idled on `Balanced`, re-applying the
  1228/1593/1651 MHz `scaling_max_freq` cap every sample while policy was
  Performance — starving the CPU mid-game even though the game was janking at
  33-40%. `MIN_JANK_SAMPLES` lowered to 5, matching the gaming-log threshold
  (`frame_count() >= 5`) so the governor actually escalates to Max when jank
  exceeds 15% and releases the cap.
- UI monitor `gfx[n/a]` on every sample. `read_top_window` returns the full
  `mCurrentFocus` component (`pkg/Activity`), but `dumpsys gfxinfo` needs the
  bare package name — so the frame/jank summary parse always failed. The
  monitor now extracts the package before calling gfxinfo; `frames/jank/p50/
  p90/missVsync/slowUI` and the jank WARN lines are live again.
- Display refresh rate read as `?Hz` on Android 16. Modern `dumpsys display`
  no longer emits a literal `refreshRate` token; the active rate is carried as
  `renderFrameRate 120.000002` / `mActiveRenderFrameRate=120.000002` /
  `vsyncRate=...` / `fps=...`, with the number after `=` or in the next token.
  The parser now scans `mOverrideDisplayInfo`→`renderFrameRate`→`refreshRate`
  →`DisplayModeRecord`→`fps=` (in that order) and extracts values
  case-insensitively. Unit-tested against real capture formats.
- Animation scales that read `null` (unset, e.g. `animator_duration_scale`)
  are displayed as `n/a` instead of a raw `null` in the UI log.

## v3.2.13 (versionCode 333)
### Fixed
- UI monitor now actually samples. `UiMonitor::execute` used `Option::is_none_or`,
  which returns `true` when no sample has been taken yet, so `last_sample` stayed
  `None` forever and the monitor early-returned every tick — `thermalai_ui.log`
  was always 0 bytes (first emitted only after the first 5 s gap was wrongly
  satisfied, then the screen-off reset kept clearing it). Switched to
  `is_some_and`, which skips only when a sample was taken within the last 5 s.
- WebUI was still pointed at the old pre-v3.2.12 paths:
  `STATE_DIR="/data/local/tmp/thermalai_state"` and `LOG_DIR="/data/local/tmp"`
  made every dashboard/charging/logs/hardware read fail after logs moved to
  `/data/local/tmp/AIThermal`. `webroot/app.js` now uses the relocated
  `/data/local/tmp/AIThermal` and `/data/local/tmp/AIThermal/state` directories.
- Added the `thermalai_ui.log` stream to the WebUI Logs tab (new "UI" button),
  so the monitor output is viewable in-app instead of only via adb.
- Display refresh-rate detection now works on Android 16 `dumpsys display`
  output. The old parser only matched lines containing the literal token
  `refreshRate`, but modern dumpsys emits the active mode as
  `mActiveDisplayModeInfo: 1080x2400 120.00002Hz`. The parser now scans for
  `mActiveDisplayModeInfo` first, falls back to `refreshRate` lines, then any
  mode line, extracting `<number>Hz`/`<number>fps` tokens (value bounded to
  a sane 0-1000 range).

## v3.2.12 (versionCode 332)
### Changed
- Logs now live under `/data/local/tmp/AIThermal` instead of `/data/local/tmp`:
  every daemon/CLI/tool default (`main.rs`, `thermalair`, `thermalai-detect`,
  `service.sh`, `customize.sh`, `uninstall.sh`, the panic-hook state path and
  the pid/lock files) resolves to the new directory, with state in
  `/data/local/tmp/AIThermal/state`. Installer and README paths updated.
- New `thermalai_ui.log` stream: a dedicated system-process / animation /
  frame-rate / UI monitor samples every 5 s while the screen is on and logs a
  compact single line — current policy, display refresh rate (dumpsys display,
  cached 30 s), top focused window, animation scales (window/transition/animator),
  per-process CPU share for surfaceflinger / system_server / SystemUI / launchers /
  joyose / perfd / mi_thermald / the daemon itself (pidof + /proc stat deltas),
  per-policy CPU governor + current frequency, and a `dumpsys gfxinfo` summary
  (total frames, janky frames + %, 50th/90th percentile, missed vsync, slow UI
  thread). Anomalies emit a `WARN` line (jank > 10%, p90 > 16.7 ms, slow UI
  thread, or an animation scale of 0.0).
- Balanced (screen-on normal usage) now prefers the stock `walt` CPU governor
  with `schedutil` as fallback instead of forcing `schedutil`. On the peridot
  (SM8635) WALT kernel, `walt` carries the vendor's input-boost / load-tracking
  tuned for the 120 Hz UI; generic `schedutil` under-ramps bursty UI workloads
  and shows up as missed frame deadlines. The clamped policies still use
  `schedutil`/`powersave`, and `walt` remains the Performance governor first
  choice — this only restores the vendor tuning for everyday screen-on usage.

## v3.2.11 (versionCode 331)
### Fixed
- Eliminated the UI stutter right after every screen wake. The Suspend policy
  (screen-off power saving) exited only after the 6 s policy debounce measured
  from Suspend entry, so unlocking left every cluster on the bare `powersave`
  governor (min frequency) for up to ~5 s of interaction. Three changes:
  1. Policy engine now exits Suspend immediately on the first tick where the
     screen is on — the exit debounce/hysteresis no longer applies to the
     Suspend->* direction, so the governor flips back to `schedutil` on the
     wake tick itself. Escalations toward Conservative/Powersave/EmergencyCool
     for real heat still win.
  2. The daemon now wakes for a screen-on event within one 250 ms sleep segment
     at every sleep tier (previously only during long idle sleeps), cutting the
     worst-case response from ~2 s to ~250 ms.
  3. Defense-in-depth in the orchestrator: a wake immediately restores the
     `schedutil` governor if the last applied governor was `powersave`, even if
     the actuation throttle (1.5 s) or a policy override would have delayed it.
- Added regression tests for the screen-on Suspend escape, the screen-off hold
  (no boundary flapping) and real-heat escalation on wake.

## v3.2.10 (versionCode 330)
### Changed
- Reworked the installer so the full installation log actually shows during
  install on every manager. KernelSU and KernelSU-Next ignore
  `META-INF/com/google/android/update-binary` and only run `customize.sh`, so
  the version banner, device/ROM/KSU info, log/state clearing and the feature
  summary have moved into `customize.sh` (the single source for both Magisk
  and KernelSU flows). Magisk's `update-binary` now only does the
  extract + permission pass and hands off to `customize.sh`. The banner reads
  the version straight from `module.prop` (`ThermalAI v3.2.10 - Rust Edition`).

## v3.2.9 (versionCode 329)
### Fixed
- Eliminated mid-game CPU/GPU clamps caused by Balanced<->Conservative policy
  flapping (~30 transitions in one COD session). Two changes:
  1. Added a gaming floor: while a game is active and the SoC is below
     `temp_hot` (58C), the policy can never drop below Balanced. Mid-game
     Conservative/Powersave dips (from trend/comfort noise) previously rewrote
     the CPU Fmax cap to 85% and dropped the GPU to its lowest power level in
     the middle of a frame.
  2. GPU power levels were discovered inverted on this device (`default_pwrlevel`
     current=10, min=10, max=0 — lower index = higher performance). The old code
     wrote the raw "min" (10 = power-save) for Performance/Balanced-gaming, so
     the GPU ran at its slowest level for most of the session, and the runtime
     tuner's restore path re-wrote power level 10 whenever the policy wasn't
     literally "Performance". Boost is now derived from the best of the min/max
     pair and applied for the whole gaming session regardless of policy string.
- Fixed the major post-game UI stutter: the learned cooldown (`cooldown_sec=120`
  for known-hot games) forced the CPU to 85% Fmax for two full minutes after
  game exit even at 44C. Cooldown now only holds the clamp while the SoC is at
  or above `temp_warm` (48C) — once the device cools, the clamp releases early
  (and is re-armed only if it reheats inside the window). Cooldown also uses the
  gentler 90% Recovery clamp instead of 85%.
- Fixed a fake EmergencyCool after warm reboots: the score hit 91 at ~50C (a
  +25 trend, a +25 comfort weight and the +6 normal-use guard) which entered
  `Recovery -> Thermal` and clamped the CPU for 45s right after boot. The hard
  clamp states now require real heat: EmergencyCool needs score > 90 AND
  SoC >= temp_powersave (or composite/predicted >= temp_critical), Powersave
  needs score > 65 AND SoC >= temp_hot. A genuine EmergencyCool also now wins
  over the cooldown/recovery Conservative override instead of being silently
  downgraded to 85%.
- Reduced comfort-weight inflation: base 10->5, skin >= 42 was +15 (now +5,
  +10 only at >= 45), battery inflation halved. A warm-but-okay phone no longer
  scores into Powersave territory on comfort alone.
- The game modifier (known-hot -12 etc.) was keyed on the lingering package name
  and leaked into the post-game scoring window after exit; it is now zeroed the
  moment gaming stops.
- Added a daemon keeper to `service.sh`: if the thermal daemon dies mid-session
  (crash, OOM, manual kill), the watcher loop restarts it instead of leaving the
  device unmanaged until the next reboot.

## v3.2.8 (versionCode 328)
### Fixed
- Eliminated mid-game animation stutter caused by the policy flapping
  Performance<->Balanced every ~15-30s during gaming. Two changes:
  1. The stock thermal engine (`thermal_message/sconfig`) is now disabled for
     the entire gaming session and gated on the *gaming state*, not the policy
     name. Previously each Balanced dip mid-game re-armed mi_thermald
     (`sconfig=0`), which re-asserted stock frequency caps on a hot SoC and
     caused repeated dropped frames (21 toggles in one 9.5-minute session).
  2. Added a gaming policy latch: Performance is held against brief score dips
     from noisy trend/comfort terms and only softens to Balanced after the
     score holds above the latch threshold for 3 consecutive ticks. Escalation
     to Conservative/Powersave is never blocked.
- Fixed the calibration offset drifting to -6C during warm-but-flat normal use,
  which masked all gaming heat (the thermal model saw 39C while the SoC was at
  45-55C). Calibration now shifts only on a genuine rising ramp, and the offset
  is reset to 0 at the start of every gaming session so heat is read honestly.
- Softened the post-game recovery clamp so the exit animation/home-screen
  transition right after a game is not capped at 1.7GHz for 20 seconds.
- The adaptive governor no longer trusts jank statistics derived from only 2-4
  parsed frames (dumpsys on Android 16 yields too few durations); it now
  requires at least 10 samples before a jank value can drive a tier decision,
  otherwise it falls back to CPU utilization. Frame sampling cadence lowered
  from 1.5s to 5s to stop spawning up to 4 `dumpsys` processes per cycle while
  gaming.
- GPU load is now read from `kgsl-3d0/devfreq/busy_time` first (confirmed
  working on peridot/SM8635) instead of `gpu_busy_percentage`, which returns
  near-zero during gaming on some HyperOS builds and under-weighted GPU heat in
  the composite temperature.
- Fixed cosmetic `p90=n/ams` log formatting (now `p90=n/a` or `p90=123.4ms`).
- Downgraded the per-tick "framestats parse yielded 0 durations" warning to
  debug level (normal fallback path on newer Android builds).

## v3.2.7 (versionCode 327)
### Changed
- Completely reworked the KernelSU WebUI: modern glassmorphism theme with
  animated ambient gradient background, blurred cards, gradient typography
  and a sliding pill tab indicator.
- Added a full-screen navigation button in the header (uses
  `ksu.fullScreen` on KernelSU, `setDisplayState` on older managers) with
  an expand/compress icon toggle.
- Added horizontal swipe navigation between tabs (left/right), with slide
  transitions that match the swipe direction. Vertical scrolling stays
  native; swipes starting on buttons/tabs are ignored.
- Thermal-zone cards now sort hottest-first with a proportional gradient
  bar; zone names remain HTML-escaped.
- Polling now pauses when the WebUI is hidden (KernelSU visibility hooks +
  `visibilitychange` fallback).

## v3.2.6 (versionCode 326)
### Fixed
- Hardened peridot (POCO F6 / Redmi Turbo 3, SM8635) detection against the
  HyperOS platform-quirk where `ro.board.platform` reports `pineapple`
  instead of the nominal `sun` for the 8s Gen 3. `ro.product.board`
  (`peridot`) is now probed and used as a first-class device corroborator,
  and the matcher accepts every known alias without false-positive on
  genuine SM8650 (`pineapple`) devices.
- Hardware cache schema bumped to v5 to carry the new `product_board`
  identity field; the cache now invalidates if `ro.product.board` changes
  (e.g. ROM swap), matching the existing fingerprint/device/board-platform
  validation.
- Installer (`update-binary`) peridot-family banner no longer warns on the
  target device when HyperOS reports `ro.board.platform=sun` instead of
  `pineapple`; the check now accepts `peridot`/model-ID device names and
  both platform aliases.
- Periodic Joyose suppressor spawned by `service.sh` now records its PID
  and self-exits when the module directory is removed; `uninstall.sh`
  kills it explicitly so no background loop survives module removal until
  reboot.
- KernelSU WebUI thermal-zone list escapes zone-type strings before
  rendering (prevents HTML injection from root-controlled sysfs data).
- Version metadata synced: `package.json` was still on 3.2.4; now 3.2.6
  to match `module.prop` / `Cargo.toml`.
- `thermalai-detect` CSV and `hardware_report.txt` now include
  `ro.product.device` and `ro.product.board` for easier diagnostics.

## v3.2.5 (versionCode 325)
### Fixed
- Reduced AOSP Android 17 UI stutter by holding Balanced during normal screen-on interaction unless temperatures or trends require real thermal tightening.
- Prevented adaptive frequency tier writes from running on the same tick as policy-transition CPU tuning, removing repeated `scaling_max_freq` tug-of-war during scrolling/gameplay.
- Stopped writing KGSL `force_clk_on`, which current Peridot AOSP logs show is rejected by the kernel and then poisoned as unsupported.

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
