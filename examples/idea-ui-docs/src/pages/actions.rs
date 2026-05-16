//! Actions — Pressable, IconButton, Badge, Tag.
//!
//! Each demo wraps its preview in `DocControls::reactive_preview`
//! so twiddling a control rebuilds the preview subtree. Without
//! that wrap, the preview snapshots the signal values at page
//! mount and never updates — user components like Pressable
//! aren't reactive per-prop the way built-in `Text` / `Button`
//! are.

use std::rc::Rc;

use framework_core::{ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    badge, pressable, stack, BadgeProps, IconButtonProps, PressableProps, StackGap, TagProps,
};
// idea-ui's own component invocation macros must be in scope.
use idea_ui::{iconbutton, tag};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Actions",
                "Pressable, IconButton, Badge, and Tag. Every action component honors the \
                 global Intent vocabulary; pick one in each demo's control panel to see the \
                 themed coloring update live."
            ) }

            { pressable_demo() }
            { icon_button_demo() }
            { badge_demo() }
            { tag_demo() }
        }
    }
}

fn pressable_demo() -> Primitive {
    let state = PressableProps::init_state();
    state.label.set("Click me".to_string());

    let preview = PressableProps::reactive_preview(&state, |props| {
        let label = props.label;
        let intent = props.intent;
        let size = props.size;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            Pressable(
                label = label,
                on_click = on_click,
                intent = intent,
                size = size
            )
        }
    });
    let controls = PressableProps::render_controls(&state);
    demo_card(
        "Pressable",
        "Themed button. Intent drives coloring; size drives padding & font.",
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
        let size = props.size;
        let on_click: Rc<dyn Fn()> = Rc::new(|| {});
        ui! {
            IconButton(
                glyph = glyph,
                on_click = on_click,
                intent = intent,
                size = size
            )
        }
    });
    let controls = IconButtonProps::render_controls(&state);
    demo_card(
        "IconButton",
        "Square Pressable variant — takes a glyph string (Unicode symbol, font icon ligature, \
         single character).",
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
        ui! { Badge(label = label, intent = intent) }
    });
    let controls = BadgeProps::render_controls(&state);
    demo_card(
        "Badge",
        "Small pill for status indicators. Intent drives the surface; the label is just a \
         string.",
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
        ui! { Tag(label = label, intent = intent) }
    });
    let controls = TagProps::render_controls(&state);
    demo_card(
        "Tag",
        "Like Badge, with an optional close button. The close button is wired through \
         `on_remove`; the docs panel skips that field (it's a callback).",
        preview,
        controls,
    )
}
