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

use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::{
    component, effect, icon, ui, Color, Element, IconData, IdealystSchema, IntoElement, Reactive,
    Ref, Signal, StyleApplication, Tokenized, ViewHandle,
};

use idea_theme::extensible::{installed_switch_sheet, ToneRef, VariantRef};

use crate::components::ControlSize;
use crate::stylesheets::{ControlRow, FieldLabel, SwitchThumb};

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
            value: Signal::new(false),
            on_change: Rc::new(|_| {}),
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ControlSize::default(),
            icon: None,
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
    let size = props.size;

    // Per-instance appearance/size keys (static). The `checked` axis is
    // the only reactive piece — it flips the track between the tone
    // fill and the muted off-track.
    let appearance = format!("{}_{}", props.tone.key(), props.variant.key());
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
    let thumb_kids: Vec<Element> = match props.icon {
        Some(data) => vec![icon(data)
            .size(icon_px)
            .color(|| Tokenized::token("color-text", Color("#1a1a1f".into())).resolve())
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
    let track_style = move || {
        StyleApplication::new(installed_switch_sheet())
            .with("appearance", appearance.clone())
            .with("checked", if value.get() { "on" } else { "off" }.to_string())
            .with("size", size_key.clone())
    };
    let toggle = move || (on_change)(!value.get());
    let track = runtime_core::pressable(vec![thumb], toggle)
        .with_style(track_style)
        .into_element();
    // Forward the test id to the interactive track so a robot suite can
    // locate + click it. Gated: `with_test_id` only exists under
    // `runtime-core/robot`.
    #[cfg(feature = "robot")]
    let track = match props.test_id {
        Some(tid) => track.with_test_id(tid),
        None => track,
    };

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
