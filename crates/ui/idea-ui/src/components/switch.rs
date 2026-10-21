//! `Switch` — a styled slide-toggle: a tone-colored pill track with a
//! white thumb that slides between the off (left) and on (right)
//! edges.
//!
//! ```ignore
//! ui! {
//!     Switch(
//!         label = Some("Notifications".into()),
//!         value = on,
//!         on_change = move |v: bool| on.set(v),
//!         tone = tone::Success,
//!     )
//! }
//! ```
//!
//! Unlike the framework's raw `toggle` primitive (which renders the
//! platform-native switch), this is drawn from primitives — `pressable`
//! track + `view` thumb — so it carries the same `tone` × `variant` ×
//! `size` styling axes as the rest of idea-ui and looks identical on
//! every backend. Styling is [`SwitchStyles`]: `track` (the pill — the
//! tone × variant fill while on, a muted track while off), `thumb` (the
//! white puck), `thumb_icon`, `row` (track + label) and `label`. The
//! thumb's horizontal travel is animated via `AnimProp::TranslateX`.
//!
//! `disabled` follows `Button`: it blocks the toggle through the track
//! pressable's own disabled binding (so `on_change` never fires, and the
//! host gets the native/a11y disabled state) and dims the track via the
//! sheet's `dimmed` axis. The track never flex-shrinks (the sheet pins
//! `flex_shrink: 0`): the thumb's travel is a fixed distance, so a
//! squeezed track would leave the "on" thumb hanging off its end.

use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::{
    component, component_styles, derived, effect, icon, ui, AlignItems, Color, Cursor, Element, FlexDirection,
    FontWeight, IconData, IdealystSchema, IntoElement, JustifyContent, Reactive, Ref, Signal, SlotRestyle, ViewHandle,
};

use idea_theme::extensible::{semantic_tones, ToneRef};

use crate::components::DISABLED_CONTROL_OPACITY;
use crate::components::tone_slots::{focus_ring_compounds, recipe_compounds, with_tone};
use crate::components::ControlSize;
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;
use idea_theme::tokens;

component_styles! {
    /// The styled sections of [`Switch`].
    pub Switch<IdeaThemeRef> {
        /// The "on" fill — a tone-aware recipe per value.
        axis variant { #[default] filled, soft, outlined, ghost }

        /// The pill track (the pressable that toggles).
        slot track {
            extend(track_fills)
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                border_radius: t.radius.pill(),
                padding: 2.0,
                // Fixed-size control: never flex-shrink. In a row narrower
                // than its content the default shrink squeezes the track
                // below its width while the thumb's travel stays fixed, so
                // the thumb overshoots the track.
                flex_shrink: 0.0,
            }
            /// Off: a muted track, no border. On: the `(tone, variant)`
            /// fill (`track_fills`).
            variant checked {
                #[default]
                off(t) {
                    background: t.color.border(),
                    border_width: 0.0,
                }
                on(_t) {}
            }
            /// Track width × height.
            variant size {
                sm(_t) { width: 30.0, height: 18.0 }
                #[default]
                md(_t) { width: 38.0, height: 22.0 }
                lg(_t) { width: 48.0, height: 28.0 }
            }
            /// The author's `disabled`: a deterministic dim whether or not
            /// the host marks the node disabled.
            variant dimmed {
                #[default]
                off(_t) {}
                on(_t) { opacity: DISABLED_CONTROL_OPACITY }
            }
            transitions {
                background: 180ms EaseOut,
            }
        }

        /// The white puck. Diameter = track height − 4 (2px inset on each
        /// edge).
        slot thumb {
            base(t) {
                background: Color("#ffffff".into()),
                border_radius: t.radius.pill(),
                // Fixed-size: the slide travel assumes the declared diameter.
                flex_shrink: 0.0,
                // Center an optional thumb icon.
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                shadow: runtime_core::Shadow {
                    x: 0.0,
                    y: 1.0,
                    blur: 3.0,
                    color: Color("rgba(15, 17, 21, 0.30)".into()).into(),
                },
            }
            variant size {
                sm(_t) { width: 14.0, height: 14.0 }
                #[default]
                md(_t) { width: 18.0, height: 18.0 }
                lg(_t) { width: 24.0, height: 24.0 }
            }
        }

        /// The optional icon inside the thumb.
        slot thumb_icon {
            base(_t) { flex_shrink: 0.0 }
            variant size {
                sm(_t) { width: 9.0, height: 9.0 }
                #[default]
                md(_t) { width: 11.0, height: 11.0 }
                lg(_t) { width: 14.0, height: 14.0 }
            }
        }

        /// Label + track, vertically centered. No focus ring here: the
        /// focusable host is the track inside it.
        slot row {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
                cursor: Cursor::Pointer,
            }
        }

        /// The inline label.
        slot label {
            base(t) {
                color: t.color.text(),
                font_size: t.typography.body_sm_size(),
                font_weight: FontWeight::Medium,
            }
        }
    }
}

/// The "on" fills, and the focus ring above them (a 2px border in the
/// focus-ring color — the cross-platform indicator replacing the native
/// ring).
fn track_fills(s: &mut SlotRestyle<'_, SwitchStyles>) {
    let tones = semantic_tones();
    recipe_compounds(s, &tones, &[("checked", "on")], |ctx, recipe| recipe.render(ctx));
    focus_ring_compounds(s, &tones, &[("checked", "on")], 2.0);
}

marker_into_axis!(SwitchVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

/// The control-wide size scale converts into Switch's own size axis.
impl From<ControlSize> for SwitchSize {
    fn from(s: ControlSize) -> Self {
        SwitchSize::define(s.as_variant_str())
    }
}

impl From<ControlSize> for Reactive<SwitchSize> {
    fn from(s: ControlSize) -> Self {
        Reactive::Static(s.into())
    }
}

/// Duration of the thumb-slide / track-color animation.
const SWITCH_ANIM_MS: u64 = 180;

/// Thumb travel distance (px) per size — the width the thumb slides
/// from off to on. `track_width − thumb_diameter − 2·inset`, matching the
/// `track` and `thumb` size arms (inset = 2px each edge):
///   sm: 30 − 14 − 4 = 12   md: 38 − 18 − 4 = 16   lg: 48 − 24 − 4 = 20
/// An app-defined size travels `md`'s distance unless its arms keep that
/// geometry.
fn travel_for(size: SwitchSize) -> f32 {
    match size {
        SwitchSize::Sm => 12.0,
        SwitchSize::Lg => 20.0,
        _ => 16.0,
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (tone/variant/size/icon/disabled). The controlled `value` `Signal`
// stays bare (a reactive *source*), `on_change` is a handler, `label` is
// already `Reactive`. NOTE `size` ALSO drives structure (thumb travel
// distance + icon px feed the animation/layout), read once at build — only
// the track *style* re-resolves on a live tone/variant; see the body TODO.
#[runtime_core::props(styles = SwitchStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SwitchProps {
    /// Optional inline label rendered to the left of the track.
    /// `Reactive<Option<String>>` — static (`None`/`Some`) or live.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub label: Reactive<Option<String>>,
    /// Controlled bool state. The host owns the signal.
    pub value: Signal<bool>,
    /// Fires with the new value when the user flips the switch.
    pub on_change: Rc<dyn Fn(bool)>,
    /// Semantic palette for the "on" track fill. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton for the "on" track fill (or an app value). Default
    /// Filled.
    pub variant: SwitchVariant,
    /// Track + thumb scale (a `ControlSize` converts). Default Md.
    pub size: SwitchSize,
    /// When `true`, blocks the toggle (`on_change` never fires) and dims the
    /// track — the same opacity drop a disabled `Button` gets — and marks the
    /// track disabled for the host (native disabled state / a11y). Default
    /// `false`. Reactive: pass a `Signal<bool>`/`rx!` and the switch enables
    /// and disables in place.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub disabled: bool,
    /// Optional icon shown inside the thumb (e.g. a check/power glyph).
    /// Tinted with the muted text color so it reads on the white thumb.
    /// `None` = a plain thumb.
    pub icon: Option<IconData>,
    /// Optional robot/E2E test id, forwarded to the interactive track
    /// (the pressable that toggles). Only honored when idea-ui's `robot`
    /// feature is on; ignored otherwise.
    pub test_id: Option<&'static str>,
}

impl Default for SwitchProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            value: runtime_core::signal(false),
            on_change: Rc::new(|_| {}),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(SwitchVariant::default()),
            size: Reactive::Static(SwitchSize::default()),
            disabled: Reactive::Static(false),
            icon: Reactive::Static(None),
            test_id: None,
            styles: Default::default(),
        }
    }
}

/// Renders a controlled slide-toggle: a tone-colored pill track with a
/// thumb that animates between off (left) and on (right), with an
/// optional inline label.
#[component]
pub fn Switch(props: &SwitchProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    // TODO(reactive-sweep): `size` drives STRUCTURE here — the thumb travel
    // distance + icon px feed the animation (`AnimatedValue`) and thumb
    // layout, which can't re-derive without rebuilding those. Read once at
    // build; only the track *style* re-resolves on a live size below.
    let size = props.size.get();

    let styles = props.styles.clone();
    // The track's tone/variant are read LIVE so a reactive tone/variant
    // re-styles the track in place; `checked` flips it between the tone
    // fill and the muted off-track.
    let values = with_tone(SwitchStyles::values().set(props.variant.clone()).set(props.size.clone()), &props.tone);

    // --- thumb: a white puck whose TranslateX animates the slide ---
    let thumb_ref: Ref<ViewHandle> = Ref::new();
    let travel = travel_for(size);
    let av: AnimatedValue<f32> = AnimatedValue::new(if value.get() { travel } else { 0.0 });
    av.bind(thumb_ref, AnimProp::TranslateX);
    // Scope-adopted: the component's reactive scope owns this effect and
    // frees it on teardown, so the handle's drop is a no-op — no
    // `mem::forget` (a leak outside framework core).
    effect!({
        let target = if value.get() { travel } else { 0.0 };
        av.animate(TweenTo::new(target, Duration::from_millis(SWITCH_ANIM_MS)).ease_out());
    });

    // Optional glyph centered in the thumb, sized per step by the
    // `thumb_icon` slot and tinted with the ink text color so it reads on
    // the white thumb in both states.
    // TODO(reactive-sweep): a custom thumb `icon` is read once at build (it's
    // baked into the thumb's children); a reactive icon swap would need a
    // `switch` around the thumb contents. The common case is a fixed icon.
    let thumb_kids: Vec<Element> = match props.icon.get() {
        Some(data) => vec![icon(data)
            .with_style(SwitchStyles::thumb_icon().values(&values).overridden(styles.thumb_icon.as_ref()))
            .color(|| tokens().color.text().resolve())
            .into_element()],
        None => Vec::new(),
    };
    let thumb = runtime_core::view(thumb_kids)
        .with_style(SwitchStyles::thumb().values(&values).overridden(styles.thumb.as_ref()))
        .bind(thumb_ref)
        .into_element();

    // --- track: a pressable that toggles, styled by the checked axis ---
    // `disabled` is read LIVE here so the dim follows a reactive prop in
    // place (the `dimmed` axis — Button parity).
    let track_style = SwitchStyles::track()
        .values(&values)
        .set(derived(move || SwitchChecked::from(value.get())))
        .set(props.disabled.clone().map(SwitchDimmed::from))
        .overridden(styles.track.as_ref());
    let toggle = move || (on_change)(!value.get());
    let track = runtime_core::pressable(vec![thumb], toggle).with_style(track_style);
    // Block the press through the pressable's own `disabled` binding: the
    // mount handler wraps the callback in a press-block flag (mouse,
    // keyboard and programmatic activation alike), calls the host's
    // `set_disabled` (native disabled / a11y state) and flips the DISABLED
    // state bit. Attached only when the switch can be disabled — a
    // `Static(false)` switch carries no binding, like Button.
    let track = match props.disabled.clone() {
        Reactive::Static(false) => track,
        Reactive::Static(true) => track.disabled(true),
        live => track.disabled(move || live.get()),
    };
    // Forward the test id to the interactive track so a robot suite can
    // locate + click it — set on the BUILDER (`.test_id` on the pressable
    // wrapper). Gated: the id slot only registers under `robot`, and the
    // prop only exists there.
    #[cfg(feature = "robot")]
    let track = match props.test_id {
        Some(tid) => track.test_id(tid),
        None => track,
    };
    let track = track.into_element();

    let label_style = SwitchStyles::label().values(&values).overridden(styles.label.as_ref());
    match crate::components::optional_reactive_text(props.label.clone(), label_style) {
        Some(label) => {
            let row_style = SwitchStyles::row().values(&values).overridden(styles.row.as_ref());
            ui! {
                view(style = row_style) {
                    label
                    track
                }
            }
        }
        None => track,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    /// The track draws the `(tone, variant)` fill only while on, the muted
    /// track while off, and its focus ring over the fill when focused on:
    /// the fill is a compound (it outranks states), so the ring is one too.
    #[test]
    fn track_fill_follows_checked_and_the_focus_ring_wins_over_it() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let track = |checked: SwitchChecked| {
                SwitchStyles::track().raw("tone", "success").set(SwitchVariant::Filled).set(checked).into_style_application()
            };
            let on = resolve_style(&track(SwitchChecked::On));
            let off = resolve_style(&track(SwitchChecked::Off));
            assert_eq!(on.background.as_ref().and_then(|c| c.name()), Some("intent-success-solid-bg"));
            assert_eq!(off.background.as_ref().and_then(|c| c.name()), Some("color-border"));
            for checked in [SwitchChecked::On, SwitchChecked::Off] {
                let focused = resolve_style(&track(checked).with("__state_focused", "on"));
                assert_eq!(
                    focused.border_left_color.as_ref().and_then(|c| c.name()),
                    Some("color-focus-ring"),
                    "{checked:?}"
                );
                assert_eq!(focused.border_left_width.as_ref().map(|w| w.resolve()), Some(2.0));
            }
        });
    }
}
