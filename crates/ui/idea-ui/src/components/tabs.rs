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
    component, component_styles, pressable, resolve_style, text, ui, view, AlignItems, Color, Cursor, Element,
    FlexDirection, FontWeight, IdealystSchema, IntoElement, JustifyContent, Length, Position, Reactive, Signal,
    SlotStyle, StyleApplication, StyleRules, StyleSheet, Tokenized, VariantEnum,
};
use std::rc::Rc;

use crate::components::stack::StackPadding;
use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Tabs`]: the `host` frames the scrolling
    /// `bar` and holds the divider (`rule`); an underline tab is `tab`, or
    /// `tab_active` when selected; a dot tab is `tab_dot` with an `active`
    /// axis. `panel` is a content area for below the bar (the app places
    /// it): vertical padding sits there, not on the bar, so the active
    /// underline stays on the divider.
    pub Tabs<IdeaThemeRef> {
        /// The full-width frame around the scrolling strip. It holds the
        /// DIVIDER (the `rule` slot) — a rule spanning the viewport that stays
        /// put while the tabs move over it — and bottom-aligns the strip so
        /// the active tab's 2px underline COVERS that rule rather than
        /// floating above it.
        ///
        /// That alignment is the whole point of a tab bar's bottom edge: the
        /// bar IS the line between chrome and content, and the selection mark
        /// is a thickening of that line under one tab. A screen that draws its
        /// own divider under a tab bar draws a second one.
        slot host {
            base(t) {
                flex_direction: FlexDirection::Column,
                // The containing block for `TabBarRule`.
                position: Position::Relative,
                // Sit the strip on the bottom edge. The host carries a height
                // FLOOR (below), so in the common case it has a point or two
                // of slack; left at the default `flex-start` that slack fell
                // BELOW the tabs and pushed the underline off the rule.
                justify_content: JustifyContent::FlexEnd,
                // No `width: 100%` (circular against a content-sized row) and
                // NO `flex_grow`: `flex_grow` grows along the PARENT's main
                // axis, so in a column (a bottom tab bar) it took height and
                // split the screen with the content above. A column already
                // stretches the bar across. A bar in a row that should take
                // the row's leftover width opts in with `Tabs(fill = true)`.
                // `flex_shrink` + a zero `min_width` let a bar in a row shrink
                // below its tabs and scroll; neither grows anything.
                flex_shrink: 1.0,
                min_width: Length::Px(0.0),
                // A FLOOR, not a height. A horizontal scroll_view has no
                // definite cross-size of its own, and in a row that centres
                // its children it can report none at all — which collapses
                // the strip to nothing and makes the tabs vanish rather
                // than overflow. The floor is one tab: `spacing-sm` above
                // and below the body line, plus the 2px selection rule.
                min_height: Length::Px(40.0),
                // No border here: the divider is `TabBarRule`, a child, so
                // the active underline can paint OVER it. A border cannot be
                // overlapped — on a native layer it draws on top of every
                // subview, so a flush strip had the divider cut through the
                // underline (the reason this once carried 1px of bottom
                // padding, which kept the underline just above the line).
            }
        }

        /// The divider: a 1px rule along the host's bottom edge, under the
        /// strip. The active tab's 2px underline sits on the same bottom edge,
        /// so its lower point covers the rule — the selection mark reads as a
        /// thickening of the divider under one tab.
        ///
        /// Paint order is what makes it overlap, and it is the same on every
        /// backend: the rule is the host's FIRST child and the scroller is
        /// positioned too (the `scroller` slot). Native views paint siblings in
        /// child order whatever their positioning; the web paints positioned
        /// siblings in DOM order, and only puts an absolute element above an
        /// UNpositioned sibling — which is how an earlier version of this rule
        /// ended up covering the underline instead.
        slot rule {
            base(t) {
                position: Position::Absolute,
                left: Length::Px(0.0),
                right: Length::Px(0.0),
                bottom: Length::Px(0.0),
                height: 1.0,
                background: t.color.border(),
            }
            transitions {
                background: 250ms EaseInOut,
            }
        }

        /// The horizontal scroller between the host and the strip.
        ///
        /// `scroll_view` seeds `flex_grow:1 / flex_basis:0` — the "fill a
        /// bounded parent" shape. A tab bar is content-HEIGHT, so that shape
        /// would collapse it; these put it back to sizing from its content,
        /// with `min_width: 0` so it may be narrower than the strip inside
        /// it (which is what gives it something to scroll).
        slot scroller {
            base(_t) {
                flex_grow: 0.0,
                flex_basis: Length::Auto,
                min_width: Length::Px(0.0),
                // Take the host's full height rather than negotiating one:
                // the host already states the floor, and a scroller left to
                // size itself across the axis it does NOT scroll is exactly
                // what collapsed here.
                align_self: runtime_core::AlignSelf::Stretch,
                flex_shrink: 0.0,
                // No scroll indicator. A tab bar reads as a CONTROL, not as
                // a scroll region, and iOS draws the horizontal indicator
                // inside the strip — a third line between the tabs and their
                // selection underline, above the divider. Nothing about the
                // bar's own styling could remove it; see
                // `ScrollbarVisibility`.
                scrollbar: runtime_core::ScrollbarVisibility::Hidden,
                // Positioned, so the web paints it above `TabBarRule` (an
                // absolute element paints over unpositioned siblings there,
                // whatever the DOM order). Native paints in child order and
                // ignores this. No offsets: it stays where layout put it.
                position: Position::Relative,
            }
        }

        /// The row of tabs itself. The divider is on the `host`, not here:
        /// on the strip it would be only as wide as the tabs, and would
        /// slide away with them when the strip scrolls.
        slot bar {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.xs(),
                // Do NOT shrink inside the scroll viewport. This is the
                // horizontal twin of `modal_body_sheet`'s note: a scroll
                // view's child is an ordinary flex item, so without this
                // the strip shrinks to the scroller's width and the tabs —
                // which refuse to shrink themselves — are clipped away.
                // The visible result is a tab bar with space but nothing in
                // it, not a bar that overflows.
                flex_shrink: 0.0,
                flex_grow: 0.0,
                flex_basis: Length::Auto,
                // Size to the tabs on the CROSS axis as well. `flex_shrink`
                // above stops the strip being compressed; this stops it being
                // STRETCHED to the scroll viewport, which is a quieter bug
                // with the same cause: the strip's own box stayed viewport-
                // wide while its tabs were laid out past the edge. They
                // painted (nothing clips them) and they scrolled, but UIKit
                // hit-tests a subview only where its SUPERVIEW's bounds
                // contain the point — so every tab past the first viewport
                // was visible, moved with the strip, and silently ignored
                // taps.
                align_self: runtime_core::AlignSelf::FlexStart,
            }
            // `Tabs(padding_horizontal = …)`: insets the tabs from the bar's
            // ends while the divider still spans the full width (it is on
            // the host, outside the scroller). On the strip, so the inset
            // scrolls with the tabs and the last tab can scroll clear of the
            // edge. Horizontal only — vertical padding would lift the
            // underline off the divider.
            variant pad {
                #[default]
                none(_t) { padding_horizontal: Length::Px(0.0) }
                xs(t)    { padding_horizontal: t.spacing.xs() }
                sm(t)    { padding_horizontal: t.spacing.sm() }
                md(t)    { padding_horizontal: t.spacing.md() }
                lg(t)    { padding_horizontal: t.spacing.lg() }
                xl(t)    { padding_horizontal: t.spacing.xl() }
            }
        }

        /// An inactive underline tab. Underline tabs are one slot per
        /// selection state (`tab`, `tab_active`) rather than an `active`
        /// axis because `state` blocks win over axis arms: on one slot, the
        /// hover underline below would also turn the ACTIVE tab's primary
        /// underline gray. Each slot states its own hover / focus / press.
        ///
        /// Every tab sits ON the divider (the `rule` slot), and a state background
        /// fills the whole box, border band included — so a hovered inactive tab
        /// would hide the divider under it. Instead its states draw a 2px gray
        /// underline over the divider: the line thickens under the hovered tab and
        /// never breaks.
        slot tab {
            base(t) {
                // Hold the label's natural width. Without this the tabs are
                // ordinary flex children and a strip that does not fit
                // compresses them instead of scrolling.
                flex_shrink: 0.0,
                background: Color("transparent".into()),
                color: t.color.text_muted(),
                padding_vertical: t.spacing.sm(),
                padding_horizontal: t.spacing.md(),
                font_weight: FontWeight::Medium,
                font_size: t.typography.body_size(),
                border_radius: 0.0,
                cursor: Cursor::Pointer,
                // Transparent at rest, so the divider shows through; the
                // same 2px as the active underline, so selecting a tab never
                // changes its size.
                border_bottom_width: 2.0,
                border_bottom_color: Color("transparent".into()),
            }
            // Hover/press carry a translucent surface scrim (not just a text
            // brighten) so tabs read as interactive controls — the toolbar-
            // button feel — plus the gray underline (see above).
            state hovered(t) {
                color: t.color.text(),
                background: t.color.surface_alt(),
                border_bottom_color: t.color.border(),
            }
            // Focus mirrors hover (a surface wash), not a box border — a rectangular
            // ring reads wrong on an underline tab. Replaces the native macOS ring.
            state focused(t) {
                color: t.color.text(),
                background: t.color.surface_alt(),
                border_bottom_color: t.color.border(),
            }
            state pressed(t) {
                color: t.color.text(),
                background: t.color.border(),
                border_bottom_color: t.color.border(),
            }
            transitions {
                color: 150ms EaseOut,
                background: 120ms EaseOut,
                border_bottom_color: 200ms EaseOut,
            }
        }

        /// The selected underline tab. The `tab` slot's box exactly (so a tab
        /// never changes size when selected), with the primary underline, which
        /// every state keeps.
        slot tab_active {
            base(t) {
                flex_shrink: 0.0,
                background: Color("transparent".into()),
                color: t.color.text(),
                padding_vertical: t.spacing.sm(),
                padding_horizontal: t.spacing.md(),
                font_weight: FontWeight::Medium,
                font_size: t.typography.body_size(),
                border_radius: 0.0,
                cursor: Cursor::Pointer,
                border_bottom_width: 2.0,
                border_bottom_color: t.intent.primary.solid_bg(),
            }
            state hovered(t) {
                background: t.color.surface_alt(),
                border_bottom_color: t.intent.primary.solid_bg(),
            }
            state focused(t) {
                background: t.color.surface_alt(),
                border_bottom_color: t.intent.primary.solid_bg(),
            }
            state pressed(t) {
                background: t.color.border(),
                border_bottom_color: t.intent.primary.solid_bg(),
            }
            transitions {
                color: 150ms EaseOut,
                background: 120ms EaseOut,
                border_bottom_color: 200ms EaseOut,
            }
        }

        /// Dot-indicator tab: instead of an underline, the active tab gets a
        /// chip (surface-alt) background and a colored leading dot.
        slot tab_dot {
            base(t) {
                background: Color("transparent".into()),
                color: t.color.text_muted(),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
                padding_vertical: t.spacing.sm(),
                padding_horizontal: t.spacing.md(),
                border_radius: t.radius.md(),
                font_weight: FontWeight::Medium,
                font_size: t.typography.body_size(),
            }
            variant active {
                #[default]
                off(_t) {}
                on(t) {
                    color: t.color.text(),
                    background: t.color.surface_alt(),
                }
            }
            state hovered(t) {
                color: t.color.text(),
                background: t.color.surface_alt(),
            }
            // Focus mirrors hover (chip wash) — replaces the native macOS ring.
            state focused(t) {
                color: t.color.text(),
                background: t.color.surface_alt(),
            }
            state pressed(t) {
                background: t.color.border(),
            }
            transitions {
                color: 150ms EaseOut,
                background: 120ms EaseOut,
            }
        }

        /// The colored leading dot for a dot-indicator tab: muted when inactive, the
        /// primary intent color when active.
        slot dot {
            base(t) {
                width: 7.0,
                height: 7.0,
                border_radius: t.radius.pill(),
                background: t.color.text_muted(),
            }
            variant active {
                #[default]
                off(_t) {}
                on(t) {
                    background: t.intent.primary.solid_bg(),
                }
            }
            transitions {
                background: 150ms EaseOut,
            }
        }

        /// A content area for below the bar.
        slot panel {
            base(t) {
                padding_vertical: t.spacing.lg(),
                flex_direction: FlexDirection::Column,
                gap: t.spacing.md(),
            }
        }
    }
}

/// How the active tab is marked.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema, runtime_core::Remote)]
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

runtime_tls::thread_local! {
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
#[derive(IdealystSchema, runtime_core::Remote)]
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
#[runtime_core::props(styles = TabsStyles)]
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
    /// Take the leftover width of a **row** parent. Default `false`: the bar
    /// is as wide as its tabs, shrinking and scrolling when the row is too
    /// narrow, and a column stretches it across. `fill` is a row-only opt-in
    /// because it is `flex-grow`, which grows along the parent's main axis:
    /// in a column it would grow the bar's height instead.
    pub fill: bool,
    /// Inset the tabs from the bar's left and right ends. Default
    /// [`StackPadding::None`]. The divider still spans the bar's full width;
    /// the inset scrolls with the tabs. Horizontal only, so the active
    /// underline stays on the divider.
    pub padding_horizontal: StackPadding,
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
            fill: Reactive::Static(false),
            padding_horizontal: Reactive::Static(StackPadding::None),
            styles: Default::default(),
        }
    }
}

/// `Tabs(padding_horizontal = …)` selects the bar's `pad` arm.
impl From<StackPadding> for TabsPad {
    fn from(p: StackPadding) -> Self {
        TabsPad::define(p.as_variant_str())
    }
}

/// The host's style: the `host` slot, plus `flex-grow` on the inline layer
/// when the bar fills a row (inline so the application still premints, as
/// `Surface`'s `grow` does).
fn host_style(fill: Reactive<bool>, styles: &TabsSlotStyles) -> SlotStyle<TabsStyles> {
    let grow = |on: bool| StyleRules { flex_grow: on.then_some(Tokenized::Literal(1.0)), ..Default::default() };
    let host = match fill {
        Reactive::Static(false) => TabsStyles::host(),
        Reactive::Static(true) => TabsStyles::host().inline(grow(true)),
        live => TabsStyles::host().inline_with(move || grow(live.get())),
    };
    host.overridden(styles.host.as_ref())
}

/// The underline / dot tab's slot for the chosen indicator and selection.
fn button_style(indicator: TabIndicator, on: bool, styles: &TabsSlotStyles) -> StyleApplication {
    match indicator {
        TabIndicator::Dot => {
            TabsStyles::tab_dot().set(TabsActive::from(on)).overridden(styles.tab_dot.as_ref())
        }
        TabIndicator::Underline if on => TabsStyles::tab_active().overridden(styles.tab_active.as_ref()),
        TabIndicator::Underline => TabsStyles::tab().overridden(styles.tab.as_ref()),
    }
    .into_style_application()
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
    let fill = props.fill;
    let pad = props.padding_horizontal;
    let styles = props.styles;

    // Reactive, keyed list — tabs reconcile by `Tab::id` (a surviving tab keeps
    // its scope when the list changes). `for` over a `Signal<Vec<_>>` lowers to
    // the keyed `each`; each row is one `tab_button`.
    // The `pad` arm is a selection, so a static inset still premints.
    let strip_style = TabsStyles::bar().set(pad.map(TabsPad::from)).overridden(styles.bar.as_ref());
    let row_styles = styles.clone();
    let strip = ui! {
        view(style = strip_style) {
            for tab in tabs, key = tab.id.clone() {
                tab_button(tab, active.clone(), on_change.clone(), indicator.clone(), &row_styles)
            }
        }
    };

    // The strip scrolls sideways when it does not fit.
    //
    // A tab bar is a flex ROW, so its children shrink by default — a strip
    // with more tabs than fit did not clip or overflow, it SQUASHED, and the
    // labels compressed until the strip was unusable. That is the state a
    // phone-width viewport put every multi-tab screen in. Tabs now hold their
    // natural width (`flex_shrink: 0` on the `tab` slot) and the overflow is
    // scrolled instead.
    //
    // It costs nothing when everything fits: a scroller whose content is
    // narrower than its viewport has nothing to scroll, so wide layouts are
    // unchanged.
    //
    // The divider lives on the HOST, not on the strip, so it spans the full
    // width and stays put while the tabs move over it — on the strip it would
    // have been only as wide as the tabs and would have slid away with them.
    // It is the host's FIRST child so the tabs (and the active underline)
    // paint over it on every backend — see the `rule` slot.
    let scroller = runtime_core::primitives::scroll_view::scroll_view(vec![strip])
        .horizontal(true)
        .with_style(TabsStyles::scroller().overridden(styles.scroller.as_ref()))
        .into_element();
    let rule_style = TabsStyles::rule().overridden(styles.rule.as_ref());
    ui! {
        view(style = host_style(fill, &styles)) {
            view(style = rule_style)
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
    styles: &TabsSlotStyles,
) -> Element {
    let id = tab.id;
    let label = tab.label;

    // The button style for the chosen indicator and selection. Dot is one
    // slot with an `active` axis; Underline is a slot per state (`tab` /
    // `tab_active` — see the slot docs for why). Reads `indicator` live so a
    // reactive indicator re-selects the slot in place (the style sink). The
    // DOT CHILD's *presence* is structural, not a style sink, so it
    // snapshots below.
    let button_app = {
        let indicator = indicator.clone();
        let styles = styles.clone();
        move |on: bool| button_style(indicator.get(), on, &styles)
    };

    let press = {
        let id = id.clone();
        move || on_change(id.clone())
    };

    let tab_style = {
        let active = active.clone();
        let id = id.clone();
        let button_app = button_app.clone();
        move || button_app(active.get() == id)
    };

    let label_style = {
        let active = active.clone();
        let id = id.clone();
        move || {
            let on = active.get() == id;
            let app = button_app(on);
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
        let dot_override = styles.dot.clone();
        move || {
            let active = active.clone();
            let id = id.clone();
            let dot_style = TabsStyles::dot()
                .set(Reactive::Dynamic(Rc::new(move || TabsActive::from(active.get() == id))))
                .overridden(dot_override.as_ref());
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
        // idealyst-lint-disable-next-line prefer-ui-control-flow -- static-prop fast path (see `components/mod.rs`)
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

    /// The color the underline tab sheet resolves for a given active state
    /// (`TabButtonActive` when "on") — the color the label MUST carry on its
    /// own node (native won't inherit it from the pressable).
    fn tabbutton_color(active: &str) -> runtime_core::Color {
        let app = if active == "on" {
            TabsStyles::tab_active().into_style_application()
        } else {
            TabsStyles::tab().into_style_application()
        };
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
                &TabsSlotStyles::default(),
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
                tab_button(Tab::new("a", "A"), active.clone(), on_change.clone(), indicator.clone(), &TabsSlotStyles::default());
            let inactive_tab = tab_button(Tab::new("b", "B"), active, on_change, indicator, &TabsSlotStyles::default());

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
            assert_eq!(
                host_children.len(),
                2,
                "the host holds the divider rule, then the scroller"
            );
            match host_children.remove(1) {
                Element::Item { data, .. } => assert!(
                    data.downcast_ref::<runtime_vocabulary::prims::PrimCell<
                        runtime_vocabulary::prims::ScrollViewPrim,
                    >>()
                    .is_some_and(|c| c.take().axis == runtime_core::ScrollAxis::Horizontal),
                    "the strip must sit in a HORIZONTAL scroll_view"
                ),
                _ => panic!("the host's child must be the scroll_view item"),
            }

            // The other half: without this the tabs are ordinary flex
            // children and the scroller never has anything to scroll,
            // because they shrink to fit instead.
            let rules = runtime_core::resolve_style(
                &TabsStyles::tab().into_style_application(),
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
            let rules = runtime_core::resolve_style(&TabsStyles::bar().into_style_application());
            assert_eq!(
                rules.flex_shrink.as_ref().map(|t| t.resolve()),
                Some(0.0),
                "the strip must not shrink to the scroller's width"
            );
        });
    }

    /// The bar IS the divider, and the selection mark sits ON it.
    ///
    /// The host bottom-aligns the strip, so the active tab's underline ends
    /// on the bar's bottom edge — where `TabBarRule` draws the divider, so
    /// the underline covers it. Left at the default `flex-start` the host's
    /// height FLOOR put its slack below the tabs and the underline floated
    /// clear of the divider. The divider is a rule, never a border on the
    /// host: a native layer draws its border over every subview, so the
    /// underline could not cover it. Geometry and paint order are pinned
    /// on a laid-out tree in `tests/tabs_underline_layout.rs`.
    #[test]
    fn the_bar_is_the_divider_and_the_underline_sits_on_it() {
        with_test_world(|| {
            theme();
            let host = runtime_core::resolve_style(&TabsStyles::host().into_style_application());
            assert_eq!(
                host.justify_content,
                Some(runtime_core::JustifyContent::FlexEnd),
                "the strip must sit on the bar's bottom edge, or the \
                 underline floats above the divider"
            );
            assert!(
                host.border_bottom_width.is_none() && host.padding_bottom.is_none(),
                "no border and no bottom padding on the host — either keeps \
                 the underline off the divider"
            );
            let rule = runtime_core::resolve_style(&TabsStyles::rule().into_style_application());
            assert_eq!(rule.position, Some(runtime_core::Position::Absolute));
            assert_eq!(
                rule.bottom.as_ref().map(|t| t.resolve()),
                Some(runtime_core::Length::Px(0.0)),
                "the divider sits on the bar's bottom edge"
            );
        });
    }

    /// Every tab sits on the divider, and a state background fills the
    /// tab's whole box — so a hovered inactive tab hid the divider under
    /// it. Its states draw a gray underline over the divider instead, and
    /// the ACTIVE tab's states keep its primary underline (on one sheet
    /// with an `active` axis they could not: states win over variants).
    #[test]
    fn hover_underline_keeps_the_divider_and_the_selection() {
        with_test_world(|| {
            theme();
            let name = |c: &Option<runtime_core::Tokenized<runtime_core::Color>>| {
                c.as_ref().and_then(|c| c.name()).map(str::to_string)
            };
            for state in ["hovered", "focused", "pressed"] {
                let key = format!("__state_{state}");
                let inactive = runtime_core::resolve_style(
                    &TabsStyles::tab().into_style_application().with(key.as_str(), "on"),
                );
                assert_eq!(
                    name(&inactive.border_bottom_color).as_deref(),
                    Some("color-border"),
                    "{state} inactive tab: a gray underline over the divider"
                );
                let active = runtime_core::resolve_style(
                    &TabsStyles::tab_active().into_style_application().with(key.as_str(), "on"),
                );
                let primary = runtime_core::resolve_style(&TabsStyles::tab_active().into_style_application());
                assert_eq!(
                    name(&active.border_bottom_color),
                    name(&primary.border_bottom_color),
                    "{state} active tab keeps its primary underline"
                );
                assert!(name(&primary.border_bottom_color).is_some_and(|n| n != "color-border"));
            }
            // Same box either way, so selecting a tab never resizes it.
            let a = runtime_core::resolve_style(&TabsStyles::tab().into_style_application());
            let b = runtime_core::resolve_style(&TabsStyles::tab_active().into_style_application());
            assert_eq!(
                a.border_bottom_width.as_ref().map(|t| t.resolve()),
                b.border_bottom_width.as_ref().map(|t| t.resolve())
            );
            assert_eq!(a.padding_top, b.padding_top);
            assert_eq!(a.padding_left, b.padding_left);
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
            let rules = runtime_core::resolve_style(&TabsStyles::scroller().into_style_application());
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
            let rules = runtime_core::resolve_style(&TabsStyles::bar().into_style_application());
            assert_eq!(
                rules.align_self,
                Some(runtime_core::AlignSelf::FlexStart),
                "the strip must size to its tabs on the CROSS axis too, or \
                 its box clips hit-testing to the viewport"
            );
        });
    }

    /// Regression: `TabBarHost` carried `flex_grow: 1` so a bar in a ROW
    /// took the row's leftover width. `flex_grow` grows along the parent's
    /// main axis, so in a COLUMN (a bottom tab bar under the screen area)
    /// it grew the bar's height and split the screen with the content
    /// above, on every backend. The default must not grow.
    #[test]
    fn regression_tab_bar_does_not_grow_in_a_column_by_default() {
        with_test_world(|| {
            theme();
            let host = runtime_core::resolve_style(&host_style(Reactive::Static(false), &TabsSlotStyles::default()).into_style_application());
            let grow = host.flex_grow.as_ref().map(|t| t.resolve()).unwrap_or(0.0);
            assert_eq!(grow, 0.0, "a default tab bar must size to its tabs, not grow");
            assert_eq!(
                host.min_width.as_ref().map(|t| t.resolve()),
                Some(runtime_core::Length::Px(0.0)),
                "a bar in a row must still be able to shrink below its tabs and scroll"
            );
        });
    }

    /// `fill` is the row opt-in: it adds the grow back, on the inline layer
    /// so the application still premints.
    #[test]
    fn fill_takes_a_rows_leftover_width_and_still_premints() {
        with_test_world(|| {
            theme();
            let host = runtime_core::resolve_style(&host_style(Reactive::Static(true), &TabsSlotStyles::default()).into_style_application());
            assert_eq!(host.flex_grow.as_ref().map(|t| t.resolve()), Some(1.0));
            assert!(
                host_style(Reactive::Static(true), &TabsSlotStyles::default()).into_style_application().preminted_class_list().is_some(),
                "fill must not disqualify preminting"
            );
        });
    }
}
