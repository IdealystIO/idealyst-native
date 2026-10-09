//! `SegmentedControl` — a row of mutually-exclusive options.
//!
//! The "iOS segmented picker" pattern: a short, fixed row where exactly
//! one segment is selected at a time. Before this existed you hand-rolled
//! it as a `Stack(axis = Row)` of `Button`s and tracked selection
//! yourself; `SegmentedControl` packages that into one controlled
//! component.
//!
//! ```ignore
//! let view = signal("list".to_string());
//! let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));
//! ui! {
//!     SegmentedControl(
//!         value = view,
//!         on_change = on_change,
//!         options = vec![
//!             SegmentOption::new("list", "List"),
//!             SegmentOption::new("grid", "Grid"),
//!             SegmentOption::new("map", "Map"),
//!         ],
//!     )
//! }
//! ```
//!
//! Like [`Select`](super::select::Select) and the other selection
//! controls, it's **controlled by value**: the host owns a
//! `Signal<String>` holding the selected option's `id`, and `on_change`
//! commits the newly-picked `id`. The segment whose `id` equals the
//! current `value` paints selected; mutual exclusivity is automatic
//! because exactly one `id` can equal `value`.
//!
//! ## Adornments
//!
//! A segment can carry a `leading` and a `trailing` [`Adornment`] beside its
//! label — the same type `Field` uses. The control lays out the row and
//! renders the label, so the text is written once:
//!
//! ```ignore
//! SegmentOption::new("day", "Day shift")
//!     .leading(Adornment::element(|| ui! { view(style = ShiftDot()) }))
//! SegmentOption::new("map", "Map").leading(Adornment::Icon(icons_lucide::MAP))
//! ```
//!
//! An `Icon` adornment takes the segment's own foreground — muted at rest,
//! full text color when selected — exactly like the label. `Element` is any
//! component, rendered as built; `Button` is a tappable icon that eats its
//! own tap (the segment is not selected by it); `Group` sits several side
//! by side.
//!
//! ## Appearance
//! A bordered, tinted track (`SegmentedGroup`) holding one `SegmentButton`
//! per option; the selected segment is filled in (the `selected` axis flips
//! between `on` and `off`). It is deliberately unlike [`Tabs`](super::tabs::Tabs):
//! tabs navigate, a segmented control picks a value, and the two must not
//! read the same when they share a page.

use std::rc::Rc;

use runtime_core::{
    component, memo, pressable, recipe, resolve_style, ui, Color, Element, IdealystSchema, IntoElement,
    Memo, Reactive, StyleApplication, StyleRules, StyleSheet,
};

use crate::components::field::{adornment_button_sheet, Adornment};
use crate::stylesheets::{SegmentButton, SegmentInner, SegmentedGroup};
use crate::Icon;

/// Point size of an `Icon`/`Button` adornment in a segment — matches the
/// segment label's body text size.
const SEGMENT_ICON_PX: f32 = 16.0;

thread_local! {
    static SEG_LABEL_BASE_SHEET: std::cell::RefCell<Option<Rc<StyleSheet>>> =
        const { std::cell::RefCell::new(None) };
}

/// A single shared, empty base sheet for segment labels. The per-state color
/// rides a `with_computed` layer keyed on the selected state, so the resolution
/// cache key (sheet Rc pointer + computed key) stays stable across renders —
/// the same shape as `Tabs::tab_label_base_sheet`.
fn seg_label_base_sheet() -> Rc<StyleSheet> {
    SEG_LABEL_BASE_SHEET.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(StyleSheet::r#static(StyleRules::default()).premint_as("idea-ui.v1.segmented_control.empty"));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// One segment in a [`SegmentedControl`]. `id` is the value committed to
/// the bound signal when this segment is chosen; `label` is what the user
/// sees, with optional `leading` / `trailing` adornments beside it.
#[derive(Clone, IdealystSchema, runtime_core::Remote)]
pub struct SegmentOption {
    /// Stable value committed to the control's `value` signal when this
    /// segment is chosen. Compared against the current value to mark the
    /// selected segment.
    pub id: String,
    /// Segment label. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// Adornment before the label — an icon, a status dot, any component.
    pub leading: Adornment,
    /// Adornment after the label — a count badge, a trailing icon.
    pub trailing: Adornment,
}

impl SegmentOption {
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            leading: Adornment::None,
            trailing: Adornment::None,
        }
    }

    /// Put `adornment` before the label: `.leading(Adornment::Icon(MAP))`,
    /// or `.leading(Adornment::element(|| ui! { … }))` for any component.
    pub fn leading(mut self, adornment: Adornment) -> Self {
        self.leading = adornment;
        self
    }

    /// Put `adornment` after the label.
    pub fn trailing(mut self, adornment: Adornment) -> Self {
        self.trailing = adornment;
        self
    }
}

// Reactive-by-default: `#[props]` auto-skips EVERY field here — `value` is
// already `Reactive<String>`, `on_change` is a handler (`Rc`), and `options`
// is a `Vec`. No scalar-DATA prop to wrap, so the struct/Default/body are
// unchanged; the attribute is added for uniformity with the other controls.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SegmentedControlProps {
    /// Controlled selected value — the `id` of the chosen [`SegmentOption`].
    /// `Reactive<String>` — a `Signal<String>` the host owns, or a model-derived
    /// `rx!(...)` that maps a typed enum/bool to the matching option `id` (so the
    /// control fits state that isn't a standalone string signal). Tapping a
    /// segment reports the new `id` via `on_change`; the segment whose `id`
    /// equals this paints selected.
    pub value: Reactive<String>,
    /// Fires with the chosen segment's `id` when the user taps a segment.
    /// Default is a no-op so an unwired control doesn't silently mutate —
    /// pass `move |id| value.set(id)` to make taps switch segments.
    pub on_change: Rc<dyn Fn(String)>,
    /// The segments, left-to-right.
    pub options: Vec<SegmentOption>,
}

impl Default for SegmentedControlProps {
    // Manual impl: `Signal<String>` and `Rc<dyn Fn(String)>` don't derive
    // `Default`. Mirrors `SelectProps` / `TabsProps`.
    fn default() -> Self {
        Self {
            value: Reactive::Static(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
        }
    }
}

/// Renders a row of mutually-exclusive segments with reactive selected
/// highlighting. Pure UI: it draws one pressable per `SegmentOption`,
/// lights the one whose `id` matches `value`, and reports taps via
/// `on_change` — the host owns the selected value.
#[component]
pub fn SegmentedControl(props: SegmentedControlProps) -> Element {
    let options = props.options;
    let value = props.value;
    let on_change = props.on_change;

    ui! {
        view(style = SegmentedGroup()) {
            for option in options {
                segment(option, value.clone(), on_change.clone())
            }
        }
    }
}

/// Build one segment pressable: the `value`-matched selected style and its
/// label.
///
/// `Pressable` isn't a ui!-level tag (the framework macro omits it so
/// idea-ui owns the styled wrapper), so the segment is built with the
/// builder fns — the same shape `Tabs::tab_button` uses.
fn segment(option: SegmentOption, value: Reactive<String>, on_change: Rc<dyn Fn(String)>) -> Element {
    let id = option.id;

    // The press commits this segment's own `id`.
    let id_for_press = id.clone();
    let press = move || on_change(id_for_press.clone());

    // One derived flag per segment, shared by the segment's own style, its
    // label, and its adornments' glyphs — a memo, so a value change that leaves
    // this segment's state alone wakes none of them.
    let selected = memo(move || value.get() == id);

    // Reactive style: re-runs whenever `selected` flips, switching the
    // `selected` axis between `on` and `off`.
    let seg_style = move || StyleApplication::new(SegmentButton::sheet()).with("selected", selected_arm(selected.get()));

    let label = option.label;
    let leading = render_adornment(&option.leading, selected);
    let trailing = render_adornment(&option.trailing, selected);
    let content = if leading.is_empty() && trailing.is_empty() {
        ui! { SegmentLabel(text = label, selected = selected) }
    } else {
        // `leading` / `trailing` are the adornments the CALLER handed in,
        // already built — splatted, not authored here.
        ui! {
            view(style = SegmentInner()) {
                leading
                SegmentLabel(text = label, selected = selected)
                trailing
            }
        }
    };
    pressable(vec![content], press).with_style(seg_style).into()
}

/// Build an adornment for a segment (empty for `None` / an empty group).
/// `Icon` and `Button` glyphs take the segment's own foreground, live on
/// `selected`, so they recolor with the label; `Element` is left as built.
fn render_adornment(adornment: &Adornment, selected: Memo<bool>) -> Vec<Element> {
    // The plain segment's on/off foreground, from style tokens (no sheet
    // resolve, so this also holds on a `--premint-only` build).
    let glyph_color = move || -> Option<Color> {
        let c = idea_theme::tokens().color;
        Some(if selected.get() { c.text() } else { c.text_muted() }.resolve())
    };
    match adornment {
        Adornment::None => Vec::new(),
        Adornment::Element(build) => vec![build()],
        Adornment::Icon(data) => {
            vec![ui! { Icon(data = data.clone(), size = SEGMENT_ICON_PX, color = Reactive::Dynamic(Rc::new(glyph_color))) }]
        }
        Adornment::Button(data, on_press) => {
            let glyph = ui! { Icon(data = data.clone(), size = SEGMENT_ICON_PX, color = Reactive::Dynamic(Rc::new(glyph_color))) };
            let on_press = on_press.clone();
            // An icon-sized pressable inside the segment's: its recognizer
            // consumes the tap, so pressing it does not also select the
            // segment — the same "button in a clickable row" contract as
            // `TableRow`. Shares `Field`'s icon-button sheet.
            vec![pressable(vec![glyph], move || on_press())
                .with_style(StyleApplication::new(adornment_button_sheet()))
                .into_element()]
        }
        Adornment::Group(items) => items.iter().flat_map(|a| render_adornment(a, selected)).collect(),
    }
}

/// The `selected` axis arm for a segment's state.
fn selected_arm(selected: bool) -> &'static str {
    if selected { "on" } else { "off" }
}

/// A segment's label text: the muted foreground at rest, the full text
/// color when `selected`.
///
/// The SegmentButton sheet's on/off foreground lives on the segment, but
/// native TextView/UILabel/NSTextField don't inherit text color from their
/// parent — only web's CSS cascade does. So this resolves that color and
/// stamps it on the text node itself, reactively (re-runs on `selected` and
/// on a theme swap). Without it a segment label renders in the widget
/// default on native: it never flips on selection and never follows a
/// light/dark swap. Same as `Tabs`.
///
/// ```ignore
/// SegmentLabel(text = label, selected = selected)
/// ```
#[component]
fn SegmentLabel(text: String, selected: bool) -> Element {
    let label_style = move || {
        let arm = selected_arm(selected.get());
        let app = StyleApplication::new(SegmentButton::sheet()).with("selected", arm);
        let base = StyleApplication::new(seg_label_base_sheet());
        if app.attaches_preminted() {
            // Premint web build: the segment pressable's preminted class
            // carries the on/off foreground and the label inherits it via
            // the CSS cascade — the resolve-read below exists ONLY because
            // native text doesn't inherit, and under `--premint-only` it
            // would panic (sheets carry no rule closures). Same as `Tabs`.
            return base;
        }
        let color = resolve_style(&app).color.clone();
        let key = if arm == "on" { "seg_label_on" } else { "seg_label_off" };
        // ENGINE-PATH ONLY: the `attaches_preminted()` early return
        // above guarantees this layer never runs on a premint build,
        // so the computed-layer disqualifier can't fire.
        // idealyst-lint-disable-next-line premint-computed-layer
        base.with_computed(key, move || StyleRules {
            color: color.clone(),
            ..Default::default()
        })
    };
    ui! { text(style = label_style) { text } }
}

recipe!(
    SegmentedControl,
    /// A controlled segmented picker. The host owns the `value` signal
    /// (the selected segment's `id`); `on_change` writes the picked id
    /// back. Build the segments with `SegmentOption::new(id, label)`.
    pub fn segmented_control_view_switch() -> ::runtime_core::Element {
        use crate::components::segmented_control::{SegmentOption, SegmentedControl};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let view = signal("list".to_string());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));
        ui! {
            SegmentedControl(
                value = view,
                on_change = on_change,
                options = vec![
                    SegmentOption::new("list", "List"),
                    SegmentOption::new("grid", "Grid"),
                    SegmentOption::new("map", "Map"),
                ],
            )
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    /// Resolved color on a segment's label text node (NOT the pressable's).
    /// `TStyle::resolve` evaluates the reactive-or-static style either way.
    fn seg_label_color(seg: Element) -> Option<runtime_core::Color> {
        let label = match classify(seg) {
            P::Pressable { mut children, .. } => children.remove(0),
            _ => panic!("a segment is a Pressable"),
        };
        match classify(label) {
            P::Text { style, .. } => {
                style.and_then(|s| s.resolve().color.clone().map(|c| c.resolve()))
            }
            _ => panic!("a segment label is a Text node"),
        }
    }

    /// The color the SegmentButton sheet resolves for a given selected state.
    fn segment_color(selected: &str) -> runtime_core::Color {
        let app = StyleApplication::new(SegmentButton::sheet()).with("selected", selected.to_string());
        resolve_style(&app)
            .color
            .clone()
            .expect("SegmentButton resolves a foreground")
            .resolve()
    }

    // The `--premint-only` read-back: same shape as `Tabs` — the segment
    // label resolved the SegmentButton color into a `with_computed` layer, which
    // disqualifies preminting and panics under `--premint-only`. On a premint
    // build the label application must premint BARE (the segment pressable's
    // preminted class carries the foreground; web inherits it); live/native
    // builds keep the computed stamp.
    #[test]
    fn regression_premint_segment_label_application_premints() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let el = SegmentedControl(SegmentedControlProps {
                options: vec![SegmentOption::new("a", "A")],
                value: runtime_core::signal("a".to_string()).into(),
                ..Default::default()
            });
            let mut children = match classify(el) {
                P::View { children, .. } => children,
                _ => panic!("SegmentedControl renders a row View"),
            };
            let label = match classify(children.remove(0)) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("a segment is a Pressable"),
            };
            let style = match classify(label) {
                P::Text { style, .. } => style.expect("segment label carries a style"),
                _ => panic!("a segment label is a Text node"),
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
                    "and it resolves the SegmentButton foreground for the label node"
                );
            }
        });
    }

    // Regression: the segment label was a bare text node whose color lived only on
    // the wrapping pressable, so on native (no CSS cascade) it rendered in the
    // widget default — the selected segment never took the accent color and the
    // labels never followed a theme swap. Each label must carry its OWN color
    // matching its selected state, like `Tabs`.
    #[test]
    fn regression_segment_labels_carry_their_own_active_color() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let el = SegmentedControl(SegmentedControlProps {
                options: vec![SegmentOption::new("a", "A"), SegmentOption::new("b", "B")],
                value: runtime_core::signal("a".to_string()).into(),
                ..Default::default()
            });
            let mut children = match classify(el) {
                P::View { children, .. } => children,
                _ => panic!("SegmentedControl renders a row View"),
            };
            let second = children.remove(1);
            let first = children.remove(0);
            let on = seg_label_color(first).expect("selected label carries a color");
            let off = seg_label_color(second).expect("unselected label carries a color");
            assert_eq!(on, segment_color("on"), "selected segment label = SegmentButton `on`");
            assert_eq!(off, segment_color("off"), "unselected segment label = SegmentButton `off`");
            assert_ne!(on, off, "selection must change the label color");
    });
    }

    #[test]
    fn defaults_are_empty_and_inert() {
        with_test_world(|| {
            let p = SegmentedControlProps::default();
            assert!(p.options.is_empty());
            assert_eq!(p.value.get(), String::new());
    });
    }

    /// Regression (CrewForge want_2ed95ee5): SegmentedControl drew with the
    /// Tabs sheets (`TabBar` + `TabButton`), so a value choice rendered as an
    /// underlined tab strip, identical to the page's `Tabs`. It must be a
    /// bordered group whose selected segment is filled in, and must not
    /// carry the tab underline.
    #[test]
    fn regression_segmented_control_is_a_bordered_group_not_a_tab_strip() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let el = SegmentedControl(SegmentedControlProps {
                options: vec![SegmentOption::new("a", "A"), SegmentOption::new("b", "B")],
                value: runtime_core::signal("a".to_string()).into(),
                ..Default::default()
            });
            let (group, mut children) = match classify(el) {
                P::View { style, children, .. } => (style.expect("the group is styled").resolve(), children),
                _ => panic!("SegmentedControl renders a row View"),
            };
            let width = |w: &Option<runtime_core::Tokenized<f32>>| w.as_ref().map_or(0.0, |w| w.resolve());
            assert!(width(&group.border_left_width) > 0.0, "the group draws a border around all segments");
            assert!(group.background.is_some(), "the group is a tinted track");

            let seg_style = |seg: Element| match classify(seg) {
                P::Pressable { style, .. } => style.expect("a segment is styled").resolve(),
                _ => panic!("a segment is a Pressable"),
            };
            let off = seg_style(children.remove(1));
            let on = seg_style(children.remove(0));
            let bg = |s: &StyleRules| s.background.as_ref().map(|c| c.resolve());
            assert_ne!(bg(&on), bg(&off), "the selected segment is filled in");
            for seg in [&on, &off] {
                assert_eq!(
                    width(&seg.border_bottom_width),
                    width(&seg.border_left_width),
                    "no tab underline: a segment's border is the same on every side"
                );
            }
        });
    }

    /// The control's two sheets are its own, not the Tabs sheets it used to
    /// borrow: distinct sheets with distinct premint identities, a single
    /// author axis `selected` (default `off`) instead of the tab's `active`,
    /// and the hovered/pressed/focused state overlays — focus draws the
    /// themed ring rather than the platform default.
    #[test]
    fn segmented_sheets_are_their_own_not_the_tab_sheets() {
        use crate::stylesheets::{TabBar, TabButton};
        use runtime_core::StateBits;
        with_test_world(|| {
            install_idea_theme(light_theme());
            for (ours, tabs, what) in [
                (SegmentedGroup::sheet(), TabBar::sheet(), "track"),
                (SegmentButton::sheet(), TabButton::sheet(), "segment"),
            ] {
                assert!(!Rc::ptr_eq(&ours, &tabs), "the {what} sheet is not the Tabs sheet");
                assert_ne!(ours.premint_class(), tabs.premint_class(), "the {what} premints under its own class");
            }

            let seg = SegmentButton::sheet();
            let axes: Vec<_> = seg.premint_author_axes().iter().map(|(a, d)| (a.as_str(), d.as_deref())).collect();
            assert_eq!(axes, vec![("selected", Some("off"))], "one `selected` axis, resting off");
            let mut arms: Vec<_> = seg
                .variant_keys()
                .into_iter()
                .filter(|(axis, _)| axis == "selected")
                .map(|(_, v)| v)
                .collect();
            arms.sort();
            assert_eq!(arms, vec!["off".to_string(), "on".to_string()]);

            let states: Vec<StateBits> = seg.state_axes().iter().map(|(b, _)| *b).collect();
            for (bit, name) in [(StateBits::HOVERED, "hovered"), (StateBits::PRESSED, "pressed"), (StateBits::FOCUSED, "focused")] {
                assert!(states.contains(&bit), "SegmentButton declares a `{name}` state");
            }
            let focused = resolve_style(&StyleApplication::new(seg.clone()).with("__state_focused", "on".to_string()));
            assert_eq!(
                focused.border_left_color.as_ref().and_then(|c| c.name()),
                Some("color-focus-ring"),
                "focus draws the themed ring"
            );
        });
    }

    /// Snapshot of the arms, by TOKEN name: every themed value the track and
    /// the segments paint is a style token, so a theme swap retints them and
    /// no color or spacing is a literal. The selected segment is a raised
    /// key — the surface color with a border — on a `surface_alt` track.
    #[test]
    fn segmented_arms_are_built_from_style_tokens() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let name = |c: &Option<runtime_core::Tokenized<runtime_core::Color>>| c.as_ref().and_then(|c| c.name());

            let group = resolve_style(&StyleApplication::new(SegmentedGroup::sheet()));
            assert_eq!(name(&group.background), Some("color-surface-alt"), "the track is tinted");
            assert_eq!(name(&group.border_left_color), Some("color-border"), "and bordered");
            assert!(group.border_top_left_radius.as_ref().and_then(|r| r.name()).is_some(), "track radius is a token");
            // The inset is a named hairline constant, not a spacing token
            // (the scale's smallest step, `xs` = 4, left the selected key
            // floating loose inside the track).
            assert_eq!(
                group.padding_left.as_ref().map(|p| p.resolve()),
                Some(runtime_core::Length::Px(crate::stylesheets::SEGMENT_TRACK_INSET)),
                "the selected segment sits SEGMENT_TRACK_INSET inside the track"
            );

            let seg = |selected: &str| {
                resolve_style(&StyleApplication::new(SegmentButton::sheet()).with("selected", selected.to_string()))
            };
            let (on, off) = (seg("on"), seg("off"));
            assert_eq!(name(&on.background), Some("color-surface"), "the selected segment is filled in");
            assert_eq!(name(&on.border_left_color), Some("color-border"), "and edged like a raised key");
            assert_eq!(name(&on.color), Some("color-text"));
            assert_eq!(name(&off.background), None, "an unselected segment is transparent");
            assert_eq!(name(&off.color), Some("color-text-muted"));
            assert!(on.border_top_left_radius.as_ref().and_then(|r| r.name()).is_some(), "segment radius is a token");
        });
    }

    /// One pressable segment per option, wrapped in a single row view.
    #[test]
    fn builds_one_segment_per_option() {
        with_test_world(|| {
            let el = SegmentedControl(SegmentedControlProps {
                options: vec![
                    SegmentOption::new("a", "A"),
                    SegmentOption::new("b", "B"),
                    SegmentOption::new("c", "C"),
                ],
                ..Default::default()
            });
            let children = match classify(el) {
                P::View { children, .. } => children,
                _ => panic!("SegmentedControl should render a row View"),
            };
            assert_eq!(children.len(), 3, "one segment per option");
            assert!(
                children.into_iter().all(|c| matches!(classify(c), P::Pressable { .. })),
                "each segment must be a pressable"
            );
    });
    }

    /// The segment's row: leading adornments, the label, trailing ones.
    fn segment_row(seg: Element) -> Vec<Element> {
        let inner = match classify(seg) {
            P::Pressable { mut children, .. } => children.remove(0),
            _ => panic!("a segment is a Pressable"),
        };
        match classify(inner) {
            P::View { children, .. } => children,
            _ => panic!("an adorned segment lays out a row view"),
        }
    }

    /// Adornments: the label is rendered ONCE by the control, between the
    /// leading and trailing adornments — the caller never restates it.
    #[test]
    fn adornments_sit_either_side_of_the_label() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let el = SegmentedControl(SegmentedControlProps {
                options: vec![SegmentOption::new("day", "Day shift")
                    .leading(Adornment::element(|| ui! { view() }))
                    .trailing(Adornment::Icon(crate::components::icon::EMPTY_ICON))],
                value: runtime_core::signal("day".to_string()).into(),
                ..Default::default()
            });
            let seg = match classify(el) {
                P::View { mut children, .. } => children.remove(0),
                _ => panic!("row view"),
            };
            let mut row = segment_row(seg).into_iter();
            assert!(matches!(classify(row.next().unwrap()), P::View { .. }), "leading element first");
            match classify(row.next().unwrap()) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Day shift")),
                _ => panic!("the label sits between the adornments"),
            }
            assert!(matches!(classify(row.next().unwrap()), P::Icon { .. }), "trailing icon last");
            assert!(row.next().is_none());
        });
    }

    /// An `Icon` adornment recolors with its segment, like the label:
    /// muted at rest, the text color when selected.
    #[test]
    fn icon_adornment_follows_the_selected_color() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let c = idea_theme::tokens().color;
            let (on, off) = (Some(c.text().resolve()), Some(c.text_muted().resolve()));
            for (picked, expect) in [("a", [on.clone(), off.clone()]), ("b", [off.clone(), on.clone()])] {
                let opt = |id: &str| SegmentOption::new(id, id).leading(Adornment::Icon(crate::components::icon::EMPTY_ICON));
                let el = SegmentedControl(SegmentedControlProps {
                    options: vec![opt("a"), opt("b")],
                    value: runtime_core::signal(picked.to_string()).into(),
                    ..Default::default()
                });
                let segs = match classify(el) {
                    P::View { children, .. } => children,
                    _ => panic!("row view"),
                };
                let colors: Vec<Option<runtime_core::Color>> = segs
                    .into_iter()
                    .map(|seg| match classify(segment_row(seg).remove(0)) {
                        P::Icon { color, .. } => color,
                        _ => panic!("leading icon"),
                    })
                    .collect();
                assert_eq!(colors, expect.to_vec(), "picked {picked}");
            }
        });
    }

    /// No adornment keeps the bare label (no wrapper row), exactly as
    /// before adornments existed.
    #[test]
    fn unadorned_segment_is_just_its_label() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let el = SegmentedControl(SegmentedControlProps {
                options: vec![SegmentOption::new("a", "A")],
                ..Default::default()
            });
            let seg = match classify(el) {
                P::View { mut children, .. } => children.remove(0),
                _ => panic!("row view"),
            };
            match classify(seg) {
                P::Pressable { mut children, .. } => {
                    assert!(matches!(classify(children.remove(0)), P::Text { .. }))
                }
                _ => panic!("pressable"),
            }
        });
    }
}
