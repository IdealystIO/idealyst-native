//! Overlays — Modal, Popover, Drawer.
//!
//! These don't auto-derive cleanly (they own children + anchor
//! targets that aren't reflective). The demos are hand-written
//! with live "Open" Pressables that flip a Signal<bool>. The
//! Overlay primitive's Presence-driven enter/exit animations are
//! built into the framework, so the demos animate without extra
//! wiring here.

use std::rc::Rc;

use framework_core::primitives::overlay::{BackdropMode, OverlayAnchor, ViewportPlacement};
use framework_core::{signal, ui, Easing, Primitive, PresenceAnim, PresenceState, Signal};
use idea_ui::{
    body, card, heading, hstack, pressable, vstack, BodyTone, HeadingKind, IntoRcIntent, Neutral,
    Primary, StackGap,
};

use crate::shell::page_header;

pub fn page() -> Primitive {
    ui! {
        VStack(gap = StackGap::Xl) {
            { page_header(
                "Overlays",
                "Modal, Drawer, and Popover. All built on the framework's Overlay primitive — \
                 portaled to the document body so they escape parent overflow / stacking \
                 contexts. The framework's Presence primitive handles enter/exit animations."
            ) }

            { modal_demo() }
            { drawer_demo() }
        }
    }
}

fn modal_demo() -> Primitive {
    let open = signal!(false);
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    ui! {
        Card {
            Heading(content = "Modal".to_string(), kind = HeadingKind::H2)
            Body(content = "Viewport-centered overlay with a dismiss-on-click scrim. \
                              Press Escape or click outside to dismiss.".to_string(),
                 tone = BodyTone::Muted)
            Pressable(
                label = "Open modal".to_string(),
                on_click = on_open,
                intent = Primary.into_rc()
            )
            Presence(
                present = move || open.get(),
                enter = PresenceAnim::new(
                    PresenceState::default().opacity(0.0).translate_y(8.0),
                    200,
                    Easing::EaseOut,
                ),
                exit = PresenceAnim::new(
                    PresenceState::default().opacity(0.0).translate_y(8.0),
                    150,
                    Easing::EaseIn,
                ),
            ) {
                Overlay(
                    anchor = OverlayAnchor::Viewport(ViewportPlacement::Center),
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = {
                        let oc = on_close.clone();
                        move || (oc)()
                    }
                ) {
                    Card {
                        Heading(content = "Confirm".to_string(), kind = HeadingKind::H3)
                        Body(content = "Click outside or press Escape to dismiss.".to_string())
                        Pressable(
                            label = "OK".to_string(),
                            on_click = on_close.clone(),
                            intent = Primary.into_rc()
                        )
                    }
                }
            }
        }
    }
}

fn drawer_demo() -> Primitive {
    let open = signal!(false);
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    ui! {
        Card {
            Heading(content = "Drawer".to_string(), kind = HeadingKind::H2)
            Body(content = "Same Overlay primitive, pinned to the right edge with a slide-in \
                              transition.".to_string(),
                 tone = BodyTone::Muted)
            Pressable(
                label = "Open drawer".to_string(),
                on_click = on_open,
                intent = Neutral.into_rc()
            )
            Presence(
                present = move || open.get(),
                enter = PresenceAnim::new(
                    PresenceState::default().translate_x(360.0),
                    260,
                    Easing::EaseOut,
                ),
                exit = PresenceAnim::new(
                    PresenceState::default().translate_x(360.0),
                    220,
                    Easing::EaseIn,
                ),
            ) {
                Overlay(
                    anchor = OverlayAnchor::Viewport(ViewportPlacement::Right),
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = {
                        let oc = on_close.clone();
                        move || (oc)()
                    }
                ) {
                    Card {
                        Heading(content = "Drawer".to_string(), kind = HeadingKind::H3)
                        Body(content = "Right-edge drawer content.".to_string())
                        Pressable(
                            label = "Close".to_string(),
                            on_click = on_close.clone(),
                            intent = Neutral.into_rc()
                        )
                    }
                }
            }
        }
    }
}
