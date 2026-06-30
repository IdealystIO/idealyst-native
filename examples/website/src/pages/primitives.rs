//! Primitives — the framework's leaf building blocks, all on one screen.
//!
//! Every Idealyst app (and every component library built on top, including
//! idea-ui) reduces to a tree of these. There's no way to add a new
//! primitive without changing the framework itself — each one maps to a
//! method on the `Backend` trait. What you compose out of them is
//! unbounded; the set itself is fixed and small.
//!
//! This page demonstrates each leaf primitive LIVE, using its lowercase
//! `ui!` tag (primitives are snake_case; PascalCase is reserved for
//! `#[component]`s — see CLAUDE.md §9.2). Every cell is real: the sliders
//! slide, the overlays open, the list virtualizes, all on whatever backend
//! is rendering this page right now.

use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide, ViewportPlacement};
use runtime_core::primitives::presence::PresenceAnim;
use runtime_core::{component, signal, ui, Color, Easing, Element, Ref, ViewHandle};
use idea_ui::{Stack, StackGap, Typography};

use crate::branding::LIGHT_LOGO;
use crate::pages::common::{PageHeader, PageSection};
use crate::routes::{CONCEPTS_ROUTE, HOME_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};
use crate::styles::{
    PrimCard, PrimControl, PrimGrid, PrimIcon, PrimPopCard, PrimRow, PrimScrollBox, PrimSwatch,
    PrimTag,
};

pub fn page() -> Element {
    let containers_ref: Ref<ViewHandle> = Ref::new();
    let content_ref: Ref<ViewHandle> = Ref::new();
    let inputs_ref: Ref<ViewHandle> = Ref::new();
    let feedback_ref: Ref<ViewHandle> = Ref::new();
    let nav_ref: Ref<ViewHandle> = Ref::new();
    let lists_ref: Ref<ViewHandle> = Ref::new();
    let floating_ref: Ref<ViewHandle> = Ref::new();
    let flow_ref: Ref<ViewHandle> = Ref::new();
    let gpu_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: containers_ref, label: "Containers" },
        TocEntry { handle: content_ref, label: "Content" },
        TocEntry { handle: inputs_ref, label: "Inputs" },
        TocEntry { handle: feedback_ref, label: "Feedback" },
        TocEntry { handle: nav_ref, label: "Navigation" },
        TocEntry { handle: lists_ref, label: "Lists" },
        TocEntry { handle: floating_ref, label: "Floating & transitions" },
        TocEntry { handle: flow_ref, label: "Control flow" },
        TocEntry { handle: gpu_ref, label: "GPU" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Primitives",
                blurb = "The fixed, minimal set of things the framework knows how to put on \
                 screen. Everything else \u{2014} every component, every screen, idea-ui \
                 itself \u{2014} composes out of these. Each cell below is the real primitive \
                 rendering live on this backend.",
            )
            PageSection(handle = containers_ref) { containers() }
            PageSection(handle = content_ref) { content_primitives() }
            PageSection(handle = inputs_ref) { inputs() }
            PageSection(handle = feedback_ref) { feedback() }
            PageSection(handle = nav_ref) { navigation() }
            PageSection(handle = lists_ref) { lists() }
            PageSection(handle = floating_ref) { floating() }
            PageSection(handle = flow_ref) { control_flow() }
            PageSection(handle = gpu_ref) { gpu() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// PrimSection — H2 + intro paragraph + a wrapping grid of primitive cells.
// =============================================================================

#[derive(Default)]
pub struct PrimSectionProps {
    pub title: String,
    pub blurb: String,
    pub children: Vec<Element>,
}

/// A titled group of primitive cells. The children are the cells (one
/// `PrimCell` per primitive); they flow into the `PrimGrid` wrapping row.
#[component]
pub fn PrimSection(props: PrimSectionProps) -> Element {
    let title = props.title;
    let blurb = props.blurb;
    let cells = props.children;
    let grid_style = PrimGrid();
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = title, kind = idea_ui::typography_kind::H2)
            Typography(content = blurb, muted = true)
            view(style = grid_style) { cells }
        }
    }
}

// =============================================================================
// PrimCell — one bordered cell: a monospace tag, a one-line blurb, and the
// live primitive instance. Container component (children move out of props),
// matching the `Card` / `Center` pattern (CLAUDE.md §9.3).
// =============================================================================

#[derive(Default)]
pub struct PrimCellProps {
    pub tag: &'static str,
    pub blurb: &'static str,
    pub children: Vec<Element>,
}

#[component]
pub fn PrimCell(props: PrimCellProps) -> Element {
    let tag = props.tag.to_string();
    let blurb = props.blurb.to_string();
    let demo = props.children;
    let card_style = PrimCard();
    let tag_style = PrimTag();
    ui! {
        view(style = card_style) {
            text(style = tag_style) { tag }
            Typography(content = blurb, kind = idea_ui::typography_kind::Caption, muted = true)
            demo
        }
    }
}

// =============================================================================
// Containers — view, scroll_view
// =============================================================================

fn containers() -> Element {
    ui! {
        PrimSection(
            title = "Containers",
            blurb = "The structural primitives. Everything else nests inside one of these.",
        ) {
            PrimCell(tag = "view", blurb = "A flex box. The structural workhorse — no behavior of its own.") {
                view(style = PrimSwatch()) {}
            }
            PrimCell(tag = "scroll_view", blurb = "A view that scrolls its overflowing content.") {
                scroll_view(style = PrimScrollBox()) {
                    for i in 0..14 {
                        text { format!("Scrollable row {}", i + 1) }
                    }
                }
            }
        }
    }
}

// =============================================================================
// Content — text, image, icon
// =============================================================================

fn content_primitives() -> Element {
    // A self-contained SVG data URI — no network dependency, so the
    // `image` demo paints reliably on the web target without an asset
    // pipeline. (`#` is percent-encoded as `%23` inside the data URI.)
    let img_src = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20\
                   width='96'%20height='96'%3E%3Crect%20width='96'%20height='96'%20\
                   rx='16'%20fill='%235a4fcf'/%3E%3Ccircle%20cx='48'%20cy='40'%20r='16'%20\
                   fill='%23ffd24a'/%3E%3C/svg%3E";
    ui! {
        PrimSection(
            title = "Content",
            blurb = "Primitives that render media — text runs, bitmaps, and vector icons.",
        ) {
            PrimCell(tag = "text", blurb = "A run of text. Reactive when its child reads a signal.") {
                text { "The quick brown fox jumps over the lazy dog." }
            }
            PrimCell(tag = "image", blurb = "A bitmap from a URL (here, an inline SVG data URI).") {
                image(src = img_src, alt = "A purple square with a yellow dot")
            }
            PrimCell(tag = "icon", blurb = "A vector icon — inline SVG on web, CAShapeLayer on iOS, VectorDrawable on Android.") {
                icon(data = LIGHT_LOGO, color = move || Color("#5a4fcf".into()), style = PrimIcon())
            }
        }
    }
}

// =============================================================================
// Inputs — button, text_input, toggle, slider
// =============================================================================

fn inputs() -> Element {
    // text_input
    let name = signal!("Idealyst".to_string());
    // toggle
    let enabled = signal!(true);
    // slider
    let volume = signal!(0.5_f32);
    // button
    let clicks = signal!(0_i32);

    ui! {
        PrimSection(
            title = "Inputs",
            blurb = "Controlled primitives — the host owns the value as a signal; the input \
             round-trips through on_change.",
        ) {
            PrimCell(tag = "button", blurb = "A labeled tappable with native button semantics.") {
                view(style = PrimRow()) {
                    button(label = "Tap me", on_click = move || clicks.update(|n| *n += 1))
                    text { format!("{} clicks", clicks.get()) }
                }
            }
            PrimCell(tag = "text_input", blurb = "A text field whose value is owned by the parent.") {
                view(style = PrimControl()) {
                    text_input(
                        value = name,
                        on_change = move |s| name.set(s),
                        placeholder = "Type here".to_string(),
                    )
                }
            }
            PrimCell(tag = "toggle", blurb = "A switch bound to a Signal<bool>.") {
                toggle(value = enabled, on_change = move |v| enabled.set(v))
            }
            PrimCell(tag = "slider", blurb = "A numeric slider with min/max bounds and a step.") {
                view(style = PrimControl()) {
                    slider(
                        value = volume,
                        on_change = move |v| volume.set(v),
                        min = 0.0,
                        max = 1.0,
                        step = 0.05,
                    )
                }
            }
        }
    }
}

// =============================================================================
// Feedback — activity_indicator
// =============================================================================

fn feedback() -> Element {
    ui! {
        PrimSection(
            title = "Feedback",
            blurb = "Primitives whose only job is to show that something is happening.",
        ) {
            PrimCell(tag = "activity_indicator", blurb = "An indeterminate loading spinner. No value — it just spins.") {
                view(style = PrimRow()) {
                    activity_indicator(size = ActivityIndicatorSize::Small)
                    activity_indicator(size = ActivityIndicatorSize::Large, color = Color("#5a4fcf".into()))
                }
            }
        }
    }
}

// =============================================================================
// Navigation — link
// =============================================================================

fn navigation() -> Element {
    ui! {
        PrimSection(
            title = "Navigation",
            blurb = "Declarative navigation — a link dispatches against the closest ambient \
             navigator (and emits a real <a href> on web).",
        ) {
            PrimCell(tag = "link", blurb = "Wraps a child that navigates when pressed.") {
                link(route = &CONCEPTS_ROUTE, params = ()) {
                    text(style = PrimTag()) { "Go to Core concepts \u{2192}" }
                }
            }
        }
    }
}

// =============================================================================
// Lists — flat_list
// =============================================================================

fn lists() -> Element {
    let people: runtime_core::Signal<Vec<(u32, &'static str)>> = signal!(vec![
        (1, "Aria"),
        (2, "Bram"),
        (3, "Cleo"),
        (4, "Dario"),
        (5, "Esme"),
        (6, "Faye"),
        (7, "Gus"),
        (8, "Hana"),
        (9, "Iris"),
        (10, "Jonas"),
        (11, "Kira"),
        (12, "Liam"),
    ]);
    ui! {
        PrimSection(
            title = "Lists",
            blurb = "The virtualized-list entry point — only the visible rows are realized. \
             Backends drive their native virtualization widget.",
        ) {
            PrimCell(tag = "flat_list", blurb = "A virtualized list. Pass data, a stable key, an item size, and a row renderer.") {
                flat_list(
                    data = people,
                    key = |_idx, item: &(u32, &'static str)| item.0 as u64,
                    size = runtime_core::primitives::flat_list::fixed_size(36.0),
                    render = |idx, item: &(u32, &'static str)| ui! {
                        view(style = PrimRow()) {
                            text(style = PrimTag()) { format!("{}", idx + 1) }
                            text { item.1.to_string() }
                        }
                    },
                    style = PrimScrollBox(),
                )
            }
        }
    }
}

// =============================================================================
// Floating & transitions — overlay, anchored_overlay, presence
// =============================================================================

fn floating() -> Element {
    ui! {
        PrimSection(
            title = "Floating & transitions",
            blurb = "Subtrees that escape the parent's layout and clipping (modals, popovers) \
             plus animated mount/unmount. The host owns the open/close signal.",
        ) {
            overlay_cell()
            anchored_overlay_cell()
            presence_cell()
        }
    }
}

fn overlay_cell() -> Element {
    let open = signal!(false);
    ui! {
        PrimCell(tag = "overlay", blurb = "A viewport-anchored portal — centered, with a dismissible backdrop. Use for modals.") {
            view(style = PrimRow()) {
                button(label = "Open modal", on_click = move || open.set(true))
            }
            // `if` inside `ui!` lowers to `when`: the overlay mounts only
            // while `open` is true, and unmounting closes it.
            if open.get() {
                overlay(
                    placement = ViewportPlacement::Center,
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = move || open.set(false),
                ) {
                    view(style = PrimPopCard()) {
                        Typography(content = "I'm a modal".to_string(), kind = idea_ui::typography_kind::H3)
                        Typography(content = "Rendered in a window-level portal, above everything else.".to_string(), muted = true)
                        button(label = "Close", on_click = move || open.set(false))
                    }
                }
            }
        }
    }
}

fn anchored_overlay_cell() -> Element {
    let open = signal!(false);
    let anchor: Ref<ViewHandle> = Ref::new();
    ui! {
        PrimCell(tag = "anchored_overlay", blurb = "A portal that tracks a trigger element — popovers, dropdowns, tooltips.") {
            view(style = PrimRow()) {
                button(label = "Toggle popover", on_click = move || open.update(|o| *o = !*o))
            }.bind(anchor)
            if open.get() {
                anchored_overlay(
                    target = AnchorTarget::from(anchor),
                    side = ElementSide::Below,
                    align = ElementAlign::Start,
                    offset = 6.0,
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = move || open.set(false),
                ) {
                    view(style = PrimPopCard()) {
                        Typography(content = "Anchored below the trigger".to_string(), muted = true)
                    }
                }
            }
        }
    }
}

fn presence_cell() -> Element {
    let shown = signal!(true);
    ui! {
        PrimCell(tag = "presence", blurb = "Mounts and unmounts with enter/exit animations — the exit plays before the node leaves.") {
            view(style = PrimRow()) {
                button(label = "Toggle", on_click = move || shown.update(|s| *s = !*s))
            }
            presence(
                present = move || shown.get(),
                enter = PresenceAnim::fade(220, Easing::EaseOut),
                exit = PresenceAnim::fade(180, Easing::EaseIn),
            ) {
                view(style = PrimSwatch()) {}
            }
        }
    }
}

// =============================================================================
// Control flow — when
// =============================================================================

fn control_flow() -> Element {
    let on = signal!(false);
    ui! {
        PrimSection(
            title = "Control flow",
            blurb = "Reactive structure. You write a plain `if` (or `match`, or `for`) inside \
             ui!, and the macro lowers it to the `when` primitive — the old branch's scope \
             drops and the new one builds fresh when the signal changes.",
        ) {
            PrimCell(tag = "when", blurb = "Reactive if/else. Toggling the signal swaps the branch.") {
                view(style = PrimRow()) {
                    button(label = "Flip", on_click = move || on.update(|v| *v = !*v))
                    if on.get() {
                        text(style = PrimTag()) { "Branch: ON" }
                    } else {
                        text { "Branch: OFF" }
                    }
                }
            }
        }
    }
}

// =============================================================================
// GPU — graphics
// =============================================================================

fn gpu() -> Element {
    ui! {
        PrimSection(
            title = "GPU",
            blurb = "The escape hatch into raw rendering. The framework hands you a native \
             GPU surface (a wgpu device) and stays out of the way — you own every pixel.",
        ) {
            PrimCell(tag = "graphics", blurb = "A GPU canvas. on_ready gives you a wgpu surface; the framework never interprets what you draw.") {
                // `graphics` only shows something once the author plugs a
                // renderer into its `on_ready` surface — a no-op canvas would
                // paint nothing. Rather than embed a second heavyweight wgpu
                // context here, point at the home page, where this exact
                // primitive drives the live device simulator.
                link(route = &HOME_ROUTE, params = ()) {
                    text(style = PrimTag()) { "See it running on the home page \u{2192}" }
                }
            }
        }
    }
}
