#!/usr/bin/env bash
# On-device regression run for the Android press-down `pressable`
# (`PressActivation::Down`, see src/lib.rs): builds the probe app with the
# in-repo CLI, installs it on $ANDROID_SERIAL, and checks:
#
#   1. the PressDownProbe cases (synthesized events on a bare
#      RustPressDownLayout: finger, mouse primary / secondary / no button,
#      cancel, keyboard, TalkBack ACTION_CLICK, performClick, disabled) pass;
#   2. through the real backend, an injected finger DOWN on "Menu" counts one
#      press before the UP arrives;
#   3. the moves and the UP count nothing more, and the release-mode "Outer"
#      pressable around it never fires;
#   4. a swipe that starts on "Menu" counts one press and does not scroll the
#      page (the down claims the gesture from the scroll view);
#   5. PressableHandle::click on "Menu" counts one press.
#
# ANDROID_SERIAL is required, not defaulted: `adb` without it picks whatever
# device is attached, which may be someone's kiosk tablet.
#
# Needs: ANDROID_HOME with an NDK and build-tools, and the
# aarch64-linux-android rust target.
set -euo pipefail

: "${ANDROID_SERIAL:?set ANDROID_SERIAL to the emulator to run on (see adb devices)}"
export ANDROID_SERIAL

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../../.." && pwd)"
out="$repo/target/press-down-probe"
pkg=io.idealyst.pressdown

rm -rf "$out/app"; mkdir -p "$out/app"
cp -R "$here/src" "$here/kotlin" "$out/app/"
sed "s|path = \"../../../../../|path = \"$repo/crates/|" "$here/Cargo.toml" >"$out/app/Cargo.toml"
cargo build -q -p idealyst-cli --manifest-path "$repo/Cargo.toml"
(cd "$out/app" && CARGO_TARGET_DIR="$out/cargo" "$repo/target/debug/idealyst" build --android >"$out/build.log" 2>&1) \
    || { tail -30 "$out/build.log"; exit 1; }
adb install -r "$out/app/dist/android/press-down-probe-0.0.1.apk" >/dev/null

failures=0
check() { if [[ "$2" == true ]]; then echo "PASS $1"; else echo "FAIL $1 ($3)"; failures=$((failures + 1)); fi; }
dump() {
    adb shell uiautomator dump /sdcard/press-down.xml >/dev/null
    adb shell cat /sdcard/press-down.xml | tr '>' '\n'
}
# "x1 y1 x2 y2" of the node whose text is $1.
bounds() {
    dump | grep "text=\"$1\"" | head -1 \
        | sed -E 's/.*bounds="\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]".*/\1 \2 \3 \4/'
}
center() { set -- $(bounds "$1"); [[ $# == 4 ]] || { echo "no \"$1\" on screen" >&2; exit 1; }; echo "$(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 ))"; }
status() { dump | grep -o 'text="presses: [^"]*"' | head -1; }
tap() { adb shell input tap $(center "$1"); }

adb shell am force-stop "$pkg"
adb logcat -c
adb shell am start -n "$pkg/.MainActivity" >/dev/null
sleep 4

tap Run
sleep 2
done_line=$(adb logcat -d | grep -o '\[press-down\] DONE.*' | tail -1 || true)
check "RustPressDownLayout cases" "$([[ "$done_line" == *", 0 failed" ]] && echo true || echo false)" "${done_line:-no result logged}"
adb logcat -d | grep -o '\[press-down\] FAIL.*' || true

set -- $(center Menu); mx=$1; my=$2
adb shell input motionevent DOWN "$mx" "$my"
sleep 1
s=$(status)
check "a finger down counts one press before the release" \
    "$([[ "$s" == *'presses: 1, outer: 0'* ]] && echo true || echo false)" "$s"
adb shell input motionevent MOVE "$((mx + 5))" "$((my + 5))"
adb shell input motionevent UP "$((mx + 5))" "$((my + 5))"
sleep 1
s=$(status)
check "the moves and the release count nothing more; the outer pressable never fires" \
    "$([[ "$s" == *'presses: 1, outer: 0'* ]] && echo true || echo false)" "$s"

before=$(bounds Menu)
adb shell input swipe "$mx" "$my" "$mx" "$((my - 600))" 400
sleep 1
s=$(status); after=$(bounds Menu)
check "a swipe starting on the press-down pressable counts one press" \
    "$([[ "$s" == *'presses: 2, outer: 0'* ]] && echo true || echo false)" "$s"
check "that swipe does not scroll the page" "$([[ "$before" == "$after" ]] && echo true || echo false)" "Menu moved: $before -> $after"

tap "Click handle"
sleep 1
s=$(status)
check "PressableHandle::click counts one press" \
    "$([[ "$s" == *'presses: 3, outer: 0'* ]] && echo true || echo false)" "$s"

[[ $failures == 0 ]] && echo "all checks passed" || { echo "$failures check(s) failed"; exit 1; }
