//! Layout — Stack, Card, Divider, Center, Spacer.

use runtime_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    badge, btn, card, center, divider, spacer, stack, typography,
    CardProps, DividerProps, StackAxis, StackGap, StackProps,
};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Layout",
                "Stack, Card, Divider, Center, Spacer. Stack is the workhorse \u{2014} gap, axis, \
                 alignment, and justification are all variant axes (discrete and cacheable)."
            ) }

            { stack_demo() }
            { card_demo() }
            { divider_demo() }
            { center_demo() }
            { spacer_demo() }
        }
    }
}

fn filler_children() -> Vec<Primitive> {
    vec![
        ui! { Badge(label = "one".to_string(), tone = idea_ui::tone::Primary.into(), variant = idea_ui::variant::Soft.into()) },
        ui! { Badge(label = "two".to_string(), tone = idea_ui::tone::Success.into(), variant = idea_ui::variant::Soft.into()) },
        ui! { Badge(label = "three".to_string(), tone = idea_ui::tone::Warning.into(), variant = idea_ui::variant::Soft.into()) },
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
        "Generic flex container. Axis defaults to column; flip to row for toolbars / button \
         rows. Gap, alignment, and justification are all enum-typed variant axes — discrete and \
         cacheable.",
        preview,
        controls,
    )
}

fn card_demo() -> Primitive {
    let state = CardProps::init_state();
    let preview = CardProps::reactive_preview(&state, |props| {
        let variant = props.variant;
        let padding = props.padding;
        ui! {
            Card(variant = variant, padding = padding) {
                Typography(content = "Card heading".to_string(), kind = idea_ui::typography_kind::H3.into())
                Typography(content = "Cards group related content. Tone variants pick the surface; \
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

fn center_demo() -> Primitive {
    // Center has no props beyond `children`, so the preview is static.
    // The point of the demo is showing what Center does at all — every
    // child lands on both axes' midpoint of the available box.
    let preview = ui! {
        Center {
            Badge(
                label = "Centered".to_string(),
                tone = idea_ui::tone::Primary.into(),
                variant = idea_ui::variant::Soft.into(),
            )
        }
    };
    let notes = ui! {
        Typography(
            content = "Container that centers its children on both axes. Equivalent to a \
                       Stack with align: center, justify: center \u{2014} the shorthand exists so \
                       the common case (empty-state icon, spinner) doesn't need a one-off \
                       stylesheet.".to_string(),
            muted = true,
        )
    };
    demo_card("Center", "Two-axis centering container.", preview, notes)
}

fn spacer_demo() -> Primitive {
    // Spacer takes no controllable props — it's a flex item that
    // grows to fill available space. Show it pushing two siblings to
    // opposite ends of a row.
    let noop: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(|| {});
    let preview = ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
            Typography(content = "Title".to_string(), kind = idea_ui::typography_kind::H3.into())
            Spacer()
            Btn(
                label = "Save".to_string(),
                on_click = noop,
                tone = idea_ui::tone::Primary.into(),
                variant = idea_ui::variant::Filled.into(),
            )
        }
    };
    let notes = ui! {
        Typography(
            content = "Empty flex item that grows to fill the available space. Drop one between \
                       siblings inside a row Stack to push them to opposite ends without \
                       computing margins.".to_string(),
            muted = true,
        )
    };
    demo_card("Spacer", "Flex grow filler.", preview, notes)
}
