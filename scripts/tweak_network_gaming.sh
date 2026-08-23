#!/system/bin/sh
# ThermalAI - Gaming Network Optimization
# Applies ROM-conditional network tweaks for online gaming (CODM, PUBG, etc.)
# Tuned for: WiFi power save disable, TCP/UDP buffer optimization,
#            DNS fast resolution, NIC ring buffer, IRQ affinity.
#
# Usage: tweak_network_gaming.sh <enable|disable> [state_dir] [log_dir]
# Called by daemon on gaming session start/end.

MODDIR="${0%/*}/.."
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
    local path="$1"
    local value="$2"
    if [ ! -f "$path" ]; then return; fi
    local current=$(cat "$path" 2>/dev/null)
    if [ "$current" != "$value" ]; then
        echo "$value" > "$path" 2>/dev/null
        local new_val=$(cat "$path" 2>/dev/null)
        if [ "$new_val" = "$value" ]; then
            log "Wrote $path: $current -> $value"
            return 0
        else
            log "Write rejected $path: wanted=$value got=$new_val"
            return 1
        fi
    fi
    return 0
}

backup_and_write() {
    local path="$1"
    local value="$2"
    local key=$(echo "$path" | tr '/' '_')

    if [ "$ACTION" = "enable" ]; then
        if [ ! -f "$BACKUP_DIR/$key" ]; then
            local orig=$(cat "$path" 2>/dev/null)
            echo "${orig:-__UNWRITABLE__}" > "$BACKUP_DIR/$key" 2>/dev/null
        fi
        write_if_different "$path" "$value"
    elif [ "$ACTION" = "disable" ]; then
        if [ -f "$BACKUP_DIR/$key" ]; then
            local orig=$(cat "$BACKUP_DIR/$key" 2>/dev/null)
            if [ "$orig" != "__UNWRITABLE__" ]; then
                write_if_different "$path" "$orig"
            fi
        fi
    fi
}

detect_rom() {
    local os_version=$(getprop ro.mi.os.version.incremental 2>/dev/null)
    local miui_version=$(getprop ro.miui.ui.version.name 2>/dev/null)
    local brand=$(getprop ro.product.brand 2>/dev/null)

    if echo "$brand" | grep -qi "xiaomi\|poco\|redmi"; then
        if [ -n "$os_version" ] || [ -n "$miui_version" ]; then
            echo "hyperos"
            return
        fi
    fi
    echo "aosp"
}

tweak_wifi_power_save() {
    local enable="$1"
    if [ "$enable" = "enable" ]; then
        if command -v iw >/dev/null 2>&1; then
            iw wlan0 set power_save off 2>/dev/null
            log "WiFi power save disabled via iw"
        fi
        local ps_file="/sys/class/net/wlan0/power_save"
        if [ -f "$ps_file" ]; then
            backup_and_write "$ps_file" "0"
        fi
        for f in /sys/kernel/debug/ieee80211/phy*/ath9k/ps_timeout; do
            if [ -f "$f" ]; then
                backup_and_write "$f" "0"
            fi
        done
    else
        if command -v iw >/dev/null 2>&1; then
            iw wlan0 set power_save on 2>/dev/null
            log "WiFi power save re-enabled via iw"
        fi
        local ps_file="/sys/class/net/wlan0/power_save"
        if [ -f "$ps_file" ]; then
            backup_and_write "$ps_file" "1"
        fi
    fi
}

tweak_network_buffers() {
    local enable="$1"
    if [ "$enable" = "enable" ]; then
        backup_and_write /proc/sys/net/ipv4/tcp_rmem "4096 131072 16777216"
        backup_and_write /proc/sys/net/ipv4/tcp_wmem "4096 65536 8388608"
        backup_and_write /proc/sys/net/ipv4/tcp_mem "786432 1048576 1572864"
        backup_and_write /proc/sys/net/core/rmem_max "16777216"
        backup_and_write /proc/sys/net/core/wmem_max "8388608"
        backup_and_write /proc/sys/net/ipv4/udp_mem "32768 65536 131072"
        backup_and_write /proc/sys/net/core/netdev_max_backlog "5000"
        backup_and_write /proc/sys/net/core/netdev_budget "600"
        backup_and_write /proc/sys/net/core/busy_poll "50"
        backup_and_write /proc/sys/net/core/busy_read "50"
        backup_and_write /proc/sys/net/core/dev_weight "64"
        backup_and_write /proc/sys/net/ipv4/tcp_fastopen "3"
        backup_and_write /proc/sys/net/ipv4/tcp_keepalive_time "1200"
        log "Network buffers tuned for gaming"
    else
        backup_and_write /proc/sys/net/ipv4/tcp_rmem "4096 131072 6291456"
        backup_and_write /proc/sys/net/ipv4/tcp_wmem "4096 16384 4194304"
        backup_and_write /proc/sys/net/ipv4/tcp_mem "383508 511344 767016"
        backup_and_write /proc/sys/net/core/rmem_max "212992"
        backup_and_write /proc/sys/net/core/wmem_max "212992"
        backup_and_write /proc/sys/net/ipv4/udp_mem "768 1024 1536"
        backup_and_write /proc/sys/net/core/netdev_max_backlog "1000"
        backup_and_write /proc/sys/net/core/netdev_budget "300"
        backup_and_write /proc/sys/net/core/busy_poll "0"
        backup_and_write /proc/sys/net/core/busy_read "0"
        backup_and_write /proc/sys/net/core/dev_weight "64"
        backup_and_write /proc/sys/net/ipv4/tcp_fastopen "1"
        backup_and_write /proc/sys/net/ipv4/tcp_keepalive_time "7200"
        log "Network buffers restored to defaults"
    fi
}

tweak_dns() {
    local enable="$1"
    if [ "$enable" = "enable" ]; then
        setprop net.dns1 "1.1.1.1" 2>/dev/null
        setprop net.dns2 "8.8.8.8" 2>/dev/null
        setprop net.dns3 "1.0.0.1" 2>/dev/null
        log "DNS set to Cloudflare+Google"
    fi
}

tweak_irq_affinity() {
    local enable="$1"
    if [ "$enable" != "enable" ]; then return; fi

    local big_mask="f0"
    local num_cores=$(grep -c "^processor" /proc/cpuinfo 2>/dev/null || echo "8")
    if [ "$num_cores" -le 4 ] 2>/dev/null; then
        big_mask="c"
    fi

    local wlan_irq=$(grep -r "wlan" /proc/interrupts 2>/dev/null | head -1 | awk '{print $1}' | tr -d ':')
    if [ -n "$wlan_irq" ]; then
        local irq_path="/proc/irq/$wlan_irq/smp_affinity"
        if [ -f "$irq_path" ]; then
            backup_and_write "$irq_path" "$big_mask"
            log "WiFi IRQ $wlan_irq pinned to big cores ($big_mask)"
        fi
    fi

    for pattern in rmnet ccci qmi geni; do
        local modem_irq=$(grep -r "$pattern" /proc/interrupts 2>/dev/null | head -1 | awk '{print $1}' | tr -d ':')
        if [ -n "$modem_irq" ]; then
            local irq_path="/proc/irq/$modem_irq/smp_affinity"
            if [ -f "$irq_path" ]; then
                backup_and_write "$irq_path" "$big_mask"
                log "Modem IRQ $modem_irq pinned to big cores ($big_mask)"
            fi
        fi
    done
}

tweak_fast_dormancy() {
    local enable="$1"
    local rom=$(detect_rom)

    if [ "$enable" = "enable" ]; then
        setprop persist.radio.fast_dormancy "0" 2>/dev/null
        if [ "$rom" = "aosp" ]; then
            setprop persist.ril.fast.dormancy "0" 2>/dev/null
            log "Fast dormancy disabled (AOSP mode)"
        else
            log "Fast dormancy hint set (HyperOS mode)"
        fi
    else
        setprop persist.radio.fast_dormancy "1" 2>/dev/null
        setprop persist.ril.fast.dormancy "1" 2>/dev/null
        log "Fast dormancy re-enabled"
    fi
}

tweak_txqueuelen() {
    local enable="$1"
    if [ "$enable" = "enable" ]; then
        for iface in /sys/class/net/*/tx_queue_len; do
            local name=$(dirname "$iface" | xargs basename)
            if [ "$name" = "lo" ]; then continue; fi
            backup_and_write "$iface" "3000"
        done
        log "TX queue length set to 3000"
    else
        for iface in /sys/class/net/*/tx_queue_len; do
            backup_and_write "$iface" "1000"
        done
        log "TX queue length restored"
    fi
}

main() {
    log "=== Gaming network tweak: $ACTION ==="

    local rom=$(detect_rom)
    log "ROM=$rom"

    tweak_wifi_power_save "$ACTION"
    tweak_network_buffers "$ACTION"
    tweak_dns "$ACTION"
    tweak_irq_affinity "$ACTION"
    tweak_fast_dormancy "$ACTION"
    tweak_txqueuelen "$ACTION"

    log "=== Gaming network tweak complete ==="
}

main
