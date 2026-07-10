//! `TabBar` — a themed tab bar wired to a swap navigator's outlet.
//!
//! Piggybacks on `idea-ui`'s pure-UI [`Tabs`](idea_ui::Tabs) strip: `TabBar`
//! is the thin bridge from the navigator's
//! [`SwapContext`](runtime_core::primitives::navigator::SwapContext) — its
//! `active_route` signal and `on_select` dispatcher — to the strip's
//! id-keyed `active` / `on_change` contract. The strip renders and highlights;
//! `TabBar` translates route names ↔ tab ids and dispatches `Select`.

use idea_ui::{Tab, TabIndicator, Tabs};
use runtime_core::{component, rx, ui, Element, IdealystSchema, Signal};
use std::rc::Rc;

/// One entry in a [`TabBar`]: the screen it selects and its visible label.
///
/// `route` is the swap navigator's route *name* (`&'static str`) — the same
/// key `SwapContext::active_route` reports and `on_select` expects — so the bar
/// highlights the live screen and switching dispatches the right `Select`.
#[derive(Clone, Default, IdealystSchema)]
pub struct TabItem {
    /// The route name this tab selects (matches `SwapContext::active_route`).
    pub route: &'static str,
    /// The visible label.
    pub label: String,
}

impl TabItem {
    /// A tab for `route` labelled `label`.
    pub fn new(route: &'static str, label: impl Into<String>) -> Self {
        Self { route, label: label.into() }
    }
}

// Reactive-by-default: `#[props]` wraps the scalar `indicator` →
// `Reactive<TabIndicator>`; the `items` `Vec` collection, the `active_route`
// `Signal`, and the `on_select` handler are all auto-skipped.
/// Props for [`TabBar`] — the tab list plus the navigator's live
/// `active_route` and `on_select` from its `SwapContext`.
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct TabBarProps {
    /// The tabs, left-to-right — each a `(route, label)`.
    pub items: Vec<TabItem>,
    /// The navigator's live active route (from `SwapContext::active_route`).
    /// The tab whose `route` equals this paints selected.
    pub active_route: Signal<&'static str>,
    /// The navigator's screen switcher (from `SwapContext::on_select`). Tapping
    /// a tab dispatches `Select` for its route. Default no-op so an unwired bar
    /// doesn't silently navigate.
    pub on_select: Rc<dyn Fn(&'static str)>,
    /// How the active tab is marked — see [`TabIndicator`]. Default underline.
    pub indicator: TabIndicator,
}

impl Default for TabBarProps {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            active_route: Signal::new(""),
            on_select: Rc::new(|_| {}),
            indicator: runtime_core::Reactive::Static(TabIndicator::default()),
        }
    }
}

/// Renders a themed tab bar for a swap navigator. Bridges the navigator's
/// `active_route` / `on_select` to `idea-ui`'s [`Tabs`] strip: the strip keys
/// on tab `id` (here the route name), so highlighting tracks the live route and
/// a tap dispatches `Select` for the tapped route.
#[component]
pub fn TabBar(props: TabBarProps) -> Element {
    let items = props.items;
    let active_route = props.active_route;
    let on_select = props.on_select;
    let indicator = props.indicator;

    // The strip's id-keyed tab list. `Tab::id` is the route name; a fixed bar
    // wraps a literal list in a static `Signal`.
    let tabs = Signal::new(
        items.iter().map(|i| Tab::new(i.route, i.label.clone())).collect::<Vec<_>>(),
    );

    // The strip's `active` is the route name as a live string; `rx!` re-runs the
    // highlight when the navigator's `active_route` changes.
    let active = rx!(active_route.get().to_string());

    // Translate the tapped tab's `id` (a `String`) back to its `&'static str`
    // route and dispatch `Select`. The route names are `&'static str`, so the
    // lookup is over the original static keys — no leak, no allocation-to-static.
    let routes: Vec<&'static str> = items.iter().map(|i| i.route).collect();
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| {
        if let Some(route) = routes.iter().copied().find(|r| *r == id.as_str()) {
            on_select(route);
        }
    });

    ui! {
        Tabs(tabs = tabs, active = active, on_change = on_change, indicator = indicator)
    }
}
