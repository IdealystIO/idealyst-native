#!/usr/bin/env bash
# On-device regression run for device-admin's end of provisioning
# (`RustDeviceAdmin.finishProvisioning`, called by PolicyComplianceActivity
# before it reports RESULT_OK). Builds the probe app with the in-repo CLI,
# makes it device owner on $ANDROID_SERIAL, runs the compliance activity the
# way the setup wizard does at the end of QR provisioning, and checks:
#
#   1. the app (home_launcher = true) is the persistent home app, so the
#      HOME the wizard ends with starts it, with nobody opening it;
#   2. its events subscriber gets ProvisioningComplete on that first start;
#   3. a package in provisioning_hide is hidden, and one that is not
#      installed is skipped without failing anything;
#   4. both still hold after a reboot.
#
# The QR code's admin extras (`idealyst.hide_packages`) take the same path
# as the manifest list but cannot be sent from adb (`am start` has no
# PersistableBundle extra), so only the manifest list is driven here.
#
# Starting the BIND_DEVICE_ADMIN-protected activity needs `adb root`, and
# `dpm set-device-owner` needs a device with no accounts: run on a FRESH,
# THROWAWAY emulator (a google_apis image) and delete it afterwards — there
# is no undoing device owner short of a wipe. ANDROID_SERIAL is required,
# not defaulted: `adb` without it picks whatever device is attached, which
# may be someone's kiosk tablet.
set -euo pipefail

: "${ANDROID_SERIAL:?set ANDROID_SERIAL to a throwaway emulator (see adb devices)}"
export ANDROID_SERIAL
[[ "$ANDROID_SERIAL" == emulator-* ]] || { echo "refusing to run on $ANDROID_SERIAL: emulators only" >&2; exit 1; }

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../../.." && pwd)"
out="$repo/target/provisioning-home-probe"
pkg=io.idealyst.provisioninghome
hidden_pkg=com.android.camera2

rm -rf "$out/app"; mkdir -p "$out/app"
cp -R "$here/src" "$out/app/"
sed -e "s|path = \"../../../../../|path = \"$repo/crates/|" \
    -e "s|path = \"../..\"|path = \"$repo/crates/sdk/client/device-admin\"|" \
    "$here/Cargo.toml" >"$out/app/Cargo.toml"
cargo build -q -p idealyst-cli --manifest-path "$repo/Cargo.toml"
(cd "$out/app" && CARGO_TARGET_DIR="$out/cargo" "$repo/target/debug/idealyst" build --android >"$out/build.log" 2>&1) \
    || { tail -30 "$out/build.log"; exit 1; }

adb root >/dev/null; sleep 2; adb wait-for-device
adb install -r "$out/app/dist/android/provisioning-home-probe-0.0.1.apk" >/dev/null
adb shell dpm set-device-owner "$pkg/io.idealyst.deviceadmin.AdminReceiver" >/dev/null

failures=0
check() { if [[ "$2" == true ]]; then echo "PASS $1"; else echo "FAIL $1 ($3)"; failures=$((failures + 1)); fi; }
home() { adb shell cmd package resolve-activity --brief -a android.intent.action.MAIN -c android.intent.category.HOME | tail -1 | tr -d '\r'; }
is_hidden() { adb shell dumpsys package "$hidden_pkg" | grep -q 'hidden=true' && echo true || echo false; }
top() { adb shell dumpsys activity activities | grep -o 'topResumedActivity=[^ ]* [^ ]* [^ ]*' | head -1; }

check "before provisioning ends, the app is not home" "$([[ "$(home)" != "$pkg/"* ]] && echo true || echo false)" "$(home)"
check "before provisioning ends, $hidden_pkg is visible" "$([[ "$(is_hidden)" == false ]] && echo true || echo false)" "hidden"

# The wizard's last step, then the HOME it ends with.
adb shell am force-stop "$pkg"
adb logcat -c
adb shell am start -W -n "$pkg/io.idealyst.deviceadmin.PolicyComplianceActivity" >/dev/null
sleep 1
adb shell input keyevent KEYCODE_HOME
sleep 5

check "the app is the persistent home app" "$([[ "$(home)" == "$pkg/"* ]] && echo true || echo false)" "$(home)"
t=$(top)
check "HOME starts the app with nobody opening it" "$([[ "$t" == *"$pkg/.MainActivity"* ]] && echo true || echo false)" "$t"
ev=$(adb logcat -d | grep -o '\[provisioning-home\] ProvisioningComplete.*' | head -1 || true)
check "its subscriber gets ProvisioningComplete on that first start" "$([[ -n "$ev" ]] && echo true || echo false)" "nothing logged"
check "$hidden_pkg (in provisioning_hide) is hidden" "$(is_hidden)" "not hidden"
warn=$(adb logcat -d | grep -E 'device_admin.*finishProvisioning' || true)
check "the missing package is skipped without a warning" "$([[ -z "$warn" ]] && echo true || echo false)" "$warn"

adb reboot; adb wait-for-device
until [[ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == 1 ]]; do sleep 2; done
sleep 5
check "after a reboot the app is still home" "$([[ "$(home)" == "$pkg/"* ]] && echo true || echo false)" "$(home)"
check "after a reboot $hidden_pkg is still hidden" "$(is_hidden)" "not hidden"
t=$(top)
check "after a reboot the app is in front" "$([[ "$t" == *"$pkg/.MainActivity"* ]] && echo true || echo false)" "$t"

echo "$failures failure(s)"
[[ $failures == 0 ]]
