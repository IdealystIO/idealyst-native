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
    component, pressable, resolve_style, text, ui, view, Element, IdealystSchema, Reactive, Signal,
    StyleApplication, StyleRules, StyleSheet, VariantEnum,
};
use std::rc::Rc;

use crate::stylesheets::{TabBar, TabButton, TabButtonDot, TabDot};

/// How the active tab is marked.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
pub enum TabIndicator {
    /// A 2px accent underline beneath the active tab (the default tab strip).
    #[default]
    Underline,
    /// A leading colored dot + a chip background on the active tab — the
    /// compact, pill-like switcher look.
    Dot,
}

impl VariantEnum for TabIndicator {
    fn as_variant_str(self) -> &'static str {
        match self {
            TabIndicator::Underline => "underline",
            TabIndicator::Dot => "dot",
        }
    }
    fn all_variants() -> &'static [Self] {
        &[TabIndicator::Underline, TabIndicator::Dot]
    }
}

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

/// One entry in the tab strip. `id` is the tab's stable identity — the
/// reconciliation key for a reactive `tabs` list AND the value `active` /
/// `on_change` match on (an *id*, not a position, so a tab keeps its identity
/// as the list grows, shrinks, or reorders).
#[derive(Clone, Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct Tab {
    /// Stable, unique identity: the keyed-list reconciliation key and the value
    /// `active`/`on_change` compare against. For a fixed strip, any unique
    /// string (e.g. `"overview"`); for a dynamic list, the item's own id.
    pub id: String,
    /// Human-readable label shown on the tab. `Reactive<String>` —
    /// static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
}

impl Tab {
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

// Reactive-by-default: `#[props]` wraps the scalar `indicator` →
// `Reactive<TabIndicator>` (routes to the per-tab style sink). `tabs` is a
// reactive `Signal` LIST, `active` is already `Reactive`, and `on_change` is a
// handler — all auto-skipped.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TabsProps {
    /// The tabs, left-to-right. `Signal<Vec<Tab>>` — a **reactive list**: tabs
    /// added / removed / reordered reconcile by `Tab::id`, so a surviving tab
    /// keeps its component-local state (the strip updates without a full
    /// rebuild). A fixed strip wraps a literal in `signal!(vec![...])`.
    pub tabs: Signal<Vec<Tab>>,
    /// The **active tab's `id`**. `Reactive<String>` — a `Signal<String>` the
    /// host owns, or a model-derived `rx!(...)` (e.g. mapping a document's
    /// active page index to its id). The tab whose `id` equals this paints
    /// selected.
    pub active: Reactive<String>,
    /// Fires with the **tapped tab's `id`**. Default is a no-op so an unwired
    /// Tabs doesn't silently mutate — pass `move |id| ...` to switch.
    pub on_change: Rc<dyn Fn(String)>,
    /// How the active tab is marked. Default [`TabIndicator::Underline`];
    /// [`TabIndicator::Dot`] gives the compact dot + chip switcher.
    /// `Reactive<TabIndicator>` — static or live; the strip re-styles in place.
    pub indicator: TabIndicator,
}

impl Default for TabsProps {
    // Manual impl: `Signal`, `Reactive`, and `Rc<dyn Fn>` don't derive
    // `Default`. Mirrors the same pattern as `SwitchProps`.
    fn default() -> Self {
        Self {
            tabs: Signal::new(Vec::new()),
            active: Reactive::Static(String::new()),
            on_change: Rc::new(|_| {}),
            indicator: Reactive::Static(TabIndicator::default()),
        }
    }
}

/// Renders a clickable tab strip with reactive active highlighting over a
/// reactive, id-keyed `tabs` list. Pure UI: one pressable per `Tab`; the one
/// whose `id` equals `active` is highlighted; a tap reports the tapped tab's
/// `id` via `on_change` — the caller owns the active state and renders the
/// corresponding content itself.
#[component]
pub fn Tabs(props: TabsProps) -> Element {
    let tabs = props.tabs;
    let active = props.active;
    let on_change = props.on_change;
    let indicator = props.indicator;
    let container_style = TabBar();

    // Reactive, keyed list — tabs reconcile by `Tab::id` (a surviving tab keeps
    // its scope when the list changes). `for` over a `Signal<Vec<_>>` lowers to
    // the keyed `each`; each row is one `tab_button`.
    ui! {
        view(style = container_style) {
            for tab in tabs, key = tab.id.clone() {
                tab_button(tab, active.clone(), on_change.clone(), indicator.clone())
            }
        }
    }
}

/// Build one tab pressable: the `id`-matched active style, the label, and (in
/// Dot mode) a leading colored dot.
///
/// Native TextView/UILabel/NSTextField don't inherit text color from the
/// pressable (only web's CSS cascade does), so the label resolves the
/// active/inactive foreground and carries it on its own node, reactively
/// (re-runs on `active` + theme). The color rides a `with_computed` keyed by
/// the active state on a shared base sheet so the resolution-cache key stays
/// stable. The dot's color sits on the view node directly (backgrounds aren't
/// inherited, so no such dance is needed there).
fn tab_button(
    tab: Tab,
    active: Reactive<String>,
    on_change: Rc<dyn Fn(String)>,
    indicator: Reactive<TabIndicator>,
) -> Element {
    let id = tab.id;
    let label = tab.label;

    // The button sheet for the chosen indicator — chip (dot) vs underline.
    // Reads `indicator` live so a reactive indicator re-selects the sheet in
    // place (the style sink). The DOT CHILD's *presence* is structural, not a
    // style sink, so it snapshots below (see TODO).
    let button_sheet = {
        let indicator = indicator.clone();
        move || {
            if matches!(indicator.get(), TabIndicator::Dot) {
                TabButtonDot::sheet()
            } else {
                TabButton::sheet()
            }
        }
    };

    // TODO(reactive-sweep): route `indicator` to the dot CHILD presence below
    // (structural — adding/removing the leading dot node on a live indicator
    // flip needs a `when`/keyed splice, not a style closure). Snapshot for now.
    let dot_mode = matches!(indicator.get(), TabIndicator::Dot);

    let press = {
        let id = id.clone();
        move || on_change(id.clone())
    };

    let tab_style = {
        let active = active.clone();
        let id = id.clone();
        let button_sheet = button_sheet.clone();
        move || {
            let variant = if active.get() == id { "on" } else { "off" };
            StyleApplication::new(button_sheet()).with("active", variant.to_string())
        }
    };

    let label_style = {
        let active = active.clone();
        let id = id.clone();
        move || {
            let on = active.get() == id;
            let variant = if on { "on" } else { "off" };
            let app = StyleApplication::new(button_sheet()).with("active", variant.to_string());
            let color = resolve_style(&app).color.clone();
            let key = if on { "tab_label_on" } else { "tab_label_off" };
            StyleApplication::new(tab_label_base_sheet()).with_computed(key, move || StyleRules {
                color: color.clone(),
                ..Default::default()
            })
        }
    };

    let label_primitive: Element = text(label).with_style(label_style).into();
    let mut tab_children: Vec<Element> = Vec::with_capacity(2);
    if dot_mode {
        let dot_style = {
            let active = active.clone();
            let id = id.clone();
            move || {
                let variant = if active.get() == id { "on" } else { "off" };
                StyleApplication::new(TabDot::sheet()).with("active", variant.to_string())
            }
        };
        tab_children.push(view(Vec::new()).with_style(dot_style).into());
    }
    tab_children.push(label_primitive);
    pressable(tab_children, press).with_style(tab_style).into()
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
        // The reactive `tabs` list wraps the pressables in a keyed `each`, so
        // exercise the per-tab builder directly. `active = "a"` ⇒ the "a" tab
        // is selected, the "b" tab is not.
        let on_change: Rc<dyn Fn(String)> = Rc::new(|_| {});
        let active = Reactive::Static("a".to_string());
        let indicator = Reactive::Static(TabIndicator::Underline);
        let active_tab =
            tab_button(Tab::new("a", "A"), active.clone(), on_change.clone(), indicator.clone());
        let inactive_tab = tab_button(Tab::new("b", "B"), active, on_change, indicator);

        let active_color =
            tab_label_color(&active_tab).expect("active tab label carries a color");
        assert_eq!(
            active_color,
            tabbutton_color("on"),
            "selected (id-matched) tab label is the TabButton `on` color"
        );

        let inactive_color =
            tab_label_color(&inactive_tab).expect("inactive tab label carries a color");
        assert_eq!(
            inactive_color,
            tabbutton_color("off"),
            "unselected tab label is the TabButton `off` (muted) color"
        );

        // The two states must differ — proves the label color tracks selection
        // (by id) rather than being a single inherited value.
        assert_ne!(active_color, inactive_color);
    }
}
