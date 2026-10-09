#!/usr/bin/env bash
#
# Fetch the glTF model `canvas3d-demo` embeds: Khronos' "WaterBottle" sample
# (CC0 1.0 — see crates/sdk/client/canvas3d/examples/canvas3d-demo/assets/README.md).
#
# Why this isn't committed: cargo materializes a FULL worktree of this repo
# per pinned git rev under ~/.cargo/git/checkouts, so a tracked 9 MB blob costs
# 9 MB in every release a consumer ever pinned. Khronos publishes the exact
# bytes, so we fetch (pinned + checksummed) instead of vendoring — the same
# policy as scripts/fetch-denoise-model.sh. Not a build.rs download: that would
# put the network in the compile path.

set -euo pipefail

REV="5bad5aaa0bbb5d0f9cdc934e626f27d0df1e79b8" # glTF-Sample-Assets main, 2026-10
SHA256="074341e0c9b991244ea88fed77221754dfa33b53ee13523fa5f3c10a28a30160"
URL="https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/${REV}/Models/WaterBottle/glTF-Binary/WaterBottle.glb"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${repo_root}/crates/sdk/client/canvas3d/examples/canvas3d-demo/assets/WaterBottle.glb"

verify() { [ -f "$1" ] && [ "$(shasum -a 256 "$1" | cut -d' ' -f1)" = "$SHA256" ]; }

if verify "$dest"; then
    echo "canvas3d demo model already present and verified: ${dest#"$repo_root"/}"
    exit 0
fi

mkdir -p "$(dirname "$dest")"
echo "fetching WaterBottle.glb (8.9 MB) from glTF-Sample-Assets@${REV:0:7}…"
# Temp path first so an interrupted transfer can't leave a truncated file that
# `include_bytes!` would happily compile in.
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
