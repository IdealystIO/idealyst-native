//! Stateful — Tabs and Avatar.

use std::rc::Rc;

use runtime_core::{signal, ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{Avatar, Typography, Card, Stack, Tabs, AvatarProps, StackGap, Tab};

use crate::shell::{demo_card, page_header};

pub fn page() -> Element {
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

fn avatar_demo() -> Element {
    let state = AvatarProps::init_state();
    state.initials.set("AB".to_string());

    let preview = AvatarProps::reactive_preview(&state, |props| {
        let initials = props.initials;
        let color = props.color;
        let size = props.size;
        ui! {
            Avatar(initials = initials, color = color, size = size)
        }
    });
    let controls = AvatarProps::render_controls(&state);
    demo_card(
        "Avatar",
        "Circular user-identity element. Renders an image when `src` is set, otherwise \
         falls back to initials on a `color`-tinted background — the color is a separate \
         axis from Intent because avatars don't represent semantic actions.",
        preview,
        controls,
    )
}

fn tabs_demo() -> Element {
    // `Tabs` is intentionally minimal: it owns the strip and the
    // active-index signal, nothing else. Panel switching is wired
    // by the caller via `runtime_core::switch`, keyed off the
    // same signal. This keeps `Tabs` composable with any panel
    // content the framework knows how to render — including future
    // navigator-routed integrations — without baking a panel slot
    // into the component's surface.
    let active = signal!(0usize);
    let on_change: Rc<dyn Fn(usize)> = Rc::new(move |idx| active.set(idx));

    let panel = runtime_core::switch(
        move || active.get(),
        |idx: &usize| match idx {
            0 => ui! {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "Overview".to_string(), kind = idea_ui::typography_kind::H3)
                    Typography(content = "High-level summary of the active project. The Overview \
                                      tab is mounted whenever the active index is 0; switching \
                                      tabs disposes this subtree and mounts a fresh one for the \
                                      newly-active panel.".to_string(),
                         muted = true)
                }
            },
            1 => ui! {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "Activity".to_string(), kind = idea_ui::typography_kind::H3)
                    Typography(content = "Recent events would render here. Because the panel is \
                                      rebuilt from scratch on every tab change, any signal \
                                      subscriptions inside it are released when the user \
                                      switches away — no stale effects accumulate.".to_string(),
                         muted = true)
                }
            },
            _ => ui! {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "Settings".to_string(), kind = idea_ui::typography_kind::H3)
                    Typography(content = "Per-project configuration would render here. The strip \
                                      doesn't dictate panel layout — each branch can return \
                                      whatever primitive tree makes sense for that view.".to_string(),
                         muted = true)
                }
            },
        },
    );

    ui! {
        Card {
            Typography(content = "Tabs".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Controlled by a `Signal<usize>` indexing the active tab. \
                              Tap a tab to swap the panel below — panel content is wired \
                              by the caller via `runtime_core::switch`, keyed off the same \
                              signal that drives the strip's highlight.".to_string(),
                 muted = true)
            Tabs(
                active = active,
                on_change = on_change,
                tabs = vec![
                    Tab::new("Overview"),
                    Tab::new("Activity"),
                    Tab::new("Settings"),
                ]
            )
            panel
        }
    }
}
