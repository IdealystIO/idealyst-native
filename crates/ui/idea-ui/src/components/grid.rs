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
//! It's a real CSS grid: the container sets `display: grid` with
//! `columns` equal `1fr` tracks, and children flow into the tracks,
//! wrapping to implicit rows. Because children are placed by the layout
//! engine (not chunked into row-views at build time), a **reactive
//! list** works — a `for item in signal { … }` lowers to one
//! `Element::Each` that every backend splices as direct grid items, so
//! the rows land in successive cells instead of piling into one. A live
//! `columns` also re-resolves the tracks in place. This is a *layout*
//! grid (all children mount); for very large data sets that need
//! recycling, use `virtualized::grid`.

use runtime_core::{
    component, IdealystSchema, IntoElement, DisplayKind, Element, Reactive, StyleApplication,
    StyleRules, TrackSize, VariantEnum,
};

use crate::components::stack::StackGap;
use crate::stylesheets::GridContainer;

// Reactive-by-default: `#[props]` wraps `columns`/`gap` → `Reactive<…>`. Both
// route into the GridContainer style sink reading `.get()` live: `gap` picks the
// spacing variant, `columns` re-resolves the grid track list (a computed layer),
// so a live column count re-lays-out in place — no rebuild. `children` is the
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
    // A single `display: grid` container. Children — static siblings AND a
    // reactive `for`'s spliced rows — are placed by the layout engine into the
    // track list, so nothing is chunked at build time. `gap` and `columns` both
    // read `.get()` INSIDE the style closure so either changing re-resolves the
    // grid in place (gap variant + the computed track list keyed by count).
    let cols = props.columns.clone();
    let gap = props.gap.clone();
    runtime_core::view(props.children)
        .with_style(move || {
            let n = cols.get().max(1) as usize;
            StyleApplication::new(GridContainer::sheet())
                .with("gap", gap.get().as_variant_str().to_string())
                // `grid_template_columns` is a `Vec` (not a string-keyed variant
                // value), so it rides a computed layer keyed by the count — the
                // key changes with `n`, re-resolving the tracks on a live change.
                .with_computed(format!("grid-cols-{n}"), move || StyleRules {
                    display: Some(DisplayKind::Grid),
                    grid_template_columns: Some(vec![TrackSize::Fr(1.0); n]),
                    ..Default::default()
                })
        })
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, view, Element, StyleRules as RCStyleRules, StyleSource};

    fn view_children(el: &Element) -> &Vec<Element> {
        match el {
            Element::View { children, .. } => children,
            _ => panic!("expected a View"),
        }
    }

    fn leaf() -> Element {
        view(vec![]).into_element()
    }

    /// Resolve the grid container's (reactive) style to its `StyleRules`.
    fn container_rules(el: &Element) -> std::rc::Rc<RCStyleRules> {
        let style = match el {
            Element::View { style, .. } => style.as_ref().expect("Grid attaches a style"),
            _ => panic!("Grid renders a View"),
        };
        let app = match style {
            StyleSource::Reactive(f) => f(),
            StyleSource::Static(a) => a.clone(),
            _ => panic!("Grid uses a reactive style closure"),
        };
        resolve_style(&app)
    }

    // The Grid is a real CSS grid: `display: grid` with one `1fr` track per
    // column. This is what lets the layout engine place children into cells
    // (the previous build-time flex chunking couldn't, which broke reactive
    // lists — see the module docs).
    #[test]
    fn grid_is_a_css_grid_with_one_fr_track_per_column() {
        install_idea_theme(light_theme());
        let grid = Grid(GridProps {
            columns: Reactive::Static(3),
            gap: Reactive::Static(StackGap::Md),
            children: vec![leaf(), leaf(), leaf(), leaf()],
        });
        let rules = container_rules(&grid);
        assert_eq!(rules.display, Some(DisplayKind::Grid), "container is display:grid");
        assert_eq!(
            rules.grid_template_columns.as_deref(),
            Some([TrackSize::Fr(1.0), TrackSize::Fr(1.0), TrackSize::Fr(1.0)].as_slice()),
            "3 columns → three equal 1fr tracks",
        );
    }

    // Children pass through as DIRECT grid items — no per-cell wrapper views and
    // no row grouping. This is precisely why a reactive `for` (one Element::Each)
    // works: every backend splices its rows directly into this container, so
    // they become successive grid items instead of piling into a single wrapped
    // cell.
    #[test]
    fn children_are_direct_grid_items_not_wrapped() {
        install_idea_theme(light_theme());
        let grid = Grid(GridProps {
            columns: Reactive::Static(2),
            gap: Reactive::Static(StackGap::Md),
            children: vec![leaf(), leaf(), leaf()],
        });
        // 3 children in → 3 direct children out (no padding fillers, no row
        // views, no cell wrappers).
        assert_eq!(view_children(&grid).len(), 3, "children are the grid items themselves");
    }

    // `columns` drives the track count directly.
    #[test]
    fn columns_prop_sets_track_count() {
        install_idea_theme(light_theme());
        let grid = Grid(GridProps {
            columns: Reactive::Static(5),
            gap: Reactive::Static(StackGap::Sm),
            children: vec![leaf()],
        });
        assert_eq!(
            container_rules(&grid).grid_template_columns.as_ref().map(|t| t.len()),
            Some(5),
            "5 columns → five tracks",
        );

        // Clamps to at least one track.
        let zero = Grid(GridProps {
            columns: Reactive::Static(0),
            gap: Reactive::Static(StackGap::Sm),
            children: vec![],
        });
        assert_eq!(
            container_rules(&zero).grid_template_columns.as_ref().map(|t| t.len()),
            Some(1),
            "columns is clamped to >= 1",
        );
    }
}
