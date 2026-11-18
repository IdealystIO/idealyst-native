//! [`Page`] — the frame every screen stands in: its header pinned at
//! the top and, under it, the body in the seat its variant names.

use runtime_core::Tokens as _;
use runtime_core::{
    component, derived, stylesheet, ui, ChildList, Element, FlexDirection, IdealystSchema, Length, Position,
    Reactive, Ref, SafeAreaSides, ScrollbarVisibility, ViewHandle,
};

use super::{watch_height, PageFrame};
use crate::LayoutTheme;

/// What a [`Page`] puts under its header. The variant decides three
/// things and nothing else: who scrolls, whether the body is bounded by
/// the window, and what gutter it stands in.
///
/// | Variant | Scrolls | Body height | Gutter |
/// |---|---|---|---|
/// | `Content` | the page | its content's | all round |
/// | `Table` | the table inside | what the header leaves | sides only |
/// | `Blank` | whatever the body brings | what the header leaves | none |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, IdealystSchema)]
pub enum PageVariant {
    /// Rule `page-content`. The body scrolls as one column under the
    /// header, in the page gutter. What a screen is unless it is one of
    /// the others: a record's detail, a form, a settings section.
    #[default]
    Content,
    /// Rule `page-table`. No page scroller: the body is bounded to what
    /// the header leaves and the table in it owns the one scroller, so
    /// everything above its rows holds still. The body stands in the
    /// side gutter with no inset above or below; the toolbar's padding
    /// is the space above and the table's own foot ends it.
    Table,
    /// Rule `page-blank`. The body is bounded to what the header leaves
    /// and nothing else is decided: no gutter, no scroller. For a body
    /// that lays itself out edge to edge: a workbench's canvas and
    /// inspector, a [`Flow`](crate::Flow), a custom grid, a map.
    Blank,
}

impl PageVariant {
    /// Whether the page itself scrolls.
    pub fn scrolls(self) -> bool {
        self == PageVariant::Content
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct PageProps {
    /// What sits under the header. Static: the variants are different
    /// seats, and a screen does not change layout.
    #[prop(static)]
    pub variant: PageVariant,
    /// The screen's header: normally a [`PageHeader`](super::PageHeader).
    pub header: Element,
    /// A row pinned under the header, in the page gutter: normally a
    /// [`Toolbar`](crate::Toolbar) (rule `toolbar-row`). `None` takes no
    /// height.
    pub toolbar: Option<Element>,
    /// Everything below the header.
    pub body: Element,
    /// `Content` and `Table`: the body runs edge to edge and insets its
    /// own parts. It must still line its first text up with the title.
    #[prop(static)]
    pub bleed: bool,
    /// The page's measurements, filled in here and read by what pins
    /// inside the body: a [`SplitView`](crate::SplitView), a
    /// [`PinnedPane`](super::PinnedPane).
    #[prop(static)]
    pub frame: Option<PageFrame>,
    /// A shell whose body is a navigator outlet: the variant of the tab
    /// on show, read live, in place of `variant`. The frame restyles
    /// rather than rebuilds, so the tab in the outlet is never torn down
    /// by a change of variant.
    #[prop(static)]
    pub live_variant: Option<Reactive<PageVariant>>,
    /// The screen's dialogs and sheets, mounted beside the frame so
    /// nothing the body rebuilds can tear one down.
    pub children: Vec<Element>,
}

impl Default for PageProps {
    fn default() -> Self {
        Self {
            variant: PageVariant::Content,
            header: ui! { view {} },
            toolbar: None,
            body: ui! { view {} },
            bleed: false,
            frame: None,
            live_variant: None,
            children: Vec::new(),
        }
    }
}

/// The frame of a screen: `header` and `toolbar` pinned at the top,
/// `body` under them in the seat `variant` names. It owns the page
/// gutter, the one-scroller invariant, and the top and side safe areas,
/// so nothing inside pads to the screen's edge or adds a page scroller
/// of its own. Content runs under the bottom safe area and is padded
/// clear of it (rule `safe-areas`): a `Content` page does that itself;
/// in a `Table` or `Blank` page the body's own scroller ends with a
/// [`BottomInset`](crate::BottomInset). It paints the page's surface: a screen a
/// navigator pushes must be opaque, or the transition shows through it.
///
/// A layout mounted as a tab's body inside a shell takes no `Page`: the
/// shell's is already there.
///
/// ```ignore
/// ui! {
///     Page(header = header, body = ui! {
///         Stack(gap = StackGap::Lg) {
///             Section(title = "Shift".to_string()) { … }
///             Section(title = "Crew".to_string()) { … }
///         }
///     }) {
///         EditShiftDialog(…)
///     }
/// }
///
/// // A shell around a navigator outlet, its variant the tab's:
/// ui! { Page(live_variant = Some(rx!(tab_variant())), header = header, body = nav.outlet) }
/// ```
#[component(children)]
pub fn Page(props: PageProps) -> Element {
    let PageProps { variant, header, toolbar, body, bleed, frame, live_variant, children } = props;
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut overlays = Vec::with_capacity(children.len());
    for c in children {
        ChildList::append_to(c, &mut overlays);
    }
    let frame = frame.unwrap_or_default();
    let edge = if bleed { PageGutterEdge::Bleed } else { PageGutterEdge::Inset };
    let framed = match live_variant {
        Some(live) => live_page(header, toolbar, body, live, edge, frame),
        None if variant.scrolls() => scroller(header, toolbar, body, Reactive::Static(variant), edge, frame),
        None => bounded_page(header, toolbar, body, variant, edge, frame),
    };
    if overlays.is_empty() {
        return framed;
    }
    ui! {
        framed
        overlays
    }
}

/// The header block: the header, then the toolbar's seat. Its height is
/// what a pinned pane rests under.
fn header_block(header: Element, toolbar: Option<Element>, bind: Ref<ViewHandle>) -> Element {
    ui! {
        view(
            style = PageHeaderBlock(),
            bind = bind,
            safe_area = SafeAreaSides::TOP | SafeAreaSides::HORIZONTAL,
        ) {
            header
            if let Some(toolbar) = toolbar {
                view(style = PageToolbarSeat()) { toolbar }
            }
        }
    }
}

/// `Table` and `Blank` as a screen's own frame: no scroller at all. A
/// page scroller around a bounded body is not inert just because its
/// content fits: on iOS it rubber-bands the header off screen, its pan
/// competes with the inner scroller's, and an end-reached callback on
/// it is true from the first frame.
fn bounded_page(
    header: Element,
    toolbar: Option<Element>,
    body: Element,
    variant: PageVariant,
    edge: PageGutterEdge,
    frame: PageFrame,
) -> Element {
    let header_ref: Ref<ViewHandle> = Ref::new();
    let port_ref: Ref<ViewHandle> = Ref::new();
    watch_height(header_ref, frame.header_height);
    watch_height(port_ref, frame.port_height);
    let kind = gutter_kind(variant);
    let head = header_block(header, toolbar, header_ref);
    ui! {
        view(style = PageColumn(), bind = port_ref) {
            head
            // No bottom inset: a bounded body runs to the screen's
            // bottom edge, under the home indicator, and what scrolls in
            // it ends with a `BottomInset` (rule `safe-areas`).
            view(style = PageBounded(), safe_area = SafeAreaSides::HORIZONTAL) {
                view(style = PageGutter().kind(kind).edge(edge)) { body }
            }
        }
    }
}

/// A shell around a navigator outlet, whose variant follows the tab on
/// show. One tree for all three variants, restyled: swapping the
/// container around a live outlet would tear the mounted tab down every
/// time the variant changed.
fn live_page(
    header: Element,
    toolbar: Option<Element>,
    body: Element,
    variant: Reactive<PageVariant>,
    edge: PageGutterEdge,
    frame: PageFrame,
) -> Element {
    let outlet_variant = variant.clone();
    let mode = derived(move || match outlet_variant.get().scrolls() {
        true => PageOutletMode::Scrolling,
        false => PageOutletMode::Fixed,
    });
    let body = ui! { view(style = PageOutlet().mode(mode)) { body } };
    scroller(header, toolbar, body, variant, edge, frame)
}

/// The page scroller under the header: what a `Content` page is, and
/// what a live shell restyles between its variants.
fn scroller(
    header: Element,
    toolbar: Option<Element>,
    body: Element,
    variant: Reactive<PageVariant>,
    edge: PageGutterEdge,
    frame: PageFrame,
) -> Element {
    let header_ref: Ref<ViewHandle> = Ref::new();
    let port_ref: Ref<ViewHandle> = Ref::new();
    watch_height(header_ref, frame.header_height);
    watch_height(port_ref, frame.port_height);
    let head = header_block(header, toolbar, header_ref);
    // One variant drives four sheets; each takes a source of its own.
    let scroll_mode = {
        let v = variant.clone();
        derived(move || if v.get().scrolls() { PageScrollMode::Scrolling } else { PageScrollMode::Fixed })
    };
    let body_mode = {
        let v = variant.clone();
        derived(move || if v.get().scrolls() { PageScrollBodyMode::Scrolling } else { PageScrollBodyMode::Fixed })
    };
    let fill_mode = {
        let v = variant.clone();
        derived(move || if v.get().scrolls() { PageFillMode::Scrolling } else { PageFillMode::Fixed })
    };
    let kind = derived(move || gutter_kind(variant.get()));
    // The header sits ABOVE the scroller, as its sibling: its rule runs
    // the page's full width, and the scroll bar belongs to the area
    // under it.
    //
    // The scroller runs to the screen's bottom edge, so content passes
    // under the home indicator as it scrolls; the bottom inset is
    // padding on the page column inside it, so the last of the content
    // rests clear. The scroller opts out of the platform's own
    // adjustment (`safe_area = NONE`; on
    // iOS an unset scroller keeps the automatic inset). On a fixed page
    // the automatic inset manufactures scroll extent out of content that
    // fits exactly, and a drag outside the inner scroller carries the
    // whole page up. As padding it is layout the framework sees: a fixed
    // page spends it out of the growing slot and still fits.
    //
    // `always_bounce = false` drops the spring for content that fits and
    // keeps it for content that overflows, so a fixed page does not rock
    // under a finger that meant its inner list.
    ui! {
        view(style = PageColumn(), bind = port_ref) {
            head
            scroll_view(
                style = PageScroll().mode(scroll_mode),
                bind = frame.scroller,
                safe_area = SafeAreaSides::NONE,
                always_bounce = false,
            ) {
                view(style = PageScrollBody().mode(body_mode)) {
                    view(
                        style = PageFill().mode(fill_mode),
                        safe_area = SafeAreaSides::BOTTOM | SafeAreaSides::HORIZONTAL,
                    ) {
                        view(style = PageGutter().kind(kind).edge(edge)) { body }
                    }
                }
            }
        }
    }
}

fn gutter_kind(variant: PageVariant) -> PageGutterKind {
    match variant {
        PageVariant::Content => PageGutterKind::Content,
        PageVariant::Table => PageGutterKind::Table,
        PageVariant::Blank => PageGutterKind::Blank,
    }
}

stylesheet! {
    // The page: a non-growing header over a growing body. `min_height:
    // 0` is load-bearing: a flex item's automatic minimum is its
    // content, and the column would grow to it instead of letting the
    // body scroll.
    //
    // It paints the surface because nothing under it does. A screen
    // left transparent shows whatever is behind it, and during a
    // navigator's push that is the transition container, not the page.
    PageColumn<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    // Opaque and above the body, which scrolls beneath it.
    PageHeaderBlock<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            position: Position::Relative,
            z_index: 10,
            background: t.color.surface(),
        }
    }
}

stylesheet! {
    // The toolbar row: in the page gutter, `sm` above and below at
    // every width. A compact band, charged to every screenful.
    PageToolbarSeat<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            padding_left: LayoutTheme::tokens().gutter.phone(),
            padding_right: LayoutTheme::tokens().gutter.phone(),
            padding_top: t.spacing.sm(),
            padding_bottom: t.spacing.sm(),
        }
        breakpoint sm(t) {
            padding_left: LayoutTheme::tokens().gutter.tablet(),
            padding_right: LayoutTheme::tokens().gutter.tablet(),
        }
        breakpoint lg(t) {
            padding_left: LayoutTheme::tokens().gutter.desktop(),
            padding_right: LayoutTheme::tokens().gutter.desktop(),
        }
    }
}

stylesheet! {
    // Everything below the header on a bounded page. `min_height: 0`
    // against `flex_grow: 1` is the whole mechanism: it takes the height
    // the header leaves rather than its content's, which is what lets
    // the scroller inside it window anything.
    PageBounded<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // The page scroller. A fixed page never scrolls here, so its bar is
    // hidden: otherwise the inner scroller reserves a bar track and this
    // one does too, and the body ends a bar's width short of the edge.
    PageScroll<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
        }
        variant mode {
            #[default]
            scrolling(_t) {}
            fixed(_t) {
                scrollbar: ScrollbarVisibility::Hidden,
            }
        }
    }
}

stylesheet! {
    // The scroller's one child. A scrolling page grows past the
    // scrollport; a fixed one is clamped to it, so its own region owns
    // the scrolling.
    PageScrollBody<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
        }
        variant mode {
            #[default]
            scrolling(_t) {
                flex_shrink: 0.0,
            }
            fixed(_t) {
                flex_shrink: 1.0,
            }
        }
    }
}

stylesheet! {
    // The page column inside the scroller, carrying the bottom inset.
    PageFill<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
        }
        variant mode {
            #[default]
            scrolling(_t) {
                flex_shrink: 0.0,
            }
            fixed(_t) {
                flex_shrink: 1.0,
            }
        }
    }
}

stylesheet! {
    // The page gutter (rule `gutter`): `lg` on a phone, `xl` from `sm`,
    // `xxl` from `lg`, the same steps the toolbar seat and a pinned
    // action bar take, so a body's first text stands under the title.
    //
    // Variant arms sort after the breakpoint arms, so `table`, `blank`
    // and `bleed` win at every width.
    PageGutter<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
            min_width: Length::Px(0.0),
            padding_top: t.spacing.sm(),
            padding_left: LayoutTheme::tokens().gutter.phone(),
            padding_right: LayoutTheme::tokens().gutter.phone(),
            padding_bottom: t.spacing.lg(),
        }
        breakpoint sm(t) {
            padding_top: t.spacing.lg(),
            padding_left: LayoutTheme::tokens().gutter.tablet(),
            padding_right: LayoutTheme::tokens().gutter.tablet(),
            padding_bottom: t.spacing.xl(),
        }
        breakpoint lg(t) {
            padding_left: LayoutTheme::tokens().gutter.desktop(),
            padding_right: LayoutTheme::tokens().gutter.desktop(),
        }
        variant kind {
            #[default]
            content(_t) {}
            table(_t) {
                padding_top: Length::Px(0.0),
                padding_bottom: Length::Px(0.0),
            }
            blank(_t) {
                padding_top: Length::Px(0.0),
                padding_left: Length::Px(0.0),
                padding_right: Length::Px(0.0),
                padding_bottom: Length::Px(0.0),
            }
        }
        variant edge {
            #[default]
            inset(_t) {}
            bleed(_t) {
                padding_top: Length::Px(0.0),
                padding_left: Length::Px(0.0),
                padding_right: Length::Px(0.0),
                padding_bottom: Length::Px(0.0),
            }
        }
    }
}

stylesheet! {
    // The box a shell mounts its navigator's outlet in. The outlet's own
    // node is `flex: 1 1 0`. In a column that zero basis is its HEIGHT,
    // so on a scrolling page the outlet, and the page with it, would
    // come out the scrollport's height with the content overflowing it.
    // So a scrolling outlet sits in a ROW: there the zero basis is a
    // width, and the height is the content's. A fixed page keeps the
    // column that bounds it.
    PageOutlet<idea_ui::IdeaThemeRef> {
        base(_t) {
            width: Length::pct(100.0),
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
            min_width: Length::Px(0.0),
        }
        variant mode {
            #[default]
            scrolling(_t) {
                flex_direction: FlexDirection::Row,
            }
            fixed(_t) {
                flex_direction: FlexDirection::Column,
                height: Length::pct(100.0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_content_page_scrolls() {
        assert!(PageVariant::Content.scrolls());
        assert!(!PageVariant::Table.scrolls());
        assert!(!PageVariant::Blank.scrolls());
    }
}
