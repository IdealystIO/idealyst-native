//! Premint style-dump assembly — stage 3 of the preminted-styles
//! pipeline (see `runtime_core::premint` for the collection side and
//! `StyleSource::Preminted` for the runtime side).
//!
//! [`dump_all_css`] walks every sheet registered in
//! `runtime_core::premint::PREMINT_SHEETS`, enumerates each sheet's
//! full variant space, resolves every combo through the SAME layer
//! resolution the web backend uses at runtime
//! (`premint::resolve_layers`), and emits rule groups through the SAME
//! builder (`css::class_rule_group`) — so a preminted rule body is
//! byte-identical in semantics to what the live engine would have
//! minted for the same styles.
//!
//! Class naming mirrors the `stylesheet!` runtime branch exactly:
//! `<base_class>` plus one `-<value>` segment per axis in declaration
//! order; an axis with no `#[default]` arm contributes `_` for the
//! unset combo (emitted here in addition to every declared value).

use runtime_core::premint::{PremintSheet, PREMINT_SHEETS};
use runtime_core::StyleApplication;

/// Assemble the full preminted stylesheet for every registered sheet.
/// Deterministic given the same binary — the CLI fingerprints the
/// output like any other asset.
pub fn dump_all_css() -> String {
    let mut out = String::new();
    for sheet in PREMINT_SHEETS {
        dump_sheet(sheet, &mut out);
    }
    out
}

fn dump_sheet(sheet: &PremintSheet, out: &mut String) {
    // Per-axis value lists for enumeration. An axis with no `#[default]`
    // arm gets the `_` pseudo-value prepended: the runtime emits `_` for
    // that axis when unset, and "unset" resolves differently from every
    // declared value (no overlay at all), so it needs its own class.
    // With a default arm, "unset" and the explicit default resolve to
    // the same rules, so the default value's class covers both.
    let value_sets: Vec<Vec<&'static str>> = sheet
        .axes
        .iter()
        .map(|axis| {
            let mut vals: Vec<&'static str> = Vec::with_capacity(axis.values.len() + 1);
            if axis.default_value == "_" {
                vals.push("_");
            }
            vals.extend_from_slice(axis.values);
            vals
        })
        .collect();

    // Odometer over the cartesian product (a no-axis sheet yields the
    // single base-only combo).
    let mut idx = vec![0usize; value_sets.len()];
    loop {
        let mut app = StyleApplication::new((sheet.sheet)());
        let mut class = String::from(sheet.base_class);
        for (axis_i, axis) in sheet.axes.iter().enumerate() {
            let val = value_sets[axis_i][idx[axis_i]];
            class.push('-');
            class.push_str(val);
            if val != "_" {
                app = app.with(axis.name, val);
            }
        }

        let (base, states, bps, cqs) = runtime_core::premint::resolve_layers(&app);
        for rule in css::class_rule_group_with(&class, &base, &states, &bps, &cqs, emit_rules) {
            out.push_str(&rule);
            out.push('\n');
        }

        // Increment the odometer; done when it wraps.
        let mut carry = true;
        for (i, slot) in idx.iter_mut().enumerate() {
            if !carry {
                break;
            }
            *slot += 1;
            if *slot < value_sets[i].len() {
                carry = false;
            } else {
                *slot = 0;
            }
        }
        if carry {
            break;
        }
    }
}

/// Rule-body lowering for preminted classes: [`css::rules_to_css`] plus
/// the theme default-font hook. At runtime, the live engine fills an
/// absent `font_family` with the installed theme's default text font at
/// apply time (`with_default_text_font`) — but the dump runs in a build
/// step where no theme exists yet, so a rules layer that sets no font
/// gets `font-family: var(--iy-default-font, inherit)` instead. The
/// premint host driver defines that variable from the live theme
/// (`Backend::apply_default_text_font`); when no default font is
/// installed the `inherit` fallback reproduces plain cascade — exactly
/// the live engine's no-default behavior. Layers that DO set a font are
/// untouched (the author's explicit font always wins, as at runtime).
fn emit_rules(rules: &runtime_core::StyleRules) -> String {
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
    use runtime_core::premint::{PremintAxis, PremintSheet, PREMINT_SHEETS};
    use runtime_core::{stylesheet, Color};

    // A sheet with a variant axis + a state overlay, registered manually
    // with the exact entry shape the `stylesheet!` macro emits under
    // `cfg(idealyst_premint_dump)` — this test owns the assembly side
    // (enumeration, resolution, rule emission); the macro↔runtime class
    // agreement is covered by the CLI's premint e2e.
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
    static TEST_SHEET: PremintSheet = PremintSheet {
        base_class: "iy-test",
        sheet: chip_style,
        axes: &[PremintAxis {
            name: "tone",
            values: &["neutral", "danger"],
            default_value: "neutral",
        }],
    };

    #[test]
    fn dump_emits_every_variant_combo_with_state_rules() {
        let out = dump_all_css();
        assert!(out.contains(".iy-test-neutral { "), "neutral base rule; got:\n{out}");
        assert!(out.contains(".iy-test-danger { "), "danger base rule; got:\n{out}");
        assert!(out.contains("color: #ff0000"), "danger overlay folded in; got:\n{out}");
        assert!(
            out.contains(".iy-test-neutral:hover { ") && out.contains(".iy-test-danger:hover { "),
            "state overlays as pseudo-class rules on every combo; got:\n{out}"
        );
        assert!(out.contains("color: #222222"), "hover body present; got:\n{out}");
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
        PremintSheet { base_class: "iy-mono", sheet: mono_style, axes: &[] };

    /// Fontless rule bodies carry the theme-default hook; explicit fonts
    /// win untouched — mirrors the live engine's `with_default_text_font`
    /// fill exactly (see `emit_rules`).
    #[test]
    fn regression_fontless_rules_reference_default_font_var() {
        let out = dump_all_css();
        // Chip sets no font on any layer → every Chip rule body gets the var.
        let neutral = out
            .lines()
            .find(|l| l.starts_with(".iy-test-neutral { "))
            .expect("neutral rule present");
        assert!(
            neutral.contains("; font-family: var(--iy-default-font, inherit)"),
            "fontless base must reference the default-font var, correctly \
             separated from the preceding declaration; got:\n{neutral}"
        );
        // Mono sets an explicit font → raw value, no var.
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

    /// THE premint contract, in-repo: the class the shipped binary's
    /// builder assembles at runtime must appear in the CSS the dump
    /// emits — with no manifest linking them, only the shared source
    /// hash computed inside `stylesheet!`. Both cfgs are set for this
    /// crate by its build.rs (cargo can't set custom cfgs per-test),
    /// so the macro's runtime fast path AND its dump registration are
    /// simultaneously live here — exactly the shipped-build/dump-build
    /// pair the CLI produces.
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
                    assert!(
                        css.contains(&format!(".{class} {{")),
                        "runtime class {class} missing from dump css:\n{css}"
                    );
                    assert!(
                        css.contains(&format!(".{class}:hover {{")),
                        "hover rule for {class} missing from dump css:\n{css}"
                    );
                }
                _ => panic!("builder must emit Preminted under --cfg idealyst_premint"),
            }
        }
    }
}
