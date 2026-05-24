//! Typography — Heading, Body, Caption.

use runtime_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    body, caption, heading, stack, BodyProps, CaptionProps, HeadingProps, StackGap,
};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Typography",
                "Heading, Body, and Caption. All read color from the active theme and pick \
                 size from a discrete variant axis (no continuous font-size knob)."
            ) }

            { heading_demo() }
            { body_demo() }
            { caption_demo() }
        }
    }
}

fn heading_demo() -> Primitive {
    let state = HeadingProps::init_state();
    state.content.set("The quick brown fox jumps over the lazy dog".to_string());
    let preview = HeadingProps::reactive_preview(&state, |props| {
        let content = props.content;
        let kind = props.kind;
        let align = props.align;
        ui! { Heading(content = content, kind = kind, align = align) }
    });
    let controls = HeadingProps::render_controls(&state);
    demo_card(
        "Heading",
        "Title text. `kind` ranges from `display` through `h3`; size + weight + line-height \
         scale together.",
        preview,
        controls,
    )
}

fn body_demo() -> Primitive {
    let state = BodyProps::init_state();
    state.content.set(
        "Body text — the bulk of most pages. The `tone` axis chooses which theme color \
         the text reads from; align is for centering or right-aligning blocks."
            .to_string(),
    );
    let preview = BodyProps::reactive_preview(&state, |props| {
        let content = props.content;
        let tone = props.tone;
        let align = props.align;
        ui! { Body(content = content, tone = tone, align = align) }
    });
    let controls = BodyProps::render_controls(&state);
    demo_card(
        "Body",
        "Paragraph text. `tone` picks the theme color (default / muted / primary / danger / \
         success / warning).",
        preview,
        controls,
    )
}

fn caption_demo() -> Primitive {
    let state = CaptionProps::init_state();
    state.content.set("Helper text under a control".to_string());
    let preview = CaptionProps::reactive_preview(&state, |props| {
        let content = props.content;
        let tone = props.tone;
        let align = props.align;
        ui! { Caption(content = content, tone = tone, align = align) }
    });
    let controls = CaptionProps::render_controls(&state);
    demo_card(
        "Caption",
        "Small muted text — for hints, labels, helper rows.",
        preview,
        controls,
    )
}
