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
//! let active = signal(0_usize);
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
    component, pressable, resolve_style, text, ui, view, Element, IdealystSchema, IntoElement,
    Reactive, Signal, StyleApplication, StyleRules, StyleSheet, VariantEnum,
};
use std::rc::Rc;

use crate::stylesheets::{TabBar, TabBarHost, TabBarScroller, TabButton, TabButtonDot, TabDot};

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
            *s.borrow_mut() = Some(StyleSheet::r#static(StyleRules::default()).premint_as("idea-ui.v1.tabs.empty"));
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

/// Equality for `Signal<Vec<Tab>>` (the guarded `set` needs
/// `T: PartialEq`). Static labels compare by value so an in-place label
/// edit still notifies; Dynamic labels compare by closure identity (the
/// closure re-reads its signals on every render, so identity is the
/// honest value here).
impl PartialEq for Tab {
    fn eq(&self, other: &Self) -> bool {
        if self.id != other.id {
            return false;
        }
        match (&self.label, &other.label) {
            (Reactive::Static(a), Reactive::Static(b)) => a == b,
            (Reactive::Dynamic(a), Reactive::Dynamic(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
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
    /// rebuild). A fixed strip wraps a literal in `signal(vec![...])`.
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
            tabs: runtime_core::signal(Vec::new()),
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
    let strip = ui! {
        view(style = container_style) {
            for tab in tabs, key = tab.id.clone() {
                tab_button(tab, active.clone(), on_change.clone(), indicator.clone())
            }
        }
    };

    // The strip scrolls sideways when it does not fit.
    //
    // A tab bar is a flex ROW, so its children shrink by default — a strip
    // with more tabs than fit did not clip or overflow, it SQUASHED, and the
    // labels compressed until the strip was unusable. That is the state a
    // phone-width viewport put every multi-tab screen in. Tabs now hold their
    // natural width (`flex_shrink: 0` on `TabButton`) and the overflow is
    // scrolled instead.
    //
    // It costs nothing when everything fits: a scroller whose content is
    // narrower than its viewport has nothing to scroll, so wide layouts are
    // unchanged.
    //
    // The underline lives on the HOST, not on the strip, so it spans the full
    // width and stays put while the tabs move under it — on the strip it would
    // have been only as wide as the tabs and would have slid away with them.
    let scroller = runtime_core::primitives::scroll_view::scroll_view(vec![strip])
        .horizontal(true)
        .with_style(StyleApplication::new(TabBarScroller::sheet()))
        .into_element();
    ui! {
        view(style = TabBarHost()) {
            scroller
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
            let base = StyleApplication::new(tab_label_base_sheet());
            if app.attaches_preminted() {
                // Premint web build: the pressable's preminted class carries
                // the on/off foreground and the label inherits it via the CSS
                // cascade — the resolve-read below exists ONLY because native
                // text doesn't inherit, and under `--premint-only` it would
                // panic (sheets carry no rule closures).
                return base;
            }
            let color = resolve_style(&app).color.clone();
            let key = if on { "tab_label_on" } else { "tab_label_off" };
            // ENGINE-PATH ONLY: the `attaches_preminted()` early return
            // above guarantees this layer never runs on a premint build,
            // so the computed-layer disqualifier can't fire.
            // idealyst-lint-disable-next-line premint-computed-layer
            base.with_computed(key, move || StyleRules {
                color: color.clone(),
                ..Default::default()
            })
        }
    };

    let label_primitive: Element = text(label).with_style(label_style).into();

    // The leading dot's *presence* is structural — adding/removing the dot
    // node on a live `indicator` flip. Routed via `when(|| indicator == Dot,
    // dot, empty)` so a reactive indicator splices the dot in/out in place.
    // The dot's own style stays a sink (re-resolves on `active`). When
    // `indicator` is `Static` we keep the build-time branch (no `When` anchor).
    let build_dot = {
        let active = active.clone();
        let id = id.clone();
        move || {
            let dot_style = {
                let active = active.clone();
                let id = id.clone();
                move || {
                    let variant = if active.get() == id { "on" } else { "off" };
                    StyleApplication::new(TabDot::sheet()).with("active", variant.to_string())
                }
            };
            view(Vec::new()).with_style(dot_style).into()
        }
    };

    let mut tab_children: Vec<Element> = Vec::with_capacity(2);
    if indicator.is_static() {
        if matches!(indicator.get(), TabIndicator::Dot) {
            tab_children.push(build_dot());
        }
    } else {
        let indicator = indicator.clone();
        tab_children.push(runtime_core::when(
            move || matches!(indicator.get(), TabIndicator::Dot),
            build_dot,
            || runtime_core::fragment(Vec::new()),
        ));
    }
    tab_children.push(label_primitive);
    pressable(tab_children, press).with_style(tab_style).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    fn theme() {
        install_idea_theme(light_theme());
    }

    /// Resolves the color on the label text node of one tab pressable.
    /// The Tabs label style is reactive (re-runs on `active`);
    /// `TStyle::resolve` evaluates the reactive-or-static style either way.
    fn tab_label_color(tab: Element) -> Option<runtime_core::Color> {
        let label = match classify(tab) {
            P::Pressable { mut children, .. } => children.remove(0),
            _ => panic!("a tab is a Pressable"),
        };
        match classify(label) {
            P::Text { style, .. } => {
                style.and_then(|s| s.resolve().color.clone().map(|c| c.resolve()))
            }
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

    // The `--premint-only` read-back: the label style resolved the TabButton
    // sheet's color in Rust and carried it via a `with_computed` layer — which
    // both disqualifies preminting and panics under `--premint-only` (sheets
    // carry no rule closures). On a premint build the label application must
    // premint BARE — the pressable's preminted class carries the on/off
    // foreground and the web label inherits it via the CSS cascade. On live/
    // native builds the computed stamp must remain (native doesn't inherit).
    #[test]
    fn regression_premint_tab_label_application_premints() {
        with_test_world(|| {
            theme();
            let on_change: Rc<dyn Fn(String)> = Rc::new(|_| {});
            let tab = tab_button(
                Tab::new("a", "A"),
                Reactive::Static("a".to_string()),
                on_change,
                Reactive::Static(TabIndicator::Underline),
            );
            let label = match classify(tab) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("a tab is a Pressable"),
            };
            let style = match classify(label) {
                P::Text { style, .. } => style.expect("tab label carries a style"),
                _ => panic!("a tab label is a Text node"),
            };
            let app = style.application();
            #[cfg(idealyst_premint)]
            assert!(
                app.preminted_class_list().is_some(),
                "premint build: the label application premints bare — no computed layer"
            );
            #[cfg(not(idealyst_premint))]
            {
                assert!(
                    app.preminted_class_list().is_none(),
                    "live/native build: the computed color layer rides the application"
                );
                assert!(
                    resolve_style(&app).color.is_some(),
                    "and it resolves the TabButton foreground for the label node"
                );
            }
        });
    }

    // Field report 3.1b (audit): the tab label was a bare text node whose
    // color lived only on the wrapping pressable, so on native it rendered
    // in the widget default — the selected tab wouldn't darken, the rest
    // wouldn't mute. Each label must carry its OWN color matching its
    // active state. Asserting the label node's resolved color (not the
    // pressable's) is what makes this a valid regression test.
    #[test]
    fn regression_tab_labels_carry_their_own_active_color() {
        with_test_world(|| {
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
                tab_label_color(active_tab).expect("active tab label carries a color");
            assert_eq!(
                active_color,
                tabbutton_color("on"),
                "selected (id-matched) tab label is the TabButton `on` color"
            );

            let inactive_color =
                tab_label_color(inactive_tab).expect("inactive tab label carries a color");
            assert_eq!(
                inactive_color,
                tabbutton_color("off"),
                "unselected tab label is the TabButton `off` (muted) color"
            );

            // The two states must differ — proves the label color tracks selection
            // (by id) rather than being a single inherited value.
            assert_ne!(active_color, inactive_color);
    });
    }

    /// A tab bar that does not fit must SCROLL, not squash.
    ///
    /// The failure this pins is not a crash or a clip: a flex row's
    /// children shrink by default, so a strip with more tabs than fit
    /// compressed every label until the bar was unusable — which is the
    /// state every multi-tab screen was in at phone width. Two halves
    /// make it scroll instead, and both are asserted: the strip is
    /// wrapped in a HORIZONTAL scroller, and the tabs refuse to shrink.
    #[test]
    fn a_tab_strip_scrolls_sideways_rather_than_squashing() {
        with_test_world(|| {
            theme();
            let host = Tabs(TabsProps {
                tabs: runtime_core::signal(vec![Tab::new("a", "A"), Tab::new("b", "B")]),
                active: Reactive::Static("a".to_string()),
                ..Default::default()
            });
            let mut host_children = match crate::test_support::classify(host) {
                crate::test_support::P::View { children, .. } => children,
                _ => panic!("Tabs lowers to a host view"),
            };
            assert_eq!(host_children.len(), 1, "the host wraps exactly the scroller");
            match host_children.remove(0) {
                Element::Item { data, .. } => assert!(
                    data.downcast_ref::<runtime_vocabulary::prims::PrimCell<
                        runtime_vocabulary::prims::ScrollViewPrim,
                    >>()
                    .is_some_and(|c| c.take().horizontal),
                    "the strip must sit in a HORIZONTAL scroll_view"
                ),
                _ => panic!("the host's child must be the scroll_view item"),
            }

            // The other half: without this the tabs are ordinary flex
            // children and the scroller never has anything to scroll,
            // because they shrink to fit instead.
            let rules = runtime_core::resolve_style(
                &StyleApplication::new(crate::stylesheets::TabButton::sheet()),
            );
            assert_eq!(
                rules.flex_shrink.as_ref().map(|t| t.resolve()),
                Some(0.0),
                "a tab must hold its natural width, or the strip squashes"
            );
        });
    }

    /// The strip must keep its CONTENT width inside the scroller.
    ///
    /// A scroll view's child is an ordinary flex item, so a strip left
    /// to shrink collapses to the viewport while the tabs inside it
    /// refuse to shrink — and they are clipped away. What that looks
    /// like on a device is a tab bar with space in it and no tabs,
    /// which reads as "the tabs are gone" rather than as a layout bug.
    #[test]
    fn the_strip_keeps_its_content_width_inside_the_scroller() {
        with_test_world(|| {
            theme();
            let rules = runtime_core::resolve_style(&StyleApplication::new(
                crate::stylesheets::TabBar::sheet(),
            ));
            assert_eq!(
                rules.flex_shrink.as_ref().map(|t| t.resolve()),
                Some(0.0),
                "the strip must not shrink to the scroller's width"
            );
        });
    }

    /// The bar IS the divider, and the selection mark sits ON it.
    ///
    /// The host draws the divider and bottom-aligns the strip, so the
    /// active tab's underline lands directly on that line. Left at the
    /// default `flex-start` the host's height FLOOR put its slack below
    /// the tabs instead, and the underline floated two points clear of
    /// the divider — a tab bar hovering over a separate rule rather than
    /// a tab bar that is one.
    ///
    /// Both halves are pinned because either alone restores the gap.
    #[test]
    fn the_bar_is_the_divider_and_the_underline_sits_on_it() {
        with_test_world(|| {
            theme();
            let host = runtime_core::resolve_style(&StyleApplication::new(
                crate::stylesheets::TabBarHost::sheet(),
            ));
            assert_eq!(
                host.justify_content,
                Some(runtime_core::JustifyContent::FlexEnd),
                "the strip must sit on the bar's bottom edge, or the \
                 underline floats above the divider"
            );
            assert_eq!(
                host.border_bottom_width.as_ref().map(|t| t.resolve()),
                Some(1.0),
                "the bar draws the divider itself — a screen must not have \
                 to add one under it"
            );
        });
    }

    /// The strip scrolls, but it must not LOOK like a scroll region.
    ///
    /// iOS draws a horizontal scroll indicator inside the scroller, a
    /// couple of points above its bottom edge — which on a tab bar puts
    /// a third grey line between the tabs and their selection underline,
    /// itself just above the divider. Nothing about the bar's own
    /// styling can reach it; the scroller has to be told.
    #[test]
    fn the_strip_shows_no_scroll_indicator() {
        with_test_world(|| {
            theme();
            let rules = runtime_core::resolve_style(&StyleApplication::new(
                crate::stylesheets::TabBarScroller::sheet(),
            ));
            assert_eq!(
                rules.scrollbar,
                Some(runtime_core::ScrollbarVisibility::Hidden),
                "a tab bar is a control, not a scroll region"
            );
        });
    }

    /// Every tab is TAPPABLE, not just the ones that fit unscrolled.
    ///
    /// `flex_shrink: 0` on the strip stops the tabs being squashed, but
    /// it says nothing about the strip's own box on the cross axis,
    /// where a flex item stretches to its container by default. So the
    /// strip stayed as wide as the scroll viewport while Taffy laid its
    /// tabs out past the edge. Nothing clips them, so they PAINTED, and
    /// the scroller's contentSize comes from a deep walk, so they
    /// SCROLLED — but UIKit hit-tests a subview only where its
    /// superview's bounds contain the point, so every tab past the first
    /// viewport quietly ignored taps. Measured on an iPhone 17 Pro Max:
    /// tabs answered up to content x=440 (the viewport) and were dead
    /// beyond it, at every scroll offset.
    ///
    /// The symptom is invisible in a screenshot, which is why this is a
    /// test and not something to eyeball.
    #[test]
    fn a_tab_is_tappable_however_far_along_the_strip_it_sits() {
        with_test_world(|| {
            theme();
            let rules = runtime_core::resolve_style(&StyleApplication::new(
                crate::stylesheets::TabBar::sheet(),
            ));
            assert_eq!(
                rules.align_self,
                Some(runtime_core::AlignSelf::FlexStart),
                "the strip must size to its tabs on the CROSS axis too, or \
                 its box clips hit-testing to the viewport"
            );
        });
    }
}
