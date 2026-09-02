#!/usr/bin/env bash
#
# Fetch the DeepFilterNet 3 ONNX weights that `denoise-demo` embeds on web.
#
# Why this isn't committed: cargo materializes a FULL worktree of this repo
# per pinned git rev under ~/.cargo/git/checkouts. A 7.8 MB tracked blob is
# therefore 7.8 MB multiplied by every release a consumer has ever pinned —
# 28 checkouts on one dev machine at last count. Upstream already publishes
# the exact bytes, so we fetch instead of vendoring.
#
# Why the demo needs a standalone copy at all, when `deep_filter` can embed
# the model itself: on wasm32 the dep is declared WITHOUT `default-model`
# precisely so the weights stay out of the main bundle. The demo re-adds them
# through its own `include_bytes!` behind a `wasm_split::lazy_loader!`
# boundary, which is what lets the splitter hoist the data segment into a
# separate chunk_N_split.wasm that downloads on first use. Pointing at the
# dep's embedded const instead would put all 7.8 MB back in main.wasm.
# See crates/sdk/client/denoise/examples/denoise-demo/src/lib.rs::web_model.
#
# REV must stay in lockstep with the `deep_filter` pin in
# crates/sdk/client/denoise/Cargo.toml — the model format and the tract
# codegen that reads it are versioned together.

set -euo pipefail

REV="978576aa8400552a4ce9730838c635aa30db5e61" # v0.5.6
SHA256="c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616"
URL="https://raw.githubusercontent.com/Rikorose/DeepFilterNet/${REV}/models/DeepFilterNet3_onnx.tar.gz"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${repo_root}/crates/sdk/client/denoise/examples/denoise-demo/assets/DeepFilterNet3_onnx.tar.gz"

verify() { [ -f "$1" ] && [ "$(shasum -a 256 "$1" | cut -d' ' -f1)" = "$SHA256" ]; }

if verify "$dest"; then
    echo "denoise model already present and verified: ${dest#"$repo_root"/}"
    exit 0
fi

mkdir -p "$(dirname "$dest")"
echo "fetching DeepFilterNet3 weights (7.8 MB) from DeepFilterNet@${REV:0:7}…"
# Download to a temp path first so an interrupted transfer can't leave a
# truncated file that later `include_bytes!` happily compiles into a broken
# model chunk.
tmp="${dest}.partial"
trap 'rm -f "$tmp"' EXIT
curl --fail --location --progress-bar --output "$tmp" "$URL"

if ! verify "$tmp"; then
    echo "error: checksum mismatch — expected $SHA256, got $(shasum -a 256 "$tmp" | cut -d' ' -f1)" >&2
    exit 1
fi

mv "$tmp" "$dest"
trap - EXIT
echo "wrote ${dest#"$repo_root"/}"
