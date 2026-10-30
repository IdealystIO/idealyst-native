#!/usr/bin/env bash
# On-device regression run for the Android host lifecycle (see src/lib.rs):
# builds the probe app with the in-repo CLI, installs it on the one device
# `adb` sees (or $ANDROID_SERIAL), and checks:
#
#   1. system Back on a root screen with back_enabled(false) leaves the app
#      in front;
#   2. after the Activity finishes and a new one starts in the same process,
#      a timer and an HTTPS request started by the new tree both complete;
#   3. Activity.finish() through ndk_context reaches the NEW Activity.
#
# With --keep-alive (a fresh emulator only: it makes the probe the device
# owner, which needs no accounts on the device) it also checks
#
#   4. a device owner that called start_keep_alive keeps running while
#      backgrounded: heartbeat ticks continue and the process is not frozen.
#
# Needs: ANDROID_HOME with an NDK and build-tools, the aarch64-linux-android
# rust target, network on the device (check 2 fetches https://example.com).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../../.." && pwd)"
out="$repo/target/android-lifecycle-probe"
pkg=ai.truday.idealyst.lifecycleprobe
keep_alive=false
[[ "${1:-}" == "--keep-alive" ]] && keep_alive=true

rm -rf "$out/app"; mkdir -p "$out/app"
cp -R "$here/src" "$out/app/"
sed "s|path = \"../../../../../|path = \"$repo/crates/|" "$here/Cargo.toml" >"$out/app/Cargo.toml"
cargo build -q -p idealyst-cli --manifest-path "$repo/Cargo.toml"
(cd "$out/app" && CARGO_TARGET_DIR="$out/cargo" "$repo/target/debug/idealyst" build --android >"$out/build.log" 2>&1) \
    || { tail -30 "$out/build.log"; exit 1; }
adb install -r "$out/app/dist/android/android-lifecycle-probe-0.0.1.apk" >/dev/null

failures=0
check() { if [[ "$2" == true ]]; then echo "PASS $1"; else echo "FAIL $1 ($3)"; failures=$((failures + 1)); fi; }
top_is_probe() { adb shell dumpsys activity activities | grep -m1 topResumedActivity | grep -q "$pkg"; }
activity_count() { adb shell dumpsys activity activities | grep -c "$pkg/.MainActivity" || true; }
screen_texts() {
    adb shell uiautomator dump /sdcard/lifecycle.xml >/dev/null
    adb shell cat /sdcard/lifecycle.xml | tr '>' '\n' | grep -o 'text="[a-z]*: [^"]*"' | tr '\n' ' '
}
tap_finish() {
    adb shell uiautomator dump /sdcard/lifecycle.xml >/dev/null
    local b; b=$(adb shell cat /sdcard/lifecycle.xml | tr '>' '\n' | grep 'text="Finish"' \
        | sed -E 's/.*bounds="\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]".*/\1 \2 \3 \4/')
    set -- $b
    adb shell input tap $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 ))
}
wait_for_texts() {
    for _ in $(seq 1 20); do
        local t; t=$(screen_texts)
        if [[ "$t" == *"timer: ok"* && "$t" == *"http: Ok(200)"* ]]; then echo "$t"; return 0; fi
        sleep 1
    done
    screen_texts; return 1
}

adb shell am force-stop "$pkg"
adb shell am start -n "$pkg/.MainActivity" >/dev/null
cold=$(wait_for_texts) || true
check "cold start: async work completes" "$([[ "$cold" == *"timer: ok"*"http: Ok(200)"* ]] && echo true || echo false)" "$cold"

adb shell input keyevent KEYCODE_BACK; sleep 1; adb shell input keyevent KEYCODE_BACK; sleep 2
check "Back on a root screen with back_enabled(false) keeps the app in front" \
    "$(top_is_probe && echo true || echo false)" "Back left the app"

pid=$(adb shell pidof "$pkg")
tap_finish; sleep 2
check "Finish destroys the first Activity" "$([[ "$(activity_count)" == 0 ]] && echo true || echo false)" \
    "$(activity_count) activity lines"
adb shell am start -n "$pkg/.MainActivity" >/dev/null
again=$(wait_for_texts) || true
check "relaunch in the same process (pid $pid): async work completes" \
    "$([[ "$(adb shell pidof "$pkg")" == "$pid" && "$again" == *"timer: ok"*"http: Ok(200)"* ]] && echo true || echo false)" \
    "pid $(adb shell pidof "$pkg"), screen: $again"
tap_finish; sleep 2
check "ndk_context publishes the relaunched Activity (its finish() lands)" \
    "$([[ "$(activity_count)" == 0 ]] && echo true || echo false)" "the relaunched Activity is still up"

if $keep_alive; then
    adb shell dpm set-device-owner "$pkg/io.idealyst.deviceadmin.AdminReceiver" >/dev/null
    adb shell am force-stop "$pkg"
    adb shell am start -n "$pkg/.MainActivity" >/dev/null; sleep 6
    adb shell input keyevent KEYCODE_HOME
    adb logcat -c
    # The cached-apps freezer froze an unkept process within a minute.
    sleep 75
    ticks=$(adb logcat -d | grep -c "\[lifecycle\] tick" || true)
    frozen=$(adb shell dumpsys activity processes | grep -A40 "ProcessRecord{.*$pkg" | grep -m1 -oE "isFrozen=[a-z]+" || true)
    check "start_keep_alive: backgrounded 75 s, still ticking and not frozen" \
        "$([[ "$ticks" -ge 12 && "$frozen" == "isFrozen=false" ]] && echo true || echo false)" "$ticks ticks, $frozen"
fi

if [[ "$failures" != 0 ]]; then echo "android lifecycle probe: $failures FAILED" >&2; exit 1; fi
echo "android lifecycle probe: all passed"
