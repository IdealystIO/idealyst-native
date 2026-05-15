//! `Spinner` — themed wrapper around the `ActivityIndicator` primitive.
//!
//! The framework primitive already handles platform-native spinner
//! animation; this wrapper just adds size tokens so call sites don't
//! reach into framework-core for the underlying enum.

use framework_core::primitives::activity_indicator::ActivityIndicatorSize;
use framework_core::{ui, Primitive};

#[derive(Copy, Clone, Default)]
pub enum SpinnerSize {
    #[default]
    Small,
    Large,
}

#[derive(Default)]
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
