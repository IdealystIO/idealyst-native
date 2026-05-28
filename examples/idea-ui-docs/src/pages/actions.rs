//! Actions — Button, IconButton, Badge, Tag.
//!
//! Each demo wraps its preview in `DocControls::reactive_preview`
//! so twiddling a control rebuilds the preview subtree.

use std::rc::Rc;

use runtime_core::{ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{Badge, Btn, Stack, BadgeProps, ButtonProps, IconButtonProps, StackGap, TagProps};
// idea-ui's own component invocation macros must be in scope.
use idea_ui::{IconButton, Tag};

use crate::shell::{demo_card, page_header};

pub fn page() -> Element {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Actions",
                "Button, IconButton, Badge, and Tag. Every action component pairs an `intent` \
                 (semantic meaning) with a `kind` (visual treatment) — pick both in each demo's \
                 control panel to see the live combination."
            ) }

            { button_demo() }
            { icon_button_demo() }
            { badge_demo() }
            { tag_demo() }
        }
    }
}

fn button_demo() -> Element {
    let state = ButtonProps::init_state();
    state.label.set("Click me".to_string());

    let preview = ButtonProps::reactive_preview(&state, |props| {
        let label = props.label;
        let tone = props.tone;
        let variant = props.variant;
        let size = props.size;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            Btn(
                label = label,
                on_click = on_click,
                tone = tone,
                variant = variant,
                size = size,
            )
        }
    });
    let controls = ButtonProps::render_controls(&state);
    demo_card(
        "Button",
        "Themed clickable. Intent picks the palette (Primary / Success / Danger / …); kind \
         picks the visual (Solid filled, Soft tinted, Outlined, Ghost).",
        preview,
        controls,
    )
}

fn icon_button_demo() -> Element {
    let state = IconButtonProps::init_state();
    state.glyph.set("+".to_string());

    let preview = IconButtonProps::reactive_preview(&state, |props| {
        let glyph = props.glyph;
        let tone = props.tone;
        let variant = props.variant;
        let size = props.size;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            IconButton(
                glyph = glyph,
                on_click = on_click,
                tone = tone,
                variant = variant,
                size = size,
            )
        }
    });
    let controls = IconButtonProps::render_controls(&state);
    demo_card(
        "IconButton",
        "Square Button variant. Takes a glyph string instead of a label — same intent / kind / \
         size vocabulary as Button.",
        preview,
        controls,
    )
}

fn badge_demo() -> Element {
    let state = BadgeProps::init_state();
    state.label.set("New".to_string());

    let preview = BadgeProps::reactive_preview(&state, |props| {
        let label = props.label;
        let tone = props.tone;
        let variant = props.variant;
        ui! { Badge(label = label, tone = tone, variant = variant) }
    });
    let controls = BadgeProps::render_controls(&state);
    demo_card(
        "Badge",
        "Small pill for status indicators. Kinds are Solid / Soft / Outlined — no Ghost (a \
         transparent badge would be invisible).",
        preview,
        controls,
    )
}

fn tag_demo() -> Element {
    let state = TagProps::init_state();
    state.label.set("Rust".to_string());

    let preview = TagProps::reactive_preview(&state, |props| {
        let label = props.label;
        let tone = props.tone;
        let variant = props.variant;
        ui! { Tag(label = label, tone = tone, variant = variant) }
    });
    let controls = TagProps::render_controls(&state);
    demo_card(
        "Tag",
        "Like Badge, with an optional close affordance via `on_remove` (omitted from the docs \
         panel since it's a callback).",
        preview,
        controls,
    )
}
