#!/system/bin/sh
# ThermalAI Magisk/KernelSU Late-Start Boot Script

MODDIR=${0%/*}
LOG_DIR="${THERMALAI_LOG_DIR:-/data/local/tmp}"
STATE_DIR="${THERMALAI_STATE_DIR:-/data/local/tmp/thermalai_state}"
PID_FILE="$LOG_DIR/thermalai.pid"
STARTUP_LOG="$LOG_DIR/thermalai_startup.log"
BIN_DIR="$MODDIR/system/bin"
DAEMON="$BIN_DIR/thermalai-daemon"

export THERMALAI_MODULE_DIR="$MODDIR"
export THERMALAI_CONFIG_DIR="$MODDIR/config"
export THERMALAI_LOG_DIR="$LOG_DIR"
export THERMALAI_STATE_DIR="$STATE_DIR"

# Force wall-clock formatting to IST for daemon-emitted logs and for
# the `$(date ...)` calls further down in this script.
export TZ="Asia/Kolkata"

mkdir -p "$LOG_DIR" "$STATE_DIR" 2>/dev/null

log_startup() {
    echo "$(TZ=Asia/Kolkata date '+%Y-%m-%d %H:%M:%S%z') $*" >> "$STARTUP_LOG"
}

is_alive() {
    [ -n "$1" ] && kill -0 "$1" 2>/dev/null
}

wait_for_boot_completed() {
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24
    do
        if [ "$(getprop sys.boot_completed 2>/dev/null)" = "1" ]
        then
            log_startup "boot_completed observed"
            return 0
        fi
        sleep 5
    done
    log_startup "boot_completed wait timed out; continuing conservatively"
    return 0
}

prepare_binary_contexts() {
    for bin in thermalai-daemon thermalai-detect thermalair
    do
        path="$BIN_DIR/$bin"
        if [ -f "$path" ]
        then
            chmod 0755 "$path" 2>/dev/null
            chcon u:object_r:su_file:s0 "$path" 2>/dev/null || log_startup "chcon skipped or failed path=$path"
        fi
    done
}

wait_for_boot_completed
prepare_binary_contexts

if [ -f "$PID_FILE" ]
then
    OLD_PID="$(cat "$PID_FILE" 2>/dev/null)"
    if is_alive "$OLD_PID"
    then
        log_startup "daemon already alive pid=$OLD_PID module=$MODDIR log_dir=$LOG_DIR state_dir=$STATE_DIR"
        exit 0
    fi
    log_startup "removing stale pid file pid=$OLD_PID"
    rm -f "$PID_FILE" "$PID_FILE.lock" 2>/dev/null
fi

# Stop the stock mi_thermald if it exists
if pgrep -f mi_thermald > /dev/null 2>&1
then
    log_startup "requesting mi_thermald stop"
    killall mi_thermald 2>/dev/null
fi
setprop ctl.stop mi_thermald 2>/dev/null

# Stop other Xiaomi/HyperOS performance daemons that can compete with our
# own governor/cpuset/GPU tuning by silently overwriting the same sysfs
# nodes moments after we write them.
for svc in vendor.perfservice miuibooster perfd; do
    if pgrep -f "$svc" > /dev/null 2>&1; then
        log_startup "requesting $svc stop"
        killall "$svc" 2>/dev/null
    fi
    setprop ctl.stop "$svc" 2>/dev/null
done


# Run the Rust daemon
if [ ! -x "$DAEMON" ]
then
    log_startup "daemon binary missing or not executable path=$DAEMON"
    exit 1
fi

log_startup "starting daemon path=$DAEMON module=$MODDIR log_dir=$LOG_DIR state_dir=$STATE_DIR"
"$DAEMON" >/dev/null 2>&1 &

for _ in 1 2 3 4 5 6 7 8 9 10
do
    if [ -f "$PID_FILE" ]
    then
        PID="$(cat "$PID_FILE" 2>/dev/null)"
        if is_alive "$PID"
        then
            sleep 2
            if is_alive "$PID"
            then
                log_startup "daemon validated pid=$PID"
                exit 0
            fi
            log_startup "daemon pid died during validation pid=$PID"
            exit 1
        fi
        log_startup "pid file exists but pid is not alive pid=$PID"
    fi
    sleep 1
done

log_startup "daemon failed health validation pid_file=$PID_FILE"
exit 1
