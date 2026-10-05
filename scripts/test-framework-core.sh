#!/usr/bin/env bash
# Run the framework-core tests across the feature matrix.
#
# Framework core is the runtime crates CLAUDE.md names (runtime-world,
# runtime-scene, runtime-vocabulary, runtime-shared, runtime-layout), the
# macros that emit against them, and the remote-component bridge. Each
# invocation here is one row of the coverage matrix. A regression in any
# row is a real bug: feature gating is a production path, and several
# suites ONLY run under a feature (the bridged kernel's behaviour under
# `loopback-engine`, the element codec under `remote-loopback`), so a plain
# `cargo test -p <crate>` never sees them.
#
# Usage:
#   scripts/test-framework-core.sh            # full matrix
#   scripts/test-framework-core.sh fast       # default features only
#   scripts/test-framework-core.sh coverage   # branch coverage report (HTML), $CRATE
#   scripts/test-framework-core.sh mutants    # mutation testing (slow), $CRATE
#   scripts/test-framework-core.sh <package>  # one package, default features
#
# CRATE (coverage / mutants) defaults to runtime-world.

set -euo pipefail

CRATE="${CRATE:-runtime-world}"
mode="${1:-matrix}"

run() {
    echo
    echo "════════════════════════════════════════════════════════════════════"
    echo "  $*"
    echo "════════════════════════════════════════════════════════════════════"
    "$@"
}

case "$mode" in
matrix)
    # The reactive kernel: native engine, the whole suite again through
    # the bridged engine (parity), the hot-reload state carry, and the
    # bridge compiled in beside the native engine (an app hosting remote
    # components).
    run cargo test -p runtime-world
    run cargo test -p runtime-world --features loopback-engine
    run cargo test -p runtime-world --features hot-reload
    run cargo test -p runtime-world --features bridge
    run cargo test -p runtime-scene
    run cargo test -p runtime-shared
    run cargo test -p runtime-layout
    # The vocabulary: default, the remote codec on the app side, and both
    # halves of the codec in one process.
    run cargo test -p runtime-vocabulary
    run cargo test -p runtime-vocabulary --features remote
    run cargo test -p runtime-vocabulary --features remote-loopback
    run cargo test -p runtime-macros
    run cargo test -p runtime-macros --features catalog
    # Remote components over real wasm (builds bundles for
    # wasm32-unknown-unknown; without that target the build scripts embed
    # placeholders and these fail at load).
    run cargo test -p stream-host
    run cargo test -p stream-spike
    run cargo test -p remote-showcase
    run cargo test -p remote-showcase --features inline
    echo
    echo "✓ All matrix rows passed."
    ;;

fast)
    run cargo test -p runtime-world
    run cargo test -p runtime-vocabulary
    ;;

coverage)
    if ! cargo llvm-cov --version >/dev/null 2>&1; then
        echo "cargo-llvm-cov not installed. Install with:"
        echo "    cargo install cargo-llvm-cov"
        exit 1
    fi
    run cargo llvm-cov --html --branch -p "$CRATE"
    echo
    echo "✓ HTML report at target/llvm-cov/html/index.html"
    ;;

coverage-summary)
    if ! cargo llvm-cov --version >/dev/null 2>&1; then
        echo "cargo-llvm-cov not installed. Install with:"
        echo "    cargo install cargo-llvm-cov"
        exit 1
    fi
    run cargo llvm-cov --branch --summary-only -p "$CRATE"
    ;;

mutants)
    if ! cargo mutants --version >/dev/null 2>&1; then
        echo "cargo-mutants not installed. Install with:"
        echo "    cargo install cargo-mutants"
        exit 1
    fi
    # Mutation testing is slow (minutes to hours per crate). Run it
    # nightly, not per-commit. The report at ./mutants.out/ shows
    # which mutants survived — each survivor is a hole in the suite.
    run cargo mutants -p "$CRATE"
    ;;

*)
    # A package name: its tests under default features.
    run cargo test -p "$mode"
    ;;
esac
