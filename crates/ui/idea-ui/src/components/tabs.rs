//! `Tabs` — a horizontal tab strip plus a switched panel.
//!
//! Controlled: the host owns a `Signal<String>` naming the active
//! tab's id, and `Tabs` reads it to decide which panel to render.
//! Clicking a tab button writes to the signal.
//!
//! ```ignore
//! use std::rc::Rc;
//! use framework_core::{signal, ui, Primitive};
//! use idea_ui::{Tab, TabsProps, tabs};
//!
//! let active = signal!("overview".to_string());
//! ui! {
//!     Tabs(
//!         selected = active,
//!         tabs = vec![
//!             Tab::new("overview", "Overview", Rc::new(|| ui! { Body(content = "Overview".to_string()) })),
//!             Tab::new("activity", "Activity", Rc::new(|| ui! { Body(content = "Activity".to_string()) })),
//!         ]
//!     )
//! }
//! ```
//!
//! Panels are mounted lazily: only the active tab's content tree
//! exists at any time. Switching tabs drops the old panel's scope —
//! signals and refs inside it free deterministically. State that
//! must outlive a tab switch belongs in the host, above the `Tabs`.

use std::rc::Rc;

use framework_core::{switch, ui, ChildList, IntoPrimitive, Primitive, Signal, StyleApplication};

use crate::stylesheets::{TabBar, TabButton, TabPanel};
use crate::theme::IdeaThemeRef;

/// One tab definition. Construct via [`Tab::new`].
pub struct Tab {
    pub id: String,
    pub label: String,
    pub content: Rc<dyn Fn() -> Primitive>,
}

impl Tab {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        content: Rc<dyn Fn() -> Primitive>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            content,
        }
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TabsProps {
    /// Controlled selection. The string is the active tab's `id`.
    pub selected: Signal<String>,
    pub tabs: Vec<Tab>,
}

impl Default for TabsProps {
    fn default() -> Self {
        Self {
            selected: Signal::new(String::new()),
            tabs: Vec::new(),
        }
    }
}

pub fn tabs(props: TabsProps) -> Primitive {
    let selected = props.selected;
    let tabs = props.tabs;

    // Build the tab-bar buttons. Each button writes its id to the
    // selected signal on click. The `active` variant on TabButton
    // is read reactively from the signal so flipping selections
    // re-styles only the affected buttons (no rebuild).
    let mut bar_children: Vec<Primitive> = Vec::with_capacity(tabs.len());
    for t in &tabs {
        let this_id = t.id.clone();
        let label = t.label.clone();
        let on_click_id = this_id.clone();
        let active_id = this_id.clone();
        // The style closure reads `selected` so each button's apply-style
        // effect re-fires on tab change, flipping the `active` variant.
        let style = move || {
            let _ = framework_core::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let active = selected.get() == active_id;
            StyleApplication::new(TabButton::sheet())
                .with("active", if active { "on" } else { "off" }.to_string())
        };
        // Tab "button" is a framework pressable styled by TabButton.
        // We use the pressable primitive (not idea-ui's Button) so
        // the TabButton stylesheet fully owns the visual.
        let label_child = framework_core::text(label).into_primitive();
        let button = framework_core::pressable(
            vec![label_child],
            move || selected.set(on_click_id.clone()),
        )
        .with_style(style)
        .into_primitive();
        ChildList::append_to(button, &mut bar_children);
    }

    let bar_style = TabBar();
    let bar = ui! { View(style = bar_style) { bar_children } };

    // Build a lookup of id → content builder, then drive the panel
    // through `switch`. The switch primitive rebuilds only when the
    // active id flips, so unrelated signal reads in `selected` don't
    // tear the active panel down.
    //
    // The Rc-cloned content closures let the build fn pick the right
    // one each rebuild. Missing id → empty View (the panel just
    // disappears, no panic).
    let builders: Rc<Vec<(String, Rc<dyn Fn() -> Primitive>)>> = Rc::new(
        tabs.into_iter()
            .map(|t| (t.id, t.content))
            .collect(),
    );

    let panel_builders = builders.clone();
    let panel = switch(
        move || selected.get(),
        move |id: &String| {
            for (tab_id, build) in panel_builders.iter() {
                if tab_id == id {
                    return build();
                }
            }
            // No matching tab — render nothing.
            ui! { View {} }.into_primitive()
        },
    );

    let panel_style = TabPanel();
    ui! {
        View {
            bar
            View(style = panel_style) { panel }
        }
    }
}
