//! Premint style-dump assembly — the CSS-emission side of build-time
//! styles (see `runtime_core::premint` for the collection side and
//! `StyleProp::Preminted` for the runtime side).
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
//! # Compound variants
//!
//! Compounds (runtime-API-only; the `stylesheet!` grammar cannot express
//! them) lower to CSS COMPOUND SELECTORS and are emitted last, after the
//! single-axis arms. Since the runtime stamps one class per axis, "these
//! axes coincide" is directly `.iy-x-a-1.iy-x-b-2` — a leg naming a
//! `__state_*` axis becomes that state's pseudo/attribute instead, because
//! states are never stamped as classes.
//!
//! These are the one deliberate exception to the flat-(0,1,0) rule above:
//! a two-leg compound is (0,2,0), and it is NOT `:where()`-wrapped. That is
//! required, not incidental — `resolve` merges compounds after every axis,
//! so the compound has to outrank the arms it overlaps, and shedding its
//! specificity would hand the win back to the later-emitted arm rules.
//!
//! A compound with a state leg on its `off` arm cannot be expressed (there
//! is no "not hovered" pseudo in this model) and is skipped; such a sheet
//! keeps that overlay only on the live engine.

use runtime_core::premint::{PremintSheet, PREMINT_SHEETS};
use runtime_core::{StyleRules, StyleSheet};

/// Assemble the full preminted stylesheet for every registered sheet.
/// Deterministic given the same binary — the CLI fingerprints the
/// output like any other asset.
pub fn dump_all_css() -> String {
    let mut rules = String::new();
    // The reactive-path font-inheritance rule, FIRST — before every
    // sheet rule, deliberately. It is specificity (0,1,0): first
    // position makes any sheet-declared `font-family` (base or arm,
    // also (0,1,0)) beat it on source order, while the default-font
    // hook below sits at (0,0,0) and loses to it on specificity. That
    // ordering sandwich is what encodes the live engine's per-path
    // semantics in one static asset — see
    // `runtime_core::PREMINT_FONT_INHERIT_CLASS` for the full contract.
    push_rule(
        &mut rules,
        format!(
            ".{} {{ font-family: inherit }}",
            runtime_core::PREMINT_FONT_INHERIT_CLASS
        ),
    );
    let mut fonts = FontCollector::default();
    for sheet in PREMINT_SHEETS {
        dump_sheet(sheet, &mut rules, &mut fonts);
    }
    // Runtime-assembled sheets (`StyleSheet::premint_as` — idea-theme's
    // component sheets). These have no `stylesheet!` expansion site to
    // hang a link-time registration on, so they register as the app
    // installs them; the caller has already run `app()`, so by now they
    // exist. Deduped on the base class: an app that reinstalls a theme
    // registers the same identity twice, and emitting its rules twice
    // would double the CSS for no cascade difference.
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for sheet in runtime_core::premint::assembled_sheets() {
        let Some(base_class) = sheet.premint_class() else { continue };
        if !seen.insert(base_class.to_string()) {
            continue;
        }
        dump_sheet_parts(base_class, &sheet, &mut rules, &mut fonts);
    }
    // AUTO-preminted static sheets (`StyleSheet::r#static` derives the
    // class from the rules' content key — no hand-written identity).
    // The registry stores raw (class, rules) parts because a bare
    // `r#static` isn't `Rc`-wrapped at construction; rebuilding a
    // transient sheet here reuses the exact same lowering (fonts, the
    // default-font hook, token → var()) as every other registered
    // sheet. Deduped on the class: content-equal sheets share one by
    // construction. Entries whose sheet later took an explicit identity
    // or grew layers were retracted at that point and never appear.
    for (class, static_rules) in runtime_core::premint::static_rules() {
        if !seen.insert(class.to_string()) {
            continue;
        }
        let sheet = std::rc::Rc::new(runtime_core::StyleSheet::r#static(static_rules));
        dump_sheet_parts(&class, &sheet, &mut rules, &mut fonts);
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
    dump_sheet_parts(entry.base_class, &sheet, out, fonts);
}

/// The per-sheet emission, shared by the link-time (`stylesheet!`) and
/// runtime-assembled (`premint_as`) registration paths. Split out so the
/// two entry points cannot drift: a selector emitted here is the one the
/// runtime's `preminted_class_list` stamps, and the halves agree by
/// construction rather than by convention.
fn dump_sheet_parts(
    base_class: &str,
    sheet: &StyleSheet,
    out: &mut String,
    fonts: &mut FontCollector,
) {
    let base = sheet.premint_base();
    fonts.collect(&base);
    // The sheet's BASE declares a non-flex `display` (e.g. `grid`): no
    // delta layer can flex-promote the MERGED set — the explicit display
    // wins — so arm/overlay deltas lower promotion-suppressed and emit
    // no `flex-direction: column` pin companions. Without this, a
    // gap-only arm of a grid sheet emitted `display: flex` (stomping the
    // base's `grid` from later source order) plus a column pin the live
    // engine never mints (it decides on the merged rules). See
    // `css::rules_to_css_delta_unpromoted`.
    let display_locked = matches!(
        base.display,
        Some(d) if d != runtime_core::DisplayKind::Flex
    );
    let lower_delta = |r: &runtime_core::StyleRules| {
        if display_locked {
            css::rules_to_css_delta_unpromoted(r)
        } else {
            css::rules_to_css_delta(r)
        }
    };

    // 1. Base — full lowering (the base rule pins the framework's
    //    `flex-direction: column` default itself when it promotes — at
    //    (0,1,0), which every later-source explicit direction still
    //    beats), plus the theme default-font hook as a (0,0,0)
    //    companion when the base names no font (see `font_hook_rule`).
    push_rule(out, css::class_rule(base_class, &css::rules_to_css(&base)));
    if let Some(hook) = font_hook_rule(base_class, &base) {
        push_rule(out, hook);
    }

    // 2. Breakpoint deltas, rank ascending (the style engine's resolver sorts
    //    the same way; higher buckets win by stacking).
    let mut bps: Vec<_> = sheet.premint_breakpoint_axes().to_vec();
    bps.sort_by_key(|(bp, _)| bp.rank());
    for (bp, axis) in &bps {
        if let Some(delta) = sheet.premint_delta(axis, "on") {
            fonts.collect(&delta);
            if let Some(rule) =
                css::breakpoint_media_rule(base_class, *bp, &lower_delta(&delta))
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
            if !display_locked && css::flex_promoted(&delta) && delta.flex_direction.is_none() {
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
                css::container_query_rule(base_class, *threshold, &lower_delta(&delta)),
            );
            if !display_locked && css::flex_promoted(&delta) && delta.flex_direction.is_none() {
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
        let mut body = lower_delta(&delta);
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
        if !display_locked && css::flex_promoted(&delta) && delta.flex_direction.is_none() {
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
                    &lower_delta(&delta),
                ),
            );
            if !display_locked && css::flex_promoted(&delta) && delta.flex_direction.is_none() {
                push_rule(
                    out,
                    format!(":where(.{base_class}-{axis}-{value}) {{ flex-direction: column }}"),
                );
            }
        }
    }

    // 6. Compound variants, declaration order (the order `resolve` merges
    //    them, after every axis).
    //
    //    A compound fires when several axes coincide. The runtime stamps one
    //    class per axis, so that condition IS a CSS compound selector over
    //    those classes — no extra stamped class is needed, the selector does
    //    the matching. A leg naming a `__state_*` axis becomes that state's
    //    pseudo-class/attribute instead, because states are never stamped as
    //    classes (see step 4).
    //
    //    Specificity falls out right: two class legs are (0,2,0) and a
    //    class+pseudo leg is (0,2,0), both above the (0,1,0) single-axis arms
    //    emitted in step 5 — which is exactly `resolve`'s ordering
    //    (base → axes → compounds). Note this does NOT use `:where()`: the
    //    state deltas in step 4 wrap their pseudo to *shed* specificity so
    //    later author-axis rules still win, but a compound is supposed to
    //    beat those arms, so its specificity has to stand.
    for (when, rules) in sheet.premint_compounds() {
        fonts.collect(&rules);
        // Class legs first, then pseudo legs — a CSS compound selector must
        // lead with the type/class part (`.cls:hover`, never `:hover.cls`…
        // which is legal but reads wrong and breaks the `[disabled]` form).
        // `when` is a BTreeMap, so its own order is alphabetical by axis and
        // would otherwise put `__state_*` first.
        let mut classes = String::new();
        let mut pseudos = String::new();
        // `__bp_*` / `__cq_*` legs become PRELUDES around the compound
        // rule, never class legs: those axes are not stamped as classes
        // (their single arms lower to `@media`/`@container` on the base
        // class — steps 2–3), so a class leg would make the whole compound
        // dead CSS that matches nothing. (AppShell's pin-beats-open
        // compound, `(open=on ∧ __bp_lg=on)`, shipped exactly that way
        // once.) Like states, only the "on" arm exists for an overlay leg.
        let mut preludes: Vec<String> = Vec::new();
        let mut skip = false;
        for (axis, value) in &when {
            if let Some(bit) = sheet.premint_state_axis_bit(axis) {
                // A state leg only matches on its "on" arm; "off" has no
                // pseudo to hang the condition on, so such a compound cannot
                // be expressed and stays live.
                match (css::state_pseudo(bit), value.as_str()) {
                    (Some(pseudo), "on") => pseudos.push_str(pseudo),
                    _ => {
                        skip = true;
                        break;
                    }
                }
            } else if let Some(bp) = sheet
                .premint_breakpoint_axes()
                .iter()
                .find(|(_, a)| a == axis)
                .map(|(bp, _)| *bp)
            {
                match (css::breakpoint_media_query(bp), value.as_str()) {
                    (Some(query), "on") => preludes.push(query),
                    _ => {
                        skip = true;
                        break;
                    }
                }
            } else if let Some(threshold) = sheet
                .premint_container_axes()
                .iter()
                .find(|(_, a)| a == axis)
                .map(|(t, _)| *t)
            {
                if value.as_str() == "on" {
                    preludes.push(css::container_query_prelude(threshold));
                } else {
                    skip = true;
                    break;
                }
            } else {
                classes.push_str(&format!(".{base_class}-{axis}-{value}"));
            }
        }
        if skip || (classes.is_empty() && pseudos.is_empty() && preludes.is_empty()) {
            continue;
        }
        // A compound whose legs are all states/overlays has no class anchor
        // of its own; anchor it on the base class so it can't match other
        // elements.
        if classes.is_empty() {
            classes.push_str(&format!(".{base_class}"));
        }
        let selector = format!("{classes}{pseudos}");
        let body = lower_delta(&rules);
        if body.is_empty() {
            continue;
        }
        let wrap = |rule: String| {
            preludes
                .iter()
                .rev()
                .fold(rule, |r, p| format!("{p} {{ {r} }}"))
        };
        // `class_rule` prepends the leading `.`; the selector already carries
        // its own, so emit the rule directly.
        push_rule(out, wrap(format!("{selector} {{ {body} }}")));
        if !display_locked && css::flex_promoted(&rules) && rules.flex_direction.is_none() {
            push_rule(
                out,
                wrap(format!(":where({selector}) {{ flex-direction: column }}")),
            );
        }
    }
}

/// The theme default-font hook for a fontless BASE, as a standalone
/// specificity-(0,0,0) rule. The live engine fills an absent
/// `font_family` with the installed theme's default text font at apply
/// time (`with_default_text_font`) — but the dump runs in a build step
/// where no theme exists yet, so a fontless base gets
/// `font-family: var(--iy-default-font, inherit)` and the premint host
/// driver defines that variable from the live theme
/// (`Backend::apply_default_text_font`); with no default installed the
/// `inherit` fallback reproduces plain cascade. Delta layers never get
/// the hook — a delta that sets a font overrides the base's on the same
/// element, and one that doesn't leaves it standing, which is exactly
/// the live fill's semantics.
///
/// A COMPANION `:where()` rule rather than a declaration inside the base
/// body, for cascade rank: at (0,0,0) it loses to any DECLARED
/// `font-family` from any layer, and — the load-bearing part — to the
/// (0,1,0) `.iy-font-inherit` rule the reactive attach paths stamp
/// (emitted first in this asset). That is what lets one static asset
/// carry both live semantics: static applications take the hook (the
/// theme-default fold), reactive ones take `inherit` (the author's
/// ancestor font). Same pattern as the flex-direction pin above.
fn font_hook_rule(base_class: &str, rules: &StyleRules) -> Option<String> {
    if rules.font_family.is_some() {
        return None;
    }
    Some(format!(
        ":where(.{base_class}) {{ font-family: var({}, inherit) }}",
        css::DEFAULT_TEXT_FONT_VAR
    ))
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

    /// A sheet whose BASE declares `display: grid` must not have its arm
    /// deltas flex-promote: a gap-only arm used to emit `display: flex`
    /// (stomping the base's `grid` from later source order) plus a
    /// `flex-direction: column` pin the live engine never mints for a
    /// grid. Fails against the promoted lowering on both counts.
    #[test]
    fn regression_grid_sheet_arms_do_not_flex_promote() {
        use runtime_core::{StyleRules, StyleSheet, Tokenized};
        let sheet = StyleSheet::new(|_vs: &runtime_core::VariantSet| StyleRules {
            display: Some(runtime_core::DisplayKind::Grid),
            ..Default::default()
        })
        .variant("gap", "roomy", |_vs| StyleRules {
            gap: Some(Tokenized::Literal(Length::Px(37.0))),
            ..Default::default()
        })
        .variant_default("gap", "roomy")
        .premint_as("dumptest.v1.gridsheet");

        let mut out = String::new();
        let mut fonts = FontCollector::default();
        dump_sheet_parts(sheet.premint_class().unwrap(), &sheet, &mut out, &mut fonts);

        let arm = out
            .lines()
            .find(|l| l.contains("-gap-roomy"))
            .expect("gap arm rule emitted");
        assert!(
            !arm.contains("display: flex"),
            "a grid sheet's gap arm must not auto-promote to flex; got:\n{arm}"
        );
        assert!(
            !out.contains("flex-direction: column"),
            "no column pin companion for a display-locked sheet; got:\n{out}"
        );
        assert!(
            out.contains("display: grid"),
            "the base still declares the grid; got:\n{out}"
        );
    }

    /// A compound with a `__bp_*` leg must lower that leg to an `@media`
    /// PRELUDE, not a class leg: breakpoint axes are never stamped as
    /// classes (their single arms ride `@media` on the base class), so a
    /// `.base-__bp_lg-on.base-open-on` selector matches nothing — dead
    /// CSS, and the compound's semantics silently vanish on web while
    /// native `resolve` still applies them. AppShell's pin-beats-open
    /// compound (`open=on ∧ __bp_lg=on` re-asserting the inert scrim)
    /// shipped exactly that way once: an open drawer resized past the
    /// pin threshold kept dimming/intercepting the pinned layout.
    #[test]
    fn regression_bp_leg_compound_lowers_to_media_prelude() {
        use runtime_core::{StyleRules, StyleSheet, Tokenized};
        let sheet = StyleSheet::new(|_vs: &runtime_core::VariantSet| StyleRules {
            opacity: Some(Tokenized::Literal(0.0)),
            ..Default::default()
        })
        .variant("open", "on", |_vs| StyleRules {
            opacity: Some(Tokenized::Literal(0.45)),
            ..Default::default()
        })
        .variant("__bp_lg", "on", |_vs| StyleRules {
            opacity: Some(Tokenized::Literal(0.0)),
            ..Default::default()
        })
        .compound(vec![("open", "on"), ("__bp_lg", "on")], |_vs| StyleRules {
            opacity: Some(Tokenized::Literal(0.0)),
            ..Default::default()
        })
        .premint_as("dumptest.v1.bpcompound");

        let mut out = String::new();
        let mut fonts = FontCollector::default();
        let base = sheet.premint_class().unwrap();
        dump_sheet_parts(base, &sheet, &mut out, &mut fonts);

        assert!(
            !out.contains(&format!("{base}-__bp_lg-on.")),
            "no dead class-pair selector for the bp leg; got:\n{out}"
        );
        let compound = out
            .lines()
            .find(|l| l.starts_with("@media") && l.contains(&format!(".{base}-open-on")))
            .expect("bp-leg compound emitted as a media-wrapped author-class rule");
        assert!(
            compound.contains("opacity: 0"),
            "the compound's own delta rides inside the prelude; got:\n{compound}"
        );
        // Emitted AFTER the plain open arm: same (0,1,0) specificity, so
        // source order is what lets the pinned overlay win in-range —
        // mirroring `resolve` merging compounds after every axis.
        let open_arm = out.find(&format!(".{base}-open-on {{")).expect("open arm");
        let wrapped = out.find(compound).unwrap();
        assert!(
            wrapped > open_arm,
            "compound must follow the single-axis arm in source order"
        );
    }

    /// `r#static` sheets auto-premint by content key: constructing one
    /// registers (class, rules) and the dump emits its rule with NO
    /// hand-written identity. Content-equal duplicates share one class
    /// and one rule; a sheet that graduates to `premint_as` retracts its
    /// auto entry, so the asset carries the identity class only.
    #[test]
    fn regression_static_sheets_auto_premint_without_identity() {
        use runtime_core::{StyleRules, StyleSheet, Tokenized};
        let rules = || StyleRules {
            // Distinctive value so the assertions can't match another
            // test's sheet in the shared thread-local registry.
            min_width: Some(Tokenized::Literal(Length::Px(731.0))),
            ..Default::default()
        };
        let a = StyleSheet::r#static(rules());
        let auto_class = a.premint_class().expect("auto class").to_string();
        let _b = StyleSheet::r#static(rules()); // content-equal duplicate

        let out = dump_all_css();
        assert!(
            out.contains(&format!(".{auto_class} {{ min-width: 731px")),
            "auto-preminted static rule emitted; got:\n{out}"
        );
        assert_eq!(
            out.matches(&format!(".{auto_class} ")).count(),
            1,
            "content-equal duplicates collapse to ONE rule; got:\n{out}"
        );

        // Graduating to an explicit identity retracts the auto entry:
        // exactly ONE rule carries the content (the identity class),
        // not two (identity + retired auto class).
        let named_rules = StyleRules {
            min_width: Some(Tokenized::Literal(Length::Px(733.0))),
            ..Default::default()
        };
        let _named = StyleSheet::r#static(named_rules).premint_as("dumptest.v1.named");
        let out = dump_all_css();
        assert_eq!(
            out.matches("min-width: 733px").count(),
            1,
            "premint_as retracts the auto entry — one rule, under the identity class; got:\n{out}"
        );
    }

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
    // A SHADOWED sheet — premintable since the `shadow`/`text_shadow`
    // split (the old single field lowered per node kind and disqualified
    // the whole sheet; the SwitchThumb was the measured fall-through).
    stylesheet! {
        Thumb<()> {
            base(_t) {
                background: Color("#ffffff".into()),
                shadow: runtime_core::Shadow {
                    x: 0.0,
                    y: 1.0,
                    blur: 3.0,
                    color: Color("rgba(15, 17, 21, 0.30)".into()),
                },
            }
        }
    }

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
        // The hook is a standalone (0,0,0) companion, NOT a declaration in
        // the base body — the base body at (0,1,0) would beat the
        // (0,1,0) `.iy-font-inherit` re-rank on source order and reactive
        // nodes could never restore inheritance.
        assert!(
            out.contains(":where(.iy-test) { font-family: var(--iy-default-font, inherit) }"),
            "fontless base must get the default-font hook as a :where companion; got:\n{out}"
        );
        let base = out
            .lines()
            .find(|l| l.starts_with(".iy-test { "))
            .expect("chip base rule present");
        assert!(
            !base.contains("--iy-default-font"),
            "the hook must NOT ride inside the (0,1,0) base body; got:\n{base}"
        );
        // The reactive re-rank rule exists and is the FIRST rule in the
        // asset, so sheet-declared fonts (same specificity, later source)
        // beat it while the (0,0,0) hook loses to it.
        let first_rule = out
            .lines()
            .find(|l| l.starts_with('.') || l.starts_with(':'))
            .expect("asset has rules");
        assert_eq!(
            first_rule,
            &format!(
                ".{} {{ font-family: inherit }}",
                runtime_core::PREMINT_FONT_INHERIT_CLASS
            ),
            "the font-inherit re-rank must be the first rule in the asset"
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
    /// A sheet with a `shadow` premints and its rule carries `box-shadow`
    /// — the field is the BOX shadow on every node kind now (glyph
    /// shadows are the separate `text_shadow` field). Before the split
    /// the macro disqualified shadowed sheets entirely (this was the
    /// SwitchThumb fall-through in the website premint report).
    #[test]
    fn regression_shadowed_sheet_premints_with_box_shadow() {
        let sheet = Thumb::sheet();
        assert!(
            sheet.premint_class().is_some(),
            "a shadowed sheet must carry a premint class since the shadow/text_shadow split"
        );
        let out = dump_all_css();
        let base = out
            .lines()
            .find(|l| l.starts_with(&format!(".{} {{", sheet.premint_class().unwrap())))
            .expect("thumb base rule present");
        assert!(
            base.contains("box-shadow: 0px 1px 3px rgba(15, 17, 21, 0.30)"),
            "shadow lowers to box-shadow in the preminted rule; got:\n{base}"
        );
    }

    #[test]
    fn regression_flex_pin_rides_where_rule_not_deltas() {
        let out = dump_all_css();
        assert!(
            out.contains(":where(.iy-rack-gap-md) { flex-direction: column }"),
            "the promoting ARM gets a scoped zero-specificity pin; got:\n{out}"
        );
        assert!(
            !out
                .lines()
                .any(|l| l.starts_with(":where(.iy-rack) {") && l.contains("flex-direction")),
            "no blanket UNCONDITIONAL per-sheet pin — it would set a computed \
             direction on elements whose merged rules never promote (and flip \
             real flex elements promoted by EXTERNAL sources); a pin may only \
             appear scoped inside a promoting layer's own condition (the \
             font hook shares the :where(.iy-rack) shape but is not a flex \
             pin); got:\n{out}"
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
    /// under `--cfg idealyst_premint` — where `into_style_prop()` (both
    /// cfgs are on in this crate, see build.rs) returns an opaque
    /// `Preminted` class that the old `match Sheet → unreachable!` sites
    /// panicked on (idea-ui `tag.rs`/`alert.rs`; `table.rs` silently lost
    /// row hover). The tighter test — building idea-ui's `Tag` under the
    /// cfg — would need this tools crate to dev-depend on idea-ui +
    /// idea-theme solely for one test; the seam behavior asserted here is
    /// the exact property those fixes rely on.
    #[test]
    fn regression_into_style_application_bypasses_premint() {
        use runtime_core::{resolve_style, IntoStyleProp, StyleProp};
        // The style-prop path premints…
        assert!(matches!(
            Chip().tone(ChipTone::Danger).into_style_prop(),
            StyleProp::Preminted { .. }
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
        use runtime_core::{IntoStyleProp, StyleProp};
        let css = dump_all_css();
        for src in [
            Chip().into_style_prop(),
            Chip().tone(ChipTone::Danger).into_style_prop(),
        ] {
            match src {
                StyleProp::Preminted { class, overrides, .. } => {
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

#[cfg(test)]
mod assembled_sheet_tests {
    //! Runtime-assembled sheets (`StyleSheet::premint_as`) — the path
    //! idea-theme's component sheets take, which has no `stylesheet!`
    //! expansion site to hang a link-time registration on.

    use super::*;
    use runtime_core::{Color, StyleApplication, StyleRules, TextAlign, VariantSet};
    use std::rc::Rc;

    /// A sheet shaped like idea-theme's: assembled at runtime, two
    /// author axes with defaults, one state overlay.
    fn assembled() -> Rc<StyleSheet> {
        StyleSheet::new(|_vs: &VariantSet| StyleRules {
            color: Some(Color("#111111".into()).into()),
            ..Default::default()
        })
        .variant("kind", "h1", |_vs| StyleRules {
            font_weight: Some(runtime_core::FontWeight::Bold),
            ..Default::default()
        })
        .variant("kind", "body", |_vs| StyleRules::default())
        .variant("align", "left", |_vs| StyleRules {
            text_align: Some(TextAlign::Left),
            ..Default::default()
        })
        .variant("align", "center", |_vs| StyleRules {
            text_align: Some(TextAlign::Center),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules {
            color: Some(Color("#222222".into()).into()),
            ..Default::default()
        })
        .variant_default("kind", "body")
        .variant_default("align", "left")
        .premint_as("test.assembled.v1")
    }

    /// THE cross-half invariant: every class the shipped runtime stamps
    /// must have a rule in the CSS the dump emitted.
    ///
    /// The two halves are separately-compiled binaries joined by nothing
    /// but the class-name format, so a drift here is invisible until an
    /// app renders unstyled in production. Both sides go through
    /// `StyleApplication::preminted_class_list` / `dump_sheet_parts`, and
    /// this asserts they actually meet.
    #[test]
    fn every_runtime_stamped_class_has_a_dumped_rule() {
        let sheet = assembled();
        let mut css = String::new();
        let mut fonts = FontCollector::default();
        dump_sheet_parts(sheet.premint_class().unwrap(), &sheet, &mut css, &mut fonts);

        // Two call sites: one setting both axes, one relying on defaults.
        let apps = [
            StyleApplication::new(Rc::clone(&sheet)).with("kind", "h1").with("align", "center"),
            StyleApplication::new(Rc::clone(&sheet)),
        ];
        for app in apps {
            let class_list = app.preminted_class_list().expect("sheet premints");
            for class in class_list.split(' ') {
                assert!(
                    css.contains(&format!(".{class} ")) || css.contains(&format!(".{class}:")),
                    "runtime stamps `{class}` but the dump emitted no rule for it.\n\
                     class list: {class_list}\nCSS:\n{css}"
                );
            }
        }
    }

    /// An axis the call site omits still contributes its DEFAULT class —
    /// the live resolver applies the default arm, so dropping it here
    /// would silently lose a layer (the `align-left` / `kind-body` rules
    /// on every unset Typography).
    #[test]
    fn unset_axes_contribute_their_declared_default() {
        let sheet = assembled();
        let base = sheet.premint_class().unwrap();
        let class = StyleApplication::new(Rc::clone(&sheet))
            .with("kind", "h1")
            .preminted_class_list()
            .unwrap();
        assert_eq!(class, format!("{base} {base}-align-left {base}-kind-h1"));
    }

    /// Overlay axes are stamped as pseudo-class CSS on the BASE class,
    /// never as their own class — `__state_hovered` must not appear.
    #[test]
    fn overlay_axes_are_not_stamped_as_classes() {
        let sheet = assembled();
        let class = StyleApplication::new(Rc::clone(&sheet)).preminted_class_list().unwrap();
        assert!(!class.contains("__state"), "overlay axis leaked into the class list: {class}");
        let mut css = String::new();
        dump_sheet_parts(
            sheet.premint_class().unwrap(),
            &sheet,
            &mut css,
            &mut FontCollector::default(),
        );
        assert!(css.contains(":where(:hover)"), "state overlay missing from CSS:\n{css}");
    }

    /// Runtime-valued layers have no build-time class: an application
    /// carrying overrides or a `with_computed` closure must fall through
    /// to the live engine rather than wear a class that names rules the
    /// dump never saw.
    #[test]
    fn runtime_valued_layers_refuse_to_premint() {
        let sheet = assembled();
        assert!(
            StyleApplication::new(Rc::clone(&sheet))
                .with_overrides(StyleRules {
                    color: Some(Color("#abcdef".into()).into()),
                    ..Default::default()
                })
                .preminted_class_list()
                .is_none(),
            "an application with overrides must not premint"
        );
        assert!(
            StyleApplication::new(Rc::clone(&sheet))
                .with_computed("k", || StyleRules::default())
                .preminted_class_list()
                .is_none(),
            "an application with a computed layer must not premint"
        );
    }

    /// A sheet that never opted in has no build-time CSS, so it must not
    /// produce a class at all.
    #[test]
    fn sheet_without_premint_as_produces_no_class() {
        let plain = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules::default()));
        assert!(StyleApplication::new(plain).preminted_class_list().is_none());
    }

    /// The identity is the ONLY thing tying the dump's class names to the
    /// runtime's, so distinct content must not collide — an app that
    /// registers an extra kind has to get its own CSS.
    #[test]
    fn distinct_identities_produce_distinct_classes() {
        let a = runtime_core::premint_class_name("idea-theme.v1.typography|h1,body|");
        let b = runtime_core::premint_class_name("idea-theme.v1.typography|h1,body,hero|");
        assert_ne!(a, b);
        assert!(a.starts_with("iy-") && a.len() == 15, "unexpected class shape: {a}");
    }

    /// A sheet shaped like idea-theme's Button: an appearance axis plus the
    /// per-appearance hover/press COMPOUND overlays `add_state_overlay_compounds`
    /// attaches.
    fn assembled_with_compounds() -> Rc<StyleSheet> {
        StyleSheet::new(|_vs: &VariantSet| StyleRules {
            color: Some(Color("#111111".into()).into()),
            ..Default::default()
        })
        .variant("appearance", "solid", |_vs| StyleRules {
            background: Some(Color("#0000ff".into()).into()),
            ..Default::default()
        })
        .variant("appearance", "ghost", |_vs| StyleRules {
            background: Some(Color("transparent".into()).into()),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules::default())
        .variant_default("appearance", "solid")
        .compound(
            vec![("appearance", "solid"), ("__state_hovered", "on")],
            |_vs| StyleRules {
                background: Some(Color("#000088".into()).into()),
                ..Default::default()
            },
        )
        .compound(
            vec![("appearance", "ghost"), ("__state_hovered", "on")],
            |_vs| StyleRules {
                background: Some(Color("#eeeeee".into()).into()),
                ..Default::default()
            },
        )
        .premint_as("test.compounds.v1")
    }

    /// A sheet with compounds must GET a premint class.
    ///
    /// `premint_as` used to bail on `!self.compounds.is_empty()` and return a
    /// classless sheet, which silently sent every application to the live
    /// engine. Button and IconButton were disqualified that way — the
    /// state-overlay helper attaches a hover and a press compound per
    /// appearance arm — and nothing surfaced it, because the sheet came back
    /// looking fine.
    #[test]
    fn regression_compound_sheet_still_gets_a_premint_class() {
        let sheet = assembled_with_compounds();
        assert!(
            sheet.premint_class().is_some(),
            "a sheet with compound variants must still premint"
        );
    }

    /// A compound lowers to a CSS COMPOUND SELECTOR over the per-axis classes
    /// the runtime already stamps — a state leg becoming that state's pseudo.
    #[test]
    fn compound_lowers_to_a_compound_selector() {
        let sheet = assembled_with_compounds();
        let base = sheet.premint_class().unwrap().to_string();
        let mut out = String::new();
        let mut fonts = FontCollector::default();
        dump_sheet_parts(&base, &sheet, &mut out, &mut fonts);

        assert!(
            out.contains(&format!(".{base}-appearance-solid:hover {{")),
            "solid+hover compound must lower to `.<base>-appearance-solid:hover`; got:\n{out}"
        );
        assert!(
            out.contains(&format!(".{base}-appearance-ghost:hover {{")),
            "ghost+hover compound must lower too; got:\n{out}"
        );
        assert!(
            out.contains("#000088"),
            "the compound's own rules must be emitted; got:\n{out}"
        );
    }

    /// The compound rule must come AFTER the single-axis arm rules, matching
    /// `resolve` (base → axes → compounds). Combined with its higher
    /// specificity — `.cls:hover` is (0,2,0) vs the arm's (0,1,0) — that makes
    /// the compound win its conflicting properties in the browser exactly as
    /// it does in the engine.
    #[test]
    fn compound_rules_follow_axis_rules_in_source_order() {
        let sheet = assembled_with_compounds();
        let base = sheet.premint_class().unwrap().to_string();
        let mut out = String::new();
        let mut fonts = FontCollector::default();
        dump_sheet_parts(&base, &sheet, &mut out, &mut fonts);

        let arm = out
            .find(&format!(".{base}-appearance-solid {{"))
            .expect("arm rule present");
        let compound = out
            .find(&format!(".{base}-appearance-solid:hover {{"))
            .expect("compound rule present");
        assert!(
            compound > arm,
            "compound must be emitted after the arm it overrides; got:\n{out}"
        );
    }

    /// THE cross-half invariant for compounds: the browser's winner for a
    /// (appearance=solid, hovered) element must be the engine's winner.
    ///
    /// Checked by resolving through the live engine and confirming the
    /// compound's value — not the arm's — is what CSS would also select,
    /// since the compound rule is both later in source order and higher in
    /// specificity.
    #[test]
    fn compound_winner_matches_the_live_engine() {
        let sheet = assembled_with_compounds();
        let vs = VariantSet::new()
            .with("appearance", "solid")
            .with("__state_hovered", "on");
        let resolved = sheet.resolve(&vs);
        assert_eq!(
            resolved.background,
            Some(Color("#000088".into()).into()),
            "engine: the compound beats the appearance arm"
        );

        // And the emitted CSS gives the compound both later source order and
        // higher specificity, so the browser reaches the same value.
        let base = sheet.premint_class().unwrap().to_string();
        let mut out = String::new();
        let mut fonts = FontCollector::default();
        dump_sheet_parts(&base, &sheet, &mut out, &mut fonts);
        let arm_rule = format!(".{base}-appearance-solid {{");
        let compound_rule = format!(".{base}-appearance-solid:hover {{");
        assert!(out.find(&compound_rule) > out.find(&arm_rule));
        // (0,2,0) vs (0,1,0): the compound selector carries one class + one
        // pseudo, and — unlike the plain state delta — is NOT :where()-wrapped.
        assert!(
            !out.contains(&format!(".{base}-appearance-solid:where(:hover)")),
            "the compound must keep its specificity; :where() would shed it \
             and let the later arm rules win instead. got:\n{out}"
        );
    }
}
