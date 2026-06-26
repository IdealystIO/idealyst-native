//! `Grid` — an N-column layout. Children flow left-to-right, top-to-
//! bottom, each column sharing the row width equally.
//!
//! ```ignore
//! ui! {
//!     Grid(columns = 3, gap = StackGap::Md) {
//!         Card { /* … */ }
//!         Card { /* … */ }
//!         Card { /* … */ }
//!         Card { /* … */ }  // wraps to row 2
//!     }
//! }
//! ```
//!
//! Built from flex (the framework has no CSS grid): children are
//! chunked into rows of `columns`, and each cell flexes equally
//! (`flex-grow: 1; flex-basis: 0`). A partial final row is padded with
//! empty filler cells so its real cells keep the same `1/columns` width
//! as full rows and stay LEFT-aligned under the columns above (rather
//! than flex-growing to fill the row width).

use runtime_core::{
    component, ChildList, IdealystSchema, IntoElement, Element, Reactive, StyleApplication,
    VariantEnum,
};

use crate::components::stack::StackGap;
use crate::stylesheets::{GridCell, GridContainer, GridRow};

// Reactive-by-default: `#[props]` wraps `columns`/`gap` → `Reactive<…>`. `gap`
// routes into the GridRow/GridContainer style sinks reading `.get()` live (the
// closures already return `StyleApplication`). `columns` is STRUCTURAL — it
// controls how children are chunked into rows, which a style sink can't express
// — so it's snapshotted at build with a flagged TODO. `children` is the
// children category and stays bare.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct GridProps {
    /// Number of columns (>= 1). Default 2.
    #[schema(constraint = ">= 1 (clamped)")]
    pub columns: u32,
    /// Gap between rows and between columns. Default Md.
    pub gap: StackGap,
    /// Cells, laid out left-to-right then wrapping to the next row.
    pub children: Vec<Element>,
}

impl Default for GridProps {
    fn default() -> Self {
        Self {
            columns: Reactive::Static(2),
            gap: Reactive::Static(StackGap::default()),
            children: Vec::new(),
        }
    }
}

/// Lays out `children` in `columns` equal-width columns, wrapping into
/// rows, with `gap` spacing between rows and columns.
#[component(children)]
pub fn Grid(props: GridProps) -> Element {
    // TODO(reactive-sweep): route `columns` to the row-chunking structure. A
    // live `columns` changes how many cells fill each row (and the partial-row
    // padding) — that's a tree-shape change, not a style sink, so it can't ride
    // a style closure. It needs the body wrapped in a `switch`/keyed rebuild on
    // `columns.get()`. For now `columns` is snapshotted at build (a live source
    // sets the initial column count but won't re-chunk on change).
    let cols = props.columns.get().max(1) as usize;

    let mut flat: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut flat);
    }

    // One equal-flex cell wrapping a child (or empty, for row padding).
    let cell = |child: Vec<Element>| {
        runtime_core::view(child)
            .with_style(|| StyleApplication::new(GridCell::sheet()))
            .into_element()
    };

    // Chunk into rows of `cols`; wrap each child in an equal-flex cell.
    let mut rows: Vec<Element> = Vec::new();
    let mut iter = flat.into_iter();
    loop {
        let chunk: Vec<Element> = iter.by_ref().take(cols).collect();
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();
        let mut cells: Vec<Element> = chunk.into_iter().map(|c| cell(vec![c])).collect();
        // Pad a partial final row with empty cells so the real cells keep
        // their 1/cols width and stay left-aligned instead of stretching.
        for _ in n..cols {
            cells.push(cell(vec![]));
        }
        // `gap` routes into the GridRow style sink: read `.get()` INSIDE the
        // closure so a live `gap` re-resolves the row spacing in place (a
        // static one collapses to one build-time resolution).
        let row_gap = props.gap.clone();
        let row = runtime_core::view(cells)
            .with_style(move || {
                StyleApplication::new(GridRow::sheet())
                    .with("gap", row_gap.get().as_variant_str().to_string())
            })
            .into_element();
        rows.push(row);
    }

    // Same sink for the container's between-rows gap.
    let container_gap = props.gap.clone();
    runtime_core::view(rows)
        .with_style(move || {
            StyleApplication::new(GridContainer::sheet())
                .with("gap", container_gap.get().as_variant_str().to_string())
        })
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{view, Element};

    fn view_children(el: &Element) -> &Vec<Element> {
        match el {
            Element::View { children, .. } => children,
            _ => panic!("expected a View"),
        }
    }

    fn leaf() -> Element {
        view(vec![]).into_element()
    }

    // Regression: a partial final row must be PADDED to `columns` cells so
    // its real cells keep their 1/columns width and stay left-aligned under
    // the columns above — not flex-grow to fill the row (the icons-page
    // "bottom row spreads out" report).
    #[test]
    fn partial_final_row_is_padded_for_left_alignment() {
        // 4 cells across 3 columns → rows of [3, 1]; the partial row is
        // padded up to 3 cells (1 real + 2 empty fillers).
        let props = GridProps {
            columns: Reactive::Static(3),
            gap: Reactive::Static(StackGap::Md),
            children: vec![leaf(), leaf(), leaf(), leaf()],
        };
        let grid = Grid(props);
        let rows = view_children(&grid);
        assert_eq!(rows.len(), 2, "4 cells in 3 columns make 2 rows");
        assert_eq!(view_children(&rows[0]).len(), 3, "first row is full");
        assert_eq!(
            view_children(&rows[1]).len(),
            3,
            "the partial final row is padded to `columns` cells so real cells left-align"
        );
    }
}
