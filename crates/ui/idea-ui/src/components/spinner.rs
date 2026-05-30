//! `Spinner` — passthrough to the framework's `ActivityIndicator`
//! primitive with a small/large size knob.
//!
//! No tone/variant axes — the framework primitive's color is
//! platform-native. When the framework primitive grows a tint hook,
//! a Tone axis would land here (so `Spinner(tone = tone::Primary)`
//! could tint the wheel). Until then this is identical to the
//! closed-enum [`crate::components::spinner`].

use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_core::{component, ui, Element, VariantEnum};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SpinnerSize {
    #[default]
    Small,
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

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SpinnerProps {
    pub size: SpinnerSize,
}

#[component]
pub fn Spinner(props: &SpinnerProps) -> Element {
    let native = match props.size {
        SpinnerSize::Small => ActivityIndicatorSize::Small,
        SpinnerSize::Large => ActivityIndicatorSize::Large,
    };
    ui! { ActivityIndicator().size(native) }
}
