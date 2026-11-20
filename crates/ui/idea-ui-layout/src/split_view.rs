//! [`SplitView`] — two regions side by side from one width per mode,
//! and one narrow form per mode (rule `layout-split`).

use std::rc::Rc;

use icons_lucide::X;
use idea_ui::{tokens, tone, variant, IconButton};
use runtime_core::{
    component, current_breakpoint, stylesheet, ui, AlignItems, Breakpoint, Element, FlexDirection, IdealystSchema,
    JustifyContent, Length, Overflow, PointerEvents, Position, Reactive, Ref, SafeAreaSides, ScrollViewHandle,
    ScrollbarVisibility, Shadow, StyleRules, Tokenized,
};

use crate::page::{pin_rules, PageFrame};

/// The first region's width beside the second in [`SplitMode::Nav`].
pub const SPLIT_NAV_WIDTH: f32 = 240.0;

/// The second region's width beside the first in [`SplitMode::Detail`]
/// and [`SplitMode::Inspector`], unless the screen passes its own.
pub const SPLIT_PANE_WIDTH: f32 = 320.0;

/// How much of the height an inspector may take as a phone's dock; past
/// that it scrolls, and the canvas keeps the rest.
pub const SPLIT_DOCK_MAX_PCT: f32 = 45.0;

/// The tablet overlay's stacking order over the canvas it covers.
const OVERLAY_Z: i32 = 10;
/// The tablet overlay's lift off the canvas: a shadow's drop and blur.
const OVERLAY_SHADOW_Y: f32 = 4.0;
const OVERLAY_SHADOW_BLUR: f32 = 16.0;

/// A carded pane's lift: a card's, not an overlay's.
const CARD_SHADOW_Y: f32 = 1.0;
const CARD_SHADOW_BLUR: f32 = 3.0;

/// What a [`SplitView`]'s two regions are, which decides where they sit
/// at each width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum SplitMode {
    /// A list of destinations (`first`) and the one that is open
    /// (`second`). Beside from `Md`, the list [`SPLIT_NAV_WIDTH`] wide.
    /// Below it the list is the screen until a destination is open, and
    /// then the destination is.
    #[default]
    Nav,
    /// A record (`first`) and its companion panel (`second`). Beside
    /// from `Md`; below it the panel stacks under the record.
    Detail,
    /// A canvas (`first`) and the settings of what is selected
    /// (`second`). Beside from `Lg`. On a tablet the pane overlays from
    /// the right while something is selected, with ✕ to close it; on a
    /// phone it docks under the canvas.
    Inspector,
}

impl SplitMode {
    /// Whether this mode's regions sit beside each other at the current
    /// width. TRACKED.
    ///
    /// For the decisions a screen makes alongside its split, never for
    /// choosing what to draw: a row that resets a stack beside the list
    /// and pushes onto it below.
    pub fn beside(self) -> bool {
        current_breakpoint().get().is_at_least(self.threshold())
    }

    fn threshold(self) -> Breakpoint {
        match self {
            SplitMode::Nav | SplitMode::Detail => Breakpoint::Md,
            SplitMode::Inspector => Breakpoint::Lg,
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SplitViewProps {
    /// Which pair this is. Static: a screen does not change layout.
    #[prop(static)]
    pub mode: SplitMode,
    /// Nav: the list. Detail: the record. Inspector: the canvas.
    pub first: Element,
    /// Nav: the open destination. Detail: the panel. Inspector: the
    /// pane.
    pub second: Element,
    /// Nav: a destination is open, so below `Md` it is the screen
    /// rather than the list. A navigator that pushes its destinations
    /// itself passes `true`. Inspector: something is selected, so a
    /// tablet shows the pane. Detail ignores it.
    pub open: Reactive<bool>,
    /// Nav only: the open destination takes the whole width at every
    /// size (an editor pushed below a destination, which the list does
    /// not lead to).
    pub full: Reactive<bool>,
    /// Inspector: what ✕ on a tablet's overlay does (clear the
    /// selection).
    #[prop(static)]
    pub on_close: Option<Rc<dyn Fn()>>,
    /// The second region's width beside the first, for Detail and
    /// Inspector. `None` is [`SPLIT_PANE_WIDTH`].
    #[prop(static)]
    pub width: Option<f32>,
    /// The page the split sits in, when that page scrolls (a `Content`
    /// page): beside the other region, the list or the panel pins under
    /// the page's header and scrolls itself only when it is taller than
    /// the room left. `None` when the split fills a bounded body (a
    /// `Blank` page), where each region is as tall as the body.
    #[prop(static)]
    pub frame: Option<PageFrame>,
    /// Inspector, beside the canvas: the pane is a card in a well
    /// (filled, rounded, lifted, no rule, stood off the canvas) for an
    /// editor whose canvas is the ground rather than the page's surface.
    #[prop(static)]
    pub card: bool,
    /// Inspector: the pane's content brings its own head, rules and
    /// padding and fills the pane's height, so the split pads nothing
    /// round it.
    #[prop(static)]
    pub bare: bool,
    /// In a page (`frame`): the split fills the height the page leaves
    /// when its content is shorter, so a Nav list's rule runs to the
    /// foot of the window.
    #[prop(static)]
    pub grow: bool,
    /// Filled with the scroller of the region that scrolls itself (Nav:
    /// the list; Detail: the panel; Inspector: the pane), to move it: a
    /// drag held near its top or foot scrolling a list in it.
    #[prop(static)]
    pub scroller: Option<Ref<ScrollViewHandle>>,
    /// Each change of that region's scroll offset `(x, y)`, however it
    /// scrolled: what a drag in it reports to its drag context. In a
    /// scrolling page the page's scroller moves the region too until it
    /// pins: report that one from the [`Page`](crate::Page)'s `on_scroll`.
    #[prop(static)]
    pub on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
}

impl Default for SplitViewProps {
    fn default() -> Self {
        Self {
            mode: SplitMode::Nav,
            first: ui! { view {} },
            second: ui! { view {} },
            open: Reactive::Static(false),
            full: Reactive::Static(false),
            on_close: None,
            width: None,
            frame: None,
            card: false,
            bare: false,
            grow: false,
            scroller: None,
            on_scroll: None,
        }
    }
}

/// Two regions that sit side by side from the mode's width and take the
/// mode's narrow form below it. Crossing the width RESTYLES both regions
/// and never remounts either, so a half-typed field, an open menu and a
/// scroll offset all survive a rotation. The list (Nav), the panel
/// (Detail) and the pane (Inspector) scroll themselves; the other
/// region's content brings its own scroller, or grows in the page
/// around it.
///
/// ```ignore
/// SplitView(
///     mode = SplitMode::Nav,
///     first = sections,
///     second = section_body,
///     open = rx!(open_section.get().is_some()),
/// )
/// SplitView(
///     mode = SplitMode::Inspector,
///     first = canvas,
///     second = inspector,
///     open = rx!(selected.get().is_some()),
///     on_close = Some(clear_selection),
/// )
/// ```
#[component]
pub fn SplitView(props: SplitViewProps) -> Element {
    let SplitViewProps { mode, first, second, open, full, on_close, width, frame, card, bare, grow, scroller, on_scroll } =
        props;
    let pane_width = width.unwrap_or(SPLIT_PANE_WIDTH);
    let state = {
        let open = open.clone();
        let full = full.clone();
        move || Layout { width: width_now(mode), open: open.get(), full: full.get() }
    };
    let frame_style = {
        let state = state.clone();
        move || frame_rules(mode, state().width, frame.is_some(), grow)
    };
    let first_style = {
        let state = state.clone();
        move || {
            let form = first_form(mode, state());
            pinned(region_rules(form, SPLIT_NAV_WIDTH), form == RegionForm::Nav, frame)
        }
    };
    let second_style = {
        let state = state.clone();
        move || {
            let form = second_form(mode, state());
            let mut rules = region_rules(form, pane_width);
            if card && mode == SplitMode::Inspector && form == RegionForm::Side {
                rules = carded(rules);
            }
            // Detail's panel stands off its rule as Nav's destination
            // does. Stacked under the record it takes none, so its text
            // stays on the record's edge. An Inspector's pane pads
            // under its own scroller instead (`SplitSideBody`).
            if mode == SplitMode::Detail && form == RegionForm::Side {
                rules.padding_left = Some(tokens().spacing.xl());
            }
            pinned(rules, form == RegionForm::Side, frame)
        }
    };
    // ✕ is the tablet overlay's alone: beside, the pane is always
    // there, and a phone's dock is part of the screen.
    let close_style = {
        let state = state.clone();
        move || close_rules(second_form(mode, state()) == RegionForm::Overlay)
    };
    let inset = match (mode, bare) {
        (_, true) => SplitSideBodyInset::Fill,
        (SplitMode::Inspector, false) => SplitSideBodyInset::Pane,
        _ => SplitSideBodyInset::None,
    };
    // In a bounded body the side region's scroller reaches the screen's
    // bottom edge, so what it scrolls ends clear of the home indicator.
    // In a scrolling page the page pads for it.
    let side_inset = if frame.is_none() { SafeAreaSides::BOTTOM } else { SafeAreaSides::NONE };
    // Which region is the side one is the mode's, so a static branch:
    // the side region scrolls itself, the main one is its content's.
    let (side, main) = match mode {
        SplitMode::Nav => (first, second),
        SplitMode::Detail | SplitMode::Inspector => (second, first),
    };
    let side = ui! {
        view(style = SplitSideBody().inset(inset), safe_area = side_inset) { side }
    };
    let scroller = scroller.unwrap_or_else(Ref::new);
    let side_scroll = match on_scroll {
        Some(on_scroll) => ui! {
            scroll_view(style = SplitSideScroll(), bind = scroller, on_scroll = move |x, y| on_scroll(x, y)) { side }
        },
        None => ui! {
            scroll_view(style = SplitSideScroll(), bind = scroller) { side }
        },
    };
    match mode {
        SplitMode::Nav => ui! {
            view(style = frame_style, test_id = "split-view") {
                view(style = first_style, test_id = "split-first") { side_scroll }
                view(style = second_style, test_id = "split-second") { main }
            }
        },
        SplitMode::Detail | SplitMode::Inspector => ui! {
            view(style = frame_style, test_id = "split-view") {
                view(style = first_style, test_id = "split-first") { main }
                view(style = second_style, test_id = "split-second") {
                    if let Some(close) = on_close {
                        view(style = close_style) {
                            view(test_id = "split-close") {
                                IconButton(
                                    icon = Some(X),
                                    on_click = close,
                                    tone = tone::Neutral,
                                    variant = variant::Ghost,
                                    label = Some("Close".to_string()),
                                )
                            }
                        }
                    }
                    side_scroll
                }
            }
        },
    }
}

/// The widths a split tells apart. Nav and Detail have no tablet form
/// of their own: below `Md` is narrow, a tablet included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Width {
    Beside,
    Tablet,
    Phone,
}

fn width_at(mode: SplitMode, breakpoint: Breakpoint) -> Width {
    if breakpoint.is_at_least(mode.threshold()) {
        Width::Beside
    } else if breakpoint.is_at_least(Breakpoint::Sm) {
        Width::Tablet
    } else {
        Width::Phone
    }
}

fn width_now(mode: SplitMode) -> Width {
    width_at(mode, current_breakpoint().get())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    width: Width,
    open: bool,
    full: bool,
}

/// What a region is at one width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegionForm {
    /// The rest of the row (or the whole of it).
    Fill,
    /// Nav's list beside the destination, ruled on its right.
    Nav,
    /// Nav's destination beside its list: the rest of the row, inset
    /// from the list's rule.
    Destination,
    /// A panel or pane beside the main region, ruled on its left.
    Side,
    /// Detail's record and panel, one under the other, each at its own
    /// height.
    Stacked,
    /// Inspector's pane over the canvas from the right, on a tablet.
    Overlay,
    /// Inspector's pane under the canvas, on a phone.
    Dock,
    /// Not on show: no room, no ink, no pointer. Still mounted.
    Hidden,
}

fn first_form(mode: SplitMode, at: Layout) -> RegionForm {
    match (mode, at.width) {
        (SplitMode::Nav, Width::Beside) if at.full => RegionForm::Hidden,
        (SplitMode::Nav, Width::Beside) => RegionForm::Nav,
        (SplitMode::Nav, _) if at.open || at.full => RegionForm::Hidden,
        // A stack is two sections at their own heights: a record that
        // grew into the room left would be clamped under its panel.
        (SplitMode::Detail, Width::Tablet | Width::Phone) => RegionForm::Stacked,
        _ => RegionForm::Fill,
    }
}

fn second_form(mode: SplitMode, at: Layout) -> RegionForm {
    match (mode, at.width) {
        (SplitMode::Nav, Width::Beside) if at.full => RegionForm::Fill,
        (SplitMode::Nav, Width::Beside) => RegionForm::Destination,
        (SplitMode::Nav, _) if at.open || at.full => RegionForm::Fill,
        (SplitMode::Nav, _) => RegionForm::Hidden,
        (SplitMode::Detail, Width::Beside) => RegionForm::Side,
        (SplitMode::Detail, _) => RegionForm::Stacked,
        (SplitMode::Inspector, Width::Beside) => RegionForm::Side,
        (SplitMode::Inspector, Width::Tablet) if at.open => RegionForm::Overlay,
        (SplitMode::Inspector, Width::Tablet) => RegionForm::Hidden,
        (SplitMode::Inspector, Width::Phone) => RegionForm::Dock,
    }
}

/// The pair's container: a row, except Detail's stack and Inspector's
/// dock on a phone, which are columns. `in_page`: the split grows in a
/// page that scrolls rather than filling a bounded parent; `grow` lets
/// it also take the height that page leaves.
fn frame_rules(mode: SplitMode, width: Width, in_page: bool, grow: bool) -> StyleRules {
    let column = match mode {
        SplitMode::Nav => false,
        SplitMode::Detail => width != Width::Beside,
        SplitMode::Inspector => width == Width::Phone,
    };
    let t = tokens();
    StyleRules {
        position: Some(Position::Relative),
        flex_direction: Some(if column { FlexDirection::Column } else { FlexDirection::Row }),
        align_items: Some(AlignItems::Stretch),
        // Detail's stack is two sections of a page; every other form is
        // ruled, edge to edge.
        gap: (mode == SplitMode::Detail && column).then(|| t.spacing.lg()),
        width: Some(Tokenized::Literal(Length::pct(100.0))),
        height: (!in_page).then_some(Tokenized::Literal(Length::pct(100.0))),
        flex_grow: Some(Tokenized::Literal(if in_page && !grow { 0.0 } else { 1.0 })),
        flex_shrink: Some(Tokenized::Literal(if in_page { 0.0 } else { 1.0 })),
        min_height: Some(Tokenized::Literal(Length::Px(0.0))),
        min_width: Some(Tokenized::Literal(Length::Px(0.0))),
        ..Default::default()
    }
}

/// One region's rules in `form`; `width` is its width beside the other.
///
/// Every form names `flex_direction`: a view is a flex container only
/// when it declares one, and without it a child scroller's grow, shrink
/// and min-height floor are inert.
fn region_rules(form: RegionForm, width: f32) -> StyleRules {
    let t = tokens();
    let px = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
    let num = |v: f32| Some(Tokenized::Literal(v));
    let column = Some(FlexDirection::Column);
    match form {
        RegionForm::Fill => StyleRules {
            flex_direction: column,
            flex_grow: num(1.0),
            flex_shrink: num(1.0),
            flex_basis: px(0.0),
            min_width: px(0.0),
            min_height: px(0.0),
            ..Default::default()
        },
        // The rule parts the two regions, and each stands off it: the
        // list by `md`, the destination by the wider `xl`, so the rule
        // reads as the list's edge.
        RegionForm::Destination => StyleRules {
            flex_direction: column,
            flex_grow: num(1.0),
            flex_shrink: num(1.0),
            flex_basis: px(0.0),
            min_width: px(0.0),
            min_height: px(0.0),
            padding_left: Some(t.spacing.xl()),
            ..Default::default()
        },
        RegionForm::Nav => StyleRules {
            flex_direction: column,
            width: px(width),
            flex_grow: num(0.0),
            flex_shrink: num(0.0),
            min_height: px(0.0),
            padding_right: Some(t.spacing.md()),
            border_right_width: num(1.0),
            border_right_color: Some(t.color.border()),
            ..Default::default()
        },
        RegionForm::Side => StyleRules {
            flex_direction: column,
            width: px(width),
            flex_grow: num(0.0),
            flex_shrink: num(0.0),
            min_height: px(0.0),
            background: Some(t.color.surface()),
            border_left_width: num(1.0),
            border_left_color: Some(t.color.border()),
            ..Default::default()
        },
        RegionForm::Stacked => StyleRules {
            flex_direction: column,
            width: Some(Tokenized::Literal(Length::pct(100.0))),
            flex_shrink: num(0.0),
            ..Default::default()
        },
        RegionForm::Overlay => StyleRules {
            flex_direction: column,
            position: Some(Position::Absolute),
            top: px(0.0),
            right: px(0.0),
            bottom: px(0.0),
            width: px(width),
            max_width: Some(Tokenized::Literal(Length::pct(100.0))),
            z_index: Some(OVERLAY_Z),
            background: Some(t.color.surface()),
            border_left_width: num(1.0),
            border_left_color: Some(t.color.border()),
            shadow: Some(Shadow { x: 0.0, y: OVERLAY_SHADOW_Y, blur: OVERLAY_SHADOW_BLUR, color: t.effect.shadow() }),
            ..Default::default()
        },
        RegionForm::Dock => StyleRules {
            flex_direction: column,
            width: Some(Tokenized::Literal(Length::pct(100.0))),
            max_height: Some(Tokenized::Literal(Length::pct(SPLIT_DOCK_MAX_PCT))),
            flex_grow: num(0.0),
            flex_shrink: num(0.0),
            min_height: px(0.0),
            background: Some(t.color.surface()),
            border_top_width: num(1.0),
            border_top_color: Some(t.color.border()),
            ..Default::default()
        },
        // Collapsed on both axes, so it takes nothing in a row or a
        // column; transparent and deaf, so what is still mounted inside
        // neither shows nor takes a press. The style vocabulary has no
        // `display: none`.
        RegionForm::Hidden => StyleRules {
            flex_direction: column,
            width: px(0.0),
            min_width: px(0.0),
            max_width: px(0.0),
            height: px(0.0),
            min_height: px(0.0),
            max_height: px(0.0),
            flex_grow: num(0.0),
            flex_shrink: num(0.0),
            flex_basis: px(0.0),
            overflow: Some(Overflow::Hidden),
            opacity: num(0.0),
            pointer_events: Some(PointerEvents::None),
            ..Default::default()
        },
    }
}

/// A side pane's `rules` as a card in a well: the control fill a `Card`
/// takes, rounded and lifted, with the well's gap between it and the
/// canvas where a plain pane has its rule.
fn carded(rules: StyleRules) -> StyleRules {
    let t = tokens();
    let radius = || Some(t.radius.lg());
    StyleRules {
        background: Some(t.color.control()),
        border_left_width: Some(Tokenized::Literal(0.0)),
        border_top_left_radius: radius(),
        border_top_right_radius: radius(),
        border_bottom_left_radius: radius(),
        border_bottom_right_radius: radius(),
        overflow: Some(Overflow::Hidden),
        shadow: Some(Shadow { x: 0.0, y: CARD_SHADOW_Y, blur: CARD_SHADOW_BLUR, color: t.effect.shadow() }),
        margin_left: Some(t.spacing.sm()),
        ..rules
    }
}

/// `rules`, pinned under `frame`'s header when `pin` and the split sits
/// in a page.
fn pinned(rules: StyleRules, pin: bool, frame: Option<PageFrame>) -> StyleRules {
    match frame {
        Some(frame) if pin => pin_rules(frame, rules),
        _ => rules,
    }
}

fn close_rules(shown: bool) -> StyleRules {
    let t = tokens();
    if !shown {
        return StyleRules {
            flex_direction: Some(FlexDirection::Row),
            height: Some(Tokenized::Literal(Length::Px(0.0))),
            min_height: Some(Tokenized::Literal(Length::Px(0.0))),
            flex_shrink: Some(Tokenized::Literal(0.0)),
            overflow: Some(Overflow::Hidden),
            opacity: Some(Tokenized::Literal(0.0)),
            pointer_events: Some(PointerEvents::None),
            ..Default::default()
        };
    }
    StyleRules {
        flex_direction: Some(FlexDirection::Row),
        justify_content: Some(JustifyContent::FlexEnd),
        flex_shrink: Some(Tokenized::Literal(0.0)),
        padding_top: Some(t.spacing.sm()),
        padding_right: Some(t.spacing.sm()),
        ..Default::default()
    }
}

stylesheet! {
    // A side region's scroller, bounded by its parent instead of by its
    // content. The three rules are one idea: `flex_grow` takes the
    // height the row hands out, `flex_shrink` lets it be clamped back to
    // that height, and `min_height: 0` is what permits the clamp, since
    // a flex item's automatic minimum is its content and a scroller that
    // may not shrink below its content never scrolls.
    //
    // `flex_basis: auto` because a phone's inspector dock sizes to its
    // content (up to its cap): the scroller must count its content toward
    // that, as the web's `flex: 1 1 auto` does. Native seeds a vertical
    // scroller with a 0 basis, under which the dock collapsed and the
    // content above took the whole screen. In the other forms the region's
    // height is set by its parent, so the basis changes nothing there.
    //
    // No bar: its track would inset the pane's content from its rule.
    SplitSideScroll<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            min_height: Length::Px(0.0),
            min_width: Length::Px(0.0),
            scrollbar: ScrollbarVisibility::Hidden,
        }
    }
}

stylesheet! {
    // What a side region scrolls: its content at its own height
    // (`flex_shrink: 0`, or the scroller would squeeze it instead of
    // scrolling it). An inspector's pane carries its padding here, under
    // the scroller, so the last row clears the foot rather than being
    // cut off by padding that never scrolled. A bare pane's panel pads
    // itself, and is at least the pane's height.
    SplitSideBody<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
        }
        variant inset {
            #[default]
            none(_t) {}
            pane(t) { padding: t.spacing.lg() }
            fill(_t) { min_height: Length::pct(100.0) }
        }
    }
}

#[cfg(test)]
mod card_tests {
    use super::*;

    /// A carded pane keeps the side pane's size and trades its rule for
    /// a card's fill, corners, lift and a gap from the canvas.
    #[test]
    fn a_carded_pane_is_the_side_pane_without_its_rule() {
        idea_theme::testing::with_test_world(|| {
            let side = region_rules(RegionForm::Side, 320.0);
            let card = carded(side.clone());
            assert_eq!(card.width, side.width);
            assert_eq!(card.flex_shrink, side.flex_shrink);
            assert_eq!(card.border_left_width, Some(Tokenized::Literal(0.0)));
            assert_eq!(card.background, Some(tokens().color.control()));
            assert!(card.shadow.is_some() && card.margin_left.is_some() && card.border_top_left_radius.is_some());
            assert_eq!(card.overflow, Some(Overflow::Hidden));
        });
    }

    /// CrewForge (iOS): a phone's inspector dock is as tall as its content,
    /// so the pane's scroller must count its content: `flex_basis: auto`
    /// over native's 0-basis scroller seed (the dock collapsed, and the
    /// content above took the screen). runtime-layout's
    /// `regression_a_scroller_in_a_content_sized_dock_is_content_tall_with_basis_auto`
    /// pins what the basis does.
    #[test]
    fn regression_the_side_scroller_counts_its_content() {
        idea_theme::testing::with_test_world(|| {
            let rules = runtime_core::resolve_style(&SplitSideScroll().into_style_application());
            assert_eq!(rules.flex_basis, Some(Tokenized::Literal(Length::Auto)));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(width: Width, open: bool) -> Layout {
        Layout { width, open, full: false }
    }

    #[test]
    fn nav_shows_both_beside_and_one_below() {
        for open in [false, true] {
            assert_eq!(first_form(SplitMode::Nav, at(Width::Beside, open)), RegionForm::Nav);
            assert_eq!(second_form(SplitMode::Nav, at(Width::Beside, open)), RegionForm::Destination);
        }
        for width in [Width::Tablet, Width::Phone] {
            assert_eq!(first_form(SplitMode::Nav, at(width, false)), RegionForm::Fill);
            assert_eq!(second_form(SplitMode::Nav, at(width, false)), RegionForm::Hidden);
            assert_eq!(first_form(SplitMode::Nav, at(width, true)), RegionForm::Hidden);
            assert_eq!(second_form(SplitMode::Nav, at(width, true)), RegionForm::Fill);
        }
    }

    #[test]
    fn a_full_destination_hides_the_list_at_every_width() {
        for width in [Width::Beside, Width::Tablet, Width::Phone] {
            let full = Layout { width, open: false, full: true };
            assert_eq!(first_form(SplitMode::Nav, full), RegionForm::Hidden);
            assert_eq!(second_form(SplitMode::Nav, full), RegionForm::Fill);
        }
    }

    #[test]
    fn detail_stacks_below_md() {
        assert_eq!(second_form(SplitMode::Detail, at(Width::Beside, false)), RegionForm::Side);
        assert_eq!(second_form(SplitMode::Detail, at(Width::Tablet, false)), RegionForm::Stacked);
        assert_eq!(second_form(SplitMode::Detail, at(Width::Phone, false)), RegionForm::Stacked);
        assert_eq!(first_form(SplitMode::Detail, at(Width::Phone, false)), RegionForm::Stacked);
        assert_eq!(first_form(SplitMode::Detail, at(Width::Beside, false)), RegionForm::Fill);
    }

    #[test]
    fn inspector_overlays_on_a_tablet_only_while_open_and_docks_on_a_phone() {
        assert_eq!(second_form(SplitMode::Inspector, at(Width::Beside, false)), RegionForm::Side);
        assert_eq!(second_form(SplitMode::Inspector, at(Width::Tablet, true)), RegionForm::Overlay);
        assert_eq!(second_form(SplitMode::Inspector, at(Width::Tablet, false)), RegionForm::Hidden);
        assert_eq!(second_form(SplitMode::Inspector, at(Width::Phone, false)), RegionForm::Dock);
        assert_eq!(first_form(SplitMode::Inspector, at(Width::Phone, true)), RegionForm::Fill);
    }

    #[test]
    fn each_mode_sits_beside_from_its_own_width() {
        assert_eq!(width_at(SplitMode::Nav, Breakpoint::Md), Width::Beside);
        assert_eq!(width_at(SplitMode::Nav, Breakpoint::Sm), Width::Tablet);
        assert_eq!(width_at(SplitMode::Detail, Breakpoint::Md), Width::Beside);
        assert_eq!(width_at(SplitMode::Inspector, Breakpoint::Md), Width::Tablet);
        assert_eq!(width_at(SplitMode::Inspector, Breakpoint::Lg), Width::Beside);
        assert_eq!(width_at(SplitMode::Inspector, Breakpoint::Xs), Width::Phone);
    }
}
