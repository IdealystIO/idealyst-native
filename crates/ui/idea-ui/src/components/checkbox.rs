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
//! unselected it's a muted outline. Styling is [`CheckboxStyles`]: `indicator` (the box),
//! `mark` (the ✓ glyph), `mark_icon` (a custom checked icon), `row` and
//! `label`.
//!
//! `disabled` follows `Switch` (and `Button`): it blocks the toggle through
//! the box pressable's own disabled binding (so `on_change` never fires, and
//! the host gets the native/a11y disabled state), guards the label row's
//! tap the same way, and dims the box via the sheet's `dimmed` axis.

use std::rc::Rc;

use runtime_core::{
    accessibility::Role, component, component_styles, derived, icon, resolve_style, tap, ui, AlignItems, Cursor,
    Element, FlexDirection, FontWeight, IconData, IdealystSchema, IntoElement, JustifyContent, Reactive, Signal,
    SlotRestyle, SlotStyle, StyleRules, TapRecognizer, TextAlign,
};

use idea_theme::extensible::{semantic_tones, ToneRef};
use idea_theme::theme::IdeaTheme;

use crate::components::DISABLED_CONTROL_OPACITY;
use crate::components::tone_slots::{focus_ring_compounds, recipe_compounds, with_tone};
use crate::components::ControlSize;
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;

/// Unicode check mark glyph rendered in the box when checked.
const CHECK_GLYPH: &str = "\u{2713}";

component_styles! {
    /// The styled sections of [`Checkbox`].
    pub Checkbox<IdeaThemeRef> {
        /// The checked fill — a tone-aware recipe per value.
        axis variant { #[default] filled, soft, outlined, ghost }

        /// The square — the pressable, so it takes keyboard focus and wears
        /// the focus ring on its own.
        slot indicator {
            extend(indicator_fills)
            base(t) {
                // Fixed-size control: never flex-shrink (see Switch).
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border_radius: t.radius.sm(),
            }
            /// Unchecked: a muted outline. Checked: the `(tone, variant)`
            /// fill (`box_fills`).
            variant checked {
                #[default]
                off(t) {
                    background: runtime_core::Color("transparent".into()),
                    color: t.color.text_muted(),
                    border_width: 1.0,
                    border_color: t.color.border(),
                }
                on(_t) {}
            }
            variant size {
                sm(_t) { width: 16.0, height: 16.0 }
                #[default]
                md(_t) { width: 20.0, height: 20.0 }
                lg(_t) { width: 24.0, height: 24.0 }
            }
            /// The author's `disabled`: on the box, so the mark inside dims
            /// with it.
            variant dimmed {
                #[default]
                off(_t) {}
                on(_t) { opacity: DISABLED_CONTROL_OPACITY }
            }
            transitions {
                background: 150ms EaseOut,
                border_color: 150ms EaseOut,
            }
        }

        /// The ✓ glyph (mounted only while checked), in the fill's
        /// foreground.
        slot mark {
            extend(mark_colors)
            base(_t) {
                font_weight: FontWeight::Bold,
                text_align: TextAlign::Center,
            }
            variant size {
                sm(_t) { font_size: runtime_core::Length::Px(11.0), line_height: 11.0 }
                #[default]
                md(_t) { font_size: runtime_core::Length::Px(14.0), line_height: 14.0 }
                lg(_t) { font_size: runtime_core::Length::Px(17.0), line_height: 17.0 }
            }
        }

        /// A custom checked icon: a 14px square (icons have no intrinsic
        /// size) in the fill's foreground.
        slot mark_icon {
            extend(mark_colors)
            base(_t) {
                width: 14.0,
                height: 14.0,
                flex_shrink: 0.0,
            }
        }

        /// Box + label, vertically centered — a plain layout view; the box
        /// inside is the focus target.
        slot row {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
                cursor: Cursor::Pointer,
            }
        }

        /// The label.
        slot label {
            base(t) {
                color: t.color.text(),
                font_size: t.typography.body_sm_size(),
                font_weight: FontWeight::Medium,
            }
        }
    }
}

/// The checked fills and, above them, the 2px focus ring.
fn indicator_fills(s: &mut SlotRestyle<'_, CheckboxStyles>) {
    let tones = semantic_tones();
    recipe_compounds(s, &tones, &[("checked", "on")], |ctx, recipe| recipe.render(ctx));
    focus_ring_compounds(s, &tones, &[("checked", "on")], 2.0);
}

fn mark_colors(s: &mut SlotRestyle<'_, CheckboxStyles>) {
    recipe_compounds(s, &semantic_tones(), &[], |ctx, recipe| StyleRules {
        color: recipe.render(ctx).color,
        ..Default::default()
    });
}

marker_into_axis!(CheckboxVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

/// The control-wide size scale converts into Checkbox's own size axis.
impl From<ControlSize> for CheckboxSize {
    fn from(s: ControlSize) -> Self {
        CheckboxSize::define(s.as_variant_str())
    }
}

impl From<ControlSize> for Reactive<CheckboxSize> {
    fn from(s: ControlSize) -> Self {
        Reactive::Static(s.into())
    }
}

/// Builds the checkmark node for a custom `IconData` glyph, tinted to the
/// `mark_icon` slot's per-(tone, variant) foreground.
///
/// When the slot ATTACHES PREMINTED (a `--premint`/`--premint-only` web
/// build), it is attached to the icon node as is: its preminted class
/// carries the arm's `color`, and the SVG tints via `currentColor`. A
/// resolve-read there would be the `--premint-only` panic (sheets carry no
/// rule closures). Otherwise — native builds, live web builds — the
/// foreground is resolved and stamped on the icon explicitly, because
/// native icons don't inherit text color (see Button).
fn checkmark_icon(data: IconData, style: SlotStyle<CheckboxStyles>, again: SlotStyle<CheckboxStyles>) -> Element {
    let app = again.into_style_application();
    if app.attaches_preminted() {
        return icon(data).with_style(style).into_element();
    }
    let el = icon(data).with_style(style);
    match resolve_style(&app).color.clone() {
        Some(c) => el.color(move || c.resolve()).into_element(),
        None => el.into_element(),
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (tone/variant/size/icon/disabled), so a `ui!` call site can pass a
// `Signal`/`rx!` and have it re-style in place. The controlled `value`
// `Signal` stays bare (a reactive *source*), `on_change` is a handler, and
// `label` is already `Reactive`.
#[runtime_core::props(styles = CheckboxStyles)]
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
    /// Surface skeleton for the checked fill (or an app value). Default
    /// Filled.
    pub variant: CheckboxVariant,
    /// Box scale (a `ControlSize` converts). Default Md.
    pub size: CheckboxSize,
    /// When `true`, blocks the toggle (`on_change` never fires, from the box
    /// or the label row) and dims the box — the same opacity drop a disabled
    /// `Button` gets — and marks the box disabled for the host (native
    /// disabled state / a11y). Default `false`. Reactive: pass a
    /// `Signal<bool>`/`rx!` and the checkbox enables and disables in place.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub disabled: bool,
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
            variant: Reactive::Static(CheckboxVariant::default()),
            size: Reactive::Static(CheckboxSize::default()),
            disabled: Reactive::Static(false),
            icon: Reactive::Static(None),
            test_id: None,
            styles: Default::default(),
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
    let icon_data = props.icon.clone();
    let styles = props.styles.clone();
    let values = with_tone(CheckboxStyles::values().set(props.variant.clone()).set(props.size.clone()), &props.tone);

    // Checkmark — mounted only while checked, tinted to the fill's
    // foreground by the `mark` slot. A custom `icon` replaces the default ✓
    // glyph, inheriting the same foreground. The `if` re-runs `mark` each
    // time `value` turns true.
    //
    // The icon choice stays a plain Rust `match` in `mark`, outside the
    // macro: `IconData` is not `PartialEq`, and a `match` on a live read
    // inside `ui!` lowers to a value-keyed `switch`, which requires it.
    let mark = {
        let values = values.clone();
        let styles = styles.clone();
        move || -> Element {
            match icon_data.get() {
                Some(data) => {
                    let slot = || CheckboxStyles::mark_icon().values(&values).overridden(styles.mark_icon.as_ref());
                    checkmark_icon(data, slot(), slot())
                }
                None => runtime_core::text(CHECK_GLYPH)
                    .with_style(CheckboxStyles::mark().values(&values).overridden(styles.mark.as_ref()))
                    .into_element(),
            }
        }
    };
    let glyph = ui! {
        if value.get() {
            mark()
        }
    };

    // The box — fill flips between the tone appearance (checked) and
    // the muted outline (unchecked) via the `checked` axis. Appearance/size
    // are read live inside so a reactive tone/variant/size re-styles the box.
    //
    // The BOX is the pressable, not the row: it's the keyboard-focusable
    // host, so the sheet's `__state_focused` ring draws around the square
    // alone. (A pressable row rings box *and* label — a stray border around
    // the text, which is not what a focus indicator should look like.)
    //
    // `toggle` also serves the label row's tap, which has no disabled
    // binding of its own (a plain view, not a pressable) — so it checks the
    // live `disabled` itself. On the box the pressable's binding has
    // already blocked the press before this runs.
    let toggle_disabled = props.disabled.clone();
    let toggle: Rc<dyn Fn()> = Rc::new(move || {
        if !toggle_disabled.get() {
            (on_change)(!value.get())
        }
    });
    // `disabled` is read LIVE in the style so the dim follows a reactive
    // prop in place (the `dimmed` axis — Switch/Button parity).
    let box_style = CheckboxStyles::indicator()
        .values(&values)
        .set(derived(move || CheckboxChecked::from(value.get())))
        .set(props.disabled.clone().map(CheckboxDimmed::from))
        .overridden(styles.indicator.as_ref());
    let box_toggle = toggle.clone();
    let box_el = runtime_core::pressable(vec![glyph], move || (box_toggle)())
        .with_style(box_style)
        .a11y_role(Role::Checkbox);
    // Block the press through the pressable's own `disabled` binding (see
    // Switch): press block, host `set_disabled`, DISABLED state bit.
    // Attached only when the checkbox can be disabled — a `Static(false)`
    // checkbox carries no binding, like Button.
    let box_el = match props.disabled.clone() {
        Reactive::Static(false) => box_el,
        Reactive::Static(true) => box_el.disabled(true),
        live => box_el.disabled(move || live.get()),
    };
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

    let label_style = CheckboxStyles::label().values(&values).overridden(styles.label.as_ref());
    let Some(label) = crate::components::optional_reactive_text(props.label.clone(), label_style)
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
        .with_style(CheckboxStyles::row().values(&values).overridden(styles.row.as_ref()))
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
    use runtime_core::StyleApplication;
    use runtime_core::FillRule;

    const CUSTOM_MARK: IconData = IconData {
        view_box: (24, 24),
        paths: &["M20 6L9 17l-5-5"],
        fill_rule: FillRule::NonZero,
        filled: false,
    };

    // The `--premint-only` read-back: the custom checkmark icon resolved the
    // glyph sheet's foreground in Rust and stamped it. On a premint build the
    // glyph application must be ATTACHED to the icon instead — its preminted
    // class carries the arm's `color` and the SVG tints via `currentColor`
    // (a resolve-read is the --premint-only panic). On live/native builds the
    // resolved stamp must remain, because native icons don't inherit color.
    #[test]
    fn regression_premint_checkmark_icon_tints_via_glyph_class() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let slot = || CheckboxStyles::mark_icon().raw("tone", "primary").set(CheckboxVariant::Filled);
            let (color, style) = match classify(checkmark_icon(CUSTOM_MARK, slot(), slot())) {
                P::Icon { color, style, .. } => (color, style),
                _ => panic!("checkmark_icon builds an Icon node"),
            };
            #[cfg(idealyst_premint)]
            {
                assert!(
                    color.is_none(),
                    "premint build: no stamped color — the tint is `currentColor`"
                );
                match style.expect("premint build: the glyph application rides the icon") {
                    TStyle::Preminted { class, .. } => {
                        assert!(
                            class.contains("variant-filled") && class.contains("tone-primary"),
                            "the stamped class list selects the tone and variant (got `{class}`)"
                        );
                    }
                    _ => panic!("the glyph application attaches as a preminted class stamp"),
                }
            }
            #[cfg(not(idealyst_premint))]
            {
                assert!(
                    color.is_some(),
                    "live/native build: the resolved foreground is stamped on the icon"
                );
                let rules = style.expect("the mark_icon slot styles the icon").resolve();
                assert_eq!(
                    rules.width,
                    Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(14.0))),
                    "the slot carries the checkmark square"
                );
            }
        });
    }

    fn has_focus_arm(app: &StyleApplication) -> bool {
        app.sheet
            .variant_keys()
            .iter()
            .any(|(axis, _)| axis == "__state_focused")
    }

    /// The focus ring must ring the BOX, not the box+label row. Before this,
    /// the row was the `pressable` and the row style carried the
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
                .expect("the row is styled by the row slot")
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

    /// Drive one tap (touch down + up in place) through a view's real
    /// `on_touch` handler.
    fn tap_row(row: Element) {
        use runtime_core::{TouchEvent, TouchId, TouchPhase, TouchPoint};
        let mut el = row;
        while let Element::Owned { element, .. } = el {
            el = *element;
        }
        let handler = match el {
            Element::Item { data, .. } => data
                .downcast_ref::<runtime_vocabulary::prims::PrimCell<runtime_vocabulary::prims::ViewPrim>>()
                .expect("a labelled Checkbox renders a View row")
                .take()
                .on_touch
                .expect("the row carries on_touch"),
            _ => panic!("a labelled Checkbox renders a View item"),
        };
        for phase in [TouchPhase::Began, TouchPhase::Ended] {
            handler(&TouchEvent {
                id: TouchId(1),
                phase,
                position: TouchPoint::new(1.0, 1.0),
                window_position: TouchPoint::new(1.0, 1.0),
                timestamp_ns: 0,
                force: None,
            });
        }
    }

    fn row_tap_hits(disabled: Reactive<bool>) -> u32 {
        let hits = Rc::new(std::cell::Cell::new(0u32));
        with_test_world(|| {
            install_idea_theme(light_theme());
            let sink = hits.clone();
            let props = CheckboxProps {
                label: Reactive::Static(Some("I agree".into())),
                on_change: Rc::new(move |_v: bool| sink.set(sink.get() + 1)),
                disabled,
                ..Default::default()
            };
            tap_row(Checkbox(&props));
        });
        hits.get()
    }

    /// Regression guard for `disabled`: the label row is a plain view with
    /// its own tap recognizer, OUTSIDE the box pressable's disabled
    /// binding — so without its own check, tapping the label of a disabled
    /// Checkbox still fired `on_change`.
    #[test]
    fn regression_disabled_checkbox_label_row_tap_does_not_fire() {
        assert_eq!(row_tap_hits(Reactive::Static(false)), 1, "an enabled row tap fires");
        assert_eq!(row_tap_hits(Reactive::Static(true)), 0, "a disabled row tap is ignored");
    }
}
