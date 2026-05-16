//! Stateful — Tabs and Avatar.

use std::rc::Rc;

use framework_core::{signal, ui, Primitive};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    avatar, body, card, heading, stack, tabs, AvatarProps, BodyTone, HeadingKind, StackGap, Tab,
};

use crate::shell::{demo_card, page_header};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Stateful",
                "Components whose appearance is driven by host-owned signals or runtime data."
            ) }

            { avatar_demo() }
            { tabs_demo() }
        }
    }
}

fn avatar_demo() -> Primitive {
    let state = AvatarProps::init_state();
    state.initials.set("AB".to_string());

    let preview = AvatarProps::reactive_preview(&state, |props| {
        let initials = props.initials;
        let intent = props.intent;
        let size = props.size;
        ui! {
            Avatar(initials = initials, intent = intent, size = size)
        }
    });
    let controls = AvatarProps::render_controls(&state);
    demo_card(
        "Avatar",
        "Circular user-identity element. Renders an image when `src` is set, otherwise \
         falls back to initials on an intent-colored background.",
        preview,
        controls,
    )
}

fn tabs_demo() -> Primitive {
    let selected = signal!("overview".to_string());

    let p1: Rc<dyn Fn() -> Primitive> = Rc::new(|| ui! {
        Body(content = "Big-picture summary content.".to_string())
    });
    let p2: Rc<dyn Fn() -> Primitive> = Rc::new(|| ui! {
        Body(content = "Activity event stream.".to_string())
    });
    let p3: Rc<dyn Fn() -> Primitive> = Rc::new(|| ui! {
        Body(content = "Configuration knobs.".to_string())
    });

    ui! {
        Card {
            Heading(content = "Tabs".to_string(), kind = HeadingKind::H2)
            Body(content = "Controlled by a `Signal<String>` naming the active tab's id. \
                              Panels mount lazily via the framework's reactive switch.".to_string(),
                 tone = BodyTone::Muted)
            Tabs(
                selected = selected,
                tabs = vec![
                    Tab::new("overview", "Overview", p1),
                    Tab::new("activity", "Activity", p2),
                    Tab::new("settings", "Settings", p3),
                ]
            )
        }
    }
}
