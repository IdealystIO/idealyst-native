//! `Spinner` — themed wrapper around the `ActivityIndicator` primitive.
//!
//! The framework primitive already handles platform-native spinner
//! animation; this wrapper just adds size tokens so call sites don't
//! reach into runtime-core for the underlying enum.

use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_core::{ui, Primitive, VariantEnum};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SpinnerSize {
    #[default]
    Small,
    Large,
}

// Hand-rolled `VariantEnum` impl so the `DocControls` derive can
// surface this enum as a Pressable-row picker. The `stylesheet!`
// macro would generate this automatically; for hand-rolled enums
// we mirror the shape ourselves.
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

pub fn spinner(props: &SpinnerProps) -> Primitive {
    let native = match props.size {
        SpinnerSize::Small => ActivityIndicatorSize::Small,
        SpinnerSize::Large => ActivityIndicatorSize::Large,
    };
    ui! { ActivityIndicator().size(native) }
}
