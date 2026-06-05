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

use runtime_core::{
    component, pressable, resolve_style, text, ui, Element, IdealystSchema, Reactive, Signal,
    StyleApplication, StyleRules, StyleSheet,
};
use std::rc::Rc;

use crate::stylesheets::{TabBar, TabButton};

thread_local! {
    static TAB_LABEL_BASE_SHEET: std::cell::RefCell<Option<Rc<StyleSheet>>> =
        const { std::cell::RefCell::new(None) };
}

/// A single shared, empty base sheet for tab labels. The per-state color
/// rides a `with_computed` layer keyed on the active state, so the
/// resolution cache key (sheet Rc pointer + computed key) stays stable
/// across renders — see the label-color comment in `Tabs`.
fn tab_label_base_sheet() -> Rc<StyleSheet> {
    TAB_LABEL_BASE_SHEET.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(Rc::new(StyleSheet::r#static(StyleRules::default())));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// One entry in the tab strip. Position in the props' `tabs` vec
/// is its identity — `Signal<usize>` indexes into the same vec.
#[derive(Clone, Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct Tab {
    /// Human-readable label shown on the tab. `Reactive<String>` —
    /// static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
}

impl Tab {
    pub fn new(label: impl Into<Reactive<String>>) -> Self {
        Self { label: label.into() }
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
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

/// Renders a clickable tab strip with reactive active highlighting. Pure
/// UI: it draws one pressable tab button per `Tab`, highlights the one at
/// `active`, and reports taps via `on_change` — the caller owns the active
/// state and renders the corresponding content itself.
#[component]
pub fn Tabs(props: TabsProps) -> Element {
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

        // The TabButton color lives on the pressable, but native
        // TextView/UILabel don't inherit text color from their parent
        // (only web's CSS cascade does) — so the label would render in the
        // widget-default color on iOS/Android. Mirror `tab_style` to
        // resolve the active/inactive foreground and stamp it on the label
        // node itself, reactively (re-runs on `active`, transitions on
        // theme + selection). Same fix as Button; matches web.
        //
        // The color rides a `with_computed` layer keyed by the active
        // state on a single shared base sheet. The stable computed key is
        // essential: the resolution cache keys on the sheet's Rc pointer,
        // so minting a fresh `StyleSheet::r#static` per resolve would risk
        // a freed-then-reused pointer aliasing a stale cached color.
        let label_style = move || {
            let on = active.get() == idx;
            let variant = if on { "on" } else { "off" };
            let app = StyleApplication::new(TabButton::sheet()).with("active", variant.to_string());
            let color = resolve_style(&app).color.clone();
            let key = if on { "tab_label_on" } else { "tab_label_off" };
            StyleApplication::new(tab_label_base_sheet())
                .with_computed(key, move || StyleRules {
                    color: color.clone(),
                    ..Default::default()
                })
        };

        // Build the tab as `Pressable { Text }` via the builder
        // functions — `Pressable` isn't a ui!-level tag (the
        // framework macro deliberately omits it so idea-ui can own
        // the styled wrapper), so we construct it directly.
        let label_primitive: Element = text(label).with_style(label_style).into();
        let tab_primitive: Element = pressable(vec![label_primitive], press)
            .with_style(tab_style)
            .into();
        children.push(tab_primitive);
    }

    ui! { view(style = container_style) { children } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, StyleSource};

    fn theme() {
        install_idea_theme(light_theme());
    }

    /// Resolves the color on the label text node of one tab pressable.
    /// The Tabs label style is reactive (re-runs on `active`), so we
    /// invoke its closure to get the current `StyleApplication`.
    fn tab_label_color(tab: &Element) -> Option<runtime_core::Color> {
        let label = match tab {
            Element::Pressable { children, .. } => &children[0],
            _ => panic!("a tab is a Pressable"),
        };
        match label {
            Element::Text { style, .. } => match style.as_ref()? {
                StyleSource::Reactive(f) => resolve_style(&f()).color.clone().map(|c| c.resolve()),
                StyleSource::Static(a) => resolve_style(a).color.clone().map(|c| c.resolve()),
                _ => None,
            },
            _ => panic!("a tab label is a Text node"),
        }
    }

    /// The color the TabButton sheet resolves for a given active state —
    /// the color the label MUST carry on its own node (native won't
    /// inherit it from the pressable).
    fn tabbutton_color(active: &str) -> runtime_core::Color {
        let app = StyleApplication::new(TabButton::sheet()).with("active", active.to_string());
        resolve_style(&app)
            .color
            .clone()
            .expect("TabButton resolves a foreground")
            .resolve()
    }

    // Field report 3.1b (audit): the tab label was a bare text node whose
    // color lived only on the wrapping pressable, so on native it rendered
    // in the widget default — the selected tab wouldn't darken, the rest
    // wouldn't mute. Each label must carry its OWN color matching its
    // active state. Asserting the label node's resolved color (not the
    // pressable's) is what makes this a valid regression test.
    #[test]
    fn regression_tab_labels_carry_their_own_active_color() {
        theme();
        let props = TabsProps {
            tabs: vec![Tab::new("One"), Tab::new("Two")],
            active: Signal::new(0),
            ..Default::default()
        };
        let children = match Tabs(props) {
            Element::View { children, .. } => children,
            _ => panic!("Tabs renders a View"),
        };

        // Tab 0 is active → its label carries the `on` color; tab 1 is
        // inactive → the `off` (muted) color.
        let active_color = tab_label_color(&children[0]).expect("active tab label carries a color");
        assert_eq!(
            active_color,
            tabbutton_color("on"),
            "active tab label is the TabButton `on` color"
        );

        let inactive_color =
            tab_label_color(&children[1]).expect("inactive tab label carries a color");
        assert_eq!(
            inactive_color,
            tabbutton_color("off"),
            "inactive tab label is the TabButton `off` (muted) color"
        );

        // The two states must differ — proves the label color tracks
        // selection rather than being a single inherited value.
        assert_ne!(active_color, inactive_color);
    }
}
