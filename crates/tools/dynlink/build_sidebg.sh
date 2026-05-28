#!/usr/bin/env bash
# A/B repro for the bindgen-side fmt bug: build the fmt-having non-bindgen
# `main` (--export-all) + a BINDGEN `side-bg` (--shared, then wasm-bindgen),
# sharing one std. The bg loader links side-bg against main and we test
# whether `format!` works in the bindgen side (vs the non-bindgen `side`,
# which works).
set -euo pipefail
cd "$(dirname "$0")"
TOOLCHAIN="${DYNLINK_TOOLCHAIN:-nightly-2025-09-01}"
TARGET="wasm32-unknown-unknown"
# DEFAULT target features (reference-types + multivalue both ON, the nightly
# default) — the realistic config. The PATCHED wasm-bindgen (wbg-fork) handles
# the PIC function table via the extract_xform guard.
export RUSTFLAGS="-C relocation-model=pic -C link-arg=--experimental-pic"
# Patched wasm-bindgen (PIC/dylink-aware extract_xform guard).
WB="$PWD/wbg-fork/cli/target/release/wasm-bindgen"

echo "[side-bg] building fmt-having main (--export-all)"
cargo "+$TOOLCHAIN" rustc -Z build-std=std,panic_abort --target "$TARGET" --release -p dynlink-main \
  -- -C link-arg=--export-all -C link-arg=--growable-table -C link-arg=--export-table

echo "[side-bg] building BINDGEN side (--shared)"
cargo "+$TOOLCHAIN" rustc -Z build-std=std,panic_abort --target "$TARGET" --release -p dynlink-side-bg \
  -- -C link-arg=--shared

echo "[side-bg] wasm-bindgen the side"
rm -rf sidebg_out
# NOTE: no --keep-lld-exports. It made wasm-bindgen try to build adapters for
# every kept internal LLD export, which panics with heavy web-sys
# ("stack size mismatch for adapter N"). We don't need it: GOT.func resolves
# via the side's elem segment + main's exports.
"$WB" --target web --keep-debug --no-demangle \
  --out-name dynlink_side_bg --out-dir sidebg_out \
  "target/$TARGET/release/dynlink_side_bg.wasm"

# expose get_imports/finalize so the loader can merge the side's bindgen glue
perl -0pi -e 's/export \{ initSync, __wbg_init as default \};/export { initSync, __wbg_init as default, __wbg_get_imports, __wbg_finalize_init };/' sidebg_out/dynlink_side_bg.js

echo "[side-bg] artifacts:"
ls -la "target/$TARGET/release/dynlink_main.wasm" sidebg_out/dynlink_side_bg_bg.wasm
