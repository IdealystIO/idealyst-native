//! Layout — Stack/HStack/VStack, Card, Divider.

use framework_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    badge, body, card, divider, heading, hstack, stack, vstack, BadgeProps, CardProps,
    DividerProps, HStackProps, HeadingKind, IntoRcIntent, Primary, StackGap, StackProps, Success,
    VStackProps, Warning,
};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
    ui! {
        VStack(gap = StackGap::Xl) {
            { page_header(
                "Layout",
                "Stack, HStack, VStack, Card, Divider. Stack is the workhorse — gap, axis, \
                 alignment, and justification are all variant axes (discrete and cacheable)."
            ) }

            { stack_demo() }
            { hstack_demo() }
            { vstack_demo() }
            { card_demo() }
            { divider_demo() }
        }
    }
}

fn filler_children() -> Vec<Primitive> {
    vec![
        ui! { Badge(label = "one".to_string(), intent = Primary.into_rc()) },
        ui! { Badge(label = "two".to_string(), intent = Success.into_rc()) },
        ui! { Badge(label = "three".to_string(), intent = Warning.into_rc()) },
    ]
}

fn stack_demo() -> Primitive {
    let state = StackProps::init_state();
    let preview = StackProps::reactive_preview(&state, |props| {
        let gap = props.gap;
        let axis = props.axis;
        let align = props.align;
        let justify = props.justify;
        ui! {
            Stack(
                gap = gap,
                axis = axis,
                align = align,
                justify = justify,
                children = filler_children()
            )
        }
    });
    let controls = StackProps::render_controls(&state);
    demo_card(
        "Stack",
        "Generic flex container. Axis defaults to column; both axes share the same gap / \
         align / justify variants.",
        preview,
        controls,
    )
}

fn hstack_demo() -> Primitive {
    let state = HStackProps::init_state();
    let preview = HStackProps::reactive_preview(&state, |props| {
        let gap = props.gap;
        let align = props.align;
        let justify = props.justify;
        ui! {
            HStack(
                gap = gap,
                align = align,
                justify = justify,
                children = filler_children()
            )
        }
    });
    let controls = HStackProps::render_controls(&state);
    demo_card(
        "HStack",
        "Stack with axis locked to row.",
        preview,
        controls,
    )
}

fn vstack_demo() -> Primitive {
    let state = VStackProps::init_state();
    let preview = VStackProps::reactive_preview(&state, |props| {
        let gap = props.gap;
        let align = props.align;
        let justify = props.justify;
        ui! {
            VStack(
                gap = gap,
                align = align,
                justify = justify,
                children = filler_children()
            )
        }
    });
    let controls = VStackProps::render_controls(&state);
    demo_card(
        "VStack",
        "Stack with axis locked to column.",
        preview,
        controls,
    )
}

fn card_demo() -> Primitive {
    let state = CardProps::init_state();
    let preview = CardProps::reactive_preview(&state, |props| {
        let tone = props.tone;
        let padding = props.padding;
        ui! {
            Card(tone = tone, padding = padding) {
                Heading(content = "Card heading".to_string(), kind = HeadingKind::H3)
                Body(content = "Cards group related content. Tone variants pick the surface; \
                                padding controls the inner spacing.".to_string())
            }
        }
    });
    let controls = CardProps::render_controls(&state);
    demo_card(
        "Card",
        "Themed surface for grouping content. Tone variants: surface, elevated, primary, \
         muted.",
        preview,
        controls,
    )
}

fn divider_demo() -> Primitive {
    let state = DividerProps::init_state();
    let preview = DividerProps::reactive_preview(&state, |props| {
        let axis = props.axis;
        ui! { Divider(axis = axis) }
    });
    let controls = DividerProps::render_controls(&state);
    demo_card(
        "Divider",
        "Thin separator line. Horizontal or vertical.",
        preview,
        controls,
    )
}
