#!/usr/bin/env bash
# On-device regression run for net's Android transport (see src/lib.rs):
# builds the probe cdylib + a one-class dex, pushes both to the connected
# emulator/device, runs them under `app_process`, and fails unless every
# case prints PASS.
#
# Needs: one device on `adb`, ANDROID_HOME with an NDK and build-tools,
# `javac`, and the aarch64-linux-android rust target. The library is linked
# with 16 KB max page size so it also loads on 16 KB-page system images.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../../.." && pwd)"
sdk="${ANDROID_HOME:?set ANDROID_HOME}"
ndk="${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* | sort -V | tail -1)}"
ndk_bin="$(ls -d "$ndk"/toolchains/llvm/prebuilt/*/bin | head -1)"
build_tools="$(ls -d "$sdk"/build-tools/* | sort -V | tail -1)"
out="$repo/target/net-android-device-probe"
mkdir -p "$out/classes" "$out/dex" "$out/crate"

# Build a copy under target/ so the resolve leaves no Cargo.lock in the
# source tree; the `net` path dependency is pinned to this checkout.
cp -R "$here/src" "$out/crate/"
sed "s|path = \"../..\"|path = \"$here/../..\"|" "$here/Cargo.toml" >"$out/crate/Cargo.toml"

CARGO_TARGET_DIR="$out/cargo" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$ndk_bin/aarch64-linux-android30-clang" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
    cargo build --manifest-path "$out/crate/Cargo.toml" --target aarch64-linux-android

javac --release 11 -d "$out/classes" "$here/NetProbe.java"
"$build_tools/d8" --min-api 30 --output "$out/dex" "$out/classes/NetProbe.class"

adb push "$out/cargo/aarch64-linux-android/debug/libnet_android_device_probe.so" \
    /data/local/tmp/libnet_android_device_probe.so >/dev/null
adb push "$out/dex/classes.dex" /data/local/tmp/net_android_device_probe.dex >/dev/null

report="$(adb shell CLASSPATH=/data/local/tmp/net_android_device_probe.dex \
    app_process /system/bin NetProbe /data/local/tmp/libnet_android_device_probe.so)"
echo "$report"
passed="$(grep -c '^PASS' <<<"$report" || true)"
if [[ "$passed" != 5 ]] || grep -q '^FAIL' <<<"$report"; then
    echo "net android device probe: FAILED ($passed/5 passed)" >&2
    exit 1
fi
echo "net android device probe: 5/5 passed"
