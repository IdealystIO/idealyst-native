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
// `Static` snapshot). A live `size` routes to the primitive's reactive
// `.size_reactive()` sink (resizes in place on web; native inherits the no-op);
// a `Static` size uses the one-shot `.size()` setter.
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
    fn to_native(s: SpinnerSize) -> ActivityIndicatorSize {
        match s {
            SpinnerSize::Small => ActivityIndicatorSize::Small,
            SpinnerSize::Large => ActivityIndicatorSize::Large,
        }
    }
    // A live `size` routes to the primitive's reactive `.size_reactive()` sink
    // (the walker installs an Effect → `update_activity_indicator_size`); a
    // `Static` size uses the one-shot `.size()` setter.
    let spinner = if props.size.is_static() {
        activity_indicator().size(to_native(props.size.get()))
    } else {
        let size = props.size.clone();
        activity_indicator().size_reactive(move || to_native(size.get()))
    };
    spinner.with_style(spinner_hug_sheet()).into_element()
}
