//! `Checkbox` — a square box with a checkmark, plus an optional label.
//!
//! ```ignore
//! ui! {
//!     Checkbox(
//!         label = Some("I agree to the terms".into()),
//!         value = agreed,
//!         on_change = move |v: bool| agreed.set(v),
//!         tone = tone::Primary,
//!     )
//! }
//! ```
//!
//! Drawn from primitives (`pressable` box + checkmark `text`, inside a
//! tappable `view` row when there's a label) so it shares the `tone` ×
//! `variant` × `size` axes with the rest of idea-ui. The box is the
//! pressable — it takes keyboard focus and wears the focus ring on its
//! own, while the row exists only to lay out and tap-forward the label.
//! The box's selected fill is the tone/variant render
//! (`variant::Filled` → solid, `Soft` → tint, `Outlined` → bordered);
//! unselected it's a muted outline. Override the appearance via
//! `install_checkbox_sheets(CheckboxSheetBuilder::new().add_tone(Hype).build())`.

use std::rc::Rc;

use runtime_core::{
    accessibility::Role, component, icon, resolve_style, tap, ui, Element, IconData,
    IdealystSchema, IntoElement, Reactive, Signal, StyleApplication, TapRecognizer,
};

use idea_theme::extensible::{installed_checkbox_sheets, ToneRef, VariantRef};

use crate::components::ControlSize;
use crate::stylesheets::{ControlRow, FieldLabel};

/// Unicode check mark glyph rendered in the box when checked.
const CHECK_GLYPH: &str = "\u{2713}";

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (tone/variant/size/icon), so a `ui!` call site can pass a
// `Signal`/`rx!` and have it re-style in place. The controlled `value`
// `Signal` stays bare (a reactive *source*), `on_change` is a handler, and
// `label` is already `Reactive`.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct CheckboxProps {
    /// Optional label rendered to the right of the box.
    /// `Reactive<Option<String>>` — static or live.
    pub label: Reactive<Option<String>>,
    /// Controlled checked state. The host owns the signal.
    pub value: Signal<bool>,
    /// Fires with the new value when the user toggles the box.
    pub on_change: Rc<dyn Fn(bool)>,
    /// Semantic palette for the checked fill. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton for the checked fill. Default Filled.
    pub variant: VariantRef,
    /// Box scale. Default Md.
    pub size: ControlSize,
    /// Optional custom checked-state icon, shown in place of the default
    /// checkmark glyph (e.g. `icons_lucide::CHECK` or a task-specific mark).
    /// Inherits the checkmark's foreground color. `None` = the default ✓.
    pub icon: Option<IconData>,
    /// Optional robot/E2E test id, forwarded to the interactive box (the
    /// pressable that owns the press + focus). Only honored when idea-ui's
    /// `robot` feature is on; ignored otherwise.
    pub test_id: Option<&'static str>,
}

impl Default for CheckboxProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            value: runtime_core::signal(false),
            on_change: Rc::new(|_| {}),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ControlSize::default()),
            icon: Reactive::Static(None),
            test_id: None,
        }
    }
}

/// Renders a tappable row: a tone/variant-styled box that shows a
/// checkmark when `value` is true, plus the optional `label`. Tapping
/// anywhere on the row fires `on_change` with the toggled value.
#[component]
pub fn Checkbox(props: &CheckboxProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size.clone();
    let icon_data = props.icon.clone();

    let sheets = installed_checkbox_sheets();

    // Style keys read LIVE from the reactive props so the apply-style Effect
    // subscribes to whichever are dynamic; bare props collapse to one static
    // resolution (no per-node Effect, no first-paint flicker).
    let appearance_for = {
        let tone = tone.clone();
        let variant = variant.clone();
        move || format!("{}_{}", tone.get().key(), variant.get().key())
    };
    let size_key_for = {
        let size = size.clone();
        move || size.get().as_variant_str().to_string()
    };

    // Checkmark — mounted only while checked, tinted to the variant
    // foreground by the glyph sheet's appearance arm. A custom `icon`
    // replaces the default ✓ glyph, inheriting the same foreground. The
    // switch re-runs on `value`; the appearance/size/icon keys are read live
    // inside so the glyph re-styles when those props change too.
    let glyph_sheet = sheets.glyph_sheet.clone();
    let glyph_appearance_for = appearance_for.clone();
    let glyph_size_for = size_key_for.clone();
    let glyph_icon = icon_data.clone();
    let glyph = runtime_core::switch(
        move || value.get(),
        move |on: &bool| {
            if !*on {
                return ui! { view {} }.into_element();
            }
            let gs = glyph_sheet.clone();
            let ga = glyph_appearance_for.clone();
            let gz = glyph_size_for.clone();
            match glyph_icon.get() {
                Some(data) => {
                    // Resolve the checkmark foreground and stamp it on the icon
                    // (native icons don't inherit text color — see Button).
                    let fg = resolve_style(
                        &StyleApplication::new(gs).with("appearance", ga()).with("size", gz()),
                    )
                    .color
                    .clone();
                    let el = icon(data).size(14.0);
                    match fg {
                        Some(c) => el.color(move || c.resolve()).into_element(),
                        None => el.into_element(),
                    }
                }
                None => runtime_core::text(CHECK_GLYPH)
                    .with_style(move || {
                        StyleApplication::new(gs.clone())
                            .with("appearance", ga())
                            .with("size", gz())
                    })
                    .into_element(),
            }
        },
    );

    // The box — fill flips between the tone appearance (checked) and
    // the muted outline (unchecked) via the `checked` axis. Appearance/size
    // are read live inside so a reactive tone/variant/size re-styles the box.
    //
    // The BOX is the pressable, not the row: it's the keyboard-focusable
    // host, so the sheet's `__state_focused` ring draws around the square
    // alone. (A pressable row rings box *and* label — a stray border around
    // the text, which is not what a focus indicator should look like.)
    let toggle: Rc<dyn Fn()> = Rc::new(move || (on_change)(!value.get()));
    let box_sheet = sheets.box_sheet.clone();
    let box_appearance_for = appearance_for;
    let box_size_for = size_key_for;
    let box_toggle = toggle.clone();
    let box_el = runtime_core::pressable(vec![glyph], move || (box_toggle)())
        .with_style(move || {
            StyleApplication::new(box_sheet.clone())
                .with("appearance", box_appearance_for())
                .with("checked", if value.get() { "on" } else { "off" }.to_string())
                .with("size", box_size_for())
        })
        .a11y_role(Role::Checkbox);
    // The label sits outside the pressable now, so the box can't derive its
    // accessible name from child content — name it explicitly. Snapshot: a
    // `Reactive` label's later values don't re-announce (the a11y prop bag is
    // plain data, not reactive), which matches every other component here.
    let box_el = match props.label.get() {
        Some(text) => box_el.a11y_label(text),
        None => box_el,
    };
    // Forward the test id to the interactive box for robot/E2E location.
    // Gated: `.test_id()` only exists under `runtime-core/robot`.
    #[cfg(feature = "robot")]
    let box_el = match props.test_id {
        Some(tid) => box_el.test_id(tid),
        None => box_el,
    };
    let box_el = box_el.into_element();

    let Some(label) = crate::components::optional_reactive_text(props.label.clone(), FieldLabel())
    else {
        // No label — the box IS the whole control; skip the wrapper row.
        return box_el;
    };

    // Row: a plain layout view, tappable so clicking the label still toggles
    // (the HTML `<label for=…>` affordance). The box's own press recognizer
    // consumes its taps before they reach this handler, so a tap on the
    // square toggles exactly once.
    //
    // Builder form, not `ui!`: the `ui!` `view` emitter takes only
    // `style`/`test_id`/a11y props and DROPS anything else, so an
    // `on_touch = …` attribute there would silently never attach.
    let row_tap = tap(TapRecognizer::new(), move || (toggle)());
    runtime_core::view(vec![box_el, label])
        .with_style(|| StyleApplication::new(ControlRow::sheet()))
        .on_touch(move |ev| row_tap(ev))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::accessibility::Role;

    fn has_focus_arm(app: &StyleApplication) -> bool {
        app.sheet
            .variant_keys()
            .iter()
            .any(|(axis, _)| axis == "__state_focused")
    }

    /// The focus ring must ring the BOX, not the box+label row. Before this,
    /// the row was the `pressable` and `ControlRow` carried the
    /// `state focused` border, so tabbing to a checkbox drew a border around
    /// the label text too.
    #[test]
    fn regression_focus_ring_rings_the_box_not_the_label_row() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let props = CheckboxProps {
                label: Reactive::Static(Some("I agree".into())),
                ..Default::default()
            };
            let (row_children, row_style, row_tap) = match classify(Checkbox(&props)) {
                P::View {
                    children,
                    style,
                    on_touch,
                    ..
                } => (children, style, on_touch),
                _ => panic!("a labelled Checkbox renders a plain View row"),
            };
            assert!(row_tap, "the row still toggles when the label is clicked");
            let row_app = row_style
                .expect("the row is styled by ControlRow")
                .application();
            assert!(
                !has_focus_arm(&row_app),
                "the label row declares no focus overlay — it is not the focus target"
            );

            let (box_style, a11y) = match classify(row_children.into_iter().next().unwrap()) {
                P::Pressable {
                    style,
                    accessibility,
                    ..
                } => (style, accessibility),
                _ => panic!("the box is the Pressable (the focusable host)"),
            };
            assert!(
                has_focus_arm(&box_style.expect("the box is styled").application()),
                "the box's own sheet carries the focus ring"
            );
            // The label is outside the pressable now, so the box names itself.
            assert_eq!(a11y.role, Some(Role::Checkbox));
            assert_eq!(a11y.label.as_deref(), Some("I agree"));
        });
    }

    /// Pressing the box reports the toggled value exactly once.
    #[test]
    fn pressing_the_box_toggles_the_value() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let value = runtime_core::signal(false);
            let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
            let sink = seen.clone();
            let props = CheckboxProps {
                label: Reactive::Static(Some("I agree".into())),
                value,
                on_change: Rc::new(move |v: bool| sink.borrow_mut().push(v)),
                ..Default::default()
            };
            let row = match classify(Checkbox(&props)) {
                P::View { children, .. } => children,
                _ => panic!("labelled Checkbox renders a row"),
            };
            match classify(row.into_iter().next().unwrap()) {
                P::Pressable { on_click, .. } => on_click(),
                _ => panic!("the box is the Pressable"),
            }
            assert_eq!(&*seen.borrow(), &[true]);
        });
    }

    /// With no label there is nothing to lay out beside the box, so the
    /// wrapper row is skipped entirely.
    #[test]
    fn unlabelled_checkbox_is_the_bare_box() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let props = CheckboxProps::default();
            assert!(
                matches!(classify(Checkbox(&props)), P::Pressable { .. }),
                "an unlabelled Checkbox is the box pressable itself"
            );
        });
    }
}
