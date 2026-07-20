#!/bin/sh
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if [ -z "$ANDROID_NDK_HOME" ] && [ -z "$ANDROID_NDK_ROOT" ]; then
    echo "WARNING: NDK not set."
else
    NDK_PATH="${ANDROID_NDK_HOME:-$ANDROID_NDK_ROOT}"
    LINKER_PATH="$NDK_PATH/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android34-clang"
    if [ -f "$LINKER_PATH" ]; then
        export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$LINKER_PATH"
    fi
fi

cd rust
cargo build --release --target aarch64-linux-android
cargo build --release --target aarch64-linux-android --bin thermalai-detect
cargo build --release --target aarch64-linux-android --bin thermalair

cd "$SCRIPT_DIR"
mkdir -p system/bin

verify_android_arm64_elf() {
    path="$1"
    if [ ! -f "$path" ]; then
        echo "ERROR: Missing binary: $path"
        exit 1
    fi
    magic="$(od -An -tx1 -N4 "$path" | tr -d ' \n')"
    machine="$(od -An -tx2 -j18 -N2 "$path" | tr -d ' \n')"
    if [ "$magic" != "7f454c46" ]; then
        echo "ERROR: Invalid ELF magic: $path"
        exit 1
    fi
    if [ "$machine" != "00b7" ] && [ "$machine" != "b700" ]; then
        echo "ERROR: Binary is not AArch64 ELF: $path (e_machine=$machine)"
        exit 1
    fi
}

SRC_PATH="rust/target/aarch64-linux-android/release/thermalai-daemon"
if [ -f "$SRC_PATH" ]; then
    STAGING_DIR="$SCRIPT_DIR/staging_zip"
    rm -rf "$STAGING_DIR"
    mkdir -p "$STAGING_DIR"
    mkdir -p "$STAGING_DIR/system/bin"
    cp -R META-INF config module.prop service.sh customize.sh sepolicy.rule uninstall.sh "$STAGING_DIR/"
    if [ -d webroot ]; then
        cp -R webroot "$STAGING_DIR/"
    fi
    cp "rust/target/aarch64-linux-android/release/thermalai-daemon" "$STAGING_DIR/system/bin/thermalai-daemon"
    cp "rust/target/aarch64-linux-android/release/thermalai-detect" "$STAGING_DIR/system/bin/thermalai-detect"
    cp "rust/target/aarch64-linux-android/release/thermalair" "$STAGING_DIR/system/bin/thermalair"
    verify_android_arm64_elf "$STAGING_DIR/system/bin/thermalai-daemon"
    verify_android_arm64_elf "$STAGING_DIR/system/bin/thermalai-detect"
    verify_android_arm64_elf "$STAGING_DIR/system/bin/thermalair"
    find "$STAGING_DIR" -type f \( -name '*.sh' -o -name '*.prop' -o -name '*.conf' -o -name '*.md' -o -name '*.rule' -o -name '*.html' -o -name '*.css' -o -name '*.js' -o -name update-binary -o -name updater-script \) -exec sed -i 's/\r$//' {} +

    echo "Zipping module..."
    rm -f AIThermal-Rust.zip
    if command -v 7z >/dev/null 2>&1; then
        (cd "$STAGING_DIR" && 7z a -tzip "$SCRIPT_DIR/AIThermal-Rust.zip" ./*)
    else
        (cd "$STAGING_DIR" && zip -r "$SCRIPT_DIR/AIThermal-Rust.zip" .)
    fi
    rm -rf "$STAGING_DIR"
    echo "Build complete."
else
    echo "ERROR: Target binary missing."
    exit 1
fi
