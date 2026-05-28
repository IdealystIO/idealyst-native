#!/usr/bin/env bash
#
# Reproducible build for the wasm dynamic-linking spike (non-bindgen path).
#
# Produces a PIC *main* module and a PIC `--shared` *side* module that
# share ONE std artifact, ONE heap, and ONE reactive arena. `node
# test.mjs` links the side against the main via loader.mjs and proves:
#   - 234 side imports resolve (0 unresolved),
#   - a `#[no_mangle]` static is shared across both modules,
#   - real runtime-core `signal!` reactive code runs *inside* the side
#     on main's arena.
#
# This is the recipe the `idealyst build web --dynamic-split` pipeline
# will drive (see project_web_dynamic_linking memory). Captured here so
# the proven artifacts are regenerable rather than hand-typed.
#
# ── Why each flag matters ────────────────────────────────────────────
#   * nightly + `-Z build-std=std,panic_abort`
#       precompiled std isn't PIC; we must rebuild it with PIC codegen.
#   * pinned nightly-2025-09-01
#       the default `nightly` here is ancient 1.80 and can't parse the
#       repo's edition2024 crates. This nightly + `rust-src` works.
#   * RUSTFLAGS `-C relocation-model=pic -C link-arg=--experimental-pic`
#       identical on BOTH crates so std builds ONCE and the symbol
#       hashes match — the loader resolves the GOT by name, so
#       `reactive::ARENA…hABC` in the side must equal main's.
#   * differing link args go on the FINAL crate only (via `cargo rustc
#       -- <args>`) so std/deps still build once and are shared:
#         main → --export-all --growable-table --export-table
#         side → --shared
#
set -euo pipefail
cd "$(dirname "$0")"

TOOLCHAIN="${DYNLINK_TOOLCHAIN:-nightly-2025-09-01}"
TARGET="wasm32-unknown-unknown"
PROFILE_DIR="release"

export RUSTFLAGS="-C relocation-model=pic -C link-arg=--experimental-pic"

echo "[dynlink] toolchain=$TOOLCHAIN target=$TARGET"
echo "[dynlink] RUSTFLAGS=$RUSTFLAGS"

# main: self-contained PIC module. Exports every symbol (so the side can
# resolve its GOT against main) + a growable, exported indirect-function
# table (so the loader can append the side's GOT.func slots).
echo "[dynlink] building main (PIC, --export-all --growable-table --export-table)"
cargo "+$TOOLCHAIN" rustc \
  -Z build-std=std,panic_abort \
  --target "$TARGET" --release -p dynlink-main \
  -- \
  -C link-arg=--export-all \
  -C link-arg=--growable-table \
  -C link-arg=--export-table

# side: PIC `--shared` module. Imports memory/table/GOT from the host
# (main) instead of defining its own; carries its OWN data segments and
# applies them at __memory_base on load (this is what moves heavy data
# out of the initial download — unlike the reloc splitters).
echo "[dynlink] building side (PIC, --shared)"
cargo "+$TOOLCHAIN" rustc \
  -Z build-std=std,panic_abort \
  --target "$TARGET" --release -p dynlink-side \
  -- \
  -C link-arg=--shared

OUT="target/$TARGET/$PROFILE_DIR"
echo "[dynlink] artifacts:"
ls -la "$OUT/dynlink_main.wasm" "$OUT/dynlink_side.wasm"

echo "[dynlink] linking + running proof (node test.mjs):"
node test.mjs
