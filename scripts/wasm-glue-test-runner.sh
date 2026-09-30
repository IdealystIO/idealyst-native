#!/bin/sh
# cargo's test runner for wasm32-unknown-unknown in this workspace
# (`.cargo/config.toml`). Builds and runs `wasm-glue-test-runner`
# (crates/tools/wasm-split/wasm-carve/src/bin/), which gives a test binary
# that links web-glue its `__idealyst_glue.js` and then hands it to
# `wasm-bindgen-test-runner`. Test binaries without web-glue pass straight
# through.
#
# The wrapper is built into its OWN target dir: the outer `cargo test`
# holds the lock on the caller's target dir while it runs tests.
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
tdir="$root/target/wasm-glue-test-runner"
env -u CARGO_BUILD_TARGET -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
    cargo build -q --release --manifest-path "$root/Cargo.toml" \
    -p wasm-carve --bin wasm-glue-test-runner --target-dir "$tdir"
exec "$tdir/release/wasm-glue-test-runner" "$@"
