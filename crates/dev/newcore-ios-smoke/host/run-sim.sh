#!/bin/bash
# Build + launch newcore-ios-smoke on the iOS Simulator, replicating
# the `idealyst run ios` (crates/tools/run/ios) bundle steps by hand.
# The smoke app ships its OWN `ios_main` (so the boot path under test is
# this crate's, not the CLI wrapper's) and this script does the shell's
# other half: swiftc link, .app assembly, simctl install/launch.
#
# Launches with the self-test armed (SIMCTL_CHILD_NEWCORE_SMOKE_SELFTEST=1);
# the app prints `[SMOKE-SELFTEST] committed=<bool> views=<n>` and
# exits 0/1 (~2.2 s after launch). Requires Xcode + a bootable iPhone
# simulator.
set -euo pipefail

HOST_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$HOST_DIR/../../../.." && pwd)"
BUNDLE_ID="com.idealyst.newcore-ios-smoke"
EXECUTABLE="NewcoreIosSmoke"
BUILD_DIR="$WORKSPACE_ROOT/target/idealyst/newcore-ios-smoke"
APP_BUNDLE="$BUILD_DIR/$EXECUTABLE.app"

# ── 1. Rust staticlib (aarch64 sim; mirror run-ios pick_target) ──────
RUST_TARGET="aarch64-apple-ios-sim"
SWIFT_TARGET="arm64-apple-ios16.0-simulator"
if [[ "$(uname -m)" == "x86_64" ]]; then
    RUST_TARGET="x86_64-apple-ios"
    SWIFT_TARGET="x86_64-apple-ios16.0-simulator"
fi
echo "[run-sim] cargo build -p newcore-ios-smoke --target $RUST_TARGET"
cargo build --manifest-path "$WORKSPACE_ROOT/Cargo.toml" \
    -p newcore-ios-smoke --target "$RUST_TARGET"
LIB_DIR="$WORKSPACE_ROOT/target/$RUST_TARGET/debug"

# ── 2. Bundle + swiftc link (mirror run-ios compile_and_link; base
#      framework set: UIKit/Foundation weak, CoreGraphics/QuartzCore
#      strong — see crates/tools/run/ios/src/frameworks.rs) ──────────
rm -rf "$APP_BUNDLE"
mkdir -p "$APP_BUNDLE"
SDK_PATH="$(xcrun --sdk iphonesimulator --show-sdk-path)"
echo "[run-sim] swiftc -target $SWIFT_TARGET"
xcrun -sdk iphonesimulator swiftc \
    -target "$SWIFT_TARGET" \
    -sdk "$SDK_PATH" \
    -import-objc-header "$HOST_DIR/BridgingHeader.h" \
    -emit-executable -O \
    -o "$APP_BUNDLE/$EXECUTABLE" \
    "$HOST_DIR/AppDelegate.swift" \
    "$HOST_DIR/ViewController.swift" \
    -L "$LIB_DIR" -lnewcore_ios_smoke \
    -Xlinker -weak_framework -Xlinker UIKit \
    -Xlinker -weak_framework -Xlinker Foundation \
    -framework CoreGraphics \
    -framework QuartzCore
cp "$HOST_DIR/Info.plist" "$APP_BUNDLE/Info.plist"
printf 'APPL????' > "$APP_BUNDLE/PkgInfo"

# ── 3. Simulator: reuse a booted device or boot the first iPhone ────
UDID="$(xcrun simctl list devices booted | grep -Eo '[0-9A-F-]{36}' | head -1 || true)"
if [[ -z "$UDID" ]]; then
    UDID="$(xcrun simctl list devices available | grep -E 'iPhone' | grep -Eo '[0-9A-F-]{36}' | head -1)"
    echo "[run-sim] booting simulator $UDID"
    xcrun simctl boot "$UDID"
    xcrun simctl bootstatus "$UDID" -b
fi
open -a Simulator || true

# ── 4. Install + launch with the self-test armed ────────────────────
xcrun simctl terminate "$UDID" "$BUNDLE_ID" 2>/dev/null || true
echo "[run-sim] simctl install → $UDID"
xcrun simctl install "$UDID" "$APP_BUNDLE"
echo "[run-sim] simctl launch --console-pty $BUNDLE_ID (selftest armed)"
SIMCTL_CHILD_NEWCORE_SMOKE_SELFTEST=1 \
    xcrun simctl launch --console-pty "$UDID" "$BUNDLE_ID"
