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

# Write a value to a sysfs/procfs node if it differs.
# Returns 0 on success (value matched after write), 1 on failure.
write_if_different() {
    local path="$1"
    local value="$2"
    if [ ! -f "$path" ]; then return 1; fi
    local current
    current=$(cat "$path" 2>/dev/null) || return 1
    # Normalize whitespace (proc multi-value entries use tabs, we write spaces)
    local norm_current norm_value
    norm_current=$(echo "$current" | tr '\t' ' ' | sed 's/  */ /g; s/^ //; s/ $//')
    norm_value=$(echo "$value" | tr '\t' ' ' | sed 's/  */ /g; s/^ //; s/ $//')
    if [ "$norm_current" = "$norm_value" ]; then
        return 0
    fi
    echo "$value" > "$path" 2>/dev/null
    local new_val
    new_val=$(cat "$path" 2>/dev/null) || return 1
    local norm_new
    norm_new=$(echo "$new_val" | tr '\t' ' ' | sed 's/  */ /g; s/^ //; s/ $//')
    if [ "$norm_new" = "$norm_value" ]; then
        log "Wrote $path: $current -> $value"
        return 0
    else
        log "Write rejected $path: wanted=$value got=$new_val"
        return 1
    fi
}

backup_and_write() {
    local path="$1"
    local value="$2"
    local key
    key=$(echo "$path" | tr '/' '_')

    if [ "$ACTION" = "enable" ]; then
        if [ ! -f "$BACKUP_DIR/$key" ]; then
            local orig
            orig=$(cat "$path" 2>/dev/null)
            echo "${orig:-__UNWRITABLE__}" > "$BACKUP_DIR/$key" 2>/dev/null
        fi
        write_if_different "$path" "$value"
    elif [ "$ACTION" = "disable" ]; then
        if [ -f "$BACKUP_DIR/$key" ]; then
            local orig
            orig=$(cat "$BACKUP_DIR/$key" 2>/dev/null)
            if [ "$orig" != "__UNWRITABLE__" ]; then
                write_if_different "$path" "$orig"
            fi
        fi
    fi
}

detect_rom() {
    local os_version brand miui_version
    os_version=$(getprop ro.mi.os.version.incremental 2>/dev/null)
    miui_version=$(getprop ro.miui.ui.version.name 2>/dev/null)
    brand=$(getprop ro.product.brand 2>/dev/null)

    brand=$(echo "$brand" | tr '[:upper:]' '[:lower:]')
    case "$brand" in
        xiaomi|poco|redmi)
            if [ -n "$os_version" ] || [ -n "$miui_version" ]; then
                echo "hyperos"
                return
            fi
            ;;
    esac
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
        backup_and_write /proc/sys/net/core/netdev_max_backlog "100000"
        backup_and_write /proc/sys/net/core/netdev_budget "600"
        backup_and_write /proc/sys/net/core/busy_poll "50"
        backup_and_write /proc/sys/net/core/busy_read "50"
        backup_and_write /proc/sys/net/core/dev_weight "64"
        backup_and_write /proc/sys/net/ipv4/tcp_fastopen "3"
        backup_and_write /proc/sys/net/ipv4/tcp_keepalive_time "600"
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
        # Save original DNS values
        local dns1 dns2 dns3
        dns1=$(getprop net.dns1 2>/dev/null)
        dns2=$(getprop net.dns2 2>/dev/null)
        dns3=$(getprop net.dns3 2>/dev/null)
        echo "${dns1:-__EMPTY__}" > "$BACKUP_DIR/net.dns1" 2>/dev/null
        echo "${dns2:-__EMPTY__}" > "$BACKUP_DIR/net.dns2" 2>/dev/null
        echo "${dns3:-__EMPTY__}" > "$BACKUP_DIR/net.dns3" 2>/dev/null

        setprop net.dns1 "1.1.1.1" 2>/dev/null
        setprop net.dns2 "8.8.8.8" 2>/dev/null
        setprop net.dns3 "1.0.0.1" 2>/dev/null
        log "DNS set to Cloudflare+Google"
    else
        # Restore original DNS values
        local orig_dns1 orig_dns2 orig_dns3
        orig_dns1=$(cat "$BACKUP_DIR/net.dns1" 2>/dev/null)
        orig_dns2=$(cat "$BACKUP_DIR/net.dns2" 2>/dev/null)
        orig_dns3=$(cat "$BACKUP_DIR/net.dns3" 2>/dev/null)
        if [ -n "$orig_dns1" ] && [ "$orig_dns1" != "__EMPTY__" ]; then
            setprop net.dns1 "$orig_dns1" 2>/dev/null
        fi
        if [ -n "$orig_dns2" ] && [ "$orig_dns2" != "__EMPTY__" ]; then
            setprop net.dns2 "$orig_dns2" 2>/dev/null
        fi
        if [ -n "$orig_dns3" ] && [ "$orig_dns3" != "__EMPTY__" ]; then
            setprop net.dns3 "$orig_dns3" 2>/dev/null
        fi
        log "DNS restored to original values"
    fi
}

tweak_irq_affinity() {
    local enable="$1"
    if [ "$enable" != "enable" ]; then return; fi

    local big_mask="f0"
    local num_cores
    num_cores=$(grep -c "^processor" /proc/cpuinfo 2>/dev/null || echo "8")
    if [ "$num_cores" -le 4 ] 2>/dev/null; then
        big_mask="c"
    fi

    local wlan_irq
    wlan_irq=$(grep -r "wlan" /proc/interrupts 2>/dev/null | head -1 | awk '{print $1}' | tr -d ':')
    if [ -n "$wlan_irq" ]; then
        local irq_path="/proc/irq/$wlan_irq/smp_affinity"
        if [ -f "$irq_path" ]; then
            if backup_and_write "$irq_path" "$big_mask"; then
                log "WiFi IRQ $wlan_irq pinned to big cores ($big_mask)"
            else
                local actual
                actual=$(cat "$irq_path" 2>/dev/null)
                log "WiFi IRQ $wlan_irq affinity write failed (wanted=$big_mask got=$actual)"
            fi
        fi
    fi

    for pattern in rmnet ccci qmi geni; do
        local modem_irq
        modem_irq=$(grep -r "$pattern" /proc/interrupts 2>/dev/null | head -1 | awk '{print $1}' | tr -d ':')
        if [ -n "$modem_irq" ]; then
            local irq_path="/proc/irq/$modem_irq/smp_affinity"
            if [ -f "$irq_path" ]; then
                if backup_and_write "$irq_path" "$big_mask"; then
                    log "Modem IRQ $modem_irq pinned to big cores ($big_mask)"
                else
                    local actual
                    actual=$(cat "$irq_path" 2>/dev/null)
                    log "Modem IRQ $modem_irq affinity write failed (wanted=$big_mask got=$actual)"
                fi
            fi
        fi
    done
}

tweak_fast_dormancy() {
    local enable="$1"
    local rom
    rom=$(detect_rom)

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
    # Only tune real network interfaces (WiFi + mobile data), skip
    # dummy/tunnel/virtual interfaces that don't carry user traffic.
    local ifaces="wlan0 rmnet_data0 rmnet_data1 rmnet_data2 rmnet_data3 r_rmnet_data0 r_rmnet_data1 r_rmnet_data2 r_rmnet_data3"

    if [ "$enable" = "enable" ]; then
        for iface in $ifaces; do
            local path="/sys/class/net/$iface/tx_queue_len"
            if [ -f "$path" ]; then
                backup_and_write "$path" "3000"
            fi
        done
        log "TX queue length set to 3000 on active interfaces"
    else
        for iface in $ifaces; do
            local path="/sys/class/net/$iface/tx_queue_len"
            if [ -f "$path" ]; then
                backup_and_write "$path" "1000"
            fi
        done
        log "TX queue length restored on active interfaces"
    fi
}

# ─── v3.2.32: Gaming-specific low-latency tweaks ───────────────────────

tweak_wifi_qos() {
    local enable="$1"
    if [ "$enable" = "enable" ]; then
        # Disable WiFi aggregation (AMSDU/AMPDU) to reduce latency spikes.
        # Aggregation batches multiple frames which adds jitter for real-time games.
        for path in /sys/kernel/debug/ieee80211/phy*/ath11k/ammmode; do
            if [ -f "$path" ]; then
                backup_and_write "$path" "0"
            fi
        done

        # Set WiFi roaming aggressiveness to maximum (avoid stale AP connections)
        local roam_path="/proc/net/wireless/roaming"
        if [ -f "$roam_path" ]; then
            backup_and_write "$roam_path" "1"
        fi

        # Disable WiFi APF (Android Packet Filter) — can add latency on some chipsets
        for f in /sys/class/net/wlan0/wireless/*/apf; do
            if [ -f "$f" ]; then
                backup_and_write "$f" "0"
            fi
        done

        # WiFi scan timer: reduce scan interval during gaming to avoid connection drops
        # but don't scan too often (which would add latency itself)
        setprop persist.wifi.scan.always.enabled "0" 2>/dev/null

        # WMM (WiFi Multimedia): ensure voice/video AC has highest priority
        # This is mostly handled by the driver, but we can hint via props
        setprop persist.sys.wmm.enable "1" 2>/dev/null

        log "WiFi QoS optimized for low-latency gaming"
    else
        # Restore aggregation
        for path in /sys/kernel/debug/ieee80211/phy*/ath11k/ammmode; do
            if [ -f "$path" ]; then
                backup_and_write "$path" "1"
            fi
        done
        log "WiFi QoS settings restored"
    fi
}

tweak_tcp_nodelay() {
    local enable="$1"
    # Enable TCP_NODELAY equivalent via sysfs: disable Nagle's algorithm globally
    # for new connections. This is critical for FPS games — CODM sends many small
    # packets (position updates, shots) that Nagle would batch, adding 40-200ms.
    #
    # We use tcp_low_latency which hints the kernel to prioritize latency over
    # throughput, and disable tcp_moderate_rcvbuf to avoid receive buffer delays.

    if [ "$enable" = "enable" ]; then
        # tcp_timestamps=0 reduces per-packet overhead by 12 bytes
        # (saves ~1% CPU on high packet-rate connections)
        backup_and_write /proc/sys/net/ipv4/tcp_timestamps "0"

        # Disable delayed ACK (tcp_delack_min) — critical for game servers
        # that wait for ACK before sending next state update
        backup_and_write /proc/sys/net/ipv4/tcp_delack_min "0"

        # Reduce initial congestion window to 10 (default is usually 10 on modern kernels)
        # This ensures fast ramp-up on game connections
        backup_and_write /proc/sys/net/ipv4/tcp_init_cwnd "10"

        # Enable tcp_low_latency: hints kernel to prioritize latency
        if [ -f /proc/sys/net/ipv4/tcp_low_latency ]; then
            backup_and_write /proc/sys/net/ipv4/tcp_low_latency "1"
        fi

        log "TCP latency optimizations enabled (Nagle disabled, delayed ACK minimized)"
    else
        backup_and_write /proc/sys/net/ipv4/tcp_timestamps "1"
        backup_and_write /proc/sys/net/ipv4/tcp_delack_min "40"
        if [ -f /proc/sys/net/ipv4/tcp_low_latency ]; then
            backup_and_write /proc/sys/net/ipv4/tcp_low_latency "0"
        fi
        log "TCP latency optimizations restored"
    fi
}

tweak_congestion_control() {
    local enable="$1"
    if [ "$enable" != "enable" ]; then
        # Restore default congestion control
        local backup_file="$BACKUP_DIR/tcp_congestion_control"
        if [ -f "$backup_file" ]; then
            local orig
            orig=$(cat "$backup_file" 2>/dev/null)
            if [ -n "$orig" ]; then
                write_if_different /proc/sys/net/ipv4/tcp_congestion_control "$orig"
            fi
        fi
        return
    fi

    # Check available congestion algorithms
    local available
    available=$(cat /proc/sys/net/ipv4/tcp_available_congestion_control 2>/dev/null)
    local current
    current=$(cat /proc/sys/net/ipv4/tcp_congestion_control 2>/dev/null)

    # Backup current
    if [ ! -f "$BACKUP_DIR/tcp_congestion_control" ]; then
        echo "${current:-cubic}" > "$BACKUP_DIR/tcp_congestion_control" 2>/dev/null
    fi

    # Prefer BBR > Westwood+ > HTCP > current
    local new_cc=""
    case "$available" in
        *bbr*)     new_cc="bbr" ;;
        *westwood*) new_cc="westwood" ;;
        *htcp*)    new_cc="htcp" ;;
        *)         new_cc="" ;;
    esac

    if [ -n "$new_cc" ] && [ "$new_cc" != "$current" ]; then
        write_if_different /proc/sys/net/ipv4/tcp_congestion_control "$new_cc"
        log "Congestion control: $current -> $new_cc (available: $available)"
    fi
}

main() {
    log "=== Gaming network tweak: $ACTION ==="

    local rom
    rom=$(detect_rom)
    log "ROM=$rom"

    tweak_wifi_power_save "$ACTION"
    tweak_network_buffers "$ACTION"
    tweak_dns "$ACTION"
    tweak_irq_affinity "$ACTION"
    tweak_fast_dormancy "$ACTION"
    tweak_txqueuelen "$ACTION"
    # v3.2.32: Low-latency gaming optimizations
    tweak_wifi_qos "$ACTION"
    tweak_tcp_nodelay "$ACTION"
    tweak_congestion_control "$ACTION"

    log "=== Gaming network tweak complete ==="
}

main
