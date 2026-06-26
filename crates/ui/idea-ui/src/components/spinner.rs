//! `Spinner` — passthrough to the framework's `ActivityIndicator`
//! primitive with a small/large size knob.
//!
//! No tone/variant axes — the framework primitive's color is
//! platform-native. When the framework primitive grows a tint hook,
//! a Tone axis would land here (so `Spinner(tone = tone::Primary)`
//! could tint the wheel). Until then this is identical to the
//! closed-enum [`crate::components::spinner`].

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::primitives::activity_indicator::{activity_indicator, ActivityIndicatorSize};
use runtime_core::{component, Element, IdealystSchema, IntoElement, StyleSheet, VariantEnum};

thread_local! {
    static SPINNER_HUG_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

/// Cached static sheet that hugs + centers the spinner on its parent's cross
/// axis, so a row of mixed-size spinners centers instead of top-aligning under
/// the default `align-items: stretch` (see `components::hug_self`).
fn spinner_hug_sheet() -> Rc<StyleSheet> {
    SPINNER_HUG_SHEET.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(Rc::new(StyleSheet::r#static(crate::components::hug_self())));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// Size knob for the [`Spinner`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
#[derive(IdealystSchema)]
pub enum SpinnerSize {
    /// Compact spinner. The default.
    #[default]
    Small,
    /// Larger spinner for prominent loading states.
    Large,
}

impl VariantEnum for SpinnerSize {
    fn as_variant_str(self) -> &'static str {
        match self {
            SpinnerSize::Small => "small",
            SpinnerSize::Large => "large",
        }
    }
    fn all_variants() -> &'static [Self] {
        &[SpinnerSize::Small, SpinnerSize::Large]
    }
}

// Reactive-by-default: `#[props]` wraps `size` → `Reactive<SpinnerSize>` so a
// `ui!` call site can pass a `Signal`/`rx!` (a bare value stays a zero-cost
// `Static` snapshot). See the TODO in `Spinner` — the framework's
// `activity_indicator` primitive has no reactive `size` sink yet, so a live
// `size` is read once at build for now.
#[runtime_core::props]
#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SpinnerProps {
    /// Spinner scale (`Small`/`Large`). Default `Small`.
    pub size: SpinnerSize,
}

/// Renders an indeterminate loading spinner — a thin passthrough to the
/// framework's `activity_indicator` primitive with a small/large size knob.
#[component]
pub fn Spinner(props: &SpinnerProps) -> Element {
    // TODO(reactive-sweep): route `size` to the activity_indicator size sink.
    // The `Element::ActivityIndicator` `size` field is a plain (non-reactive)
    // value and `.size()` is a one-shot setter — there's no reactive walker
    // path to re-apply a native indicator size in place. A live `size` is read
    // once here; wire a reactive size sink on the primitive to make it live.
    let native = match props.size.get() {
        SpinnerSize::Small => ActivityIndicatorSize::Small,
        SpinnerSize::Large => ActivityIndicatorSize::Large,
    };
    activity_indicator().size(native).with_style(spinner_hug_sheet()).into_element()
}
