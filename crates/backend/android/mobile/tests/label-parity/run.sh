#!/usr/bin/env bash
# On-device regression run for the Android `text` primitive (see src/lib.rs):
# builds the probe app with the in-repo CLI, installs it on the one device
# `adb` sees (or $ANDROID_SERIAL), and checks:
#
#   1. every LabelParity case measures and draws the same as the TextView the
#      backend used to make (content-sized and at an exact width);
#   2. a reactive text update reaches the screen, readable through
#      accessibility;
#   3. a static `text` reports itself to accessibility as a TextView, and a
#      long-press on it opens no selection;
#   4. a `text(selectable = true)` opens Android's text selection on a
#      long-press;
#   5. selectable styled runs render as a selectable TextView and open
#      selection too.
#
# Needs: ANDROID_HOME with an NDK and build-tools, and the
# aarch64-linux-android rust target.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../../.." && pwd)"
out="$repo/target/label-parity-probe"
pkg=ai.truday.idealyst.labelparity

rm -rf "$out/app"; mkdir -p "$out/app"
cp -R "$here/src" "$here/kotlin" "$out/app/"
sed "s|path = \"../../../../../|path = \"$repo/crates/|" "$here/Cargo.toml" >"$out/app/Cargo.toml"
cargo build -q -p idealyst-cli --manifest-path "$repo/Cargo.toml"
(cd "$out/app" && CARGO_TARGET_DIR="$out/cargo" "$repo/target/debug/idealyst" build --android >"$out/build.log" 2>&1) \
    || { tail -30 "$out/build.log"; exit 1; }
adb install -r "$out/app/dist/android/label-parity-probe-0.0.1.apk" >/dev/null

failures=0
check() { if [[ "$2" == true ]]; then echo "PASS $1"; else echo "FAIL $1 ($3)"; failures=$((failures + 1)); fi; }
dump() {
    adb shell uiautomator dump /sdcard/label-parity.xml >/dev/null
    adb shell cat /sdcard/label-parity.xml | tr '>' '\n'
}
tap_button() {
    local b; b=$(dump | grep "text=\"$1\"" \
        | sed -E 's/.*bounds="\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]".*/\1 \2 \3 \4/')
    [[ -n "$b" ]] || { echo "no \"$1\" button on screen" >&2; return 0; }
    set -- $b
    adb shell input tap $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 ))
}

adb shell am force-stop "$pkg"
adb logcat -c
adb shell am start -n "$pkg/.MainActivity" >/dev/null
sleep 4
tap_button Run
sleep 4

done_line=$(adb logcat -d | grep -o '\[label-parity\] DONE.*' | tail -1 || true)
check "RustLabel measures and draws as the TextView it replaced" \
    "$([[ "$done_line" == *", 0 failed" ]] && echo true || echo false)" "${done_line:-no result logged}"
adb logcat -d | grep -o '\[label-parity\] FAIL.*' || true

screen=$(dump)
check "a reactive text update reaches the screen and accessibility" \
    "$([[ "$screen" == *'text="runs: 1, failed: 0"'* ]] && echo true || echo false)" \
    "$(grep -o 'text="runs:[^"]*"' <<<"$screen" || echo 'no runs text')"
plain=$(grep 'text="Plain label"' <<<"$screen" || true)
check "a static text reports itself to accessibility as a TextView" \
    "$([[ "$plain" == *'class="android.widget.TextView"'* ]] && echo true || echo false)" "${plain:-no Plain label node}"

long_press() {
    local b; b=$(dump | grep "text=\"$1\"" \
        | sed -E 's/.*bounds="\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]".*/\1 \2 \3 \4/' || true)
    [[ -n "$b" ]] || { echo "no \"$1\" on screen" >&2; return 0; }
    set -- $b
    local x=$(( ($1 + $3) / 2 )) y=$(( ($2 + $4) / 2 ))
    adb shell input swipe $x $y $x $y 1200
}
# The selection toolbar is a popup window of its own, which only
# `--windows` dumps; the first selection after launch waits on the text
# classifier, so poll.
copy_menu() {
    for _ in $(seq 1 8); do
        if adb exec-out uiautomator dump --windows /dev/tty 2>/dev/null | grep -q 'text="Copy"'; then echo 1; return; fi
        sleep 1
    done
    echo 0
}

long_press "Plain label"
check "a long-press on a plain label opens no selection" "$([[ "$(copy_menu)" == 0 ]] && echo true || echo false)" \
    "a Copy action is showing"
long_press "Selectable label"
check "a long-press on a selectable text opens Android's text selection" \
    "$([[ "$(copy_menu)" != 0 ]] && echo true || echo false)" "no Copy action after the long-press"

adb shell input keyevent KEYCODE_ESCAPE; sleep 1
styled=$(dump | grep 'text="Styled selectable"' || true)
check "selectable styled runs render as a selectable TextView" \
    "$([[ "$styled" == *'class="android.widget.TextView"'* ]] && echo true || echo false)" "${styled:-no Styled selectable node}"
long_press "Styled selectable"
check "a long-press on selectable styled runs opens Android's text selection" \
    "$([[ "$(copy_menu)" != 0 ]] && echo true || echo false)" "no Copy action after the long-press"

[[ $failures == 0 ]] && echo "all checks passed" || { echo "$failures check(s) failed"; exit 1; }
