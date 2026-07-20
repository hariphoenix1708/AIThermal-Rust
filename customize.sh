#!/system/bin/sh
# ThermalAI install-time setup for Magisk / KernelSU.

THERMALAI_BIN_DIR="$MODPATH/system/bin"
THERMALAI_LOG_DIR="/data/local/tmp"
THERMALAI_STATE_DIR="/data/local/tmp/thermalai_state"

ui_print "  Running ThermalAI customize.sh"

mkdir -p "$THERMALAI_LOG_DIR" "$THERMALAI_STATE_DIR" 2>/dev/null

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

ui_print "  ThermalAI install-time setup complete"
