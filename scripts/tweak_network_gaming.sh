#!/system/bin/sh
# ThermalAI - compatibility wrapper for gaming network tuning.
#
# Android's ConnectivityService/netd owns DNS, routes, and per-network socket
# policy. Altering global DNS properties, radio persistence flags, IRQ masks,
# or TX queue depths races WiFi <-> mobile-data handoffs. The Rust GameTurbo
# engine owns the safe runtime optimizations: framework WiFi low-latency mode
# and RPS on the currently active RX queues.

ACTION="${1:-enable}"
STATE_DIR="${2:-${THERMALAI_STATE_DIR:-/data/local/tmp/AIThermal/state}}"
LOG_DIR="${3:-${THERMALAI_LOG_DIR:-/data/local/tmp/AIThermal}}"
LOGFILE="$LOG_DIR/network_tweak.log"
BACKUP_DIR="$STATE_DIR/network_backup"

mkdir -p "$STATE_DIR" "$LOG_DIR" "$BACKUP_DIR" 2>/dev/null

log() {
    echo "$(TZ=Asia/Kolkata date '+%Y-%m-%d %H:%M:%S%z') [NET-TWEAK] $*" >> "$LOGFILE"
}

write_if_different() {
    path="$1"
    value="$2"
    [ -f "$path" ] || return 1
    current=$(cat "$path" 2>/dev/null) || return 1
    [ "$current" = "$value" ] && return 0
    echo "$value" > "$path" 2>/dev/null || return 1
    [ "$(cat "$path" 2>/dev/null)" = "$value" ]
}

restore_backup() {
    path="$1"
    key=$(echo "$path" | tr '/' '_')
    backup="$BACKUP_DIR/$key"
    [ -f "$backup" ] || return 0
    original=$(cat "$backup" 2>/dev/null)
    [ -n "$original" ] && [ "$original" != "__UNWRITABLE__" ] || return 0
    if write_if_different "$path" "$original"; then
        log "Restored legacy setting $path"
    else
        log "Could not restore legacy setting $path"
    fi
}

restore_legacy_settings() {
    # v3.3.11 migration: unwind <= v3.3.10 values using the session's
    # captured originals, never hard-coded device defaults.
    for path in \
        /proc/sys/net/core/netdev_budget \
        /proc/sys/net/core/busy_poll \
        /proc/sys/net/core/busy_read \
        /proc/sys/net/ipv4/tcp_low_latency; do
        restore_backup "$path"
    done

    for iface in /sys/class/net/wlan0 /sys/class/net/rmnet_data* /sys/class/net/r_rmnet_data*; do
        [ -d "$iface" ] && restore_backup "$iface/tx_queue_len"
    done

    # net.dns* are ConnectivityService status properties, not DNS policy.
    # Restore only a non-empty value explicitly backed up by the legacy script.
    for name in net.dns1 net.dns2 net.dns3; do
        backup="$BACKUP_DIR/$name"
        [ -f "$backup" ] || continue
        original=$(cat "$backup" 2>/dev/null)
        if [ -n "$original" ] && [ "$original" != "__EMPTY__" ]; then
            setprop "$name" "$original" 2>/dev/null
            log "Restored legacy $name"
        fi
    done
}

if [ "$ACTION" = "disable" ]; then
    restore_legacy_settings
    log "Gaming network wrapper disabled; legacy settings restored where backed up"
else
    log "Gaming network wrapper enabled; Android and GameTurbo manage network policy"
fi

exit 0
