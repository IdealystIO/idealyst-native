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
//! (`flex-grow: 1; flex-basis: 0`). A partial final row keeps the same
//! cell width as full rows is *not* guaranteed — its cells stretch to
//! fill — which is the conventional flex-grid behavior.

use runtime_core::{
    component, ChildList, IdealystSchema, IntoElement, Element, StyleApplication, VariantEnum,
};

use crate::components::stack::StackGap;
use crate::stylesheets::{GridCell, GridContainer, GridRow};

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
        Self { columns: 2, gap: StackGap::default(), children: Vec::new() }
    }
}

/// Lays out `children` in `columns` equal-width columns, wrapping into
/// rows, with `gap` spacing between rows and columns.
#[component(children)]
pub fn Grid(props: GridProps) -> Element {
    let cols = props.columns.max(1) as usize;
    let gap_key = props.gap.as_variant_str().to_string();

    let mut flat: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut flat);
    }

    // Chunk into rows of `cols`; wrap each child in an equal-flex cell.
    let mut rows: Vec<Element> = Vec::new();
    let mut iter = flat.into_iter();
    loop {
        let chunk: Vec<Element> = iter.by_ref().take(cols).collect();
        if chunk.is_empty() {
            break;
        }
        let cells: Vec<Element> = chunk
            .into_iter()
            .map(|child| {
                runtime_core::view(vec![child])
                    .with_style(|| StyleApplication::new(GridCell::sheet()))
                    .into_element()
            })
            .collect();
        let row_gap = gap_key.clone();
        let row = runtime_core::view(cells)
            .with_style(move || StyleApplication::new(GridRow::sheet()).with("gap", row_gap.clone()))
            .into_element();
        rows.push(row);
    }

    let container_gap = gap_key;
    runtime_core::view(rows)
        .with_style(move || {
            StyleApplication::new(GridContainer::sheet()).with("gap", container_gap.clone())
        })
        .into_element()
}
