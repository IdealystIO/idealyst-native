//! Actions — Button, IconButton, Badge, Tag.
//!
//! Each demo wraps its preview in `DocControls::reactive_preview`
//! so twiddling a control rebuilds the preview subtree.

use std::rc::Rc;

use runtime_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{badge, btn, stack, BadgeProps, ButtonProps, IconButtonProps, StackGap, TagProps};
// idea-ui's own component invocation macros must be in scope.
use idea_ui::{icon_button, tag};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
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

fn button_demo() -> Primitive {
    let state = ButtonProps::init_state();
    state.label.set("Click me".to_string());

    let preview = ButtonProps::reactive_preview(&state, |props| {
        let label = props.label;
        let intent = props.intent;
        let kind = props.kind;
        let size = props.size;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            Btn(
                label = label,
                on_click = on_click,
                intent = intent,
                kind = kind,
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

fn icon_button_demo() -> Primitive {
    let state = IconButtonProps::init_state();
    state.glyph.set("+".to_string());

    let preview = IconButtonProps::reactive_preview(&state, |props| {
        let glyph = props.glyph;
        let intent = props.intent;
        let kind = props.kind;
        let size = props.size;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            IconButton(
                glyph = glyph,
                on_click = on_click,
                intent = intent,
                kind = kind,
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

fn badge_demo() -> Primitive {
    let state = BadgeProps::init_state();
    state.label.set("New".to_string());

    let preview = BadgeProps::reactive_preview(&state, |props| {
        let label = props.label;
        let intent = props.intent;
        let kind = props.kind;
        ui! { Badge(label = label, intent = intent, kind = kind) }
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

fn tag_demo() -> Primitive {
    let state = TagProps::init_state();
    state.label.set("Rust".to_string());

    let preview = TagProps::reactive_preview(&state, |props| {
        let label = props.label;
        let intent = props.intent;
        let kind = props.kind;
        ui! { Tag(label = label, intent = intent, kind = kind) }
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
