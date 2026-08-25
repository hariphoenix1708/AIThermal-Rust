#!/system/bin/sh
# ThermalAI — Active Network Quality Detection
# Measures RTT, jitter, packet loss for both WiFi and mobile data.
# Outputs JSON report to state directory for daemon consumption.
#
# Usage: detect_network_quality.sh [state_dir] [log_dir]
# Called by daemon on gaming session start and periodically during gaming.

MODDIR="${0%/*}/.."
STATE_DIR="${1:-${THERMALAI_STATE_DIR:-/data/local/tmp/AIThermal/state}}"
LOG_DIR="${2:-${THERMALAI_LOG_DIR:-/data/local/tmp/AIThermal}}"
REPORT="$STATE_DIR/network_quality.json"
LOGFILE="$LOG_DIR/network_diag.log"

mkdir -p "$STATE_DIR" "$LOG_DIR" 2>/dev/null

log() {
    echo "$(TZ=Asia/Kolkata date '+%Y-%m-%d %H:%M:%S%z') [NET-DIAG] $*" >> "$LOGFILE"
}

# ─── ROM Detection ───────────────────────────────────────────────────
detect_rom() {
    local brand=$(getprop ro.product.brand 2>/dev/null)
    local manufacturer=$(getprop ro.product.manufacturer 2>/dev/null)
    local miui_version=$(getprop ro.miui.ui.version.name 2>/dev/null)
    local os_version=$(getprop ro.mi.os.version.incremental 2>/dev/null)
    local hyperos=$(getprop ro.miui.build.version.incremental 2>/dev/null)

    brand=$(echo "$brand" | tr '[:upper:]' '[:lower:]')
    case "$brand" in
        xiaomi|poco|redmi)
            if [ -n "$os_version" ] || [ -n "$miui_version" ] || [ -n "$hyperos" ]; then
                echo "hyperos"
                return
            fi
            echo "miui"
            return
            ;;
    esac
    echo "aosp"
}

# ─── Network Interface Detection ─────────────────────────────────────
detect_active_interface() {
    # Priority: wlan0 > rmnet_data0 > any other up interface
    for iface in wlan0 rmnet_data0; do
        local state=$(cat "/sys/class/net/$iface/operstate" 2>/dev/null)
        if [ "$state" = "up" ]; then
            echo "$iface"
            return
        fi
    done

    for iface_path in /sys/class/net/*/operstate; do
        local iface=$(echo "$iface_path" | cut -d'/' -f5)
        if [ "$iface" = "lo" ]; then continue; fi
        local state=$(cat "$iface_path" 2>/dev/null)
        if [ "$state" = "up" ]; then
            echo "$iface"
            return
        fi
    done
    echo "none"
}

detect_network_type() {
    local iface="$1"
    case "$iface" in
        wlan*) echo "wifi";;
        rmnet*|ccmni*|v4-rmnet*) echo "mobile";;
        *) echo "unknown";;
    esac
}

# ─── WiFi Signal Strength ────────────────────────────────────────────
get_wifi_rssi() {
    local rssi_file="/sys/class/net/wlan0/wireless/link/level"
    if [ -f "$rssi_file" ]; then
        cat "$rssi_file" 2>/dev/null
        return
    fi
    # Fallback: iw
    if command -v iw >/dev/null 2>&1; then
        local info=$(iw wlan0 link 2>/dev/null | grep "signal")
        echo "$info" | grep -oE '[-0-9]+' | head -1
        return
    fi
    echo "unknown"
}

get_wifi_freq() {
    if command -v iw >/dev/null 2>&1; then
        iw wlan0 link 2>/dev/null | grep "freq" | awk '{print $2}'
        return
    fi
    local freq_file="/sys/class/net/wlan0/wireless/freq"
    if [ -f "$freq_file" ]; then
        cat "$freq_file" 2>/dev/null
        return
    fi
    echo "unknown"
}

# ─── Mobile Signal ───────────────────────────────────────────────────
get_mobile_signal() {
    local signal=$(dumpsys telephony.registry 2>/dev/null | grep "mSignalStrength" | head -1)
    if [ -n "$signal" ]; then
        echo "$signal" | grep -oE 'rsrp=-?[0-9]+' | head -1 | cut -d= -f2
    else
        echo "unknown"
    fi
}

get_mobile_rat() {
    local rat=$(dumpsys telephony.registry 2>/dev/null | grep "mDataNetworkType" | head -1)
    if [ -n "$rat" ]; then
        echo "$rat" | grep -oE '= [0-9]+' | head -1 | tr -d ' ='
    else
        echo "unknown"
    fi
}

# ─── Active Ping Test ────────────────────────────────────────────────
# Ping a target, collect per-packet RTTs, compute stats.
# Usage: ping_test <target> <count> <interval_ms>
# Output: avg min max jitter loss_pct (space-separated)
ping_test() {
    local target="$1"
    local count="${2:-20}"
    local interval_ms="${3:-200}"
    local interval_s=$(echo "scale=3; $interval_ms / 1000" | bc 2>/dev/null || echo "0.2")

    # Use toybox/busybox ping; collect raw output
    local ping_out
    ping_out=$(ping -c "$count" -i "$interval_s" -W 2 "$target" 2>&1)
    local rc=$?

    if [ $rc -ne 0 ] && ! echo "$ping_out" | grep -q "rtt\|round-trip"; then
        echo "0 0 0 0 100"
        return
    fi

    # Parse "rtt min/avg/max/mdev = X/Y/Z/W ms" or "round-trip min/avg/max/stddev = X/Y/Z/W ms"
    local stats=$(echo "$ping_out" | grep -E "rtt|round-trip" | head -1 | grep -oE '[0-9]+\.[0-9]+/[0-9]+\.[0-9]+/[0-9]+\.[0-9]+/[0-9]+\.[0-9]+')
    if [ -z "$stats" ]; then
        echo "0 0 0 0 100"
        return
    fi

    local min=$(echo "$stats" | cut -d'/' -f1)
    local avg=$(echo "$stats" | cut -d'/' -f2)
    local max=$(echo "$stats" | cut -d'/' -f3)
    local mdev=$(echo "$stats" | cut -d'/' -f4)

    # Count transmitted/received for loss calculation
    local transmitted=$(echo "$ping_out" | grep -oE '[0-9]+ packets transmitted' | awk '{print $1}')
    local received=$(echo "$ping_out" | grep -oE '[0-9]+ received' | awk '{print $1}')
    transmitted=${transmitted:-$count}
    received=${received:-0}
    local loss_pct=0
    if [ "$transmitted" -gt 0 ] 2>/dev/null; then
        loss_pct=$(echo "scale=1; ($transmitted - $received) * 100 / $transmitted" | bc 2>/dev/null || echo "0")
    fi

    # Jitter: approximate from mdev (mean deviation)
    local jitter=${mdev:-0}

    echo "$avg $min $max $jitter $loss_pct"
}

# ─── DNS Resolution Test ─────────────────────────────────────────────
dns_test() {
    local target="${1:-google.com}"
    local start_ms=$(date +%s%N 2>/dev/null | cut -b1-13)
    nslookup "$target" >/dev/null 2>&1
    local end_ms=$(date +%s%N 2>/dev/null | cut -b1-13)
    if [ -n "$start_ms" ] && [ -n "$end_ms" ] && [ "$end_ms" -gt "$start_ms" ] 2>/dev/null; then
        echo $(( end_ms - start_ms ))
    else
        echo "0"
    fi
}

# ─── WiFi Power Save Status ─────────────────────────────────────────
get_wifi_power_save() {
    if command -v iw >/dev/null 2>&1; then
        local ps=$(iw wlan0 get power_save 2>/dev/null | grep -oE 'on|off')
        if [ -n "$ps" ]; then
            echo "$ps"
            return
        fi
    fi
    # Try sysfs
    local ps_file="/sys/class/net/wlan0/power_save"
    if [ -f "$ps_file" ]; then
        cat "$ps_file" 2>/dev/null
        return
    fi
    # v3.3.7: Qualcomm WCN6750 — no sysfs PS node. Check via dumpsys.
    local ll=$(dumpsys wifi 2>/dev/null | grep -i "low.latency\|power.save\|mLowLatency" | head -1)
    if [ -n "$ll" ]; then
        echo "$ll"
        return
    fi
    echo "unknown"
}

# ─── TCP/UDP Buffer Current Values ───────────────────────────────────
get_network_buffers() {
    local tcp_rmem=$(cat /proc/sys/net/ipv4/tcp_rmem 2>/dev/null | tr '\n' ' ')
    local tcp_wmem=$(cat /proc/sys/net/ipv4/tcp_wmem 2>/dev/null | tr '\n' ' ')
    local udp_mem=$(cat /proc/sys/net/ipv4/udp_mem 2>/dev/null | tr '\n' ' ')
    local rmem_max=$(cat /proc/sys/net/core/rmem_max 2>/dev/null)
    local wmem_max=$(cat /proc/sys/net/core/wmem_max 2>/dev/null)
    local busy_poll=$(cat /proc/sys/net/core/busy_poll 2>/dev/null)
    local backlog=$(cat /proc/sys/net/core/netdev_max_backlog 2>/dev/null)

    echo "tcp_rmem=$tcp_rmem tcp_wmem=$tcp_wmem udp_mem=$udp_mem rmem_max=$rmem_max wmem_max=$wmem_max busy_poll=$busy_poll backlog=$backlog"
}

# ─── WiFi Frequency Band ─────────────────────────────────────────────
wifi_band_label() {
    local freq="$1"
    if [ "$freq" = "unknown" ] || [ -z "$freq" ]; then
        echo "unknown"
        return
    fi
    if [ "$freq" -lt 3000 ] 2>/dev/null; then
        echo "2.4GHz"
    elif [ "$freq" -lt 6000 ] 2>/dev/null; then
        echo "5GHz"
    else
        echo "6GHz"
    fi
}

# ─── Main ─────────────────────────────────────────────────────────────
main() {
    log "Starting network quality detection"

    local rom=$(detect_rom)
    local iface=$(detect_active_interface)
    local net_type=$(detect_network_type "$iface")
    local wifi_rssi="unknown"
    local wifi_freq="unknown"
    local wifi_band="unknown"
    local wifi_ps="unknown"
    local mobile_signal="unknown"
    local mobile_rat="unknown"

    if [ "$net_type" = "wifi" ]; then
        wifi_rssi=$(get_wifi_rssi)
        wifi_freq=$(get_wifi_freq)
        wifi_band=$(wifi_band_label "$wifi_freq")
        wifi_ps=$(get_wifi_power_save)
    elif [ "$net_type" = "mobile" ]; then
        mobile_signal=$(get_mobile_signal)
        mobile_rat=$(get_mobile_rat)
    fi

    log "ROM=$rom iface=$iface type=$net_type rssi=$wifi_rssi band=$wifi_band ps=$wifi_ps signal=$mobile_signal rat=$mobile_rat"

    # ── Ping tests to multiple targets ──
    # Google DNS (global), Cloudflare DNS, and a regional target
    local targets="8.8.8.8 1.1.1.1"
    local ping_count=20
    local ping_interval=200

    local dns_time=$(dns_test "google.com")

    # Sequential pings to avoid resource contention
    local google_stats=$(ping_test "8.8.8.8" "$ping_count" "$ping_interval")
    local cloudflare_stats=$(ping_test "1.1.1.1" "$ping_count" "$ping_interval")

    # Parse results
    local g_avg=$(echo "$google_stats" | awk '{print $1}')
    local g_min=$(echo "$google_stats" | awk '{print $2}')
    local g_max=$(echo "$google_stats" | awk '{print $3}')
    local g_jitter=$(echo "$google_stats" | awk '{print $4}')
    local g_loss=$(echo "$google_stats" | awk '{print $5}')

    local c_avg=$(echo "$cloudflare_stats" | awk '{print $1}')
    local c_min=$(echo "$cloudflare_stats" | awk '{print $2}')
    local c_max=$(echo "$cloudflare_stats" | awk '{print $3}')
    local c_jitter=$(echo "$cloudflare_stats" | awk '{print $4}')
    local c_loss=$(echo "$cloudflare_stats" | awk '{print $5}')

    # Use the better of the two as the representative
    local best_avg=$g_avg
    local best_jitter=$g_jitter
    local best_loss=$g_loss
    if [ "$(echo "$c_avg < $g_avg" | bc 2>/dev/null)" = "1" ] 2>/dev/null; then
        best_avg=$c_avg
        best_jitter=$c_jitter
        best_loss=$c_loss
    fi

    # ── Quality Rating ──
    local quality="excellent"
    local quality_score=100

    # Jitter is the most important metric for CODM bullet registration
    local jitter_int=$(echo "$best_jitter" | cut -d'.' -f1)
    jitter_int=${jitter_int:-0}

    if [ "$jitter_int" -le 3 ] 2>/dev/null; then
        quality="excellent"
        quality_score=100
    elif [ "$jitter_int" -le 8 ] 2>/dev/null; then
        quality="good"
        quality_score=80
    elif [ "$jitter_int" -le 15 ] 2>/dev/null; then
        quality="fair"
        quality_score=60
    elif [ "$jitter_int" -le 30 ] 2>/dev/null; then
        quality="poor"
        quality_score=40
    else
        quality="terrible"
        quality_score=20
    fi

    # Degrade if packet loss detected
    local loss_int=$(echo "$best_loss" | cut -d'.' -f1)
    loss_int=${loss_int:-0}
    if [ "$loss_int" -gt 5 ] 2>/dev/null; then
        quality_score=$(( quality_score - 30 ))
    elif [ "$loss_int" -gt 0 ] 2>/dev/null; then
        quality_score=$(( quality_score - 15 ))
    fi

    # Clamp
    if [ "$quality_score" -lt 0 ] 2>/dev/null; then
        quality_score=0
    fi

    local buffers=$(get_network_buffers)
    local timestamp=$(date +%s)

    # ── Write JSON Report ──
    cat > "$REPORT" << JSONEOF
{
  "timestamp": $timestamp,
  "rom": "$rom",
  "interface": "$iface",
  "network_type": "$net_type",
  "wifi": {
    "rssi": "$wifi_rssi",
    "frequency": "$wifi_freq",
    "band": "$wifi_band",
    "power_save": "$wifi_ps"
  },
  "mobile": {
    "signal_rsrp": "$mobile_signal",
    "rat": "$mobile_rat"
  },
  "ping": {
    "google_dns": {
      "avg_ms": "$g_avg",
      "min_ms": "$g_min",
      "max_ms": "$g_max",
      "jitter_ms": "$g_jitter",
      "loss_pct": "$g_loss"
    },
    "cloudflare_dns": {
      "avg_ms": "$c_avg",
      "min_ms": "$c_min",
      "max_ms": "$c_max",
      "jitter_ms": "$c_jitter",
      "loss_pct": "$c_loss"
    }
  },
  "dns_resolution_ms": $dns_time,
  "quality_score": $quality_score,
  "quality_rating": "$quality",
  "buffers": "$buffers"
}
JSONEOF

    log "Detection complete: quality=$quality score=$quality_score avg=${best_avg}ms jitter=${best_jitter}ms loss=${best_loss}%"
    log "Report written to $REPORT"
}

main "$@"
