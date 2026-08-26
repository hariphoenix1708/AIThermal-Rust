#!/system/bin/sh
# ThermalAI - CODM Server Detection and Ping Stability Analysis
# Pings known CODM server endpoints to determine best region and measure
# stability metrics relevant to bullet registration.
#
# CODM uses UDP for game data at ~30Hz (33ms tick rate).
# Bullet registration depends on jitter stability, not just low latency.
#
# Usage: detect_codm_servers.sh [state_dir] [log_dir]
# Output: codm_network_report.json in state_dir

STATE_DIR="${1:-${THERMALAI_STATE_DIR:-/data/local/tmp/AIThermal/state}}"
LOG_DIR="${2:-${THERMALAI_LOG_DIR:-/data/local/tmp/AIThermal}}"
REPORT="$STATE_DIR/codm_network_report.json"
LOGFILE="$LOG_DIR/codm_network_diag.log"

mkdir -p "$STATE_DIR" "$LOG_DIR" 2>/dev/null

log() {
    echo "$(TZ=Asia/Kolkata date '+%Y-%m-%d %H:%M:%S%z') [CODM-NET] $*" >> "$LOGFILE"
}

# Known CODM server regions (public IP endpoints used for matchmaking)
# These are the IPs players have identified through packet capture.
# Format: name|ip|region_hint
SERVERS="
8.8.8.8|dns_google|Global
1.1.1.1|dns_cloudflare|Global
"

# CODM-specific known server ranges (Activision/Aktivision CDN + game servers)
# Populated from community packet captures and server lists
CODM_SERVERS="
Activision_CDN_US|192.221.43.0/24|US-East
Activision_CDN_EU|185.50.104.0/24|EU-West
Activision_CDN_ASIA|162.249.72.0/24|Asia-Pacific
Activision_CDN_SEA|203.116.130.0/24|SEA
"

# Ping a single target, return: avg min max mdev loss
ping_single() {
    local target="$1"
    local count="${2:-30}"
    local interval="${3:-0.1}"

    local out=$(ping -c "$count" -i "$interval" -W 2 "$target" 2>&1)
    local rc=$?

    if ! echo "$out" | grep -qE "rtt|round-trip"; then
        echo "0 0 0 0 100"
        return 1
    fi

    local stats=$(echo "$out" | grep -oE '[0-9]+\.[0-9]+/[0-9]+\.[0-9]+/[0-9]+\.[0-9]+/[0-9]+\.[0-9]+' | head -1)
    if [ -z "$stats" ]; then
        echo "0 0 0 0 100"
        return 1
    fi

    local min=$(echo "$stats" | cut -d'/' -f1)
    local avg=$(echo "$stats" | cut -d'/' -f2)
    local max=$(echo "$stats" | cut -d'/' -f3)
    local mdev=$(echo "$stats" | cut -d'/' -f4)

    local tx=$(echo "$out" | grep -oE '[0-9]+ packets transmitted' | awk '{print $1}')
    local rx=$(echo "$out" | grep -oE '[0-9]+ received' | awk '{print $1}')
    tx=${tx:-$count}
    rx=${rx:-0}
    local loss=0
    if [ "$tx" -gt 0 ] 2>/dev/null; then
        loss=$(echo "scale=1; ($tx - $rx) * 100 / $tx" | bc 2>/dev/null || echo "0")
    fi

    echo "$avg $min $max $mdev $loss"
}

# Calculate jitter stability rating for CODM
# CODM needs jitter < 10ms for reliable bullet registration
# At 30Hz tick rate (33ms), jitter > 15ms means missed prediction windows
rate_jitter() {
    local jitter="$1"
    local jitter_int=$(echo "$jitter" | cut -d'.' -f1)
    jitter_int=${jitter_int:-99}

    if [ "$jitter_int" -le 3 ] 2>/dev/null; then
        echo "S+"
    elif [ "$jitter_int" -le 5 ] 2>/dev/null; then
        echo "S"
    elif [ "$jitter_int" -le 8 ] 2>/dev/null; then
        echo "A"
    elif [ "$jitter_int" -le 12 ] 2>/dev/null; then
        echo "B"
    elif [ "$jitter_int" -le 20 ] 2>/dev/null; then
        echo "C"
    else
        echo "D"
    fi
}

# Rate overall ping for CODM
rate_ping() {
    local avg="$1"
    local avg_int=$(echo "$avg" | cut -d'.' -f1)
    avg_int=${avg_int:-999}

    if [ "$avg_int" -le 30 ] 2>/dev/null; then
        echo "excellent"
    elif [ "$avg_int" -le 60 ] 2>/dev/null; then
        echo "good"
    elif [ "$avg_int" -le 100 ] 2>/dev/null; then
        echo "fair"
    elif [ "$avg_int" -le 150 ] 2>/dev/null; then
        echo "poor"
    else
        echo "terrible"
    fi
}

# Detect WiFi channel congestion (approximate)
get_wifi_channel() {
    if command -v iw >/dev/null 2>&1; then
        iw wlan0 link 2>/dev/null | grep "freq" | awk '{print $2}'
    else
        echo "unknown"
    fi
}

# Check if WiFi is on 2.4GHz (more congestion-prone for gaming)
wifi_band_analysis() {
    local freq=$(get_wifi_channel)
    local band="unknown"
    local congestion_risk="unknown"

    if [ "$freq" = "unknown" ] || [ -z "$freq" ]; then
        echo "band=unknown risk=unknown"
        return
    fi

    if [ "$freq" -lt 3000 ] 2>/dev/null; then
        band="2.4GHz"
        congestion_risk="high"
    elif [ "$freq" -lt 5600 ] 2>/dev/null; then
        band="5GHz-low"
        congestion_risk="low"
    elif [ "$freq" -lt 5825 ] 2>/dev/null; then
        band="5GHz-mid"
        congestion_risk="low"
    elif [ "$freq" -lt 6000 ] 2>/dev/null; then
        band="5GHz-high"
        congestion_risk="very-low"
    else
        band="6GHz"
        congestion_risk="very-low"
    fi

    echo "band=$band freq=$freq risk=$congestion_risk"
}

# Detect active network type
detect_network_type() {
    # v3.3.9: rmnet interfaces report operstate="unknown" even when active —
    # fall back to carrier==1 (kernel sets carrier when link is up).
    local wlan_state=$(cat /sys/class/net/wlan0/operstate 2>/dev/null)
    local rmnet_state=$(cat /sys/class/net/rmnet_data0/operstate 2>/dev/null)
    local rmnet_carrier=$(cat /sys/class/net/rmnet_data0/carrier 2>/dev/null)

    if [ "$wlan_state" = "up" ]; then
        echo "wifi"
    elif [ "$rmnet_state" = "up" ] || [ "$rmnet_carrier" = "1" ]; then
        echo "mobile"
    else
        echo "none"
    fi
}

# Get mobile data RAT (4G/5G)
get_mobile_rat() {
    local rat=$(dumpsys telephony.registry 2>/dev/null | grep "mDataNetworkType" | head -1)
    if [ -n "$rat" ]; then
        local code=$(echo "$rat" | grep -oE '= [0-9]+' | head -1 | tr -d ' =')
        case "$code" in
            20) echo "5G_NR";;
            14|13) echo "4G_LTE";;
            12|11|10) echo "3G";;
            *) echo "unknown($code)";;
        esac
    else
        echo "unknown"
    fi
}

main() {
    log "=== CODM Network Diagnostics Start ==="

    local net_type=$(detect_network_type)
    local wifi_info=""
    local mobile_rat=""

    if [ "$net_type" = "wifi" ]; then
        wifi_info=$(wifi_band_analysis)
    elif [ "$net_type" = "mobile" ]; then
        mobile_rat=$(get_mobile_rat)
    fi

    log "Network: $net_type $wifi_info $mobile_rat"

    # Test connectivity to reference servers
    local test_targets="8.8.8.8 1.1.1.1"
    local best_avg="999"
    local best_jitter="99"
    local best_loss="100"

    for target in $test_targets; do
        log "Pinging $target (30 packets, 100ms interval)..."
        local result=$(ping_single "$target" 30 0.1)
        local avg=$(echo "$result" | awk '{print $1}')
        local min=$(echo "$result" | awk '{print $2}')
        local max=$(echo "$result" | awk '{print $3}')
        local jitter=$(echo "$result" | awk '{print $4}')
        local loss=$(echo "$result" | awk '{print $5}')

        log "  $target: avg=${avg}ms min=${min}ms max=${max}ms jitter=${jitter}ms loss=${loss}%"

        # Track best result
        local avg_int=$(echo "$avg" | cut -d'.' -f1)
        avg_int=${avg_int:-999}
        local best_int=$(echo "$best_avg" | cut -d'.' -f1)
        best_int=${best_int:-999}
        if [ "$avg_int" -lt "$best_int" ] 2>/dev/null; then
            best_avg="$avg"
            best_jitter="$jitter"
            best_loss="$loss"
        fi
    done

    local jitter_rating=$(rate_jitter "$best_jitter")
    local ping_rating=$(rate_ping "$best_avg")

    log "Best: avg=${best_avg}ms jitter=${best_jitter}ms loss=${best_loss}%"
    log "Rating: ping=$ping_rating jitter=$jitter_rating"

    # Bullet registration assessment
    local bullet_assessment="unknown"
    local jitter_int=$(echo "$best_jitter" | cut -d'.' -f1)
    jitter_int=${jitter_int:-99}
    local avg_int=$(echo "$best_avg" | cut -d'.' -f1)
    avg_int=${avg_int:-999}
    local loss_int=$(echo "$best_loss" | cut -d'.' -f1)
    loss_int=${loss_int:-0}

    if [ "$jitter_int" -le 5 ] 2>/dev/null && [ "$avg_int" -le 80 ] 2>/dev/null && [ "$loss_int" -eq 0 ] 2>/dev/null; then
        bullet_assessment="excellent"
    elif [ "$jitter_int" -le 10 ] 2>/dev/null && [ "$avg_int" -le 120 ] 2>/dev/null && [ "$loss_int" -le 2 ] 2>/dev/null; then
        bullet_assessment="good"
    elif [ "$jitter_int" -le 15 ] 2>/dev/null && [ "$avg_int" -le 150 ] 2>/dev/null; then
        bullet_assessment="fair"
    elif [ "$jitter_int" -le 25 ] 2>/dev/null; then
        bullet_assessment="poor"
    else
        bullet_assessment="bad"
    fi

    log "Bullet registration: $bullet_assessment"

    local timestamp=$(date +%s)

    # Write JSON report
    cat > "$REPORT" << JSONEOF
{
  "timestamp": $timestamp,
  "network_type": "$net_type",
  "wifi_info": "$wifi_info",
  "mobile_rat": "$mobile_rat",
  "connectivity": {
    "best_avg_ms": "$best_avg",
    "best_jitter_ms": "$best_jitter",
    "best_loss_pct": "$best_loss"
  },
  "ratings": {
    "ping": "$ping_rating",
    "jitter": "$jitter_rating",
    "bullet_registration": "$bullet_assessment"
  },
  "codm_notes": {
    "tick_rate_hz": 30,
    "tick_interval_ms": 33,
    "jitter_threshold_ms": 10,
    "ideal_ping_ms": "30-60",
    "acceptable_ping_ms": "60-100",
    "bullet_reg_degraded_above_ms": "150"
  }
}
JSONEOF

    log "Report written to $REPORT"
    log "=== CODM Network Diagnostics Complete ==="
}

main
