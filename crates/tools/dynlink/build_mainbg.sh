#!/usr/bin/env bash
#
# Build the BINDGEN main (the realistic web host) as a PIC dynamic-link
# host, then run wasm-bindgen on it (same flags build-web uses). Proves a
# wasm-bindgen-processed PIC main keeps the exports a side links against.
#
# Reuses the shared-std PIC recipe from build.sh so mainbg's std artifact
# matches the side's (GOT hashes line up).
set -euo pipefail
cd "$(dirname "$0")"

TOOLCHAIN="${DYNLINK_TOOLCHAIN:-nightly-2025-09-01}"
TARGET="wasm32-unknown-unknown"
export RUSTFLAGS="-C relocation-model=pic -C link-arg=--experimental-pic"

echo "[mainbg] building PIC main (--export-all --growable-table --export-table)"
cargo "+$TOOLCHAIN" rustc \
  -Z build-std=std,panic_abort \
  --target "$TARGET" --release -p dynlink-mainbg \
  -- \
  -C link-arg=--export-all \
  -C link-arg=--growable-table \
  -C link-arg=--export-table

RAW="target/$TARGET/release/dynlink_mainbg.wasm"
echo "[mainbg] raw PIC main: $(ls -la "$RAW")"

echo "[mainbg] wasm-bindgen --target web --keep-lld-exports --keep-debug --no-demangle"
rm -rf mainbg_out
wasm-bindgen --target web \
  --keep-lld-exports --keep-debug --no-demangle \
  --out-name dynlink_mainbg --out-dir mainbg_out \
  "$RAW"

echo "[mainbg] bindgen output:"
ls -la mainbg_out
