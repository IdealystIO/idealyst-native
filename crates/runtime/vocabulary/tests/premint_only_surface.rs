//! Every path that reaches the runtime style engine must be gated behind
//! `--cfg idealyst_premint_only`.
//!
//! `--premint-only` compiles the style engine out: sheet registration, the
//! token cohort, and `StyleRules` → CSS all disappear (~73 KB raw / ~22 KB
//! brotli on a web build). That only holds while EVERY call site reaching
//! them stays gated — one ungated site re-anchors the whole engine and the
//! flag silently buys nothing, exactly as one ungated `AllBuiltins` re-anchors
//! the whole primitive vocabulary (see `boot_seam_surface.rs`).
//!
//! Whether a symbol actually leaves the binary is a link-level property no
//! unit test can observe, so this guards the reachable proxy: the source of
//! the two modules that touch the engine. A new engine call site added
//! without a gate fails here rather than quietly costing every
//! `--premint-only` app its entire size win.
//!
//! Measured on a `view`+`text` app with a constant `stylesheet!`:
//! 364,384 → 290,932 bytes of wasm, 128,761 → 106,489 brotli, with the
//! preminted class (`iy-…`) still stamped and every declared property
//! applied.

use std::path::{Path, PathBuf};

/// Modules that reach the style engine.
const ENGINE_FILES: &[&str] = &[
    "crates/runtime/vocabulary/src/style_attach.rs",
    "crates/runtime/vocabulary/src/handlers/repeat.rs",
];

/// Calls that only exist to drive the live engine.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repo root")
        .to_path_buf()
}

/// The exact gated forms every engine entry point must keep.
///
/// Deliberately exact-match rather than a structural scan. A line-based
/// "is there a gate above this call" heuristic was tried first and gave a
/// FALSE NEGATIVE: with the `StyleProp::Sheet` gate deleted, the walk found
/// the neighbouring `StyleProp::Dynamic` arm's gate two lines up and passed.
/// A guard that can silently miss the regression it exists to catch is worse
/// than none, so this pins the literal text instead.
///
/// Brittle to reformatting BY DESIGN: if you touch one of these sites the
/// test fails, and re-verifying is a two-minute build (below). That is the
/// intended workflow — whether a symbol actually leaves the binary is a
/// link-level property no unit test can observe.
///
///     idealyst build --web --release --primitives core --premint-only
///     strings <wrapper>/target/wasm32-unknown-unknown/release/<app>.wasm \
///       | grep -c ensure_sheet_registered      # must be 0
const REQUIRED_GATES: &[(&str, &str)] = &[
    (
        "crates/runtime/vocabulary/src/style_attach.rs",
        "#[cfg(not(idealyst_premint_only))]\n        StyleProp::Dynamic(f) => attach_rules_dynamic(backend, node, f),",
    ),
    (
        "crates/runtime/vocabulary/src/style_attach.rs",
        "#[cfg(not(idealyst_premint_only))]\n        StyleProp::Sheet(app) => attach_sheet_static(backend, node, *app),",
    ),
    (
        "crates/runtime/vocabulary/src/style_attach.rs",
        "#[cfg(not(idealyst_premint_only))]\n        StyleProp::SheetDynamic(f) => attach_sheet_dynamic(backend, node, f),",
    ),
    (
        "crates/runtime/vocabulary/src/style_attach.rs",
        "#[cfg(not(idealyst_premint_only))]\n        StyleProp::SignalClass(spec) => {",
    ),
    // The preminted arm's runtime-override fallback, which layers a real
    // sheet application on top of the class and is what kept
    // `attach_sheet_static` — and therefore the whole engine — anchored.
    (
        "crates/runtime/vocabulary/src/style_attach.rs",
        "#[cfg(not(idealyst_premint_only))]\n            if let Some(rules) = overrides {",
    ),
    // `repeat`'s static-sheet batching arm: the one engine reach outside
    // `attach_style`.
    (
        "crates/runtime/vocabulary/src/handlers/repeat.rs",
        "#[cfg(not(idealyst_premint_only))]\n            Some(StyleProp::Sheet(app)) => {",
    ),
];

#[test]
fn every_style_engine_entry_point_stays_gated() {
    let root = repo_root();
    let mut missing: Vec<String> = Vec::new();

    for (rel, snippet) in REQUIRED_GATES {
        let Ok(src) = std::fs::read_to_string(root.join(rel)) else {
            missing.push(format!("{rel}: unreadable"));
            continue;
        };
        if !src.contains(*snippet) {
            missing.push(format!("{rel}: missing gate on `{}`", snippet.lines().last().unwrap_or(snippet).trim()));
        }
    }

    assert!(
        missing.is_empty(),
        "style-engine entry points lost their `--premint-only` gate. A single \
         ungated site re-anchors the WHOLE engine and the flag silently buys \
         nothing (measured: 290,932 bytes of wasm gated vs 364,384 ungated). \
         If you moved or reformatted one of these deliberately, update \
         REQUIRED_GATES and re-verify with a real build — see this file's \
         docs.\n  {}",
        missing.join("\n  "),
    );
}

/// The gate must not swallow the failure. If a style reaches `attach_style`
/// that the engine-less build cannot render, it has to panic with a message
/// that names the cause — a silently unstyled subtree is far harder to
/// diagnose, and the whole flag is a promise the build cannot verify.
#[test]
fn premint_only_violation_is_loud_and_actionable() {
    let src = std::fs::read_to_string(
        repo_root().join("crates/runtime/vocabulary/src/style_attach.rs"),
    )
    .expect("style_attach.rs");

    assert!(
        src.contains("PREMINT_ONLY_VIOLATION"),
        "the engine-less arms must panic through a named diagnostic",
    );
    // The message has to point at the fix, including the sharp edge that
    // cost real debugging time: passing the raw sheet (`card_style()`)
    // instead of the builder (`Card()`) silently skips preminting.
    for needle in ["--premint-only", "style = Card()", "card_style()"] {
        assert!(
            src.contains(needle),
            "the violation message must mention {needle:?} — it is the \
             difference between a two-minute fix and a debugging session",
        );
    }
}
