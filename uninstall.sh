#!/system/bin/sh
# ThermalAI - Uninstall script (Rust Edition)

MODDIR="${0%/*}"

LOG_DIR="${THERMALAI_LOG_DIR:-/data/local/tmp/AIThermal}"
STATE_DIR="${THERMALAI_STATE_DIR:-/data/local/tmp/AIThermal/state}"

PID_FILE="$LOG_DIR/thermalai.pid"
PID_LOCK_FILE="$LOG_DIR/thermalai.pid.lock"

# Stop daemon if running
if [ -f "$PID_FILE" ]; then
    DAEMON_PID=$(cat "$PID_FILE")
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null
        # Give the daemon a moment to run its own shutdown/snapshot-restore
        # logic before we start removing its files out from under it.
        sleep 1
    fi
fi

# Belt and braces: if the daemon somehow didn't restore the voters
# in its shutdown path, do it here from shell so uninstall NEVER
# leaves the charger throttled.
for node in /sys/class/qcom-battery/restrict_chg \
            /sys/class/qcom-battery/input_suspend \
            /sys/class/qcom-battery/night_charging \
            /sys/class/power_supply/battery/input_suspend; do
    [ -w "$node" ] && echo 0 > "$node" 2>/dev/null
done

# v3.3.11 migration: restore only values recorded by the legacy wrapper.
# Never assume a generic kernel default here: custom ROMs and kernels may use
# different queue/RPS/sysctl policy. Clean daemon shutdown already restores the
# in-memory GameTurbo state, including WiFi low-latency mode and RPS.
if [ -x "$MODDIR/scripts/tweak_network_gaming.sh" ]; then
    "$MODDIR/scripts/tweak_network_gaming.sh" disable "$STATE_DIR" "$LOG_DIR" >/dev/null 2>&1
fi

# Stop the periodic Joyose suppressor spawned by service.sh. It also
# self-exits once the module directory is removed, but kill it now so
# nothing lingers between removal and the next reboot.
if [ -f "$STATE_DIR/joyose_watcher.pid" ]; then
    WATCHER_PID=$(cat "$STATE_DIR/joyose_watcher.pid" 2>/dev/null)
    if [ -n "$WATCHER_PID" ]; then
        kill "$WATCHER_PID" 2>/dev/null
    fi
    rm -f "$STATE_DIR/joyose_watcher.pid"
fi

# The daemon's own SIGTERM handler restores the hardware snapshot on clean
# shutdown. We just need to clean up every file/folder it creates under
# LOG_DIR/STATE_DIR so nothing is left behind after the module is removed.
rm -f "$PID_FILE"
rm -f "$PID_LOCK_FILE"
for f in thermalai.log \
         thermalai_verbose.log \
         thermalai_startup.log \
         thermalai_battery.log \
         thermalai_thermal.log \
         thermalai_charging.log \
         thermalai_gaming.log \
         thermalai_ui.log \
         thermalai_combat.log \
         network_diag.log \
         network_tweak.log \
         codm_network_diag.log \
         ml_features.jsonl \
         ml_model.onnx \
         ml_model.onnx.json \
         ml_model.json; do
    rm -f "$LOG_DIR/$f"
    rm -f "$STATE_DIR/$f"
    # Log rotation may leave .1 / .gz siblings; sweep them too (including incrementing .1-.5 for ml).
    for i in 1 2 3 4 5; do
        rm -f "$LOG_DIR/${f}.$i" "$LOG_DIR/${f}.$i.gz" 2>/dev/null
        rm -f "$STATE_DIR/${f}.$i" "$STATE_DIR/${f}.$i.gz" 2>/dev/null
    done
    rm -f "$LOG_DIR/${f}.gz" 2>/dev/null
    rm -f "$STATE_DIR/${f}.gz" 2>/dev/null
done
# Catch-all for any other rotated logs (thermalai*.log.*) that may have been missed
rm -f "$LOG_DIR"/thermalai*.log.* 2>/dev/null
rm -f "$LOG_DIR"/network*.log.* 2>/dev/null
rm -f "$LOG_DIR"/codm*.log.* 2>/dev/null
rm -f "$LOG_DIR"/ml_features.jsonl.* 2>/dev/null
rm -f "$STATE_DIR"/ml_features.jsonl.* 2>/dev/null
# External staging copies (from manual push before v3.7.7 bundled)
rm -f /data/local/tmp/ml_model.onnx 2>/dev/null
rm -f /data/local/tmp/ml_model.onnx.json 2>/dev/null
rm -f /data/local/tmp/ml_features.jsonl 2>/dev/null
rm -f /data/local/tmp/ml_features.jsonl.* 2>/dev/null
rm -f /sdcard/ml_model.onnx* 2>/dev/null
rm -rf "$STATE_DIR"

echo "Module uninstalled. Daemon stopped and all files under $LOG_DIR and $STATE_DIR cleaned up." >> /dev/kmsg
