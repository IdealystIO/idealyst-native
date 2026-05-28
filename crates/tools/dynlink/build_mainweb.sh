#!/usr/bin/env bash
# Build the REAL-WebBackend main as a PIC dynamic-link host + bindgen it.
# Same shared-std PIC recipe as build.sh/build_mainbg.sh.
set -euo pipefail
cd "$(dirname "$0")"

TOOLCHAIN="${DYNLINK_TOOLCHAIN:-nightly-2025-09-01}"
TARGET="wasm32-unknown-unknown"
export RUSTFLAGS="-C relocation-model=pic -C link-arg=--experimental-pic"

echo "[mainweb] building PIC main (--export-all --growable-table --export-table)"
cargo "+$TOOLCHAIN" rustc \
  -Z build-std=std,panic_abort \
  --target "$TARGET" --release -p dynlink-mainweb \
  -- \
  -C link-arg=--export-all \
  -C link-arg=--growable-table \
  -C link-arg=--export-table

RAW="target/$TARGET/release/dynlink_mainweb.wasm"
echo "[mainweb] raw PIC main: $(ls -la "$RAW")"

echo "[mainweb] wasm-bindgen --target web --keep-lld-exports --keep-debug --no-demangle"
rm -rf mainweb_out
wasm-bindgen --target web \
  --keep-lld-exports --keep-debug --no-demangle \
  --out-name dynlink_mainweb --out-dir mainweb_out \
  "$RAW"

# Expose get_imports/finalize so the harness can inject no-op describe stubs
# (--export-all keeps the describe machinery alive; never called at runtime).
perl -0pi -e 's/export \{ initSync, __wbg_init as default \};/export { initSync, __wbg_init as default, __wbg_get_imports, __wbg_finalize_init };/' mainweb_out/dynlink_mainweb.js

echo "[mainweb] bindgen output:"
ls -la mainweb_out
