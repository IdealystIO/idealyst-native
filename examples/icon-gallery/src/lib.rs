//! `icon-gallery` — a searchable, virtualized grid of the entire Lucide
//! icon pack.
//!
//! Two things this demo shows off:
//!
//!   * **The full `icons-lucide` set.** With the crate's `registry`
//!     feature on, `icons_lucide::ALL` is a `&[IconEntry]` over every icon
//!     (`ICON_COUNT` of them). Normal apps `use icons_lucide::SEARCH` and
//!     tree-shake; a gallery legitimately wants all of them, so it opts in.
//!   * **Virtualized `responsive_grid`.** ~2000 icons would be far too
//!     many native views to mount at once. We hand the flat (filtered)
//!     icon list to `virtualized::responsive_grid`, which lanes it into as
//!     many columns as the viewport fits and only realizes the cells in
//!     view. Type-and-filter and the grid rebuilds reactively; resize the
//!     window and the column count re-lanes itself.
//!
//! Layout is a fixed root column: a non-scrolling header (title + live
//! count + search `Field`) above a `responsive_grid` that fills the rest.

use std::rc::Rc;

use icons_lucide::{ALL, ICON_COUNT};
use idea_ui::{install_idea_theme, light_theme, typography_kind, Field, Icon, Typography};
use runtime_core::{
    component, effect, rx, signal, ui, AlignItems, Color, Element, FlexDirection, IntoElement,
    JustifyContent, Length, Signal, StyleRules, StyleSheet, Tokenized,
};
use virtualized::{fixed_size, responsive_grid};

/// Minimum width per cell, in px. `responsive_grid` packs as many lanes
/// as fit at this minimum — the viewport, not a hardcoded column count,
/// decides how many columns show (≈4 on a phone, many more on desktop).
const MIN_CELL_W: f32 = 96.0;
/// Cell height in px. Generous enough for a 30px glyph plus a two-line
/// wrapped name; the grid needs a known main-axis size to virtualize.
const CELL_H: f32 = 104.0;
/// Rendered glyph size in px.
const ICON_PX: f32 = 30.0;

// SDK-registration hook the CLI-generated wrappers call before mount. No
// third-party SDKs here, so it's an empty generic over `Backend`.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Gated by
// `sidecar` so device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

/// One grid cell: an icon + its name, plus a stable `key` (the icon's
/// index in `ALL`) so its mounted subtree survives across search-filter
/// changes. `IconData` is `Copy`, so the whole struct is `Copy`.
#[derive(Clone, Copy)]
struct Cell {
    key: u64,
    name: &'static str,
    data: runtime_core::IconData,
}

/// Filter `ALL` by a case-insensitive name substring into a flat cell
/// list — one cell per icon. The grid lanes them into columns; no manual
/// row-chunking or filler padding (the old fixed-`COLS` approach). An
/// empty query yields the whole set.
fn build_cells(query: &str) -> Vec<Cell> {
    let q = query.trim().to_lowercase();
    ALL.iter()
        .enumerate()
        .filter(|(_, e)| q.is_empty() || e.name.contains(&q))
        .map(|(i, e)| Cell { key: i as u64, name: e.name, data: e.data })
        .collect()
}

/// Count matches without building rows — for the header's live tally.
fn match_count(query: &str) -> usize {
    let q = query.trim().to_lowercase();
    ALL.iter()
        .filter(|e| q.is_empty() || e.name.contains(&q))
        .count()
}

fn sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

/// Viewport-filling root column. Native backends size the mounted root to
/// the window only if the root declares it (a bare flex column renders
/// blank on iOS/Android otherwise).
fn root_fill() -> Rc<StyleSheet> {
    sheet(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        flex_direction: Some(FlexDirection::Column),
        background: Some(Tokenized::Literal(Color("#ffffff".into()))),
        ..Default::default()
    })
}

fn header_style() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(Length::Px(10.0).into()),
        padding_top: Some(Length::Px(20.0).into()),
        padding_left: Some(Length::Px(20.0).into()),
        padding_right: Some(Length::Px(20.0).into()),
        padding_bottom: Some(Length::Px(12.0).into()),
        ..Default::default()
    })
}

/// Applied directly to the grid so its scroll container is *bounded* to
/// the height left over below the header, instead of growing to its full
/// content height and overflowing the viewport-height root. `min_height: 0`
/// is the crucial bit: a flex item defaults to `min-height: auto`, which
/// refuses to shrink below its content — so without it the list expands to
/// ~50,000px and never scrolls internally. This is a correct cross-platform
/// rule (Taffy applies the same flexbox automatic-minimum-size), not a
/// web-only patch.
fn list_style() -> Rc<StyleSheet> {
    sheet(StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        flex_basis: Some(Length::Px(0.0).into()),
        min_height: Some(Length::Px(0.0).into()),
        width: Some(Length::pct(100.0).into()),
        ..Default::default()
    })
}

/// One grid cell's interior. The grid sizes the cell box (lane width ×
/// `CELL_H`); this just centers the glyph + caption inside it.
fn cell_style() -> Rc<StyleSheet> {
    sheet(StyleRules {
        height: Some(Length::pct(100.0).into()),
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

/// Render one icon cell. Plain Rust, called from the grid's render closure.
fn render_cell(c: &Cell) -> Element {
    ui! {
        view(style = cell_style()) {
            Icon(data = c.data, size = ICON_PX)
            Typography(
                content = c.name.to_string(),
                kind = typography_kind::Caption,
                muted = true,
            )
        }
    }
}

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let query: Signal<String> = signal!(String::new());
    // Source of truth for the virtualized grid. Seeded with the full set so
    // the first paint shows icons; the effect below keeps it in sync.
    let cells: Signal<Vec<Cell>> = signal!(build_cells(""));

    // Rebuild the cell list whenever the search text changes. `query.get()`
    // makes this effect re-run on every edit; `cells` isn't read here.
    effect!({
        let q = query.get();
        cells.set(build_cells(&q));
    });

    let on_query: Rc<dyn Fn(String)> = Rc::new(move |s| query.set(s));

    // A responsive grid: one cell per icon, lanes derived from the viewport
    // width (≈4 columns on a phone, many more on desktop). Built directly
    // (not via `ui!`) so we can attach the bounding `list_style()` to the
    // virtualizer element itself — the web backend forwards it onto the
    // scroll container.
    let grid = responsive_grid(
        cells,
        |_idx, c: &Cell| c.key,
        fixed_size(CELL_H),
        |_idx, c: &Cell| render_cell(c),
        MIN_CELL_W,
    )
    .gap(8.0)
    .into_element()
    .with_style(list_style());

    ui! {
        view(style = root_fill()) {
            view(style = header_style()) {
                Typography(content = "Lucide Icons".to_string(), kind = typography_kind::H1)
                Typography(
                    content = rx!(format!("{} of {} icons", match_count(&query.get()), ICON_COUNT)),
                    kind = typography_kind::Caption,
                    muted = true,
                )
                Field(
                    value = query,
                    on_change = on_query,
                    placeholder = Some("Search icons by name…".to_string()),
                )
            }
            grid
        }
    }
}
