//! Premint style-dump assembly — the CSS-emission side of build-time
//! styles (see `runtime_core::premint` for the collection side and
//! `StyleSource::Preminted` for the runtime side).
//!
//! # The delta model
//!
//! [`dump_all_css`] emits, per registered sheet, one rule per LAYER —
//! base, each breakpoint/container/state overlay, each variant arm —
//! rather than one fully-merged rule per variant COMBINATION. The
//! runtime stamps one class per selected axis
//! (`iy-<hash> iy-<hash>-<axis>-<value> …`), and the CSS cascade
//! performs the merge `StyleSheet::resolve` does live. Output size is
//! therefore the SUM of a sheet's arms, not their cartesian product
//! (the per-combo model produced 3 MB on the website; this produces
//! the arms once).
//!
//! # Why source order + flat specificity reproduces `resolve`
//!
//! Every emitted selector has specificity (0,1,0): base and axis rules
//! are single classes, state pseudo-classes are wrapped in `:where()`
//! (which contributes nothing), and `@media`/`@container` preludes
//! never affect specificity. Equal-specificity rules cascade by source
//! order, per property — exactly `StyleRules::merge`'s later-wins.
//! The emission order mirrors the resolver's merge order, which is
//! `BTreeMap`-alphabetical over axis names: the `__bp_*` < `__cq_*` <
//! `__state_*` prefixes sort before every lowercase author axis, so
//! the order is base → breakpoints → containers → states → author
//! axes. This also reproduces the live WEB backend's cross-rule
//! outcomes (variant beats state, state beats breakpoint — the
//! specificity quirks of the per-combo model resolve pairwise to the
//! same winners; verified by the A/B computed-style harness).
//!
//! Compound variants (runtime-API-only; the `stylesheet!` grammar
//! cannot express them) are rejected defensively — a registered sheet
//! is always macro-generated, so this cannot fire.

use runtime_core::premint::{PremintSheet, PREMINT_SHEETS};
use runtime_core::StyleRules;

/// Assemble the full preminted stylesheet for every registered sheet.
/// Deterministic given the same binary — the CLI fingerprints the
/// output like any other asset.
pub fn dump_all_css() -> String {
    let mut rules = String::new();
    let mut fonts = FontCollector::default();
    for sheet in PREMINT_SHEETS {
        dump_sheet(sheet, &mut rules, &mut fonts);
    }
    // `@font-face` first — parse order doesn't matter to the browser,
    // but faces-before-users reads sanely and matches the SSR head's
    // cascade-order convention.
    let mut out = fonts.rules;
    out.push_str(&rules);
    out
}

/// Collects `@font-face` rules for every [`runtime_core::FontFamily::Typeface`]
/// a preminted layer references, deduped by [`runtime_core::assets::TypefaceId`].
///
/// At runtime, `@font-face` registration rides sheet registration
/// (`ensure_typefaces_registered_with` → `Backend::register_typeface`)
/// — exactly the step a preminted class skips. Emitting the faces into
/// the same `.css` asset is what makes typeface-carrying sheets
/// premintable at all (fonts are a fact of real apps; without this the
/// engine-drop only applied to font-less toys). URLs reproduce the web
/// backend's `register_asset` mapping byte-for-byte: bundled font
/// paths become root-absolute served-file URLs (`/{path}` — see
/// `backend-web/src/assets.rs` for why root-absolute), `Remote`
/// passes through. `Embedded` faces have no build-time URL (the web
/// backend mints a runtime blob URL) — those SHEETS stay premintable
/// but the face is skipped with a warning; use a bundled source for
/// preminted fonts.
#[derive(Default)]
struct FontCollector {
    seen: std::collections::BTreeSet<u64>,
    rules: String,
}

impl FontCollector {
    fn collect(&mut self, rules_layer: &StyleRules) {
        let Some(runtime_core::FontFamily::Typeface(tf)) = &rules_layer.font_family else {
            return;
        };
        if !self.seen.insert(tf.id.0) {
            return;
        }
        for face in tf.faces {
            use runtime_core::assets::AssetSource;
            let url = match &face.source {
                AssetSource::Bundled { path } | AssetSource::BundledEmbedded { path, .. } => {
                    format!("/{path}")
                }
                AssetSource::Remote { url } => (*url).to_string(),
                AssetSource::Embedded { .. } => {
                    eprintln!(
                        "[premint-dump] typeface {}: embedded-bytes face has no \
                         build-time URL; skipping its @font-face (use a bundled \
                         source for preminted fonts)",
                        tf.family_name,
                    );
                    continue;
                }
            };
            self.rules.push_str(&css::font_face_css(tf.family_name, face, &url));
            self.rules.push('\n');
        }
    }
}

fn push_rule(out: &mut String, rule: String) {
    out.push_str(&rule);
    out.push('\n');
}

fn dump_sheet(entry: &PremintSheet, out: &mut String, fonts: &mut FontCollector) {
    let sheet = (entry.sheet)();
    let base_class = entry.base_class;
    assert!(
        !sheet.has_compounds(),
        "premint sheet {base_class} declares compound variants — only \
         `stylesheet!`-generated sheets may register for preminting"
    );

    let base = sheet.premint_base();
    fonts.collect(&base);

    // 1. Base — the only layer that carries the theme default-font hook.
    //    (Full lowering: the base rule pins the framework's
    //    `flex-direction: column` default itself when it promotes — at
    //    (0,1,0), which every later-source explicit direction still
    //    beats.)
    push_rule(out, css::class_rule(base_class, &emit_base(&base)));

    // 2. Breakpoint deltas, rank ascending (the walker's resolver sorts
    //    the same way; higher buckets win by stacking).
    let mut bps: Vec<_> = sheet.premint_breakpoint_axes().to_vec();
    bps.sort_by_key(|(bp, _)| bp.rank());
    for (bp, axis) in &bps {
        if let Some(delta) = sheet.premint_delta(axis, "on") {
            fonts.collect(&delta);
            if let Some(rule) =
                css::breakpoint_media_rule(base_class, *bp, &css::rules_to_css_delta(&delta))
            {
                push_rule(out, rule);
            }
            // The live engine pins `flex-direction: column` inside any
            // MERGED rule set it promotes to flex. A delta can't make
            // that merged-set decision (a promoting delta would stomp a
            // sibling layer's explicit `row` from later source order —
            // the "Stack rows collapse to columns" bug), so the pin
            // rides a specificity-(0,0,0) `:where()` companion SCOPED to
            // the promoting layer's own condition: it applies exactly
            // when the layer does, and loses to every explicit
            // direction from any layer. Same pattern for containers,
            // states, and axis arms below.
            if css::flex_promoted(&delta) && delta.flex_direction.is_none() {
                if let Some(pin) = css::breakpoint_media_rule(
                    &format!(":where(.{base_class})"),
                    *bp,
                    "flex-direction: column",
                ) {
                    // breakpoint_media_rule prepends `.` to its class
                    // argument; splice the :where form in directly.
                    push_rule(out, pin.replace(&format!(".:where(.{base_class})"), &format!(":where(.{base_class})")));
                }
            }
        }
    }

    // 3. Container-query deltas, threshold ascending.
    let mut cqs: Vec<_> = sheet.premint_container_axes().to_vec();
    cqs.sort_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    for (threshold, axis) in &cqs {
        if let Some(delta) = sheet.premint_delta(axis, "on") {
            fonts.collect(&delta);
            push_rule(
                out,
                css::container_query_rule(base_class, *threshold, &css::rules_to_css_delta(&delta)),
            );
            if css::flex_promoted(&delta) && delta.flex_direction.is_none() {
                let pin =
                    css::container_query_rule(base_class, *threshold, "flex-direction: column");
                push_rule(out, pin.replace(&format!(".{base_class} {{"), &format!(":where(.{base_class}) {{")));
            }
        }
    }

    // 4. State deltas, declaration order. `:where()` cancels the
    //    pseudo-class's specificity so author-axis rules (emitted after,
    //    matching the resolver merging state axes before author axes)
    //    still win conflicting properties by source order.
    for (bit, axis) in sheet.premint_state_axes() {
        let Some(pseudo) = css::state_pseudo(*bit) else { continue };
        let Some(delta) = sheet.premint_delta(axis, "on") else { continue };
        fonts.collect(&delta);
        let mut body = css::rules_to_css_delta(&delta);
        // Same UA-ring suppression as the live engine's focus overlay
        // rule (see `class_rule_group_with`): a sheet that declares its
        // own focus indicator owns it.
        if *bit == runtime_core::StateBits::FOCUSED {
            body = format!("outline:none;{body}");
        }
        push_rule(
            out,
            css::class_rule(&format!("{base_class}:where({pseudo})"), &body),
        );
        if css::flex_promoted(&delta) && delta.flex_direction.is_none() {
            push_rule(
                out,
                format!(":where(.{base_class}{pseudo}) {{ flex-direction: column }}"),
            );
        }
    }

    // 5. Author-axis deltas, alphabetical by axis name (the BTreeMap
    //    order `resolve` merges them in). Arms with empty bodies still
    //    emit — every class the runtime can stamp has a rule, which
    //    keeps DevTools honest about where a class comes from.
    for (axis, values, _default) in sheet.premint_variant_axes() {
        for value in &values {
            let delta = sheet
                .premint_delta(&axis, value)
                .expect("premint_variant_axes listed this value");
            fonts.collect(&delta);
            push_rule(
                out,
                css::class_rule(
                    &format!("{base_class}-{axis}-{value}"),
                    &css::rules_to_css_delta(&delta),
                ),
            );
            if css::flex_promoted(&delta) && delta.flex_direction.is_none() {
                push_rule(
                    out,
                    format!(":where(.{base_class}-{axis}-{value}) {{ flex-direction: column }}"),
                );
            }
        }
    }
}

/// Base-rule lowering: [`css::rules_to_css`] plus the theme
/// default-font hook. The live engine fills an absent `font_family`
/// with the installed theme's default text font at apply time
/// (`with_default_text_font`) — but the dump runs in a build step where
/// no theme exists yet, so a fontless BASE gets
/// `font-family: var(--iy-default-font, inherit)` instead. The premint
/// host driver defines that variable from the live theme
/// (`Backend::apply_default_text_font`); with no default installed the
/// `inherit` fallback reproduces plain cascade. Delta layers never get
/// the hook — a delta that sets a font overrides the base's declaration
/// on the same element, and one that doesn't leaves it standing, which
/// is exactly the live fill's semantics.
fn emit_base(rules: &StyleRules) -> String {
    let mut out = css::rules_to_css(rules);
    if rules.font_family.is_none() {
        // Match `rules_to_css`'s declaration convention exactly:
        // `"; "`-separated, no trailing semicolon.
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str("font-family: var(");
        out.push_str(css::DEFAULT_TEXT_FONT_VAR);
        out.push_str(", inherit)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::premint::{PremintSheet, PREMINT_SHEETS};
    use runtime_core::{stylesheet, AlignItems, Color, FlexDirection, Length};

    // A sheet with a variant axis + a state overlay, registered manually
    // with the exact entry shape the `stylesheet!` macro emits under
    // `cfg(idealyst_premint_dump)` — this test owns the assembly side
    // (delta emission, ordering); the macro↔runtime class agreement is
    // covered by `regression_dump_and_runtime_agree_on_class_names`.
    stylesheet! {
        Chip<()> {
            base(_t) {
                color: Color("#111111".into()),
            }
            variant tone {
                #[default]
                neutral(_t) {}
                danger(_t) {
                    color: Color("#ff0000".into()),
                }
            }
            state hovered(_t) {
                color: Color("#222222".into()),
            }
        }
    }

    #[runtime_core::premint::linkme::distributed_slice(PREMINT_SHEETS)]
    #[linkme(crate = runtime_core::premint::linkme)]
    static TEST_SHEET: PremintSheet =
        PremintSheet { base_class: "iy-test", sheet: chip_style };

    #[test]
    fn dump_emits_base_plus_one_delta_per_arm() {
        let out = dump_all_css();
        // Base carries the base props; arm deltas carry ONLY their own.
        assert!(out.contains(".iy-test { color: #111111"), "base rule; got:\n{out}");
        assert!(
            out.contains(".iy-test-tone-neutral {"),
            "default arm still gets a (possibly empty) delta rule; got:\n{out}"
        );
        assert!(
            out.contains(".iy-test-tone-danger { color: #ff0000"),
            "danger delta is the arm's own props, not a full merge; got:\n{out}"
        );
        // State overlay: one rule on the BASE class, specificity-flattened.
        assert!(
            out.contains(".iy-test:where(:hover) { color: #222222"),
            "state delta as :where()-wrapped pseudo on the base; got:\n{out}"
        );
        // The whole point: no per-combo rules.
        assert!(
            !out.contains(".iy-test-neutral ") && !out.contains(".iy-test-danger "),
            "no combo-suffixed classes may remain; got:\n{out}"
        );
    }

    /// Source order is load-bearing: state deltas must precede author
    /// axis deltas (the resolver merges `__state_*` axes first —
    /// alphabetically before lowercase names — so axis arms win
    /// conflicting props by later source order).
    #[test]
    fn regression_state_rules_precede_axis_rules() {
        let out = dump_all_css();
        let state_pos = out.find(".iy-test:where(:hover)").expect("state rule present");
        let axis_pos = out.find(".iy-test-tone-danger").expect("axis rule present");
        assert!(
            state_pos < axis_pos,
            "state delta must be emitted before axis deltas; got:\n{out}"
        );
    }

    // A second sheet whose base SETS a font — the default-font var must
    // not override an explicit author font.
    stylesheet! {
        Mono<()> {
            base(_t) {
                font_family: "ui-monospace, Menlo, monospace",
            }
        }
    }

    #[runtime_core::premint::linkme::distributed_slice(PREMINT_SHEETS)]
    #[linkme(crate = runtime_core::premint::linkme)]
    static MONO_SHEET: PremintSheet =
        PremintSheet { base_class: "iy-mono", sheet: mono_style };

    /// Fontless BASE rules carry the theme-default hook; explicit fonts
    /// win untouched — mirrors the live engine's `with_default_text_font`
    /// fill exactly (see `emit_base`).
    #[test]
    fn regression_fontless_base_references_default_font_var() {
        let out = dump_all_css();
        let base = out
            .lines()
            .find(|l| l.starts_with(".iy-test { "))
            .expect("chip base rule present");
        assert!(
            base.contains("; font-family: var(--iy-default-font, inherit)"),
            "fontless base must reference the default-font var; got:\n{base}"
        );
        // Deltas never get the hook.
        let danger = out
            .lines()
            .find(|l| l.starts_with(".iy-test-tone-danger"))
            .expect("danger delta present");
        assert!(
            !danger.contains("--iy-default-font"),
            "delta rules must not restate the font hook; got:\n{danger}"
        );
        let mono = out
            .lines()
            .find(|l| l.starts_with(".iy-mono { "))
            .expect("mono rule present");
        assert!(
            mono.contains("font-family: ui-monospace, Menlo, monospace")
                && !mono.contains("--iy-default-font"),
            "explicit author font must pass through untouched; got:\n{mono}"
        );
    }

    // Stack-shaped sheet: a `direction` axis and a `gap` axis (which
    // sorts AFTER `direction`, so its delta lands later in the cascade).
    stylesheet! {
        Rack<()> {
            base(_t) {}
            variant direction {
                #[default]
                column(_t) {}
                row(_t) {
                    flex_direction: FlexDirection::Row,
                }
            }
            variant gap {
                #[default]
                md(_t) {
                    gap: Length::Px(12.0),
                }
            }
            breakpoint lg(_t) {
                align_items: AlignItems::Center,
            }
        }
    }

    #[runtime_core::premint::linkme::distributed_slice(PREMINT_SHEETS)]
    #[linkme(crate = runtime_core::premint::linkme)]
    static RACK_SHEET: PremintSheet =
        PremintSheet { base_class: "iy-rack", sheet: rack_style };

    /// Regression for the "Stack rows collapse to columns" premint bug
    /// (website Demo page, 13 nodes): the flex-direction pin is a
    /// MERGED-set decision, so a delta that promotes to flex (here the
    /// `gap` arm) must NOT pin `column` — it would ride later source
    /// order than a sibling axis's explicit `row` and stomp it. The
    /// framework default instead rides a specificity-(0,0,0) `:where`
    /// rule that loses to every explicit direction.
    #[test]
    fn regression_flex_pin_rides_where_rule_not_deltas() {
        let out = dump_all_css();
        assert!(
            out.contains(":where(.iy-rack-gap-md) { flex-direction: column }"),
            "the promoting ARM gets a scoped zero-specificity pin; got:\n{out}"
        );
        assert!(
            !out.lines().any(|l| l.starts_with(":where(.iy-rack) {")),
            "no blanket UNCONDITIONAL per-sheet pin — it would set a computed \
             direction on elements whose merged rules never promote (and flip \
             real flex elements promoted by EXTERNAL sources); a pin may only \
             appear scoped inside a promoting layer's own condition; got:\n{out}"
        );
        let gap = out
            .lines()
            .find(|l| l.starts_with(".iy-rack-gap-md"))
            .expect("gap delta present");
        assert!(
            gap.contains("display: flex") && !gap.contains("flex-direction"),
            "a promoting delta must not pin a direction; got:\n{gap}"
        );
        let row = out
            .lines()
            .find(|l| l.starts_with(".iy-rack-direction-row"))
            .expect("row delta present");
        assert!(
            row.contains("flex-direction: row"),
            "explicit direction emits normally; got:\n{row}"
        );
        // A promoting BREAKPOINT overlay (align_items, no direction) gets
        // its pin inside the same media query, :where-scoped.
        assert!(
            out.contains("{ :where(.iy-rack) { flex-direction: column } }"),
            "promoting bp overlay needs a media-scoped zero-specificity pin; got:\n{out}"
        );
        // Order sanity: row's rule precedes gap's (alphabetical axes), and
        // the :where pin loses on specificity anyway.
        assert!(
            out.find(".iy-rack-direction-row").unwrap() < out.find(".iy-rack-gap-md").unwrap(),
            "axis deltas emit in the resolver's alphabetical order; got:\n{out}"
        );
    }

    // A typeface-carrying sheet — `&TEST_INTER` is a static reference,
    // which the relaxed premintability guard accepts (the macro's
    // registration below only exists because the sheet premints).
    static TEST_INTER_FACES: &[runtime_core::assets::TypefaceFace] =
        &[runtime_core::assets::TypefaceFace {
            weight: runtime_core::FontWeight::Bold,
            style: runtime_core::FontStyle::Normal,
            asset: runtime_core::assets::AssetId(4242),
            source: runtime_core::assets::AssetSource::Bundled {
                path: "fonts/TestInter-Bold.woff2",
            },
        }];
    static TEST_INTER: runtime_core::Typeface = runtime_core::Typeface {
        id: runtime_core::assets::TypefaceId(4242),
        family_name: "TestInter",
        faces: TEST_INTER_FACES,
        fallback: runtime_core::assets::SystemFallback::SansSerif,
    };

    stylesheet! {
        Branded<()> {
            base(_t) {
                font_family: &TEST_INTER,
            }
        }
    }

    #[runtime_core::premint::linkme::distributed_slice(PREMINT_SHEETS)]
    #[linkme(crate = runtime_core::premint::linkme)]
    static BRANDED_SHEET: PremintSheet =
        PremintSheet { base_class: "iy-brand", sheet: branded_style };

    /// Fonts are a fact of real apps: a sheet referencing a static
    /// `Typeface` must premint, with the family's `@font-face` emitted
    /// into the same `.css` (served-file URL, the web backend's
    /// root-absolute mapping) — standing in for the runtime
    /// `register_typeface` that sheet registration would have done.
    #[test]
    fn regression_static_typeface_sheets_premint_with_font_face() {
        let out = dump_all_css();
        assert!(
            out.contains(".iy-brand { font-family: \"TestInter\" }"),
            "typeface sheet must premint with the quoted family; got:\n{out}"
        );
        assert!(
            out.contains("@font-face{font-family:\"TestInter\"")
                && out.contains("src:url(\"/fonts/TestInter-Bold.woff2\")"),
            "the family's @font-face must ship in the same css with the \
             served-file URL; got:\n{out}"
        );
        assert!(
            out.find("@font-face").unwrap() < out.find(".iy-").unwrap(),
            "faces precede the rules that use them; got:\n{out}"
        );
    }

    /// Regression for the website Tag crash: a component that COMPOSES a
    /// builder's styles (merge an inherited color, layer a hover) must use
    /// `into_style_application()`, which returns the live application even
    /// under `--cfg idealyst_premint` — where `into_style_source()` (both
    /// cfgs are on in this crate, see build.rs) returns an opaque
    /// `Preminted` class that the old `match Static → unreachable!` sites
    /// panicked on (idea-ui `tag.rs`/`alert.rs`; `table.rs` silently lost
    /// row hover). The tighter test — building idea-ui's `Tag` under the
    /// cfg — would need this tools crate to dev-depend on idea-ui +
    /// idea-theme solely for one test; the seam behavior asserted here is
    /// the exact property those fixes rely on.
    #[test]
    fn regression_into_style_application_bypasses_premint() {
        use runtime_core::{resolve_style, IntoStyleSource, StyleSource};
        // The source path premints…
        assert!(matches!(
            Chip().tone(ChipTone::Danger).into_style_source(),
            StyleSource::Preminted { .. }
        ));
        // …while the application path stays live and resolvable.
        let app = Chip().tone(ChipTone::Danger).into_style_application();
        let rules = resolve_style(&app);
        let color = rules.color.as_ref().expect("danger arm sets color");
        assert_eq!(color.value().0, "#ff0000");
    }

    /// THE premint contract, in-repo: every class the shipped binary's
    /// builder assembles at runtime must have a matching rule in the CSS
    /// the dump emits — with no manifest linking them, only the shared
    /// source hash computed inside `stylesheet!`. Both cfgs are set for
    /// this crate by its build.rs (cargo can't set custom cfgs
    /// per-test), so the macro's runtime fast path AND its dump
    /// registration are simultaneously live here — exactly the
    /// shipped-build/dump-build pair the CLI produces.
    #[test]
    fn regression_dump_and_runtime_agree_on_class_names() {
        use runtime_core::{IntoStyleSource, StyleSource};
        let css = dump_all_css();
        for src in [
            Chip().into_style_source(),
            Chip().tone(ChipTone::Danger).into_style_source(),
        ] {
            match src {
                StyleSource::Preminted { class, overrides } => {
                    assert!(overrides.is_none());
                    let classes: Vec<&str> = class.split_whitespace().collect();
                    assert!(classes.len() >= 2, "base + one class per axis; got {class}");
                    for c in &classes {
                        assert!(
                            css.contains(&format!(".{c} {{")) || css.contains(&format!(".{c}:where(")),
                            "runtime class {c} has no rule in dump css:\n{css}"
                        );
                    }
                }
                _ => panic!("builder must emit Preminted under --cfg idealyst_premint"),
            }
        }
    }
}
