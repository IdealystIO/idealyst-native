#!/usr/bin/env bash
#
# test-counter.sh — assemble a Phase 2 counter test package for
# hello-roku without depending on the (still-being-designed)
# framework-side reactive API. Workflow:
#
#   1. Run the normal Roku build so `methods.brs` picks up the
#      `count_value` and `increment` `#[method]` fns in
#      hello-roku/src/lib.rs (plus the dispatch_method helper that
#      build-roku emits alongside them).
#   2. Overwrite the auto-generated `data/ui.json` with the
#      hand-authored counter fixture in `counter-test.json`. The
#      fixture exercises CreateSignal / BindText / BindButton —
#      the new Phase 2 wire commands.
#   3. Re-zip the package directory into `dist/roku.zip` so it can
#      be uploaded straight to the emulator's dev web UI.
#
# Once the framework-side reactive API lands (Phase 2b), the
# author will write the same UI in Rust and `idealyst build --roku`
# will produce the right ui.json on its own — this script becomes
# unnecessary.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WS="$(cd "$HERE/../.." && pwd)"

cd "$WS"

echo "[test-counter] building hello-roku via the normal pipeline…"
cargo build -p idealyst-cli --quiet
"$WS/target/debug/idealyst" build --roku "$HERE"

echo "[test-counter] overriding ui.json with counter fixture…"
cp "$HERE/counter-test.json" "$HERE/dist/roku/data/ui.json"

echo "[test-counter] re-zipping package…"
rm -f "$HERE/dist/roku.zip"
( cd "$HERE/dist/roku" && zip -qr "$HERE/dist/roku.zip" . -x '*.DS_Store' )

echo
echo "[test-counter] done"
echo "  zip:           $HERE/dist/roku.zip"
echo "  upload at:     http://<roku-ip>/  (or run \`idealyst run --roku\`)"
echo
echo "  expected behavior: a large yellow '0' over a '+1' button;"
echo "  pressing +1 increments the digit on screen."
