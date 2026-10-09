#!/usr/bin/env bash
#
# Fetch the glTF models `canvas3d-demo` embeds, from Khronos'
# glTF-Sample-Assets (licenses and attribution in
# crates/sdk/client/canvas3d/examples/canvas3d-demo/assets/README.md):
#
# - WaterBottle.glb — a static PBR model (CC0 1.0);
# - Fox.glb — a skinned model with three animation clips (mesh CC0 1.0; rig,
#   animation and glTF conversion CC BY 4.0).
#
# Why these aren't committed: cargo materializes a FULL worktree of this repo
# per pinned git rev under ~/.cargo/git/checkouts, so a tracked 9 MB blob costs
# 9 MB in every release a consumer ever pinned. Khronos publishes the exact
# bytes, so we fetch (pinned + checksummed) instead of vendoring — the same
# policy as scripts/fetch-denoise-model.sh. Not a build.rs download: that would
# put the network in the compile path.

set -euo pipefail

REV="5bad5aaa0bbb5d0f9cdc934e626f27d0df1e79b8" # glTF-Sample-Assets main, 2026-10
BASE="https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/${REV}/Models"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
assets="${repo_root}/crates/sdk/client/canvas3d/examples/canvas3d-demo/assets"

verify() { [ -f "$1" ] && [ "$(shasum -a 256 "$1" | cut -d' ' -f1)" = "$2" ]; }

# fetch <file> <path under Models/> <sha256> <human size>
fetch() {
    local name="$1" path="$2" sha="$3" size="$4"
    local dest="${assets}/${name}"
    if verify "$dest" "$sha"; then
        echo "${name} already present and verified"
        return
    fi
    mkdir -p "$assets"
    echo "fetching ${name} (${size}) from glTF-Sample-Assets@${REV:0:7}…"
    # Temp path first so an interrupted transfer can't leave a truncated file
    # that `include_bytes!` would happily compile in.
    local tmp="${dest}.partial"
    trap 'rm -f "$tmp"' EXIT
    curl --fail --location --progress-bar --output "$tmp" "${BASE}/${path}"
    if ! verify "$tmp" "$sha"; then
        echo "error: ${name} checksum mismatch — expected ${sha}, got $(shasum -a 256 "$tmp" | cut -d' ' -f1)" >&2
        exit 1
    fi
    mv "$tmp" "$dest"
    trap - EXIT
    echo "wrote ${dest#"$repo_root"/}"
}

fetch WaterBottle.glb WaterBottle/glTF-Binary/WaterBottle.glb \
    074341e0c9b991244ea88fed77221754dfa33b53ee13523fa5f3c10a28a30160 "8.9 MB"
fetch Fox.glb Fox/glTF-Binary/Fox.glb \
    d97044e701822bac5a62696459b27d7b375aada5de8574ed4362edbba94771f7 "160 KB"
