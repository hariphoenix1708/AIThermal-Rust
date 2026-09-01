#!/system/bin/sh
# ThermalAI install-time setup for Magisk / KernelSU(-Next).
# NOTE: KernelSU and KernelSU-Next run ONLY customize.sh — they ignore
# META-INF/com/google/android/update-binary — so the full install UI has to
# live here to appear on every manager. Magisk's update-binary calls this
# script at the end, so it is the single source of the install log.

ui_print "  Running ThermalAI customize.sh"

MOD_VERSION=$(grep -E '^version=' "$MODPATH/module.prop" 2>/dev/null | cut -d= -f2)
MOD_VERSION=${MOD_VERSION:-unknown}

ui_print ""
ui_print "-----------------------------------------"
ui_print "   ThermalAI $MOD_VERSION - Rust Edition"
ui_print "   Device Edition: Universal Android"
ui_print "   Linux 4.14+ / AOSP / HyperOS"
ui_print "-----------------------------------------"
ui_print ""

DEVICE=$(getprop ro.product.device)
BOARD=$(getprop ro.product.board)
SOC=$(getprop ro.board.platform)

# Peridot family recognition. The POCO F6 / Redmi Turbo 3 (SM8635) can
# report ro.board.platform as "pineapple" on HyperOS or "sun" on other
# builds, and ro.product.device/board as "peridot". Accept every alias so
# the banner does not spam warnings on the exact device it targets.
is_peridot=false
case "$DEVICE" in
    *peridot*|*poco*f6*|*redmi*turbo*3*|*24069pc21g*|*24069ra21c*) is_peridot=true ;;
esac
case "$BOARD" in
    *peridot*) is_peridot=true ;;
esac
case "$SOC" in
    pineapple|sun) is_peridot=true ;;
esac

if [ "$is_peridot" != "true" ]
then
    ui_print "  [!] WARNING: Peridot optimizations are optional and runtime-gated"
    ui_print "  [!] Your device: $DEVICE / $SOC"
    ui_print "  [!] Generic discovery remains enabled"
    ui_print ""
fi

ui_print "  Clearing old logs and state (keeping ML dataset)..."
LOG_DIR="${THERMALAI_LOG_DIR:-/data/local/tmp/AIThermal}"
STATE_DIR="${THERMALAI_STATE_DIR:-/data/local/tmp/AIThermal/state}"
THERMALAI_BIN_DIR="$MODPATH/system/bin"
rm -f "$LOG_DIR/thermalai.log"
rm -f "$LOG_DIR/thermalai_verbose.log"
rm -f "$LOG_DIR/thermalai_startup.log"
rm -f "$LOG_DIR/thermalai_battery.log"
rm -f "$LOG_DIR/thermalai_thermal.log"
rm -f "$LOG_DIR/thermalai_charging.log"
rm -f "$LOG_DIR/thermalai_gaming.log"
rm -f "$LOG_DIR/thermalai_ui.log"
rm -f "$LOG_DIR/thermalai.pid"
rm -f "$LOG_DIR/thermalai.pid.lock"
# v3.7.7: preserve ML dataset/model across update (2MB ring is valuable).
if [ -d "$STATE_DIR" ]; then
    mkdir -p /data/local/tmp/AIThermal_preserve 2>/dev/null
    cp -a "$STATE_DIR/ml_features.jsonl"* /data/local/tmp/AIThermal_preserve/ 2>/dev/null
    cp -a "$STATE_DIR/ml_model."* /data/local/tmp/AIThermal_preserve/ 2>/dev/null
    rm -rf "$STATE_DIR"
    mkdir -p "$STATE_DIR" 2>/dev/null
    cp -a /data/local/tmp/AIThermal_preserve/* "$STATE_DIR/" 2>/dev/null
    rm -rf /data/local/tmp/AIThermal_preserve 2>/dev/null
else
    rm -rf "$STATE_DIR"
    mkdir -p "$STATE_DIR" 2>/dev/null
fi
mkdir -p "$LOG_DIR" 2>/dev/null

for bin in thermalai-daemon thermalai-detect thermalair
do
    path="$THERMALAI_BIN_DIR/$bin"
    if [ -f "$path" ]
    then
        chmod 0755 "$path" 2>/dev/null
        chcon u:object_r:su_file:s0 "$path" 2>/dev/null || true
    else
        ui_print "  [!] Missing binary: $path"
    fi
done

chmod 0755 "$MODPATH/service.sh" 2>/dev/null
chmod 0755 "$MODPATH/uninstall.sh" 2>/dev/null
chmod 0644 "$MODPATH/sepolicy.rule" 2>/dev/null

# KernelSU WebUI assets (served when user taps the module in KernelSU Manager)
if [ -d "$MODPATH/webroot" ]; then
    find "$MODPATH/webroot" -type d -exec chmod 0755 {} \; 2>/dev/null
    find "$MODPATH/webroot" -type f -exec chmod 0644 {} \; 2>/dev/null
    ui_print "  ThermalAI WebUI installed (KernelSU Manager -> tap module)"
fi

ui_print ""
ui_print "  Device  : $(getprop ro.product.model)"
ui_print "  Android : $(getprop ro.build.version.release)"
ui_print "  ROM     : $(getprop ro.build.display.id)"
ui_print "  KSU     : $(ksud -V 2>/dev/null || ksu -V 2>/dev/null || echo 'N/A (Magisk?)')"
ui_print ""
ui_print "  - AI daemon          -> system/bin/thermalai-daemon"
ui_print "  - Hardware Discovery -> capability cache with boot validation"
ui_print "  - Charge Engine      -> adaptive SOC thermal tapering"
ui_print "  - mi_thermald        -> coordinated on boot"
ui_print "  - Gaming detection   -> package scan + KGSL load awareness"
ui_print "  - CLI tool           -> thermalair (in PATH after reboot)"
ui_print ""
ui_print "  Config: /data/adb/modules/thermalai_rust/config/profiles.conf"
ui_print "  Games : /data/adb/modules/thermalai_rust/config/game_list.conf"
ui_print "  Log   : /data/local/tmp/AIThermal/thermalai.log"
ui_print "  UI    : /data/local/tmp/AIThermal/thermalai_ui.log"
ui_print "  State : /data/local/tmp/AIThermal/state"
ui_print ""
ui_print "  Reboot to activate ThermalAI"
ui_print "-----------------------------------------"

ui_print "  ThermalAI install-time setup complete"
