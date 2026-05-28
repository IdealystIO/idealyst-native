//! `Tabs` — clickable tab strip with reactive active highlighting.
//!
//! Pure UI: takes an `active: Signal<usize>` and an `on_change`
//! callback, renders the tab buttons, and leaves content swap
//! entirely to the caller. Two reasons that's the right shape:
//!
//! 1. The visual highlight stays in lockstep with whatever the
//!    caller treats as the source of truth — a local signal for an
//!    in-card switcher, a route's active-index for a tab-routed
//!    navigator, anything else with the right type. The strip
//!    never decides what "active" means.
//! 2. The author wires the content swap themselves (typically a
//!    `when()` block or a `match` on `active.get()`), so the strip
//!    composes cleanly with content the framework doesn't know how
//!    to lay out — including future navigator integrations.
//!
//! ```ignore
//! let active = signal!(0_usize);
//! ui! {
//!     Tabs(
//!         tabs = vec![
//!             Tab { label: "One".into() },
//!             Tab { label: "Two".into() },
//!         ],
//!         active = active,
//!         on_change = move |idx| active.set(idx),
//!     )
//!     // ... caller renders content driven by `active.get()` ...
//! }
//! ```

use runtime_core::{pressable, text, ui, Element, Reactive, Signal, StyleApplication};
use std::rc::Rc;

use crate::stylesheets::{TabBar, TabButton};

/// One entry in the tab strip. Position in the props' `tabs` vec
/// is its identity — `Signal<usize>` indexes into the same vec.
#[derive(Clone, Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct Tab {
    /// Human-readable label shown on the tab. `Reactive<String>` —
    /// static or live (signal/`rx!`).
    pub label: Reactive<String>,
}

impl Tab {
    pub fn new(label: impl Into<Reactive<String>>) -> Self {
        Self { label: label.into() }
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TabsProps {
    /// Tabs in left-to-right order. Position in the vec is the tab's
    /// index — what the active Signal points at and what `on_change`
    /// hands back on tap.
    pub tabs: Vec<Tab>,
    /// Currently-active tab index. Drives the per-tab highlight via
    /// a reactive style closure and is the canonical state the
    /// caller should also read to render the active tab's content.
    pub active: Signal<usize>,
    /// Fires when the user taps a tab; receives the new index.
    /// Default is a no-op so an unwired Tabs doesn't silently
    /// mutate — pass `move |idx| active.set(idx)` to make taps
    /// actually switch.
    pub on_change: Rc<dyn Fn(usize)>,
}

impl Default for TabsProps {
    // Manual impl: `Signal<usize>` and `Rc<dyn Fn(usize)>` don't
    // derive `Default`. Mirrors the same pattern as `SwitchProps`.
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: Signal::new(0),
            on_change: Rc::new(|_| {}),
        }
    }
}

pub fn tabs(props: TabsProps) -> Element {
    let tab_items = props.tabs;
    let active = props.active;
    let on_change = props.on_change;

    let container_style = TabBar();

    let mut children: Vec<Element> = Vec::with_capacity(tab_items.len());
    for (idx, item) in tab_items.into_iter().enumerate() {
        // Each tab gets its own captured copy of `idx` + `on_change`,
        // so the press closure dispatches the right index regardless
        // of how many other tabs were declared.
        let label = item.label;
        let on_change_for_tab = on_change.clone();
        let press = move || on_change_for_tab(idx);

        // Reactive style closure: re-runs whenever `active` fires,
        // flipping the `active` variant between `on` and `off`. The
        // stylesheet handles the color + underline transition.
        let tab_style = move || {
            let variant = if active.get() == idx { "on" } else { "off" };
            StyleApplication::new(TabButton::sheet()).with("active", variant.to_string())
        };

        // Build the tab as `Pressable { Text }` via the builder
        // functions — `Pressable` isn't a ui!-level tag (the
        // framework macro deliberately omits it so idea-ui can own
        // the styled wrapper), so we construct it directly.
        let label_primitive: Element = text(label).into();
        let tab_primitive: Element = pressable(vec![label_primitive], press)
            .with_style(tab_style)
            .into();
        children.push(tab_primitive);
    }

    ui! { View(style = container_style) { children } }
}
