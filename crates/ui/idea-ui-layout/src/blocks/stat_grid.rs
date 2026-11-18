//! [`StatGrid`] — figures four across when wide, two across on a phone
//! (rule `stat-grid`).

use runtime_core::{component, stylesheet, ui, ChildList, Element, FlexDirection, FlexWrap, IdealystSchema, Length};

/// A tile's share of the row on a phone: two fit and a third does not.
const TILE_BASIS_PHONE_PCT: f32 = 40.0;
/// A tile's share of the row from `Sm`: four fit and a fifth does not.
const TILE_BASIS_WIDE_PCT: f32 = 20.0;

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
pub struct StatGridProps {
    /// The stat tiles. More than eight is a table, not a grid.
    pub children: Vec<Element>,
}

/// Stat tiles four across from `Sm` up and two across on a phone. Fewer
/// than a row's worth share the row evenly; a fifth wraps to a second
/// row.
///
/// ```ignore
/// StatGrid {
///     Card { Typography(content = "Hours") Typography(content = hours, kind = typography_kind::H2) }
///     Card { Typography(content = "Cost") Typography(content = cost, kind = typography_kind::H2) }
/// }
/// ```
#[component(children)]
pub fn StatGrid(props: StatGridProps) -> Element {
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut tiles = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut tiles);
    }
    ui! {
        view(style = StatGridRow()) {
            // A static list built once; a key would reconcile nothing.
            // idealyst-lint-disable-next-line prefer-keyed-list
            for tile in tiles {
                view(style = StatGridCell()) { tile }
            }
        }
    }
}

stylesheet! {
    StatGridRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: t.spacing.md(),
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    // The basis decides how many share a row and the growth fills it,
    // which is what makes three tiles share a row rather than leave a
    // hole.
    StatGridCell<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: Length::pct(TILE_BASIS_PHONE_PCT),
            min_width: Length::Px(0.0),
        }
        breakpoint sm(_t) {
            flex_basis: Length::pct(TILE_BASIS_WIDE_PCT),
        }
    }
}
