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
//! every backend. The track fill comes from the installed Switch
//! stylesheet (override via
//! `install_switch_sheet(SwitchSheetBuilder::new().add_tone(Hype).build())`);
//! the thumb's horizontal travel is animated via `AnimProp::TranslateX`.
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
    component, effect, icon, ui, Element, IconData, IdealystSchema, IntoElement, Reactive,
    Ref, Signal, StyleApplication, ViewHandle,
};

use idea_theme::extensible::{installed_switch_sheet, ToneRef, VariantRef};

use crate::components::ControlSize;
use crate::stylesheets::{ControlRow, FieldLabel, SwitchThumb};
use idea_theme::tokens;

/// Duration of the thumb-slide / track-color animation.
const SWITCH_ANIM_MS: u64 = 180;

/// Thumb travel distance (px) per size — the width the thumb slides
/// from off to on. `track_width − thumb_diameter − 2·inset`, matching
/// `SWITCH_TRACK_DIMS` / `SwitchThumb` (inset = 2px each edge):
///   sm: 30 − 14 − 4 = 12   md: 38 − 18 − 4 = 16   lg: 48 − 24 − 4 = 20
fn travel_for(size: ControlSize) -> f32 {
    match size {
        ControlSize::Sm => 12.0,
        ControlSize::Md => 16.0,
        ControlSize::Lg => 20.0,
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (tone/variant/size/icon/disabled). The controlled `value` `Signal`
// stays bare (a reactive *source*), `on_change` is a handler, `label` is
// already `Reactive`. NOTE `size` ALSO drives structure (thumb travel
// distance + icon px feed the animation/layout), read once at build — only
// the track *style* re-resolves on a live tone/variant; see the body TODO.
#[runtime_core::props]
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
    /// Surface skeleton for the "on" track fill. Default Filled.
    pub variant: VariantRef,
    /// Track + thumb scale. Default Md.
    pub size: ControlSize,
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
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ControlSize::default()),
            disabled: Reactive::Static(false),
            icon: Reactive::Static(None),
            test_id: None,
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

    // The track appearance is read LIVE so a reactive tone/variant re-styles
    // the track in place; the `checked` axis flips the track between the tone
    // fill and the muted off-track.
    let appearance_for = {
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        move || format!("{}_{}", tone.get().key(), variant.get().key())
    };
    let size_key = size.as_variant_str().to_string();

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

    let thumb_size_key = size_key.clone();
    // Optional glyph centered in the thumb (the SwitchThumb sheet centers it),
    // sized to the thumb and tinted with the ink text color so it reads on the
    // white thumb in both states.
    let icon_px = match size {
        ControlSize::Sm => 9.0,
        ControlSize::Md => 11.0,
        ControlSize::Lg => 14.0,
    };
    // TODO(reactive-sweep): a custom thumb `icon` is read once at build (it's
    // baked into the thumb's children); a reactive icon swap would need a
    // `switch` around the thumb contents. The common case is a fixed icon.
    let thumb_kids: Vec<Element> = match props.icon.get() {
        Some(data) => vec![icon(data)
            .size(icon_px)
            .color(|| tokens().color.text().resolve())
            .into_element()],
        None => Vec::new(),
    };
    let thumb = runtime_core::view(thumb_kids)
        .with_style(move || {
            StyleApplication::new(SwitchThumb::sheet()).with("size", thumb_size_key.clone())
        })
        .bind(thumb_ref)
        .into_element();

    // --- track: a pressable that toggles, styled by the checked axis ---
    // `disabled` is read LIVE here so the dim follows a reactive prop in
    // place (the `dimmed` axis — Button parity).
    let disabled_dim = props.disabled.clone();
    let track_style = move || {
        StyleApplication::new(installed_switch_sheet())
            .with("appearance", appearance_for())
            .with("checked", if value.get() { "on" } else { "off" }.to_string())
            .with("size", size_key.clone())
            .with("dimmed", if disabled_dim.get() { "on" } else { "off" }.to_string())
    };
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

    match crate::components::optional_reactive_text(props.label.clone(), FieldLabel()) {
        Some(label) => ui! {
            view(style = ControlRow()) {
                label
                track
            }
        },
        None => track,
    }
}
