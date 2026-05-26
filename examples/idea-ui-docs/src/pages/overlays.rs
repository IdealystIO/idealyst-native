//! Overlays — Modal, Popover, Drawer.
//!
//! These don't auto-derive cleanly (they own children + anchor
//! targets that aren't reflective). The demos are hand-written
//! with live "Open" Pressables that flip a Signal<bool>. The
//! Overlay primitive's Presence-driven enter/exit animations are
//! built into the framework, so the demos animate without extra
//! wiring here.

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide, ViewportPlacement};
use runtime_core::{signal, ui, Easing, PressableHandle, Primitive, PresenceAnim, PresenceState, Ref, Signal};
use idea_ui::{
    btn, card, popover, stack, typography, ButtonKind, IntentTag, StackGap, TypographyKind,
    TypographyTone,
};

use crate::shell::page_header;

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Overlays",
                "Modal, Drawer, and Popover. All built on the framework's Overlay primitive — \
                 portaled to the document body so they escape parent overflow / stacking \
                 contexts. The framework's Presence primitive handles enter/exit animations."
            ) }

            { modal_demo() }
            { drawer_demo() }
            { popover_demo() }
        }
    }
}

fn modal_demo() -> Primitive {
    let open = signal!(false);
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    ui! {
        Card {
            Typography(content = "Modal".to_string(), kind = TypographyKind::H2)
            Typography(content = "Viewport-centered overlay with a dismiss-on-click scrim. \
                              Press Escape or click outside to dismiss.".to_string(),
                 tone = TypographyTone::Muted)
            Btn(
                label = "Open modal".to_string(),
                on_click = on_open,
                intent = IntentTag::Primary,
                kind = ButtonKind::Solid,
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
                    placement = ViewportPlacement::Center,
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = {
                        let oc = on_close.clone();
                        move || (oc)()
                    }
                ) {
                    Card {
                        Typography(content = "Confirm".to_string(), kind = TypographyKind::H3)
                        Typography(content = "Click outside or press Escape to dismiss.".to_string())
                        Btn(
                            label = "OK".to_string(),
                            on_click = on_close.clone(),
                            intent = IntentTag::Primary,
                            kind = ButtonKind::Solid,
                        )
                    }
                }
            }
        }
    }
}

fn popover_demo() -> Primitive {
    let open = signal!(false);
    let trigger: Ref<PressableHandle> = Ref::new();
    let on_toggle: Rc<dyn Fn()> = Rc::new(move || open.update(|v| *v = !*v));
    let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    ui! {
        Card {
            Typography(content = "Popover".to_string(), kind = TypographyKind::H2)
            Typography(content = "Element-anchored overlay with no scrim. The trigger element \
                              binds a `Ref<PressableHandle>`; the popover targets that ref \
                              and follows it through scrolls / resizes.".to_string(),
                 tone = TypographyTone::Muted)
            Btn(
                label = "Open menu".to_string(),
                on_click = on_toggle,
                intent = IntentTag::Neutral,
                kind = ButtonKind::Soft,
                bind_to = Some(trigger),
            )
            Presence(
                present = move || open.get(),
                enter = PresenceAnim::new(
                    PresenceState::default().opacity(0.0).translate_y(-4.0),
                    160,
                    Easing::EaseOut,
                ),
                exit = PresenceAnim::new(
                    PresenceState::default().opacity(0.0).translate_y(-4.0),
                    120,
                    Easing::EaseIn,
                ),
            ) {
                Popover(
                    target = Some(AnchorTarget::from(trigger)),
                    side = ElementSide::Below,
                    align = ElementAlign::Start,
                    offset = 6.0,
                    on_dismiss = Some({
                        let d = on_dismiss.clone();
                        Rc::new(move || (d)()) as Rc<dyn Fn()>
                    })
                ) {
                    Stack(gap = StackGap::Xs) {
                        Typography(content = "Edit".to_string())
                        Typography(content = "Duplicate".to_string())
                        Typography(content = "Delete".to_string(), tone = TypographyTone::Danger)
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
            Typography(content = "Drawer".to_string(), kind = TypographyKind::H2)
            Typography(content = "Same Overlay primitive, pinned to the right edge with a slide-in \
                              transition.".to_string(),
                 tone = TypographyTone::Muted)
            Btn(
                label = "Open drawer".to_string(),
                on_click = on_open,
                intent = IntentTag::Neutral,
                kind = ButtonKind::Soft,
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
                    placement = ViewportPlacement::Right,
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = {
                        let oc = on_close.clone();
                        move || (oc)()
                    }
                ) {
                    Card {
                        Typography(content = "Drawer".to_string(), kind = TypographyKind::H3)
                        Typography(content = "Right-edge drawer content.".to_string())
                        Btn(
                            label = "Close".to_string(),
                            on_click = on_close.clone(),
                            intent = IntentTag::Neutral,
                            kind = ButtonKind::Soft,
                        )
                    }
                }
            }
        }
    }
}
