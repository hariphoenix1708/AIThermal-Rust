#!/system/bin/sh
# AIThermal-Rust: On-device sysfs discovery script for POCO F6 (peridot)
# Run this script on the device to discover writable charging control paths
# Usage: adb shell sh /data/local/tmp/discover_peridot_sysfs.sh
# Or: adb push scripts/discover_peridot_sysfs.sh /data/local/tmp/ && adb shell sh /data/local/tmp/discover_peridot_sysfs.sh

echo "=== AIThermal-Rust Sysfs Discovery for POCO F6 (peridot) ==="
echo "Date: $(date)"
echo "Kernel: $(uname -r)"
echo "Device: $(getprop ro.product.device)"
echo ""

echo "=== CHARGING CONTROL PATHS ==="
echo ""
echo "--- QCOM Battery Voter Nodes (/sys/class/qcom-battery/) ---"
if [ -d /sys/class/qcom-battery ]; then
    for f in /sys/class/qcom-battery/*; do
        if [ -f "$f" ]; then
            name=$(basename "$f")
            perms=$(ls -la "$f" | awk '{print $1}')
            content=$(cat "$f" 2>/dev/null | head -1)
            echo "  $name [$perms] = $content"
        fi
    done
else
    echo "  /sys/class/qcom-battery/ not found"
fi

echo ""
echo "--- Power Supply Nodes (/sys/class/power_supply/) ---"
for ps in /sys/class/power_supply/*; do
    if [ -d "$ps" ]; then
        ps_name=$(basename "$ps")
        echo "  [$ps_name]"
        for f in "$ps"/*; do
            if [ -f "$f" ]; then
                name=$(basename "$f")
                perms=$(ls -la "$f" | awk '{print $1}')
                content=$(cat "$f" 2>/dev/null | head -1)
                echo "    $name [$perms] = $content"
            fi
        done
    fi
done

echo ""
echo "=== WRITABLE CHARGING NODES TEST ==="
echo "Testing which charging nodes accept writes..."

test_write() {
    local node=$1
    local current=$(cat "$node" 2>/dev/null)
    if [ -z "$current" ]; then
        echo "  SKIP $node (empty or unreadable)"
        return
    fi
    # Try writing the same value back
    echo "$current" > "$node" 2>/dev/null
    if [ $? -eq 0 ]; then
        echo "  WRITABLE $node (current=$current)"
    else
        echo "  READ-ONLY $node (current=$current)"
    fi
}

# Test QCOM voter nodes
echo ""
echo "--- QCOM Voter Nodes (test write) ---"
for node in /sys/class/qcom-battery/restrict_chg /sys/class/qcom-battery/restrict_cur \
            /sys/class/qcom-battery/input_suspend /sys/class/qcom-battery/night_charging; do
    if [ -f "$node" ]; then
        test_write "$node"
    fi
done

# Test current limit nodes
echo ""
echo "--- Current Limit Nodes (test write) ---"
for node in /sys/class/power_supply/battery/constant_charge_current_max \
            /sys/class/power_supply/bms/constant_charge_current_max \
            /sys/class/power_supply/main/constant_charge_current_max \
            /sys/class/power_supply/battery/current_max \
            /sys/class/power_supply/main/current_max \
            /sys/class/power_supply/usb/current_max \
            /sys/class/power_supply/dc/current_max \
            /sys/class/power_supply/ac/current_max \
            /sys/class/power_supply/usb/input_current_limit \
            /sys/class/power_supply/dc/input_current_limit \
            /sys/class/power_supply/ac/input_current_limit; do
    if [ -f "$node" ]; then
        test_write "$node"
    fi
done

echo ""
echo "=== NETWORK TUNING PATHS ==="
echo ""
echo "--- TCP/UDP Buffer Paths ---"
for path in /proc/sys/net/ipv4/tcp_rmem /proc/sys/net/ipv4/tcp_wmem \
            /proc/sys/net/ipv4/tcp_mem /proc/sys/net/core/rmem_max \
            /proc/sys/net/core/wmem_max /proc/sys/net/core/netdev_max_backlog \
            /proc/sys/net/core/netdev_budget /proc/sys/net/core/busy_poll \
            /proc/sys/net/core/busy_read /proc/sys/net/core/dev_weight \
            /proc/sys/net/ipv4/udp_mem; do
    if [ -f "$path" ]; then
        content=$(cat "$path" 2>/dev/null)
        echo "  $path = $content"
    else
        echo "  $path = NOT FOUND"
    fi
done

echo ""
echo "=== SCHEDULER/UCLAMP PATHS ==="
echo ""
echo "--- uclamp paths ---"
for path in /dev/cpuctl/top-app/cpu.uclamp.max \
            /dev/cpuctl/top-app/cpu.uclamp.min \
            /dev/cpuctl/foreground/cpu.uclamp.max \
            /dev/cpuctl/foreground/cpu.uclamp.min; do
    if [ -f "$path" ]; then
        content=$(cat "$path" 2>/dev/null)
        echo "  $path = $content"
    else
        echo "  $path = NOT FOUND"
    fi
done

echo ""
echo "--- CPU Frequency Paths ---"
for cluster in /sys/devices/system/cpu/cpufreq/cpu*; do
    if [ -d "$cluster" ]; then
        cluster_name=$(basename "$cluster")
        gov=$(cat "$cluster/scaling_governor" 2>/dev/null)
        min=$(cat "$cluster/scaling_min_freq" 2>/dev/null)
        max=$(cat "$cluster/scaling_max_freq" 2>/dev/null)
        cpuinfo_min=$(cat "$cluster/cpuinfo_min_freq" 2>/dev/null)
        cpuinfo_max=$(cat "$cluster/cpuinfo_max_freq" 2>/dev/null)
        echo "  $cluster_name: gov=$gov min=$min max=$max (range: $cpuinfo_min - $cpuinfo_max)"
    fi
done

echo ""
echo "=== THERMAL ZONES ==="
echo ""
for tz in /sys/class/thermal/thermal_zone*; do
    if [ -d "$tz" ]; then
        type=$(cat "$tz/type" 2>/dev/null)
        temp=$(cat "$tz/temp" 2>/dev/null)
        echo "  $(basename $tz): type=$type temp=$temp"
    fi
done

echo ""
echo "=== DISPLAY REFRESH RATE ==="
echo ""
dumpsys display 2>/dev/null | grep -i "refresh" | head -5

echo ""
echo "=== DONE ==="
echo "Report this output to the developer for peridot-specific optimizations."
