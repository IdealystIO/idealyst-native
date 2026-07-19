//! `CardTabs` — a docs-specific composition: `Card` wraps an
//! idea-ui `Tabs` strip on top of a `TabPanel` that shows the
//! active tab's body. Used for framework-comparison content where
//! the same idea is shown as Idealyst / React Native / SwiftUI /
//! etc. side-by-side.
//!
//! Invocation shape (via the `ui!` macro's `cardtabs` emitter):
//!
//! ```ignore
//! ui! {
//!     CardTabs {
//!         Tab(label = "Idealyst") {
//!             CodeBlock(code = idealyst_sample)
//!         }
//!         Tab(label = "React Native") {
//!             CodeBlock(code = react_sample)
//!         }
//!     }
//! }
//! ```
//!
//! The macro de-sugars each `Tab(...)` child into a
//! `(label_string, render_closure)` pair so panels mount lazily —
//! the inactive tabs never build their `Element` tree until the
//! user switches to them. The `switch(...)` primitive subscribes
//! to the active-index Signal and swaps which closure runs.

use std::rc::Rc;

use runtime_core::{component, signal, switch, ui, view, Element, Signal};
use idea_ui::{Card, Tab, TabPanel, Tabs, TabsProps};

/// Props delivered by the `cardtabs!` invocation macro. Each entry
/// is a `(label, render_closure)` pair — the macro wraps each
/// `Tab` child's body in `Rc::new(move || ... )` so the closure
/// can be cheaply cloned into the switch's branch closure without
/// requiring `Element: Clone`.
pub struct CardTabsProps {
    pub tabs: Vec<(String, Rc<dyn Fn() -> Element>)>,
}

impl Default for CardTabsProps {
    fn default() -> Self {
        Self { tabs: Vec::new() }
    }
}

#[component]
pub fn CardTabs(props: CardTabsProps) -> Element {
    // Local Signal owns the active tab's *id* (idea-ui `Tabs` is
    // id-keyed now — `active`/`on_change` carry a `Tab::id`, not a
    // position). For this fixed strip the ids are the indices
    // stringified, so the id round-trips to the panel index below.
    // Lives in the component's reactive scope, so it survives across
    // re-renders triggered by parent updates but tears down when the
    // component unmounts.
    let active_id: Signal<String> = signal(0.to_string());

    // Build the idea-ui `Tab` items (id + label) from the
    // user-supplied pairs. The labels are owned strings; clone
    // them so the original `tabs` Vec stays intact for the
    // `switch` closure below.
    let tab_items: Vec<Tab> = props
        .tabs
        .iter()
        .enumerate()
        .map(|(i, (label, _))| Tab::new(i.to_string(), label.clone()))
        .collect();

    // `Tabs.on_change` reports the tapped tab's id — store it in the
    // active signal. Owned `Rc` so each tap closure shares the same
    // cell.
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| active_id.set(id));

    // Active panel: `switch` subscribes to `active_id.get()` (parsed
    // back to the panel index) and re-invokes the right render
    // closure on every change. `tabs_for_switch` clones the Vec (Rc
    // closures clone for free) so the switch's `'static` body can
    // capture it.
    let tabs_for_switch = props.tabs;
    let panel_primitive = switch(
        move || active_id.get().parse::<usize>().unwrap_or(0),
        move |idx: &usize| {
            if let Some((_, render)) = tabs_for_switch.get(*idx) {
                render()
            } else {
                // Out-of-range guard — shouldn't happen given the
                // strip can't select an index outside the tab
                // list, but defensive against a runtime mismatch.
                view(Vec::new()).into()
            }
        },
    );

    // `TabsProps` carries the tab list (a reactive `Signal<Vec<Tab>>`
    // — static here) + active-id + on_change callback. Build it
    // manually instead of through the `tabs!(...)` macro so we can
    // hand over the pre-built `Vec<Tab>` directly. `..Default` keeps
    // the default underline indicator.
    let tabs_props = TabsProps {
        tabs: signal(tab_items),
        active: active_id.into(),
        on_change,
        ..Default::default()
    };
    let tabs_primitive = Tabs(tabs_props);

    let panel_style = TabPanel();

    ui! {
        Card {
            tabs_primitive
            view(style = panel_style) {
                panel_primitive
            }
        }
    }
}
