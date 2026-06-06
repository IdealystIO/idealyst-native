//! Icon gallery — the geometry bridge + virtualized row builder for the
//! `Kind::IconSet` detail page.
//!
//! The catalog carries icon NAMES only (catalog.json is names-only by
//! design). To draw the glyphs, the gallery joins a pack's `crate_name`
//! to a build-linked `&[IconEntry]` registry (name + `IconData`). Today
//! only `icons-lucide` is linked (via its `registry` feature); a new pack
//! adds one arm to [`registry`] plus the `registry` feature on its dep —
//! the same one-line-per-pack shape as `build.rs`'s force-link list. A
//! pack in the catalog whose crate this app doesn't link renders its
//! metadata with no grid (see `shell::icon_set_page`).
//!
//! The virtualization mirrors `examples/icon-gallery`: ~1600 icons are far
//! too many native views to mount at once, so the (filtered) icons are
//! chunked into fixed-height rows and handed to `flat_list`, which only
//! realizes the rows in view.

use std::rc::Rc;

use icons_lucide::IconEntry;
use idea_ui::{typography_kind, Icon, Typography};
use runtime_core::{
    ui, AlignItems, Element, FlexDirection, JustifyContent, Length, StyleRules, StyleSheet,
    Tokenized,
};

/// Columns per grid row. Fixed because the framework can't measure the
/// viewport here — 4 reads well on a phone and stays usable on desktop.
pub const COLS: usize = 4;
/// Row height in px — generous enough for a glyph plus a two-line wrapped
/// name; `flat_list` needs a known size to virtualize.
pub const ROW_H: f32 = 104.0;
/// Rendered glyph size in px.
pub const ICON_PX: f32 = 30.0;

/// The geometry registry for a pack, keyed by Cargo crate name. `None`
/// when the pack is in the catalog but its crate isn't linked into this
/// app (the page then shows metadata only).
pub fn registry(crate_name: &str) -> Option<&'static [IconEntry]> {
    match crate_name {
        "icons-lucide" => Some(icons_lucide::ALL),
        _ => None,
    }
}

/// One grid cell. `IconData` is `Copy`, so the whole struct is `Copy`.
#[derive(Clone, Copy)]
pub struct Cell {
    pub name: &'static str,
    pub data: runtime_core::IconData,
}

/// One virtualized row: a stable `key` plus up to `COLS` cells. `None`
/// cells pad the final short row so columns stay aligned.
#[derive(Clone)]
pub struct RowData {
    pub key: u64,
    pub cells: Vec<Option<Cell>>,
}

/// Filter a pack's registry by case-insensitive name substring and chunk
/// the matches into fixed-width rows. An empty query yields the whole set.
pub fn build_rows(set: &'static [IconEntry], query: &str) -> Vec<RowData> {
    let q = query.trim().to_lowercase();
    let mut rows = Vec::new();
    let mut cur: Vec<Option<Cell>> = Vec::with_capacity(COLS);
    let mut key = 0u64;
    for e in set.iter() {
        if !q.is_empty() && !e.name.contains(&q) {
            continue;
        }
        cur.push(Some(Cell { name: e.name, data: e.data }));
        if cur.len() == COLS {
            rows.push(RowData { key, cells: std::mem::take(&mut cur) });
            key += 1;
        }
    }
    if !cur.is_empty() {
        while cur.len() < COLS {
            cur.push(None);
        }
        rows.push(RowData { key, cells: cur });
    }
    rows
}

/// Count matches without building rows — for the header's live tally.
pub fn match_count(set: &'static [IconEntry], query: &str) -> usize {
    let q = query.trim().to_lowercase();
    set.iter().filter(|e| q.is_empty() || e.name.contains(&q)).count()
}

// ---- Grid styles + row renderer -------------------------------------------
//
// Layout-only (transparent rows over the themed `PageColumn` background).
// Mirrors `examples/icon-gallery`.

fn sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

/// The gallery's flex-grow column: fills the height `PageColumn` leaves
/// under the hamburger bar so the header sits at the top and the
/// `flat_list` (below) bounds to the remaining space. `min_height: 0` lets
/// it shrink so the inner list can scroll instead of overflowing.
pub fn gallery_col() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        flex_grow: Some(Tokenized::Literal(1.0)),
        flex_basis: Some(Length::Px(0.0).into()),
        min_height: Some(Length::Px(0.0).into()),
        width: Some(Length::pct(100.0).into()),
        ..Default::default()
    })
}

/// Bounds the `flat_list` to the height left under the header instead of
/// growing to full content height. `min_height: 0` is the crucial bit: a
/// flex item defaults to `min-height: auto` and refuses to shrink below
/// its content, so without it the list expands past the viewport and never
/// scrolls internally (correct cross-platform flexbox, not a web patch).
pub fn list_style() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        flex_basis: Some(Length::Px(0.0).into()),
        min_height: Some(Length::Px(0.0).into()),
        width: Some(Length::pct(100.0).into()),
        ..Default::default()
    })
}

fn row_style() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Stretch),
        gap: Some(Length::Px(8.0).into()),
        padding_left: Some(Length::Px(16.0).into()),
        padding_right: Some(Length::Px(16.0).into()),
        height: Some(Length::Px(ROW_H).into()),
        ..Default::default()
    })
}

fn cell_style() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        flex_basis: Some(Length::Px(0.0).into()),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: Some(Length::Px(8.0).into()),
        padding_top: Some(Length::Px(10.0).into()),
        padding_bottom: Some(Length::Px(10.0).into()),
        padding_left: Some(Length::Px(4.0).into()),
        padding_right: Some(Length::Px(4.0).into()),
        ..Default::default()
    })
}

/// Empty spacer holding a column open in the final short row.
fn cell_filler() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        flex_basis: Some(Length::Px(0.0).into()),
        ..Default::default()
    })
}

/// Render one row of cells — the `flat_list` per-row render closure.
pub fn render_row(row: &RowData) -> Element {
    let cells = row.cells.clone();
    ui! {
        view(style = row_style()) {
            for cell in cells {
                if let Some(c) = cell {
                    view(style = cell_style()) {
                        Icon(data = c.data, size = ICON_PX)
                        Typography(
                            content = c.name.to_string(),
                            kind = typography_kind::Caption,
                            muted = true,
                        )
                    }
                } else {
                    view(style = cell_filler()) {}
                }
            }
        }
    }
}
