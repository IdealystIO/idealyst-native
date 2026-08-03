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

// ---------------------------------------------------------------------------
// The reactive preminted path
// ---------------------------------------------------------------------------

/// `StyleProp::PremintedDynamic` must NOT be gated, and must not reach the
/// engine.
///
/// It is the reactive peer of `Preminted`: a class-list closure re-stamped
/// by a per-node effect. Every arm of a discrete axis already has CSS in the
/// shipped asset, so flipping one is a `classList` swap — no sheet
/// registration, no `StyleRules` → CSS, nothing the engine owns. That is the
/// whole point of the variant: before it existed, a `stylesheet!` builder
/// with ONE reactive axis fell through to `SheetDynamic` and re-anchored the
/// engine. Measured on the component catalog, 46 of 68 fall-throughs were a
/// single nav-item sheet whose only reactivity was `active`.
///
/// So if this arm ever grows a `#[cfg(not(idealyst_premint_only))]`, or
/// starts calling an engine entry point, the flag stops being reachable for
/// any app with a selection UI — which is most of them.
#[test]
fn preminted_dynamic_arm_is_ungated_and_engine_free() {
    let src = std::fs::read_to_string(
        repo_root().join("crates/runtime/vocabulary/src/style_attach.rs"),
    )
    .expect("read style_attach.rs");

    let arm_at = src
        .find("StyleProp::PremintedDynamic { class_of, overrides } => {")
        .expect("PremintedDynamic arm present");

    // Nothing gates the arm ITSELF. Walk back over the doc comments
    // attached to the arm and require that the first real line above it is
    // not a cfg attribute. A fixed-size window was tried first and gave the
    // exact false positive this file's header warns about: it reached past
    // the arm into the PRECEDING arm's `#[cfg(idealyst_premint_only)]` and
    // failed on a correctly-ungated arm.
    let lines: Vec<&str> = src[..arm_at].lines().collect();
    let guard = lines
        .iter()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with("//"))
        .unwrap_or("");
    assert!(
        !guard.starts_with("#[cfg("),
        "the PremintedDynamic arm must stay ungated — gating it puts every \
         reactive-axis style back on the live engine; found `{guard}` \
         directly above it"
    );

    // The arm body (to the next top-level arm) must not call the engine.
    let body_end = src[arm_at..]
        .find("StyleProp::Preminted { class, overrides, inline } => {")
        .expect("Preminted arm follows");
    let body = &src[arm_at..arm_at + body_end];
    for engine_call in
        ["ensure_sheet_registered", "attach_sheet_dynamic", "apply_style", "mint_class_for_app"]
    {
        assert!(
            !body.contains(engine_call),
            "PremintedDynamic reaches the engine via `{engine_call}` — the \
             class swap must need nothing but attach/detach_html_class:\n{body}"
        );
    }

    // The swap itself lives in the shared helper (see
    // `both_dynamic_preminted_paths_share_one_class_swap`); the arm must
    // delegate to it rather than growing a second copy.
    // (The helper takes per-evaluation `(class list, inline layer)` parts
    // since the inline-drop fix; this arm has no inline slot, so it
    // passes `None`.)
    assert!(
        body.contains(
            "attach_preminted_dynamic(backend, node, Box::new(move || (class_of(), None)))"
        ),
        "the arm must delegate to the shared class-swap helper:\n{body}"
    );
}

/// The reactive class swap must exist exactly ONCE, and both dynamic
/// preminted paths must go through it.
///
/// Two paths need it: `PremintedDynamic` (the macro's reactive preminted
/// form) and, under `--premint-only`, the `SheetDynamic` arm — which is
/// where every reactive application over one of idea-theme's
/// runtime-assembled component sheets lands, because the blanket
/// `Fn() -> StyleApplication` impl has no expansion site to premint at.
/// A second copy of the swap is how the two drift into stamping different
/// class lists for the same sheet.
#[test]
fn both_dynamic_preminted_paths_share_one_class_swap() {
    let src = std::fs::read_to_string(
        repo_root().join("crates/runtime/vocabulary/src/style_attach.rs"),
    )
    .expect("read style_attach.rs");

    assert_eq!(
        src.matches("fn attach_preminted_dynamic").count(),
        1,
        "the class swap must have exactly one definition"
    );

    let helper_at = src.find("fn attach_preminted_dynamic").expect("helper present");
    let helper_end = src[helper_at..].find("\n}\n").expect("helper body ends") + helper_at;
    let helper = &src[helper_at..helper_end];

    // The class swap needs BOTH halves; add-only would accumulate every
    // value a node ever wore (`-active-on` never coming off).
    assert!(
        helper.contains("detach_html_class") && helper.contains("attach_html_class"),
        "the swap must detach the outgoing class as well as attach the \
         incoming one:\n{helper}"
    );
    // And nothing else — the whole point is that it needs no engine.
    for engine_call in ["ensure_sheet_registered", "apply_style", "mint_class_for_app"] {
        assert!(
            !helper.contains(engine_call),
            "the class swap reaches the engine via `{engine_call}`:\n{helper}"
        );
    }

    // `SheetDynamic` under --premint-only must route here, NOT panic
    // outright. The blanket panic it replaced took down every reactive
    // idea-theme component style, which is most of a real app.
    assert!(
        src.contains(
            "StyleProp::SheetDynamic(f) => attach_sheet_dynamic_preminted(backend, node, f)"
        ),
        "SheetDynamic must premint under --premint-only rather than panic \
         unconditionally"
    );
    let preminted_at =
        src.find("fn attach_sheet_dynamic_preminted").expect("premint-only dynamic path");
    let preminted_end =
        src[preminted_at..].find("\n}\n").expect("body ends") + preminted_at;
    let preminted = &src[preminted_at..preminted_end];
    // Decided PER EVALUATION. A probe at attach time would be unsound: a
    // closure may legally return a premintable application on one run and
    // an override-carrying one on the next.
    assert!(
        preminted.contains("preminted_class_list()")
            && preminted.contains("PREMINT_ONLY_VIOLATION"),
        "each evaluation must re-derive the class list and panic loudly when \
         it cannot:\n{preminted}"
    );
}

/// Both preminted branches must assemble the class list from the SAME
/// emission, so a static and a reactive call site on one sheet can never
/// disagree about the class it wears — which would silently render one of
/// them against CSS meant for the other.
#[test]
fn macro_premint_branches_share_one_class_assembly() {
    let src =
        std::fs::read_to_string(repo_root().join("crates/runtime/macros/src/stylesheet.rs"))
            .expect("read stylesheet.rs");
    assert_eq!(
        src.matches("#(#premint_axis_pushes)*").count(),
        2,
        "expected exactly two uses of the axis-push emission (the constant \
         branch and the reactive closure); a hand-rolled second copy is how \
         the two paths drift apart"
    );
    assert!(
        src.contains("StyleProp::PremintedDynamic {"),
        "the macro must emit the reactive preminted shape"
    );
}

// ---------------------------------------------------------------------------
// Rule closures are stripped from --premint-only bundles
// ---------------------------------------------------------------------------

/// Under `--premint-only` a `StyleSheet` must carry NO author rule
/// closures — that is what drops the per-arm `StyleRules` bodies from the
/// wasm (86,630 bytes measured on login-demo, most of it idea-theme's
/// eleven component sheets: Button alone declares 28 tone×variant arms).
///
/// The gate lives in `StyleSheet::new` / `::variant` rather than in each
/// of the eleven builders, so it covers macro sheets and app sheets too.
/// Both must DROP the incoming closure — merely not calling it is not
/// enough, since a stored `Box<dyn Fn>` keeps the body reachable through
/// its vtable and LLVM emits it.
///
/// And the replacement must PANIC, not return empty rules: a handful of
/// call sites read a resolved `StyleRules` back in Rust (`Icon` tints its
/// SVG from the resolved `color`), and empty rules would tint those
/// silently wrong. Resolving a sheet at runtime IS the engine, so an app
/// doing it under this flag was already outside the contract — it should
/// hear about it.
#[test]
fn premint_only_strips_rule_closures_loudly() {
    let src = std::fs::read_to_string(repo_root().join("crates/runtime/shared/src/style.rs"))
        .expect("read style.rs");

    assert!(
        src.contains("fn premint_only_stripped_rules"),
        "the stripped-rules stub is gone; --premint-only bundles are back to \
         shipping every arm's StyleRules body"
    );
    // Loud, not empty.
    let stub_at = src.find("fn premint_only_stripped_rules").unwrap();
    let stub_end = src[stub_at..].find("\n}").unwrap();
    let stub = &src[stub_at..stub_at + stub_end];
    assert!(
        stub.contains("panic!"),
        "the stub must panic — returning StyleRules::default() would tint \
         Icon/Tabs silently wrong instead of naming the constraint:\n{stub}"
    );

    // Both closure sinks must drop their argument under the flag.
    for (sink, needle) in [
        ("StyleSheet::new", "            #[cfg(idealyst_premint_only)]\n            base: {\n                drop(f);"),
        ("StyleSheet::variant", "        #[cfg(idealyst_premint_only)]\n        {\n            drop(f);"),
    ] {
        assert!(
            src.contains(needle),
            "{sink} must drop its rules closure under --premint-only; storing \
             it keeps the body reachable and the flag stops paying"
        );
    }
}

// ---------------------------------------------------------------------------
// --premint-report
// ---------------------------------------------------------------------------

/// The report hook must sit AHEAD of the match, and must skip the two
/// preminted shapes.
///
/// Ahead of the match because that is what makes it see every `StyleProp`
/// without an arm-by-arm edit — moved inside an arm it would silently stop
/// reporting the shapes it no longer covers, which is exactly the failure
/// the flag exists to prevent. (The hand-rolled versions of this patch,
/// written four times while building the feature, each missed a shape.)
///
/// Skipping `Preminted` / `PremintedDynamic` because those ARE the goal —
/// reporting them would bury the real fall-throughs. On the component
/// catalog the signal is 12 distinct entries out of 68 attach calls.
#[test]
fn premint_report_hook_precedes_the_match_and_skips_preminted() {
    let src = std::fs::read_to_string(
        repo_root().join("crates/runtime/vocabulary/src/style_attach.rs"),
    )
    .expect("read style_attach.rs");

    let hook = src
        .find("report::note(&style);")
        .expect("the --premint-report hook is gone; the flag reports nothing");
    let match_at = src
        .find("    match style {\n        StyleProp::Static(rules) => {")
        .expect("attach_style's match");
    assert!(
        hook < match_at,
        "the report hook must run BEFORE the match — inside an arm it only \
         sees the shapes that arm covers"
    );

    // Diagnostic only: it must never be compiled into a normal build.
    let gate_line = src[..hook].lines().rev().find(|l| l.trim().starts_with("#[cfg(")).unwrap_or("");
    assert!(
        gate_line.contains("idealyst_premint_report"),
        "the hook must be gated on idealyst_premint_report so normal builds \
         pay nothing; found `{}`",
        gate_line.trim()
    );

    // The two preminted shapes are the goal, not a finding.
    let note_at = src.find("pub(crate) fn note(style: &StyleProp)").expect("note()");
    let note_body = &src[note_at..note_at + 900];
    assert!(
        note_body.contains("StyleProp::Preminted { .. } | StyleProp::PremintedDynamic { .. } => return"),
        "note() must skip both preminted shapes, or the report buries the \
         real fall-throughs in noise:\n{note_body}"
    );
}
