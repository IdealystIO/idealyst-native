//! Overlays — Modal, Popover, Drawer (one page each).

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide, ViewportPlacement};
use runtime_core::{signal, ui, Easing, Element, PresenceAnim, PresenceState, PressableHandle, Ref};
use idea_ui::{
    tone, typography_kind, variant, Button, Card, Popover, Stack, StackGap, Typography,
};

use crate::shell::{self, Callout, CodePanel, ComponentPage, DemoSurface, H2, P, Section};

// =============================================================================
// Modal
// =============================================================================

pub fn modal() -> Element {
    let open = signal!(false);
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    shell::layout(ui! {
        ComponentPage(
            title = "Modal".to_string(),
            lead = "Viewport-centered overlay with a dismiss-on-click scrim. Built on the \
                framework's Overlay primitive — portaled to the document body so it escapes \
                parent overflow and stacking contexts.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            DemoSurface {
                Button(
                    label = "Open modal".to_string(),
                    on_click = on_open,
                    tone = tone::Primary,
                    variant = variant::Filled,
                )
                presence(
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
                    overlay(
                        placement = ViewportPlacement::Center,
                        backdrop = BackdropMode::Dismiss,
                        on_dismiss = {
                            let oc = on_close.clone();
                            move || (oc)()
                        },
                    ) {
                        Card {
                            Typography(content = "Confirm".to_string(), kind = typography_kind::H3)
                            Typography(content = "Click outside or press Escape to dismiss.".to_string())
                            Button(
                                label = "OK".to_string(),
                                on_click = on_close.clone(),
                                tone = tone::Primary,
                                variant = variant::Filled,
                            )
                        }
                    }
                }
            }

            Section(title = "Pattern".to_string()) {
                P(content = "Modal is composed from three primitives: a host-owned `Signal<bool>` \
                    for open/closed state, a `presence(...)` block for enter/exit animations, and \
                    an `overlay(...)` primitive that handles backdrop + dismiss routing.".to_string())
                CodePanel(src = r##"let open = signal!(false);
let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

ui! {
    Button(label = "Open".into(), on_click = on_open, tone = tone::Primary, variant = variant::Filled)

    presence(
        present = move || open.get(),
        enter = PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(8.0),
            200, Easing::EaseOut,
        ),
        exit = PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(8.0),
            150, Easing::EaseIn,
        ),
    ) {
        overlay(
            placement = ViewportPlacement::Center,
            backdrop = BackdropMode::Dismiss,
            on_dismiss = { let oc = on_close.clone(); move || (oc)() },
        ) {
            Card {
                Typography(content = "Confirm".into(), kind = typography_kind::H3)
                Typography(content = "Are you sure?".into())
                Button(label = "OK".into(), on_click = on_close, tone = tone::Primary, variant = variant::Filled)
            }
        }
    }
}"##.to_string())
            }

            Callout(label = "Why idea-ui doesn't ship a `Modal` wrapper".to_string()) {
                P(content = "The current shape — your own signal, presence, overlay — gives \
                    you direct control over the animation, dismiss policy, and content surface. \
                    A wrapper would have to invent a vocabulary for all three and still let you \
                    escape it. A future Modal component would compose these in the same shape; \
                    until then, the pattern above is the canonical recipe.".to_string())
            }
        }
    })
}

// =============================================================================
// Popover
// =============================================================================

pub fn popover() -> Element {
    let open = signal!(false);
    let trigger: Ref<PressableHandle> = Ref::new();
    let on_toggle: Rc<dyn Fn()> = Rc::new(move || open.update(|v| *v = !*v));
    let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    shell::layout(ui! {
        ComponentPage(
            title = "Popover".to_string(),
            lead = "Element-anchored overlay with no backdrop. The trigger binds a \
                `Ref<PressableHandle>`; the popover targets that ref and follows it through \
                scrolls and resizes.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            DemoSurface {
                Button(
                    label = "Open menu".to_string(),
                    on_click = on_toggle,
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    bind_to = Some(trigger),
                )
                presence(
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
                            Typography(content = "Delete".to_string(), tone = Some(tone::Danger.into()))
                        }
                    }
                }
            }

            Section(title = "Anchoring vs portaling".to_string()) {
                P(content = "Popover uses the framework's anchored_overlay primitive — the \
                    surface is portaled to the document body (escapes parent overflow) but \
                    positioned relative to the bound trigger Ref. Scrolling the trigger's \
                    container moves the popover with it; resizing the window re-runs \
                    positioning.".to_string())
            }

            Section(title = "Recipe — context menu".to_string()) {
                CodePanel(src = r##"let trigger: Ref<PressableHandle> = Ref::new();
let open = signal!(false);

ui! {
    Button(
        label = "More".into(),
        on_click = move || open.update(|v| *v = !*v),
        tone = tone::Neutral,
        variant = variant::Soft,
        bind_to = Some(trigger),
    )
    if open.get() {
        Popover(
            target = Some(AnchorTarget::from(trigger)),
            side = ElementSide::Below,
            align = ElementAlign::Start,
            offset = 6.0,
            on_dismiss = Some(Rc::new(move || open.set(false))),
        ) {
            Stack(gap = StackGap::Xs) {
                Typography(content = "Edit".into())
                Typography(content = "Duplicate".into())
                Typography(content = "Delete".into(), tone = Some(tone::Danger.into()))
            }
        }
    }
}"##.to_string())
            }
        }
    })
}

// =============================================================================
// Drawer (pattern)
// =============================================================================

pub fn drawer() -> Element {
    let open = signal!(false);
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let on_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

    shell::layout(ui! {
        ComponentPage(
            title = "Drawer pattern".to_string(),
            lead = "Same Overlay primitive as Modal, pinned to the right edge with a \
                slide-in transition. For navigation drawers (persistent on web wide \
                viewports), use the `drawer-navigator` SDK — that's what powers this \
                site's sidebar.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            DemoSurface {
                Button(
                    label = "Open drawer".to_string(),
                    on_click = on_open,
                    tone = tone::Neutral,
                    variant = variant::Soft,
                )
                presence(
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
                    overlay(
                        placement = ViewportPlacement::Right,
                        backdrop = BackdropMode::Dismiss,
                        on_dismiss = {
                            let oc = on_close.clone();
                            move || (oc)()
                        },
                    ) {
                        Card {
                            Typography(content = "Drawer".to_string(), kind = typography_kind::H3)
                            Typography(content = "Right-edge drawer content.".to_string())
                            Button(
                                label = "Close".to_string(),
                                on_click = on_close.clone(),
                                tone = tone::Neutral,
                                variant = variant::Soft,
                            )
                        }
                    }
                }
            }

            Section(title = "Transient drawer vs navigator drawer".to_string()) {
                P(content = "The demo above is a transient drawer — opens, dismisses, gone. \
                    For navigation chrome that persists across pages (the sidebar on this \
                    site), use `drawer_navigator::DrawerNavigator` instead. The navigator owns \
                    routing, breakpoint-based pinning, and per-platform sidebars.".to_string())
                CodePanel(src = r##"use drawer_navigator::{DrawerNavigator, DrawerBuilder};

let nav = DrawerNavigator::new(&HOME_ROUTE)
    .screen(HOME_ROUTE, |_| home_page())
    .screen(ABOUT_ROUTE, |_| about_page())
    .drawer_width(280.0)
    .leading_with(move |slot| sidebar(slot));

ui! { nav.bind(nav_ref) }"##.to_string())
            }
        }
    })
}
