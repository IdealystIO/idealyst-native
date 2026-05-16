//! Feedback — Spinner, Skeleton, Alert.

use framework_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    alert, skeleton, spinner, stack, AlertProps, SkeletonProps, SpinnerProps, StackGap,
};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Feedback",
                "Spinner, Skeleton, Alert. Indications of progress, loading, and status."
            ) }

            { spinner_demo() }
            { skeleton_demo() }
            { alert_demo() }
        }
    }
}

fn spinner_demo() -> Primitive {
    let state = SpinnerProps::init_state();
    let preview = SpinnerProps::reactive_preview(&state, |props| {
        let size = props.size;
        ui! { Spinner(size = size) }
    });
    let controls = SpinnerProps::render_controls(&state);
    demo_card(
        "Spinner",
        "Wraps the framework's ActivityIndicator with size tokens.",
        preview,
        controls,
    )
}

fn skeleton_demo() -> Primitive {
    // SkeletonProps' fields don't auto-derive into controls (`f32`
    // and the `Px(f32)` variant fall to Unknown). Static preview.
    let state = SkeletonProps::init_state();
    let preview = ui! { Skeleton(height = 24.0, radius = 6.0) };
    let controls = SkeletonProps::render_controls(&state);
    demo_card(
        "Skeleton",
        "Muted placeholder block. Controls are limited here — the `width` (a non-VariantEnum \
         enum) and the raw `f32` height/radius aren't reflective. A future continuous-numeric \
         control (slider) would unlock height/radius twiddling.",
        preview,
        controls,
    )
}

fn alert_demo() -> Primitive {
    let state = AlertProps::init_state();
    state.title.set("Heads up".to_string());
    let preview = AlertProps::reactive_preview(&state, |props| {
        let title = props.title;
        let body = props.body;
        let intent = props.intent;
        ui! {
            Alert(
                title = title,
                body = body,
                intent = intent
            )
        }
    });
    let controls = AlertProps::render_controls(&state);
    demo_card(
        "Alert",
        "Inline status banner. Intent drives the surface color; body is an optional second \
         line of detail.",
        preview,
        controls,
    )
}
