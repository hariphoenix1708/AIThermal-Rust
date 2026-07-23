#!/system/bin/sh
# ThermalAI - Uninstall script (Rust Edition)

MODDIR="${0%/*}"

LOG_DIR="${THERMALAI_LOG_DIR:-/data/local/tmp}"
STATE_DIR="${THERMALAI_STATE_DIR:-/data/local/tmp/thermalai_state}"

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
         thermalai_gaming.log; do
    rm -f "$LOG_DIR/$f"
    # Log rotation may leave .1 / .gz siblings; sweep them too.
    rm -f "$LOG_DIR/${f}.1" "$LOG_DIR/${f}.gz" "$LOG_DIR/${f}.1.gz"
done
rm -rf "$STATE_DIR"

echo "Module uninstalled. Daemon stopped and all files under $LOG_DIR and $STATE_DIR cleaned up." >> /dev/kmsg
