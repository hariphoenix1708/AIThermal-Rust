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

# The daemon's own SIGTERM handler restores the hardware snapshot on clean
# shutdown. We just need to clean up every file/folder it creates under
# LOG_DIR/STATE_DIR so nothing is left behind after the module is removed.
rm -f "$PID_FILE"
rm -f "$PID_LOCK_FILE"
rm -f "$LOG_DIR/thermalai.log"
rm -f "$LOG_DIR/thermalai_verbose.log"
rm -f "$LOG_DIR/thermalai_startup.log"
rm -f "$LOG_DIR/thermalai_battery.log"
rm -rf "$STATE_DIR"

echo "Module uninstalled. Daemon stopped and all files under $LOG_DIR and $STATE_DIR cleaned up." >> /dev/kmsg
