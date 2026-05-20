#!/usr/bin/env bash
# Run framework-core tests across the feature matrix.
#
# Each invocation here is one row in the coverage matrix. A regression
# in any of them is a real bug — feature gating is a real production
# path, not a development convenience.
#
# Usage:
#   scripts/test-framework-core.sh           # full matrix
#   scripts/test-framework-core.sh fast      # default features only
#   scripts/test-framework-core.sh coverage  # branch coverage report (HTML)
#   scripts/test-framework-core.sh mutants   # mutation testing (slow)
#   scripts/test-framework-core.sh <suite>   # one suite, default features
#                                            #  (e.g. reactive, walker, style)

set -euo pipefail

CRATE="framework-core"
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
    # Full feature matrix. Each row is "what production paths flip on
    # with this feature combination" — the test suite must pass under
    # all of them, otherwise a feature-gated regression is loose.
    run cargo test -p "$CRATE"
    run cargo test -p "$CRATE" --features async-driver
    run cargo test -p "$CRATE" --features robot
    run cargo test -p "$CRATE" --features debug-stats
    run cargo test -p "$CRATE" --features hot-reload
    run cargo test -p "$CRATE" --all-features
    echo
    echo "✓ All matrix rows passed."
    ;;

fast)
    run cargo test -p "$CRATE"
    ;;

coverage)
    if ! cargo llvm-cov --version >/dev/null 2>&1; then
        echo "cargo-llvm-cov not installed. Install with:"
        echo "    cargo install cargo-llvm-cov"
        exit 1
    fi
    run cargo llvm-cov --html --branch -p "$CRATE" --all-features
    echo
    echo "✓ HTML report at target/llvm-cov/html/index.html"
    ;;

coverage-summary)
    if ! cargo llvm-cov --version >/dev/null 2>&1; then
        echo "cargo-llvm-cov not installed. Install with:"
        echo "    cargo install cargo-llvm-cov"
        exit 1
    fi
    run cargo llvm-cov --branch --summary-only -p "$CRATE" --all-features
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
    run cargo mutants -p "$CRATE" --all-features
    ;;

*)
    # Assume it's a suite name; run just that suite under default features.
    run cargo test -p "$CRATE" --test "$mode"
    ;;
esac
