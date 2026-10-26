#!/usr/bin/env bash
# Regression: a screen-pinned Android app must fill the screen, and get its
# system-bar insets back when unpinned.
#
# Pinning empties the status bar and the tablet taskbar but keeps reporting
# both insets as visible, with no insets dispatch — so the host root stayed
# inset around a blank top strip and a black taskbar slab.
# `RustSystemUi.watchPinning` hides the bars while pinned.
#
# This drives a real device because there is no pinning callback or insets
# change to unit-test: the behavior lives entirely in the system's
# lock-task handling. Run it against any framework app already in the
# foreground (e.g. `idealyst dev --android --local examples/login-demo`):
#
#   scripts/regression-android-pinned-app-fills-screen.sh ai.truday.idealyst.logindemo
#
# Pins with `am task lock`, which needs no confirmation dialog.
set -euo pipefail

pkg="${1:?usage: $0 <package>}"
adb_() { adb ${ANDROID_SERIAL:+-s "$ANDROID_SERIAL"} "$@"; }

# The framework's host root is the first view inside `android:id/content`.
root_frame() {
    adb_ shell dumpsys activity "$pkg" \
        | grep -A1 "#1020002 android:id/content" \
        | sed -n 2p \
        | grep -oE '[0-9]+,[0-9]+-[0-9]+,[0-9]+'
}

size=$(adb_ shell wm size | grep -oE '[0-9]+x[0-9]+' | tail -1)
full="0,0-${size%x*},${size#*x}"
task=$(adb_ shell dumpsys activity activities \
    | grep -oE "$pkg/[^ ]+ t[0-9]+" | head -1 | grep -oE '[0-9]+$')
[ -n "$task" ] || { echo "FAIL: $pkg has no running task"; exit 1; }

adb_ shell am task lock stop >/dev/null 2>&1 || true
sleep 2
before=$(root_frame)

adb_ shell am task lock "$task" >/dev/null
sleep 2
pinned=$(root_frame)

adb_ shell am task lock stop >/dev/null
sleep 2
after=$(root_frame)

echo "unpinned: $before  pinned: $pinned  unpinned again: $after  (screen $full)"
fail=0
[ "$pinned" = "$full" ] || { echo "FAIL: pinned root $pinned does not fill $full"; fail=1; }
[ "$after" = "$before" ] || { echo "FAIL: unpinned root $after did not return to $before"; fail=1; }
[ $fail = 0 ] && echo PASS
exit $fail
