#!/usr/bin/env bash
# The showcase's remote screens against the same source compiled into the
# app: builds `examples/measure.rs` twice (remote, and `--features inline`)
# under the profile a native app ships with, runs each ROUNDS times, and
# prints the medians side by side with the bundle's compressed sizes.
#
# The profile: cargo's default release (opt 3, no LTO, 16 codegen units),
# not this workspace's (opt "z" + fat LTO) — apps don't inherit ours, and
# the workspace profile hides cross-crate inlining. Each mode has its own
# target dir so switching never rebuilds the other.
#
#   crates/streaming/showcase/measure.sh [ROUNDS]   (default 5)
set -euo pipefail
cd "$(dirname "$0")/../../.."

ROUNDS="${1:-5}"
OUT="${MEASURE_DIR:-target/showcase-measure}"
mkdir -p "$OUT"

export CARGO_PROFILE_RELEASE_OPT_LEVEL=3
export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
export CARGO_PROFILE_RELEASE_PANIC=unwind

build() { # mode, extra cargo args…
  local mode="$1"; shift
  cargo build --release -p remote-showcase --example measure --target-dir "$OUT/$mode-target" "$@" >&2
  # Remove first: copying over an existing binary leaves macOS a stale
  # code-signature cache for that path, and it SIGKILLs the new one.
  rm -f "$OUT/measure-$mode"
  cp "$OUT/$mode-target/release/examples/measure" "$OUT/measure-$mode"
}
build remote
build inline --features inline

# Alternate the modes so drift (thermal, background load) hits both.
: >"$OUT/remote.tsv"; : >"$OUT/inline.tsv"
for i in $(seq "$ROUNDS"); do
  for mode in remote inline; do
    "$OUT/measure-$mode" | awk -F'\t' '$1 == "MEASURE" { print $2 "\t" $3 }' >>"$OUT/$mode.tsv"
  done
done

# The bundle as it would ship: compressed.
wasm="$(ls -t "$OUT"/remote-target/release/build/remote-showcase-*/out/bundle.wasm | head -1)"
raw=$(wc -c <"$wasm")
gz=$(gzip -9 -c "$wasm" | wc -c)
br=$( (command -v brotli >/dev/null && brotli -q 11 -c "$wasm" | wc -c) || echo "")

python3 - "$OUT/remote.tsv" "$OUT/inline.tsv" "$raw" "$gz" "$br" <<'EOF'
import sys, statistics
def load(p):
    d = {}
    for line in open(p):
        k, v = line.rstrip("\n").split("\t")
        d.setdefault(k, []).append(int(v))
    return {k: statistics.median(v) for k, v in d.items()}
remote, inline = load(sys.argv[1]), load(sys.argv[2])
raw, gz, br = sys.argv[3], sys.argv[4], sys.argv[5]
def t(ns):
    us = ns / 1000
    return f"{us/1000:.2f} ms" if us >= 1000 else f"{us:.1f} µs"
def b(n): return f"{n/1024:.1f} KB"
print(f"\nbundle: {b(int(raw))} raw, {b(int(gz))} gzip -9" + (f", {b(int(br))} brotli -q 11" if br else ""))
print(f"\n{'':44}{'remote':>12}{'inline':>12}{'ratio':>8}")
for k in remote:
    if k.startswith("mem.") or k.startswith("bundle."):
        continue
    r = remote[k]; n = inline.get(k)
    print(f"{k:44}{t(r):>12}{(t(n) if n else '—'):>12}{(f'{r/n:.1f}x' if n else ''):>8}")
print()
for k in remote:
    if k.startswith("mem."):
        n = inline.get(k)
        print(f"{k:44}{b(remote[k]):>12}{(b(n) if n is not None else '—'):>12}")
EOF
