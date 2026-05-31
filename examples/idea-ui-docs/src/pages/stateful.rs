//! Stateful — Tabs, Collapsible, Accordion.

use std::rc::Rc;

use runtime_core::{signal, ui, Element};
use idea_ui::{
    typography_kind, Accordion, AccordionExpand, AccordionItem, Collapsible,
    CollapsibleTransition, Stack, StackGap, Tab, Tabs, Typography,
};

use crate::shell::{
    self, Callout, CodePanel, ComponentPage, DemoSurface, H2, P, Prop, PropsTable, Section,
};

pub fn tabs() -> Element {
    let active = signal!(0usize);
    let on_change: Rc<dyn Fn(usize)> = Rc::new(move |idx| active.set(idx));

    let panel = runtime_core::switch(
        move || active.get(),
        |idx: &usize| match idx {
            0 => ui! {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "Overview".to_string(), kind = typography_kind::H3)
                    Typography(content = "The Overview tab is mounted whenever the active \
                                      index is 0; switching tabs disposes this subtree and \
                                      mounts a fresh one for the newly-active panel.".to_string(),
                         muted = true)
                }
            },
            1 => ui! {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "Activity".to_string(), kind = typography_kind::H3)
                    Typography(content = "Because the panel is rebuilt from scratch on every \
                                      tab change, signal subscriptions inside it release when \
                                      the user switches away — no stale effects accumulate.".to_string(),
                         muted = true)
                }
            },
            _ => ui! {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "Settings".to_string(), kind = typography_kind::H3)
                    Typography(content = "Per-project configuration. The strip doesn't dictate \
                                      panel layout — each branch returns whatever primitive \
                                      tree makes sense for that view.".to_string(),
                         muted = true)
                }
            },
        },
    );

    shell::layout(ui! {
        ComponentPage(
            title = "Tabs".to_string(),
            lead = "Clickable tab strip with reactive active highlighting. Owns the strip, \
                not the panel — content swap is wired by the caller via runtime_core::switch.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            DemoSurface {
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

            Section(title = "Why panel swap is the caller's job".to_string()) {
                P(content = "Tabs is intentionally minimal — it owns the strip and the active \
                    index, nothing else. The visual highlight stays in lockstep with whatever \
                    the caller treats as the source of truth (a local Signal, a route's \
                    active-index, anything). The strip never decides what \"active\" means, so \
                    it composes cleanly with content the framework doesn't know how to lay \
                    out — including future navigator integrations.".to_string())
            }

            Section(title = "Recipe".to_string()) {
                CodePanel(src = r##"let active = signal!(0_usize);
let on_change: Rc<dyn Fn(usize)> = Rc::new(move |idx| active.set(idx));

let panel = runtime_core::switch(
    move || active.get(),
    |idx: &usize| match idx {
        0 => ui! { /* Overview content */ },
        1 => ui! { /* Activity content */ },
        _ => ui! { /* Settings content */ },
    },
);

ui! {
    Tabs(
        active = active,
        on_change = on_change,
        tabs = vec![
            Tab::new("Overview"),
            Tab::new("Activity"),
            Tab::new("Settings"),
        ],
    )
    panel
}"##.to_string())
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "tabs",      ty: "Vec<Tab>",         desc: "Tabs in left-to-right order. Position in the vec is the tab's index." },
                    Prop { name: "active",    ty: "Signal<usize>",    desc: "Currently-active tab index. Drives the strip highlight and is also the caller's source of truth for panel swap." },
                    Prop { name: "on_change", ty: "Rc<dyn Fn(usize)>",desc: "Fires when the user taps a tab; receives the new index." },
                ])
            }

            Callout(label = "Tabs vs Segmented Control".to_string()) {
                P(content = "Use Tabs when each option opens a distinct content surface. For \
                    flipping a single value with three or four options (View mode: list / \
                    grid / cards), a segmented control would be more conventional — that's a \
                    follow-up component.".to_string())
            }
        }
    })
}

// =============================================================================
// Collapsible & Accordion
// =============================================================================

pub fn collapsible() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "Collapsible & Accordion".to_string(),
            lead = "Disclosure widgets: `Collapsible` for a single expand/collapse \
                section, `Accordion` for a coordinated group where only one item is \
                open at a time. Both are controlled — the host owns the open-state \
                signal so external triggers (an Expand-all button, URL params) can \
                drive them.".to_string(),
        ) {
            H2(content = "Collapsible".to_string())
            P(content = "Click the header to toggle. The body always stays mounted; \
                visibility flows through stylesheet variants or a measured animation \
                depending on the `transition` prop.".to_string())
            Section(title = "Measured (default)".to_string()) {
                P(content = "Cross-platform smooth. Measures the body's natural content \
                    height via `ViewHandle::on_layout`, then animates `MaxHeight` from \
                    0 → measured (and back) via the framework's animator. No fixed cap.".to_string())
                collapsible_demo_measured()
            }
            Section(title = "Snap".to_string()) {
                collapsible_demo_snap()
            }
            CodePanel(src = r##"let open = signal!(false);
let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| open.set(v));

ui! {
    Collapsible(
        title = "Advanced settings".into(),
        value = open,
        on_change = on_change,
    ) {
        Stack(gap = StackGap::Md) {
            Field(label = Some("API key".into()), value = key, on_change = on_key)
            Switch(label = Some("Beta features".into()), value = beta, on_change = on_beta)
        }
    }
}"##.to_string())

            Section(title = "Accordion — single-open (default)".to_string()) {
                P(content = "Multiple items, only one open at a time. Opening an item \
                    closes any other; clicking the open item closes it. The host owns the \
                    open-state vec (one bool per item); the Accordion writes to it on \
                    click and the host can read or mutate it directly for external \
                    triggers.".to_string())
                accordion_demo_single()
                CodePanel(src = r##"let open = signal!(vec![true, false, false]);  // first item open

ui! {
    Accordion(
        expand = AccordionExpand::Single,
        open = open,
        items = vec![
            AccordionItem { title: "Shipping".into(), body: ui!{ /* ... */ } },
            AccordionItem { title: "Returns".into(),  body: ui!{ /* ... */ } },
            AccordionItem { title: "Support".into(),  body: ui!{ /* ... */ } },
        ],
    )
}"##.to_string())
            }

            Section(title = "Accordion — multi-open".to_string()) {
                P(content = "Same component, `expand = AccordionExpand::Multi`. Each item \
                    is independent; clicking toggles just that one without touching \
                    the others.".to_string())
                accordion_demo_multi()
                CodePanel(src = r##"let open = signal!(vec![false; 3]);

ui! {
    Accordion(
        expand = AccordionExpand::Multi,
        open = open,
        items = vec![
            AccordionItem { title: "Push".into(),  body: ui!{ /* ... */ } },
            AccordionItem { title: "Email".into(), body: ui!{ /* ... */ } },
            AccordionItem { title: "SMS".into(),   body: ui!{ /* ... */ } },
        ],
    )
}"##.to_string())
            }

            Section(title = "Collapsible props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "title",
                        ty: "Reactive<String>",
                        desc: "Header text. Static literal, Signal<String>, or rx!(...) all work.",
                    },
                    Prop {
                        name: "value",
                        ty: "Signal<bool>",
                        desc: "Controlled open state. The host owns this — pass `signal!(false)` for a default-closed Collapsible.",
                    },
                    Prop {
                        name: "on_change",
                        ty: "Rc<dyn Fn(bool)>",
                        desc: "Fires on header click with the requested new state. Wire to `Rc::new(move |v| value.set(v))` for standard toggle behavior.",
                    },
                    Prop {
                        name: "transition",
                        ty: "CollapsibleTransition",
                        desc: "Measured (default) — animates AnimProp::MaxHeight 0↔measured-content-height via the framework animator. Snap — instant toggle.",
                    },
                    Prop {
                        name: "duration_ms",
                        ty: "u32",
                        desc: "Open/close animation duration in milliseconds. Default 240. Only meaningful when transition = Measured (Snap is instant). Keep close to 240 — chrome transitions (padding, opacity) are baked into the stylesheet at that timing.",
                    },
                    Prop {
                        name: "children",
                        ty: "Vec<Element>",
                        desc: "Body contents. Always mounted; visibility flows through the stylesheet variant axis selected by `transition`.",
                    },
                ])
            }

            Section(title = "Accordion props".to_string()) {
                PropsTable(rows = vec![
                    Prop {
                        name: "items",
                        ty: "Vec<AccordionItem>",
                        desc: "Items in order. Each is { title: Reactive<String>, body: Element }.",
                    },
                    Prop {
                        name: "expand",
                        ty: "AccordionExpand",
                        desc: "Single (only one open at a time) or Multi (any subset). Default: Single.",
                    },
                    Prop {
                        name: "transition",
                        ty: "CollapsibleTransition",
                        desc: "Per-item open/close animation. Same vocabulary as Collapsible.",
                    },
                    Prop {
                        name: "duration_ms",
                        ty: "u32",
                        desc: "Per-item duration in milliseconds. Forwarded to each Collapsible. See Collapsible.duration_ms.",
                    },
                    Prop {
                        name: "open",
                        ty: "Signal<Vec<bool>>",
                        desc: "Per-item open state, parallel to items (open[i] == true ⇔ item i is expanded). Auto-resized to match items.len() on first interaction; pass signal!(vec![false; N]) for a default-closed Accordion.",
                    },
                    Prop {
                        name: "on_change",
                        ty: "Option<Rc<dyn Fn(usize, bool)>>",
                        desc: "Optional notification callback. Fires AFTER the Accordion has already mutated `open` in response to a click. Use for analytics, persistence — the state change is the Accordion's job, this is observation.",
                    },
                ])
            }

            Callout(label = "Transition extensibility".to_string()) {
                P(content = "`CollapsibleTransition` is the extensibility seam — \
                    `Measured` measures the body via `ViewHandle::on_layout` and \
                    animates `AnimProp::MaxHeight` to the natural content height \
                    via the framework's animator; `Snap` skips animation. New \
                    flavors land here by adding stylesheet variants in idea-ui; \
                    the component picks the right one per the prop. Apps that \
                    want reduced-motion can wire `prefers-reduced-motion` to \
                    `CollapsibleTransition::Snap` themselves.".to_string())
            }
        }
    })
}

fn collapsible_demo_measured() -> Element {
    let open = signal!(false);
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| open.set(v));
    ui! {
        DemoSurface {
            Collapsible(
                title = "Measured — click to expand".to_string(),
                value = open,
                on_change = on_change,
                transition = CollapsibleTransition::Measured,
            ) {
                Stack(gap = StackGap::Sm) {
                    Typography(
                        content = "The body's natural height is measured via the \
                            framework's `ViewHandle::on_layout` (web `ResizeObserver`, \
                            iOS `layoutSubviews`, Android `OnLayoutChangeListener`). \
                            The animator tweens `AnimProp::MaxHeight` between 0 and \
                            that measured value.".to_string(),
                    )
                    Typography(
                        content = "Content changes re-measure automatically — the next \
                            toggle uses the new natural height. No fixed cap, so any \
                            content size animates smoothly end-to-end.".to_string(),
                        muted = true,
                    )
                }
            }
        }
    }
}

fn collapsible_demo_snap() -> Element {
    let open = signal!(false);
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| open.set(v));
    ui! {
        DemoSurface {
            Collapsible(
                title = "Snap — click to expand".to_string(),
                value = open,
                on_change = on_change,
                transition = CollapsibleTransition::Snap,
            ) {
                Stack(gap = StackGap::Sm) {
                    Typography(
                        content = "No animation — the body appears in one frame. Cheap, \
                            predictable, and the right call for reduced-motion users.".to_string(),
                    )
                }
            }
        }
    }
}

fn accordion_demo_single() -> Element {
    let open = signal!(vec![true, false, false]);
    ui! {
        DemoSurface {
            Accordion(
                expand = AccordionExpand::Single,
                open = open,
                items = vec![
                    AccordionItem {
                        title: "Shipping".into(),
                        body: ui! {
                            Stack(gap = StackGap::Sm) {
                                Typography(content = "Free shipping on orders over $50.".to_string())
                                Typography(
                                    content = "Standard delivery: 3–5 business days. \
                                        Expedited options available at checkout.".to_string(),
                                    muted = true,
                                )
                            }
                        },
                    },
                    AccordionItem {
                        title: "Returns".into(),
                        body: ui! {
                            Stack(gap = StackGap::Sm) {
                                Typography(content = "30-day return window from delivery date.".to_string())
                                Typography(
                                    content = "Items must be unused and in original \
                                        packaging. Refunds process within 5 business \
                                        days of receipt.".to_string(),
                                    muted = true,
                                )
                            }
                        },
                    },
                    AccordionItem {
                        title: "Support".into(),
                        body: ui! {
                            Stack(gap = StackGap::Sm) {
                                Typography(content = "Reach us by email or chat.".to_string())
                                Typography(
                                    content = "Average first-response time: under 4 hours \
                                        during business hours, under 24 hours \
                                        otherwise.".to_string(),
                                    muted = true,
                                )
                            }
                        },
                    },
                ]
            )
        }
    }
}

fn accordion_demo_multi() -> Element {
    let open = signal!(vec![false; 3]);
    ui! {
        DemoSurface {
            Accordion(
                expand = AccordionExpand::Multi,
                open = open,
                items = vec![
                    AccordionItem {
                        title: "Push notifications".into(),
                        body: ui! {
                            Stack(gap = StackGap::Sm) {
                                Typography(content = "Real-time alerts on your device.".to_string())
                                Typography(
                                    content = "Independent of other notification channels — \
                                        any combination is fine.".to_string(),
                                    muted = true,
                                )
                            }
                        },
                    },
                    AccordionItem {
                        title: "Email notifications".into(),
                        body: ui! {
                            Stack(gap = StackGap::Sm) {
                                Typography(content = "Digest delivered daily or instantly.".to_string())
                                Typography(
                                    content = "Both can be on at the same time — try opening \
                                        Push too. The Accordion only enforces single-open in \
                                        Single mode.".to_string(),
                                    muted = true,
                                )
                            }
                        },
                    },
                    AccordionItem {
                        title: "SMS notifications".into(),
                        body: ui! {
                            Stack(gap = StackGap::Sm) {
                                Typography(content = "Text messages for critical alerts only.".to_string())
                                Typography(
                                    content = "Stays open independently of the other items.".to_string(),
                                    muted = true,
                                )
                            }
                        },
                    },
                ]
            )
        }
    }
}
