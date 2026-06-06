//! `icon-gallery` — a searchable, virtualized grid of the entire Lucide
//! icon pack.
//!
//! Two things this demo shows off:
//!
//!   * **The full `icons-lucide` set.** With the crate's `registry`
//!     feature on, `icons_lucide::ALL` is a `&[IconEntry]` over every icon
//!     (`ICON_COUNT` of them). Normal apps `use icons_lucide::SEARCH` and
//!     tree-shake; a gallery legitimately wants all of them, so it opts in.
//!   * **`flat_list` virtualization.** ~2000 icons would be far too many
//!     native views to mount at once. We chunk the (filtered) icons into
//!     fixed-height rows and hand them to `flat_list`, which only realizes
//!     the rows in view. Type-and-filter and the list rebuilds reactively.
//!
//! Layout is a fixed root column: a non-scrolling header (title + live
//! count + search `Field`) above a `flat_list` that fills the rest.

use std::rc::Rc;

use icons_lucide::{ALL, ICON_COUNT};
use idea_ui::{install_idea_theme, light_theme, typography_kind, Field, Icon, Typography};
use runtime_core::{
    component, effect, fixed_size, flat_list, rx, signal, ui, AlignItems, Color, Element,
    FlexDirection, IntoElement, JustifyContent, Length, Signal, StyleRules, StyleSheet, Tokenized,
};

/// Columns per grid row. Fixed because the framework can't measure the
/// viewport here — 4 reads well on a phone and stays usable on desktop/web.
const COLS: usize = 4;
/// Row height in px. Generous enough for a 30px glyph plus a two-line
/// wrapped name; `flat_list` needs a known size to virtualize.
const ROW_H: f32 = 104.0;
/// Rendered glyph size in px.
const ICON_PX: f32 = 30.0;

// SDK-registration hook the CLI-generated wrappers call before mount. No
// third-party SDKs here, so it's an empty generic over `Backend`.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Gated by
// `sidecar` so device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

/// One grid cell. `IconData` is `Copy`, so this whole struct is `Copy` and
/// cheap to shuffle into rows.
#[derive(Clone, Copy)]
struct Cell {
    name: &'static str,
    data: runtime_core::IconData,
}

/// One virtualized row: a stable `key` plus up to `COLS` cells. `None`
/// cells pad the final short row so every column stays the same width.
#[derive(Clone)]
struct RowData {
    key: u64,
    cells: Vec<Option<Cell>>,
}

/// Filter `ALL` by a case-insensitive name substring and chunk the matches
/// into fixed-width rows. An empty query yields the whole set.
fn build_rows(query: &str) -> Vec<RowData> {
    let q = query.trim().to_lowercase();
    let mut rows = Vec::new();
    let mut cur: Vec<Option<Cell>> = Vec::with_capacity(COLS);
    let mut key = 0u64;
    for e in ALL.iter() {
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

/// Applied directly to the `flat_list` so its scroll container is *bounded*
/// to the height left over below the header, instead of growing to its full
/// content height and overflowing the viewport-height root (which would let
/// the page background show through the transparent rows). `min_height: 0`
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

/// Render one row of cells. Plain Rust (called from the `flat_list` render
/// closure), so the conditional cell/filler split lives inside `ui!` per
/// the macro's `if let` support.
fn render_row(row: &RowData) -> Element {
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

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let query: Signal<String> = signal!(String::new());
    // Source of truth for the virtualized list. Seeded with the full set so
    // the first paint shows icons; the effect below keeps it in sync.
    let rows: Signal<Vec<RowData>> = signal!(build_rows(""));

    // Rebuild the grid whenever the search text changes. `query.get()` makes
    // this effect re-run on every edit; `rows` isn't read here, so no loop.
    effect!({
        let q = query.get();
        rows.set(build_rows(&q));
    });

    let on_query: Rc<dyn Fn(String)> = Rc::new(move |s| query.set(s));

    // Build the virtualized list directly (not via `ui!`) so we can attach
    // the bounding `list_style()` to the virtualizer element itself — the
    // web backend forwards it onto the scroll container.
    let list = flat_list::<RowData, _, (), _>(
        rows,
        |_idx, r: &RowData| r.key,
        fixed_size(ROW_H),
        |_idx, r: &RowData| render_row(r),
    )
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
            list
        }
    }
}
