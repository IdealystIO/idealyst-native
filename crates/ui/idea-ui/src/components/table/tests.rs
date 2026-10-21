//! Native (non-web) lowering tests for the table components. On native a
//! `TableRow` lowers to a marker of its cells (see the `table` SDK), so a
//! cell's wiring reads straight off the built tree without a backend.
//! Introspection goes through `test_support::classify`.

use super::*;
use std::collections::BTreeSet;
use runtime_core::Cursor;
use crate::test_support::{classify, P};
use idea_theme::testing::with_test_world;

fn body_cell(text: &str) -> Element {
    TableCell(TableCellProps {
        text: Reactive::Static(Some(text.into())),
        ..Default::default()
    })
}

runtime_tls::thread_local! {
    /// Peeled row scopes, retained for the test's lifetime — a
    /// clickable row's marker arrives `Owned`-wrapped around the
    /// shared hover signal, and dropping the scope here would
    /// stale-handle the signal the cells' reactive styles read
    /// (same rationale as `test_support::classify`'s keepalive).
    static ROW_SCOPES: std::cell::RefCell<Vec<runtime_core::Owned>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn row_cells(mut row: Element) -> Vec<Element> {
    // Rows lower to the SDK's `TableRowPrim` marker item (the row
    // proxy work); its children are the cells.
    loop {
        match row {
            Element::Owned { element, owned, .. } => {
                ROW_SCOPES.with(|k| k.borrow_mut().push(owned));
                row = *element;
            }
            Element::Item { children, .. } => return children,
            _ => panic!("TableRow must lower to the SDK row marker item"),
        }
    }
}

/// With `on_row_click` set, every cell in the row becomes interactive:
/// it carries the tap handler (`on_touch`) and the shared hover handler
/// (`on_hover`), and its style is upgraded to a reactive source so the
/// whole-row hover highlight can re-apply. This is the whole feature —
/// if a refactor drops any of the three, the row stops being clickable
/// or stops highlighting. (The wiring goes through the table SDK's
/// `set_cell_style`/`set_cell_interaction` helpers — this test is what
/// fails if that seam regresses.)
/// Regression: a `--premint` build must not premint a table cell's
/// style.
///
/// A clickable row layers the pointer cursor + hover highlight over
/// each cell by re-deriving the cell's `StyleApplication` from the
/// built element. A preminted cell is an opaque class string with no
/// application behind it, so the derivation returns `None` and
/// `make_row_cell_interactive` silently skips the overlay — the row
/// stays clickable but loses its cursor and its highlight, with
/// nothing logged.
///
/// That shipped: `.into_style_application()` was written to keep cells
/// off the premint path, then `IntoStyleProp for StyleApplication`
/// gained a preminted fast path and preminted the application anyway.
/// Caught by a computed-style A/B of the catalog against a live build
/// (54 differing `cursor` properties across the table pages), not by a
/// test — which is why there are now two.
///
/// This half asserts the SEAM: the cell's style must be composable.
/// Under the default (non-premint) cfg it passes either way, so
/// `premint_must_not_reach_table_cell_styles` guards the actual
/// spelling.
#[test]
fn regression_premint_keeps_table_cells_on_the_live_engine() {
    with_test_world(|| {
        let cell = body_cell("x");
        assert!(
            table::map_cell_style(&cell, Rc::new(|app| app)),
            "a clickable row composes the pointer cursor + hover \
             highlight onto the cell's own style; a cell with no \
             application behind it is skipped silently"
        );
    });
}

/// The source-level half of the guard above.
///
/// Whether a style preminted is decided by a `--cfg` this test binary
/// is not built with, so no assertion on a value can observe the
/// regression here (same limitation `premint_only_surface.rs`
/// documents). What IS observable is the spelling: cells must hand
/// over an explicit `StyleProp::Sheet`, which no `IntoStyleProp` fast
/// path can reinterpret. A bare application can, and did.
#[test]
fn premint_must_not_reach_table_cell_styles() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/components/table/mod.rs"),
    )
    .expect("read table/mod.rs");
    // The application is built explicitly per role (so the `pinned`
    // axis can select on it)…
    for slot in ["head_cell", "body_cell"] {
        let needle = format!("let app = TableStyles::{slot}()");
        assert!(
            src.contains(&needle),
            "the {slot} cell application must be derived explicitly \
             before hand-off"
        );
    }
    // …and handed over as an explicit `StyleProp::Sheet` — a bare
    // application premints, and the clickable-row overlay is then
    // dropped without a word.
    assert!(
        src.contains("bound.with_style(StyleProp::Sheet(Box::new(app))).into_element()"),
        "cell styles must be handed over as an explicit `StyleProp::Sheet`"
    );
}

/// The clickable-row overlay must PREMINT: it selects the cell
/// sheet's `interactive`/`row_hovered` AXES, whose every arm has
/// build-time CSS, instead of layering runtime overrides — the
/// override spelling disqualified every clickable cell from
/// preminting (one of the last two `--premint-only` blockers on the
/// docs corpus). Fails against the override form: an overridden
/// application's `preminted_class_list()` is `None` by construction.
/// The live engine must resolve the same arms to the same rules the
/// overrides produced (pointer cursor; themed hover background).
#[test]
fn regression_clickable_row_overlay_premints_via_axes() {
    with_test_world(|| {
        let row = TableRow(TableRowProps {
            children: vec![body_cell("x")],
            on_row_click: Some(Rc::new(|| {})),
            ..Default::default()
        });
        let mut cells = row_cells(row);
        let style = match classify(cells.remove(0)) {
            P::View { style, .. } => style.expect("clickable cell keeps a style"),
            _ => panic!("native cell must classify as a View"),
        };
        // Evaluate the reactive style at its resting state (not hovered).
        let app = style.application();
        assert!(
            app.preminted_class_list().is_some(),
            "the axis-selected cell application must premint (overrides would return None)"
        );
        let resting = runtime_core::resolve_style(&app);
        assert_eq!(
            resting.cursor,
            Some(Cursor::Pointer),
            "interactive arm carries the pointer cursor"
        );
        // Resting STATES a background — transparent — rather than
        // saying nothing. That inversion is the fix for a stuck
        // highlight: an arm that sets no background produces a
        // re-resolve with no background in it, and a re-resolve that
        // mentions no background cannot undo one an earlier resolve
        // already applied to the view. The tint went on at press and
        // stayed on after release, on every backend that applies a
        // style by merging rather than replacing.
        //
        // Transparent, not a surface token: a body cell has no
        // background of its own, so this is the resting appearance
        // it always had — now spelled instead of implied.
        assert_eq!(
            resting.background.as_ref().map(|c| c.resolve().0),
            Some("transparent".to_string()),
            "resting (row_hovered=off) states a transparent background, so a \
             later resolve has something to overwrite the tint with"
        );

        // The hovered arm resolves to the themed row highlight.
        let hovered_app = TableStyles::body_cell()
            .into_style_application()
            .with("interactive", "on")
            .with("row_hovered", "on");
        assert!(hovered_app.preminted_class_list().is_some());
        let hovered = runtime_core::resolve_style(&hovered_app);
        // Asserted against the palette, not a literal: a restated hex
        // fallback can drift from the palette. Deriving the expectation
        // from `light_theme()` means the two can't disagree.
        let surface_alt = crate::light_theme().colors.surface_alt.value().0.to_ascii_lowercase();
        assert_eq!(
            hovered.background.as_ref().map(|b| b.resolve().0.to_ascii_lowercase()),
            Some(surface_alt),
            "row_hovered arm resolves the themed surface-alt highlight"
        );
    });
}

/// The header band reads its OWN token, not `color-surface-alt`, so
/// "retint table headers" doesn't drag every other `surface_alt`
/// consumer (cards, field wells, row hover) along. Asserting the
/// token NAME is the load-bearing half: a literal — or a
/// `color-surface-alt` reference — is exactly what an app-level
/// `color-table-header` override could never reach.
#[test]
fn head_cell_background_reads_the_table_header_token() {
    with_test_world(|| {
        let app = TableStyles::head_cell().into_style_application();
        let rules = runtime_core::resolve_style(&app);
        let bg = rules.background.as_ref().expect("head cells paint a background");
        assert_eq!(
            bg.name(),
            Some("color-table-header"),
            "the head-cell band must resolve through its own token"
        );

        // …and its default value is the `surface_alt` tint the band
        // had before the token existed, so adding the token is not
        // a visual change. Derived from the palette, never restated
        // as a literal (see `regression_clickable_row_premints…`).
        let colors = &crate::light_theme().colors;
        assert_eq!(
            colors.table_header.value().0.to_ascii_lowercase(),
            colors.surface_alt.value().0.to_ascii_lowercase(),
            "color-table-header ships as the surface-alt tint"
        );
        assert_eq!(
            bg.resolve().0.to_ascii_lowercase(),
            colors.table_header.value().0.to_ascii_lowercase(),
        );
    });
}

/// A cell styled REACTIVELY takes the row's hover the same as any
/// other. The app-side shape: an author who needs a width on a cell
/// (idea-ui's `TableCell` has none) drops to the SDK cell and hands it
/// a style CLOSURE, because the width has to move while a resize
/// handle is being dragged. A read-then-replace wiring that only sees a
/// static `StyleProp::Sheet` leaves every such column un-tinted in a
/// row whose other cells light up, with nothing logged. Composing means
/// the cell keeps its width and takes the axes.
#[test]
fn regression_reactive_cell_style_still_takes_the_row_hover() {
    with_test_world(|| {
        let width_pinned = table::table_cell(table::TableCellProps {
            header: false,
            children: Vec::new(),
            ..Default::default()
        })
        .with_style(|| {
            TableStyles::body_cell().into_style_application().with_overrides(StyleRules {
                width: Some(runtime_core::Tokenized::Literal(
                    runtime_core::Length::Px(240.0),
                )),
                ..Default::default()
            })
        })
        .into_element();

        let row = TableRow(TableRowProps {
            children: vec![width_pinned],
            on_row_click: Some(Rc::new(|| {})),
            ..Default::default()
        });
        let mut cells = row_cells(row);
        let style = match classify(cells.remove(0)) {
            P::View { style, .. } => style.expect("the cell keeps a style"),
            _ => panic!("native cell must classify as a View"),
        };
        let rules = runtime_core::resolve_style(&style.application());
        assert_eq!(
            rules.cursor,
            Some(Cursor::Pointer),
            "the interactive arm reaches a reactively-styled cell"
        );
        assert_eq!(
            rules.width,
            Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(240.0))),
            "and the cell's own width override survives the composition"
        );
    });
}

#[test]
fn clickable_row_makes_every_cell_interactive() {
    with_test_world(|| {
        let row = TableRow(TableRowProps {
            children: vec![body_cell("a"), body_cell("b")],
            on_row_click: Some(Rc::new(|| {})),
            ..Default::default()
        });
        let cells = row_cells(row);
        assert_eq!(cells.len(), 2, "both cells survive post-processing");
        for cell in cells {
            match classify(cell) {
                P::View {
                    on_hover,
                    on_touch,
                    style,
                    ..
                } => {
                    assert!(on_touch, "clickable cell carries a tap handler");
                    assert!(
                        on_hover,
                        "clickable cell reports hover into the shared row flag"
                    );
                    assert!(
                        style.expect("clickable cell keeps a style").is_reactive(),
                        "cell style is reactive so the row-hover highlight re-applies"
                    );
                }
                _ => panic!("native cell must classify as a View"),
            }
        }
    });
}

/// A pinned cell selects the sheet's `pinned` AXIS: `position:
/// Sticky` plus the matching zero inset, an opaque background
/// (content slides beneath a frozen column), and a preminting
/// application (the axis arms carry build-time CSS — a frozen
/// column must not knock the cell off the premint path). This is
/// the whole frozen-column feature at the component layer; the pin
/// itself is the sticky substrate's job.
#[test]
fn pinned_cell_selects_sticky_axis_and_premints() {
    with_test_world(|| {
        let left = TableCell(TableCellProps {
            text: Reactive::Static(Some("x".into())),
            pinned: Some(ColumnPin::Left),
            ..Default::default()
        });
        let app = match classify(left) {
            P::View { style, .. } => style
                .expect("pinned cell keeps an introspectable application")
                .application(),
            _ => panic!("native cell must classify as a View"),
        };
        assert!(
            app.preminted_class_list().is_some(),
            "the pinned axis must premint (build-time CSS arms)"
        );
        let rules = runtime_core::resolve_style(&app);
        assert_eq!(
            rules.position,
            Some(runtime_core::Position::Sticky),
            "pinned arm carries position: Sticky"
        );
        assert!(
            matches!(
                rules.left.as_ref().map(|t| t.resolve()),
                Some(runtime_core::Length::Px(v)) if v == 0.0
            ),
            "left-pinned arm pins at left: 0"
        );
        assert!(rules.right.is_none(), "left pin must not also set right");
        // `is_some()` is NOT enough, and that gap shipped: when
        // `row_hovered`'s resting arm began stating `transparent` it
        // won the merge (axes resolve in alphabetical axis order and
        // `"pinned" < "row_hovered"`), the resolve stayed `Some(...)`,
        // this assertion stayed green — and frozen columns went
        // see-through on a live device. Assert the VALUE.
        let bg = rules
            .background
            .as_ref()
            .map(|t| format!("{t:?}"))
            .expect("pinned body cell must state a background");
        assert!(
            !bg.contains("transparent"),
            "pinned body cell must be OPAQUE — content slides beneath a \
             frozen column — but resolved to `{bg}`"
        );

        let right = TableCell(TableCellProps {
            text: Reactive::Static(Some("x".into())),
            pinned: Some(ColumnPin::Right),
            ..Default::default()
        });
        let right_app = match classify(right) {
            P::View { style, .. } => style.expect("application").application(),
            _ => panic!("native cell must classify as a View"),
        };
        let rules = runtime_core::resolve_style(&right_app);
        assert_eq!(rules.position, Some(runtime_core::Position::Sticky));
        assert!(
            matches!(
                rules.right.as_ref().map(|t| t.resolve()),
                Some(runtime_core::Length::Px(v)) if v == 0.0
            ),
            "right-pinned arm pins at right: 0"
        );
        assert!(rules.left.is_none(), "right pin must not also set left");
    });
}

/// Regression: a plain `Table` — no `scroll_x`, no slot — rendered
/// square corners on web, because its sheet landed on the
/// collapsed-border `<table>`, where browsers ignore the radius (the
/// SDK wraps it; `table`'s
/// `regression_styled_plain_table_rounds_on_a_surface_not_the_collapsed_table`
/// pins that placement). The plain table's surface must carry the
/// radius AND the clip, or the header band paints square over the
/// corners the radius rounds.
#[test]
fn regression_plain_table_surface_rounds_and_clips() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![TableRow(TableRowProps {
                children: vec![body_cell("a")],
                ..Default::default()
            })],
            ..Default::default()
        });
        let vp = match peel_owned_keepalive(t) {
            Element::Item { data, .. } => data
                .downcast_ref::<runtime_vocabulary::prims::PrimCell<
                    runtime_vocabulary::prims::ViewPrim,
                >>()
                .expect("plain table root is the styled surface view")
                .take(),
            _ => panic!("plain table must lower to the surface view"),
        };
        let app = match vp.style.expect("surface carries the themed Table sheet") {
            runtime_vocabulary::StyleProp::Sheet(app) => *app,
            _ => panic!("Table sheet is a static application"),
        };
        let rules = runtime_core::resolve_style(&app);
        assert!(rules.border_top_left_radius.is_some(), "surface is rounded");
        assert_eq!(
            rules.overflow,
            Some(runtime_core::Overflow::Hidden),
            "plain table's surface must clip its cells to the rounded frame"
        );
    });
}

/// `Table(scroll_x = true)` lowers to the themed SURFACE wrapping
/// the SDK's horizontal scroller — the surface (border/radius/
/// background + overflow clip) must sit OUTSIDE the scroller so it
/// stays put while the columns scroll ("the card border scrolls
/// away and clips" regression), and without the scroller there is
/// nothing for a pinned column to pin against.
#[test]
fn scroll_x_table_lowers_to_surface_around_horizontal_scroller() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![TableRow(TableRowProps {
                children: vec![body_cell("a")],
                ..Default::default()
            })],
            scroll_x: true,
            ..Default::default()
        });
        let el = peel_owned_keepalive(t);
        // Root: the surface view carrying the themed Table sheet,
        // which must clip (rounded corners over scrolling columns).
        let mut surface_children = match el {
            Element::Item { data, children, .. } => {
                let vp = data
                    .downcast_ref::<runtime_vocabulary::prims::PrimCell<
                        runtime_vocabulary::prims::ViewPrim,
                    >>()
                    .expect("scroll_x root is the styled surface view")
                    .take();
                let style = vp.style.expect("surface carries the themed Table sheet");
                let app = match style {
                    runtime_vocabulary::StyleProp::Sheet(app) => *app,
                    _ => panic!("Table sheet is a static application"),
                };
                let rules = runtime_core::resolve_style(&app);
                assert_eq!(
                    rules.overflow,
                    Some(runtime_core::Overflow::Hidden),
                    "surface must clip the scrolling columns to its rounded frame"
                );
                assert!(rules.border_top_width.is_some(), "surface carries the border");
                children
            }
            _ => panic!("scroll_x table must lower to the surface view"),
        };
        match surface_children.pop().expect("surface wraps the scroller") {
            Element::Item { data, .. } => {
                assert!(
                    data.downcast_ref::<runtime_vocabulary::prims::PrimCell<
                        runtime_vocabulary::prims::ScrollViewPrim,
                    >>()
                    .is_some_and(|c| c.take().axis == runtime_core::ScrollAxis::Horizontal),
                    "surface must wrap a HORIZONTAL scroll_view"
                );
            }
            _ => panic!("surface must wrap the scroll_view item"),
        }
    });
}

/// Peel `Owned` wrappers, retaining the scopes (reactive styles
/// inside read signals those scopes own).
fn peel_owned_keepalive(mut el: Element) -> Element {
    loop {
        match el {
            Element::Owned { element, owned, .. } => {
                ROW_SCOPES.with(|k| k.borrow_mut().push(owned));
                el = *element;
            }
            other => return other,
        }
    }
}

/// A plain row (no `on_row_click`) leaves its cells untouched: no
/// handlers, and the static themed style is preserved. Guards against
/// accidentally making every table row interactive / reactive.
#[test]
fn static_row_leaves_cells_passive() {
    with_test_world(|| {
        let row = TableRow(TableRowProps {
            children: vec![body_cell("a")],
            on_row_click: None,
            ..Default::default()
        });
        let mut cells = row_cells(row);
        match classify(cells.remove(0)) {
            P::View {
                on_hover,
                on_touch,
                style,
                ..
            } => {
                assert!(!on_touch, "passive cell has no tap handler");
                assert!(!on_hover, "passive cell has no hover handler");
                assert!(
                    !style.expect("passive cell keeps its themed style").is_reactive(),
                    "passive cell keeps its static themed style (no per-node Effect)"
                );
            }
            _ => panic!("native cell must classify as a View"),
        }
    });
}

// =========================================================================
// CrewForge 10-09 table gaps — one regression per gap.
// =========================================================================

/// The cell's own node: a cell that sizes its column (`width`,
/// `min_width`, `max_width`, `flex`) arrives in the SDK's
/// `CellMarker`, which `table` unwraps; a test looking at one cell
/// outside a table looks through it (scopes kept alive).
fn cell_node(cell: Element) -> Element {
    match peel_owned_keepalive(cell) {
        Element::Item { data, mut children, .. }
            if data.downcast_ref::<runtime_vocabulary::prims::PrimCell<table::CellMarker>>().is_some() =>
        {
            children.pop().expect("a cell marker wraps the cell")
        }
        other => other,
    }
}

fn cell_app(cell: Element) -> runtime_core::StyleApplication {
    match classify(cell_node(cell)) {
        P::View { style, .. } => style.expect("cell keeps a style").application(),
        _ => panic!("native cell must classify as a View"),
    }
}

fn rules_of(cell: Element) -> Rc<StyleRules> {
    runtime_core::resolve_style(&cell_app(cell))
}

fn bg_token(rules: &StyleRules) -> Option<String> {
    rules.background.as_ref().and_then(|b| b.name().map(str::to_string))
}

/// The text node a `text = Some(..)` cell renders, classified.
fn cell_text_rules(cell: Element) -> Rc<StyleRules> {
    let children = match classify(cell_node(cell)) {
        P::View { children, .. } => children,
        _ => panic!("native cell must classify as a View"),
    };
    match classify(children.into_iter().next().expect("cell has its text")) {
        P::Text { style, .. } => {
            runtime_core::resolve_style(&style.expect("text is styled").application())
        }
        _ => panic!("cell content must be its text node"),
    }
}

fn px_of(t: &Option<Tokenized<Length>>) -> Option<f32> {
    match t.as_ref().map(|t| t.resolve()) {
        Some(Length::Px(v)) => Some(v),
        _ => None,
    }
}

/// Gap #1: `TableCell` had no width or alignment, so CrewForge rebuilt
/// every sized column a layer below idea-ui and copied the themed
/// sheets by hand. A sized cell must carry the EXACT-column pair
/// (`width` + `max-width`: `width` alone is a floor in a browser's auto
/// table), a floor, and the alignment on the cell AND its text.
#[test]
fn regression_cell_width_floor_and_alignment() {
    with_test_world(|| {
        let make = || {
            TableCell(TableCellProps {
                text: Reactive::Static(Some("12.5".into())),
                width: Reactive::Static(Some(120.0)),
                min_width: Reactive::Static(Some(80.0)),
                align: CellAlign::Right,
                ..Default::default()
            })
        };
        let rules = rules_of(make());
        assert_eq!(px_of(&rules.width), Some(120.0));
        assert_eq!(px_of(&rules.max_width), Some(120.0), "exact column needs max-width too");
        assert_eq!(px_of(&rules.min_width), Some(80.0));
        assert_eq!(rules.text_align, Some(runtime_core::TextAlign::Right));
        assert_eq!(
            cell_text_rules(make()).text_align,
            Some(runtime_core::TextAlign::Right),
            "the text node (what native reads) aligns too"
        );
        // A cell with no geometry keeps the pure axis selection — it
        // still premints.
        let plain = cell_app(body_cell("x"));
        assert!(plain.preminted_class_list().is_some());
    });
}

/// Gap #1, resize half: a LIVE width re-styles the cell in place (the
/// column follows a drag) rather than being snapshotted at build.
#[test]
fn reactive_cell_width_follows_its_signal() {
    with_test_world(|| {
        let w = signal(Some(100.0_f32));
        let cell = TableCell(TableCellProps {
            width: Reactive::Dynamic(Rc::new(move || w.get())),
            ..Default::default()
        });
        let style = match classify(cell_node(cell)) {
            P::View { style, .. } => style.expect("styled"),
            _ => panic!("view"),
        };
        assert!(style.is_reactive(), "a live width is a reactive style");
        let at = |s: &crate::test_support::TStyle| {
            px_of(&runtime_core::resolve_style(&s.application()).width)
        };
        assert_eq!(at(&style), Some(100.0));
        w.set(Some(240.0));
        idea_theme::testing::commit();
        assert_eq!(at(&style), Some(240.0));
    });
}

/// Truncation (gap #1's "ends in …"): `max_lines: 1` on the cell —
/// what truncates on web, where the text is an inline span — and on
/// the text node, what truncates on native.
#[test]
fn truncate_limits_cell_and_text_to_one_line() {
    with_test_world(|| {
        let make = || {
            TableCell(TableCellProps {
                text: Reactive::Static(Some("A long crew member name".into())),
                width: Reactive::Static(Some(120.0)),
                truncate: true,
                ..Default::default()
            })
        };
        assert_eq!(rules_of(make()).max_lines, Some(1));
        assert_eq!(cell_text_rules(make()).max_lines, Some(1));
        assert_eq!(rules_of(body_cell("x")).max_lines, None, "off by default");
    });
}

/// The native grid a `Table` lowers to, and its cells.
fn grid_cells(t: Element) -> Vec<Element> {
    let outer = match peel_owned_keepalive(t) {
        Element::Item { children, .. } => children,
        _ => panic!("table lowers to its outer view"),
    };
    match peel_owned_keepalive(outer.into_iter().next().expect("outer wraps the grid")) {
        Element::Item { children, .. } => children,
        _ => panic!("grid view"),
    }
}

/// Gap #2: no colspan, so a "Nobody yet" message got column 1's width
/// and wrapped inside it. `ColSpan::Rest` must cover every column.
#[test]
fn regression_rest_span_covers_the_row() {
    with_test_world(|| {
        let head = TableRow(TableRowProps {
            children: (0..4).map(|_| body_cell("h")).collect(),
            ..Default::default()
        });
        let blank = TableRow(TableRowProps {
            children: vec![TableCell(TableCellProps {
                text: Reactive::Static(Some("Nobody yet".into())),
                span: ColSpan::Rest,
                ..Default::default()
            })],
            ..Default::default()
        });
        let cells = grid_cells(Table(TableProps {
            children: vec![head, blank],
            ..Default::default()
        }));
        assert_eq!(cells.len(), 5);
        let rules = rules_of(cells.into_iter().nth(4).unwrap());
        assert_eq!(
            rules.grid_column,
            Some(runtime_core::GridPlacement::Lines(1, 5)),
            "the blank message spans all four columns"
        );
    });
}

fn tone_row(tone: RowTone, clickable: bool, pinned: Option<ColumnPin>) -> Vec<Element> {
    row_cells(TableRow(TableRowProps {
        children: vec![TableCell(TableCellProps {
            text: Reactive::Static(Some("x".into())),
            pinned,
            ..Default::default()
        })],
        tone: Reactive::Static(tone),
        on_row_click: clickable.then(|| Rc::new(|| {}) as Rc<dyn Fn()>),
        ..Default::default()
    }))
}

/// Gap #3: tinting a row by overriding each cell's background beat the
/// hover axis, so a warning row stopped responding to the pointer, and
/// it was not preminted. A tone must be an axis that premints, keeps
/// the hover (as a deeper shade of its own tint), and stays opaque on
/// a frozen column.
#[test]
fn regression_row_tone_composes_with_hover_and_pin() {
    with_test_world(|| {
        // Static tone on a plain row: a static, preminted selection.
        let app = cell_app(tone_row(RowTone::Warning, false, None).remove(0));
        assert!(app.preminted_class_list().is_some(), "a row tone premints");
        assert_eq!(
            bg_token(&runtime_core::resolve_style(&app)).as_deref(),
            Some("color-table-row-warning")
        );

        // Clickable + toned: resting shows the tone, hovered the tone's
        // hover shade — the hover is not lost.
        let resting = cell_app(tone_row(RowTone::Danger, true, None).remove(0));
        assert_eq!(
            bg_token(&runtime_core::resolve_style(&resting)).as_deref(),
            Some("color-table-row-danger")
        );
        let hovered = resting.clone().with("row_hovered", "on");
        assert_eq!(
            bg_token(&runtime_core::resolve_style(&hovered)).as_deref(),
            Some("color-table-row-danger-hover"),
            "a toned row still answers the pointer"
        );

        // Frozen + toned: the tone, not the pinned surface — and the
        // tone tokens are opaque in both themes.
        let pinned = cell_app(tone_row(RowTone::Highlight, true, Some(ColumnPin::Left)).remove(0));
        assert_eq!(
            bg_token(&runtime_core::resolve_style(&pinned)).as_deref(),
            Some("color-table-row-highlight")
        );
        for theme in [crate::light_theme(), crate::dark_theme()] {
            let c = &theme.colors;
            for tok in [
                &c.table_row_highlight, &c.table_row_highlight_hover,
                &c.table_row_warning, &c.table_row_warning_hover,
                &c.table_row_danger, &c.table_row_danger_hover,
            ] {
                let v = tok.value().0.to_ascii_lowercase();
                assert!(
                    v.starts_with('#') && v.len() == 7,
                    "row tone tokens must be opaque hex (a frozen column covers \
                     content with them), got {v}"
                );
            }
        }
    });
}

/// Gap #3, live half: a reactive tone re-tints the row in place.
#[test]
fn reactive_row_tone_retints_in_place() {
    with_test_world(|| {
        let tone = signal(RowTone::None);
        let cells = row_cells(TableRow(TableRowProps {
            children: vec![body_cell("x")],
            tone: Reactive::Dynamic(Rc::new(move || tone.get())),
            ..Default::default()
        }));
        let style = match classify(cells.into_iter().next().unwrap()) {
            P::View { style, .. } => style.expect("styled"),
            _ => panic!("view"),
        };
        let bg = |s: &crate::test_support::TStyle| {
            bg_token(&runtime_core::resolve_style(&s.application()))
        };
        assert_ne!(bg(&style).as_deref(), Some("color-table-row-warning"));
        tone.set(RowTone::Warning);
        idea_theme::testing::commit();
        assert_eq!(bg(&style).as_deref(), Some("color-table-row-warning"));
    });
}

/// Gap #4: no footer row — a totals row read as data. `TableFooter`
/// lowers to the SDK's footer section and tints its cells with the
/// header band, frozen ones included.
#[test]
fn regression_footer_row_is_a_section_with_the_band_tint() {
    with_test_world(|| {
        let footer = TableFooter(TableFooterProps {
            children: vec![TableRow(TableRowProps {
                children: vec![
                    TableCell(TableCellProps {
                        text: Reactive::Static(Some("Totals".into())),
                        pinned: Some(ColumnPin::Left),
                        ..Default::default()
                    }),
                    body_cell("38.5"),
                ],
                ..Default::default()
            })],
        });
        let rows = match peel_owned_keepalive(footer) {
            Element::Item { data, children, .. } => {
                assert!(
                    data.downcast_ref::<runtime_vocabulary::prims::PrimCell<table::TableFootPrim>>()
                        .is_some(),
                    "TableFooter lowers to the SDK footer section"
                );
                children
            }
            _ => panic!("footer section item"),
        };
        for cell in row_cells(rows.into_iter().next().unwrap()) {
            assert_eq!(bg_token(&rules_of(cell)).as_deref(), Some("color-table-header"));
        }
    });
}

/// Gap #5: content that must not scroll sideways with the columns had
/// to sit outside the table's frame. The slots land inside the clipped
/// surface, either side of the scroller, with the cells' padding.
#[test]
fn regression_frame_slots_sit_inside_the_surface() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![TableRow(TableRowProps {
                children: vec![body_cell("a")],
                ..Default::default()
            })],
            scroll_x: true,
            header_slot: Some(runtime_core::text("Holiday").into_element()),
            footer_slot: Some(runtime_core::text("Add entry").into_element()),
            ..Default::default()
        });
        let (style, kids) = match peel_owned_keepalive(t) {
            Element::Item { data, children, .. } => (
                data.downcast_ref::<runtime_vocabulary::prims::PrimCell<
                    runtime_vocabulary::prims::ViewPrim,
                >>()
                .expect("surface view")
                .take()
                .style,
                children,
            ),
            _ => panic!("surface"),
        };
        let surface = match style {
            Some(StyleProp::Sheet(app)) => runtime_core::resolve_style(&app),
            _ => panic!("themed surface sheet"),
        };
        assert_eq!(surface.overflow, Some(runtime_core::Overflow::Hidden), "surface clips");
        assert_eq!(kids.len(), 3, "header slot, scroller, footer slot");
        let mut kids = kids.into_iter();
        let header = rules_of(kids.next().unwrap());
        assert!(header.border_bottom_width.is_some(), "header slot draws its divider");
        assert!(header.padding_left.is_some(), "slot content gets the cells' padding");
    });
}

/// Gap #6: two pinned columns both stuck at `left: 0` and overlapped.
/// `pin_offset` insets the second; the first keeps the preminted zero.
#[test]
fn regression_second_pinned_column_insets_by_its_offset() {
    with_test_world(|| {
        let second = rules_of(TableCell(TableCellProps {
            pinned: Some(ColumnPin::Left),
            pin_offset: Some(48.0),
            ..Default::default()
        }));
        assert_eq!(second.position, Some(runtime_core::Position::Sticky));
        assert_eq!(px_of(&second.left), Some(48.0));
        let right = rules_of(TableCell(TableCellProps {
            pinned: Some(ColumnPin::Right),
            pin_offset: Some(30.0),
            ..Default::default()
        }));
        assert_eq!(px_of(&right.right), Some(30.0));
        assert!(right.left.is_none());
        let first = cell_app(TableCell(TableCellProps {
            pinned: Some(ColumnPin::Left),
            ..Default::default()
        }));
        assert!(first.preminted_class_list().is_some(), "offset 0 stays preminted");
    });
}

/// Gap #7: no ref on `TableRow`. `bind_to` routes to the SDK's row
/// proxy slot — the `<tr>` on web, the row backdrop on native.
#[test]
fn regression_row_bind_to_fills_the_row_proxy() {
    with_test_world(|| {
        let r: Ref<ViewHandle> = Ref::new();
        let mut row = TableRow(TableRowProps {
            children: vec![body_cell("x")],
            bind_to: Some(r),
            ..Default::default()
        });
        loop {
            match row {
                Element::Owned { element, owned, .. } => {
                    ROW_SCOPES.with(|k| k.borrow_mut().push(owned));
                    row = *element;
                }
                Element::Item { data, .. } => {
                    let prim = data
                        .downcast_ref::<runtime_vocabulary::prims::PrimCell<table::TableRowPrim>>()
                        .expect("row marker")
                        .take();
                    assert!(prim.ref_fill.is_some(), "bind_to sets the row proxy slot");
                    break;
                }
                _ => panic!("row marker item"),
            }
        }
    });
}

/// Gap #8: row padding was fixed at `spacing.md`. `density` reaches
/// every cell — head and body — as a static (preminted) selection.
#[test]
fn regression_table_density_reaches_every_cell() {
    with_test_world(|| {
        let spacing = &crate::light_theme().spacing;
        for (density, expect) in [
            (TableDensity::Compact, spacing.sm),
            (TableDensity::Standard, spacing.md),
            (TableDensity::Comfortable, spacing.lg),
        ] {
            let head = TableRow(TableRowProps {
                children: vec![TableCell(TableCellProps {
                    header: true,
                    text: Reactive::Static(Some("h".into())),
                    ..Default::default()
                })],
                ..Default::default()
            });
            let body = TableRow(TableRowProps {
                children: vec![body_cell("b")],
                ..Default::default()
            });
            let cells = grid_cells(Table(TableProps {
                children: vec![head, body],
                density,
                ..Default::default()
            }));
            for cell in cells {
                let app = cell_app(cell);
                assert!(app.preminted_class_list().is_some(), "density premints");
                let rules = runtime_core::resolve_style(&app);
                let pad = px_of(&rules.padding_top);
                assert_eq!(pad, Some(expect), "{density:?}");
            }
        }
    });
}

/// The cells of a table that may be WRAPPED (scroll-x / frame slots):
/// the surface view holds [header slot?, content, footer slot?], and a
/// scroll-x content is a horizontal scroller around the plain table.
fn any_grid_cells(t: Element, wrapped: bool, header_slot: bool, scroll_x: bool) -> Vec<Element> {
    if !wrapped {
        return grid_cells(t);
    }
    let surface = match peel_owned_keepalive(t) {
        Element::Item { children, .. } => children,
        _ => panic!("a wrapped table lowers to its surface view"),
    };
    let content = surface.into_iter().nth(header_slot as usize).expect("surface holds the rows");
    if !scroll_x {
        return grid_cells(content);
    }
    match peel_owned_keepalive(content) {
        Element::Item { children, .. } => {
            grid_cells(children.into_iter().next().expect("the scroller wraps the table"))
        }
        _ => panic!("scroll-x content is the scroller"),
    }
}

/// Each rendered cell's bottom-divider width, in render order.
fn dividers(cells: Vec<Element>) -> Vec<Option<f32>> {
    cells
        .into_iter()
        .map(|cell| {
            let app = cell_app(cell);
            assert!(app.preminted_class_list().is_some(), "the divider selection premints");
            runtime_core::resolve_style(&app).border_bottom_width.as_ref().map(|w| w.resolve())
        })
        .collect()
}

fn row_of(cells: Vec<Element>) -> Element {
    TableRow(TableRowProps { children: cells, ..Default::default() })
}

fn head_cell(text: &str) -> Element {
    TableCell(TableCellProps {
        header: true,
        text: Reactive::Static(Some(text.into())),
        ..Default::default()
    })
}

/// REGRESSION (idea-ui 4.0.0): the bottom edge of every table was a
/// double line. Every cell drew its row divider — the last row's
/// included — and the frame drew its own 1px border right under it.
/// The row rendered last must drop its divider; every other row keeps
/// one.
#[test]
fn regression_table_bottom_edge_is_not_a_double_line() {
    with_test_world(|| {
        for scroll_x in [false, true] {
            let t = Table(TableProps {
                children: vec![
                    row_of(vec![head_cell("Name"), head_cell("Hours")]),
                    row_of(vec![body_cell("Ana"), body_cell("8")]),
                    row_of(vec![body_cell("Bo"), body_cell("6")]),
                ],
                scroll_x,
                ..Default::default()
            });
            assert_eq!(
                dividers(any_grid_cells(t, scroll_x, false, scroll_x)),
                vec![Some(1.0), Some(1.0), Some(1.0), Some(1.0), Some(0.0), Some(0.0)],
                "scroll_x = {scroll_x}: only the last row drops its divider"
            );
        }
    });
}

/// A footer renders after the body rows wherever it is written, so
/// ITS last row is the one against the frame — a totals foot doubled
/// the same way. Written first here to prove the choice follows render
/// order, not source order.
#[test]
fn regression_table_footer_is_the_last_row_wherever_written() {
    with_test_world(|| {
        let foot = TableFooter(TableFooterProps {
            children: vec![row_of(vec![body_cell("Total"), body_cell("14")])],
        });
        let t = Table(TableProps {
            children: vec![
                foot,
                row_of(vec![head_cell("Name"), head_cell("Hours")]),
                row_of(vec![body_cell("Ana"), body_cell("8")]),
            ],
            ..Default::default()
        });
        assert_eq!(
            dividers(grid_cells(t)),
            vec![Some(1.0), Some(1.0), Some(1.0), Some(1.0), Some(0.0), Some(0.0)],
            "the body's last row keeps its divider above the foot"
        );
    });
}

/// With a footer slot below the rows, the last row's divider IS the
/// line between rows and slot — the frame's border is under the slot —
/// so it stays.
#[test]
fn table_footer_slot_keeps_the_last_row_divider() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![row_of(vec![head_cell("Name")]), row_of(vec![body_cell("Ana")])],
            footer_slot: Some(runtime_core::text("Add entry".to_string()).into_element()),
            ..Default::default()
        });
        assert_eq!(dividers(any_grid_cells(t, true, false, false)), vec![Some(1.0), Some(1.0)]);
    });
}

/// A table that is only its header row: the header cells are the last
/// row, so they carry the axis too.
#[test]
fn header_only_table_drops_the_header_divider() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![row_of(vec![head_cell("Name"), head_cell("Hours")])],
            ..Default::default()
        });
        assert_eq!(dividers(grid_cells(t)), vec![Some(0.0), Some(0.0)]);
    });
}

/// The cells of a `Plain` + `scroll_x` table: the surface holds the
/// viewport holder the edge watch measures, around the scroller.
fn plain_scroll_cells(t: Element) -> Vec<Element> {
    let surface = match peel_owned_keepalive(t) {
        Element::Item { children, .. } => children,
        _ => panic!("a scroll-x table lowers to its surface view"),
    };
    let holder = match peel_owned_keepalive(surface.into_iter().next().expect("surface holds the rows")) {
        Element::Item { children, .. } => children,
        _ => panic!("a watched scroller sits in its viewport holder"),
    };
    match peel_owned_keepalive(holder.into_iter().next().expect("the holder wraps the scroller")) {
        Element::Item { children, .. } => {
            grid_cells(children.into_iter().next().expect("the scroller wraps the table"))
        }
        _ => panic!("the holder's child is the scroller"),
    }
}

fn pinned_head(text: &str, pin: ColumnPin) -> Element {
    TableCell(TableCellProps {
        header: true,
        text: Reactive::Static(Some(text.into())),
        pinned: Some(pin),
        ..Default::default()
    })
}

fn pinned_body(text: &str, pin: ColumnPin) -> Element {
    TableCell(TableCellProps {
        text: Reactive::Static(Some(text.into())),
        pinned: Some(pin),
        ..Default::default()
    })
}

fn is_transparent(c: &Option<Tokenized<runtime_core::Color>>) -> bool {
    c.as_ref().is_some_and(|c| c.resolve().0 == "transparent")
}

/// `Plain` drops the card: no frame, no radius, no fill of its own —
/// and `Framed` (the default) keeps every one of them.
#[test]
fn plain_table_surface_has_no_frame() {
    with_test_world(|| {
        let plain = runtime_core::resolve_style(
            &TableStyles::frame().into_style_application().with("variant", "plain"),
        );
        for w in [
            &plain.border_top_width,
            &plain.border_right_width,
            &plain.border_bottom_width,
            &plain.border_left_width,
        ] {
            assert_eq!(w.as_ref().map(|w| w.resolve()), Some(0.0), "plain draws no frame");
        }
        assert!(is_transparent(&plain.background), "plain paints no fill of its own");

        let framed = runtime_core::resolve_style(&TableStyles::frame().into_style_application());
        assert_eq!(framed.border_top_width.as_ref().map(|w| w.resolve()), Some(1.0));
        assert_eq!(bg_token(&framed).as_deref(), Some("color-surface"));
    });
}

/// A plain table has no header band, but a FROZEN head cell must stay
/// opaque — the columns slide beneath it — so it paints the surface.
/// Every cell of the table takes the selection, as a static one.
#[test]
fn plain_table_drops_the_header_band_but_frozen_heads_stay_opaque() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![
                row_of(vec![pinned_head("Name", ColumnPin::Left), head_cell("Hours")]),
                row_of(vec![pinned_body("Ana", ColumnPin::Left), body_cell("8")]),
            ],
            variant: TableVariant::Plain,
            ..Default::default()
        });
        let mut cells = grid_cells(t).into_iter();
        let frozen_head = cell_app(cells.next().unwrap());
        let head = cell_app(cells.next().unwrap());
        assert!(head.preminted_class_list().is_some(), "the frame selection premints");
        assert!(
            is_transparent(&runtime_core::resolve_style(&head).background),
            "a plain head cell has no band"
        );
        assert_eq!(
            bg_token(&runtime_core::resolve_style(&frozen_head)).as_deref(),
            Some("color-surface"),
            "a frozen plain head cell is opaque"
        );
    });
}

/// The whole feature at the component layer: in a plain scroll-x
/// table, ONLY the frozen cells go reactive, and they start with their
/// inner edge hidden (nothing is measured as scrolled beneath them
/// yet). Every other cell stays a static, preminted selection — a
/// scroll must not re-style the table.
#[test]
fn plain_scroll_table_watches_only_the_frozen_cells() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![
                row_of(vec![
                    pinned_head("Name", ColumnPin::Left),
                    head_cell("Hours"),
                    pinned_head("", ColumnPin::Right),
                ]),
                row_of(vec![
                    pinned_body("Ana", ColumnPin::Left),
                    body_cell("8"),
                    pinned_body("…", ColumnPin::Right),
                ]),
            ],
            variant: TableVariant::Plain,
            scroll_x: true,
            ..Default::default()
        });
        for (i, cell) in plain_scroll_cells(t).into_iter().enumerate() {
            let style = match classify(cell_node(cell)) {
                P::View { style, .. } => style.expect("cell keeps a style"),
                _ => panic!("native cell must classify as a View"),
            };
            let rules = runtime_core::resolve_style(&style.application());
            match i % 3 {
                0 => {
                    assert!(style.is_reactive(), "cell {i}: a left-frozen cell follows the scroll");
                    assert_eq!(rules.border_right_width.as_ref().map(|w| w.resolve()), Some(1.0));
                    assert!(
                        is_transparent(&rules.border_right_color),
                        "cell {i}: the edge is hidden while nothing is behind it"
                    );
                }
                2 => {
                    assert!(style.is_reactive(), "cell {i}: a right-frozen cell follows the scroll");
                    assert!(is_transparent(&rules.border_left_color), "cell {i}");
                }
                _ => assert!(
                    !style.is_reactive() && style.application().preminted_class_list().is_some(),
                    "cell {i}: an unfrozen cell stays static"
                ),
            }
        }
    });
}

/// The edge follows the scroll: hidden at rest, shown once columns
/// are behind THAT edge, for the side the cell is frozen to only.
#[test]
fn pin_edge_shows_only_the_edge_with_content_behind_it() {
    with_test_world(|| {
        let edges = signal(table::ScrollEdges::default());
        let left = TableStyles::body_cell().into_style_application().with("pinned", "left");
        let right = TableStyles::body_cell().into_style_application().with("pinned", "right");
        let colours = || {
            let l = runtime_core::resolve_style(&pin_edge(left.clone(), edges));
            let r = runtime_core::resolve_style(&pin_edge(right.clone(), edges));
            (is_transparent(&l.border_right_color), is_transparent(&r.border_left_color))
        };
        assert_eq!(colours(), (true, true), "at rest with nothing behind: both hidden");
        edges.set(table::ScrollEdges { left: false, right: true });
        idea_theme::testing::commit();
        assert_eq!(colours(), (true, false), "columns run off the right: the right edge shows");
        edges.set(table::ScrollEdges { left: true, right: false });
        idea_theme::testing::commit();
        assert_eq!(colours(), (false, true), "scrolled to the end: the left edge shows");

        let shown = pin_edge(left.clone(), edges);
        assert!(shown.preminted_class_list().is_some(), "both arms premint");
        let unpinned = TableStyles::body_cell().into_style_application();
        assert!(
            !pin_edge(unpinned, edges).variants.0.contains_key("pin_edge"),
            "an unfrozen cell is returned untouched"
        );
    });
}

/// `Framed` (the default) is unchanged: a frozen column in a scroll-x
/// table keeps its hairline at rest and stays static — nothing
/// watches its scroller.
#[test]
fn framed_scroll_table_keeps_its_frozen_edge_static() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![row_of(vec![pinned_body("Ana", ColumnPin::Left), body_cell("8")])],
            scroll_x: true,
            ..Default::default()
        });
        let frozen = any_grid_cells(t, true, false, true).into_iter().next().unwrap();
        let app = cell_app(frozen);
        assert!(app.preminted_class_list().is_some(), "a framed frozen cell stays static");
        let rules = runtime_core::resolve_style(&app);
        assert!(!is_transparent(&rules.border_right_color), "and draws its edge at rest");
    });
}

// =========================================================================
// Scroll-y (pinned header) and edge inset.
// =========================================================================

/// The surface and scroller of a scrolling table: (surface rules,
/// scroller axis, rendered cells).
fn scrolling_parts(t: Element) -> (Rc<StyleRules>, runtime_core::ScrollAxis, Vec<Element>) {
    use runtime_vocabulary::prims::{PrimCell, ScrollViewPrim, ViewPrim};
    let (surface_rules, mut kids) = match peel_owned_keepalive(t) {
        Element::Item { data, children, .. } => {
            let vp = data.downcast_ref::<PrimCell<ViewPrim>>().expect("surface view").take();
            (surface_rules_of(vp.style), children)
        }
        _ => panic!("a scrolling table lowers to its surface view"),
    };
    let scroller = kids.pop().expect("surface wraps the scroller");
    let (axis, mut inner) = match peel_owned_keepalive(scroller) {
        Element::Item { data, children, .. } => (
            data.downcast_ref::<PrimCell<ScrollViewPrim>>().expect("scroll_view").take().axis,
            children,
        ),
        _ => panic!("the surface's content is the scroller"),
    };
    (surface_rules, axis, grid_cells(inner.pop().expect("the scroller wraps the table")))
}

fn surface_rules_of(style: Option<runtime_vocabulary::StyleProp>) -> Rc<StyleRules> {
    match style.expect("surface carries the Table sheet") {
        runtime_vocabulary::StyleProp::Sheet(app) => runtime_core::resolve_style(&app),
        _ => panic!("the Table sheet is a static application"),
    }
}

fn z_of(rules: &StyleRules) -> Option<i32> {
    rules.z_index
}

/// CrewForge: a header row could not stay put while the rows scroll.
/// `scroll_y` pins the leading head row to the top of the table's own
/// scroller, and with `scroll_x` the corner (a frozen head cell) pins
/// on both axes, stacked above the header row, which is stacked above
/// the frozen body column. Body cells take no top pin.
#[test]
fn regression_scroll_y_pins_the_header_row_and_stacks_the_corner_on_top() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![
                row_of(vec![pinned_head("Name", ColumnPin::Left), head_cell("Hours")]),
                row_of(vec![pinned_body("Ana", ColumnPin::Left), body_cell("8")]),
            ],
            scroll_x: true,
            scroll_y: true,
            ..Default::default()
        });
        let (_, axis, cells) = scrolling_parts(t);
        assert_eq!(axis, runtime_core::ScrollAxis::Both, "one two-axis scroller");
        let rules: Vec<_> = cells
            .into_iter()
            .map(|c| {
                let app = cell_app(c);
                assert!(app.preminted_class_list().is_some(), "the header pin premints");
                runtime_core::resolve_style(&app)
            })
            .collect();
        let (corner, head, frozen, body) = (&rules[0], &rules[1], &rules[2], &rules[3]);

        assert_eq!(corner.position, Some(runtime_core::Position::Sticky));
        assert_eq!(px_of(&corner.top), Some(0.0), "the corner pins to the top");
        assert_eq!(px_of(&corner.left), Some(0.0), "…and to the left");
        assert_eq!(head.position, Some(runtime_core::Position::Sticky));
        assert_eq!(px_of(&head.top), Some(0.0), "the header pins to the top");
        assert!(head.left.is_none(), "an unfrozen header cell scrolls sideways");
        assert!(frozen.top.is_none(), "a body cell never pins to the top");
        assert_eq!(px_of(&frozen.left), Some(0.0), "the frozen column keeps pinning left");
        assert_eq!(body.position, None, "a plain body cell stays in flow");

        let z = |r: &StyleRules| r.stacking_z();
        assert!(z(frozen) < z(head), "rows slide under the header row");
        assert!(z(head) < z(corner), "the header row slides under the corner");
        assert!(z(body) < z(frozen), "body cells slide under the frozen column");
        // A row lifted by a reorder drag rides above other rows' frozen
        // cells and still slides under the header row.
        let lifted = runtime_core::resolve_style(
            &TableStyles::body_cell().into_style_application().with("drag", "lifted"),
        );
        let lifted_frozen = runtime_core::resolve_style(
            &TableStyles::body_cell().into_style_application().with("drag", "lifted").with("pinned", "left"),
        );
        assert!(z(frozen) < z(&lifted), "a lifted row rides above frozen cells");
        assert!(z(&lifted) < z(&lifted_frozen), "its own frozen cell stays above its scrolling ones");
        assert!(z(&lifted_frozen) < z(head), "and the lifted row slides under the header");
        assert_eq!(z_of(corner), Some(5));
    });
}

/// Without `scroll_y` nothing pins to the top — the header scrolls
/// with the page, as before — and a scroll-y table without `scroll_x`
/// owns a vertical scroller.
#[test]
fn header_pins_only_in_a_scroll_y_table() {
    with_test_world(|| {
        let still = Table(TableProps {
            children: vec![row_of(vec![head_cell("Name")]), row_of(vec![body_cell("Ana")])],
            ..Default::default()
        });
        let head = rules_of(grid_cells(still).into_iter().next().unwrap());
        assert!(head.top.is_none() && head.position.is_none(), "{head:?}");

        let t = Table(TableProps {
            children: vec![row_of(vec![head_cell("Name")]), row_of(vec![body_cell("Ana")])],
            scroll_y: true,
            ..Default::default()
        });
        let (_, axis, cells) = scrolling_parts(t);
        assert_eq!(axis, runtime_core::ScrollAxis::Vertical);
        let head = rules_of(cells.into_iter().next().unwrap());
        assert_eq!(px_of(&head.top), Some(0.0));
    });
}

/// CrewForge: a header row whose cells carry a REACTIVE head style
/// (a resizable column's width follows a live drag) never pinned —
/// head-row detection read only static sheets, so the row did not
/// count as a header and `scroll_y` left it scrolling with the body.
#[test]
fn regression_scroll_y_pins_a_header_row_with_dynamic_cell_styles() {
    use runtime_vocabulary::style_attach::StyleProp;
    with_test_world(|| {
        let width = runtime_core::signal(120.0_f32);
        let dynamic_head = table::table_cell(Default::default())
            .with_style(StyleProp::SheetDynamic(Box::new(move || {
                let _ = width.get();
                TableStyles::head_cell().into_style_application().with("sticky_header", "off")
            })))
            .into_element();
        let t = Table(TableProps {
            children: vec![row_of(vec![dynamic_head]), row_of(vec![body_cell("Ana")])],
            scroll_y: true,
            ..Default::default()
        });
        let (_, _, cells) = scrolling_parts(t);
        let head = rules_of(cells.into_iter().next().unwrap());
        assert_eq!(head.position, Some(runtime_core::Position::Sticky));
        assert_eq!(px_of(&head.top), Some(0.0), "the dynamic header row pins to the top");
    });
}

/// Only the LEADING head rows pin: a row of head cells after the body
/// (a totals row built from head cells) scrolls with the rows.
#[test]
fn only_the_leading_head_rows_pin() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![
                row_of(vec![head_cell("Name")]),
                row_of(vec![head_cell("Group")]),
                row_of(vec![body_cell("Ana")]),
                row_of(vec![head_cell("Total")]),
            ],
            scroll_y: true,
            ..Default::default()
        });
        let (_, _, cells) = scrolling_parts(t);
        let tops: Vec<_> = cells.into_iter().map(|c| px_of(&rules_of(c).top)).collect();
        assert_eq!(tops, vec![Some(0.0), Some(0.0), None, None]);
    });
}

/// `scroll_y` makes the surface take its parent's height (grow, and
/// shrink below its rows) and `max_height` caps it; `max_height`
/// without `scroll_y` does nothing.
#[test]
fn scroll_y_surface_fills_and_max_height_caps_it() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![row_of(vec![body_cell("Ana")])],
            scroll_y: true,
            max_height: Some(320.0),
            ..Default::default()
        });
        let (surface, _, _) = scrolling_parts(t);
        assert_eq!(surface.flex_grow.as_ref().map(|g| g.resolve()), Some(1.0));
        assert_eq!(px_of(&surface.min_height), Some(0.0), "may shrink below its rows");
        assert_eq!(px_of(&surface.max_height), Some(320.0));

        let unscrolled = Table(TableProps {
            children: vec![row_of(vec![body_cell("Ana")])],
            max_height: Some(320.0),
            footer_slot: Some(runtime_core::text("x".to_string()).into_element()),
            ..Default::default()
        });
        let surface = match peel_owned_keepalive(unscrolled) {
            Element::Item { data, .. } => data
                .downcast_ref::<runtime_vocabulary::prims::PrimCell<runtime_vocabulary::prims::ViewPrim>>()
                .unwrap()
                .take(),
            _ => panic!("surface"),
        };
        let rules = surface_rules_of(surface.style);
        assert!(rules.max_height.is_none(), "max_height is a scroll-y setting");
        assert!(rules.flex_grow.is_none());
    });
}

/// CrewForge: cell padding was fixed, so the row dividers could not run
/// the full width while the text kept a page margin. `inset_x` pads
/// the outer side of each row's FIRST and LAST cell only — the cells
/// between keep the base padding — and a single-cell row takes both.
#[test]
fn regression_inset_x_pads_only_the_edge_cells() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![
                row_of(vec![head_cell("A"), head_cell("B"), head_cell("C")]),
                row_of(vec![body_cell("1"), body_cell("2"), body_cell("3")]),
                row_of(vec![TableCell(TableCellProps {
                    span: ColSpan::Rest,
                    text: Reactive::Static(Some("Nobody yet".into())),
                    ..Default::default()
                })]),
            ],
            inset_x: Reactive::Static(TableInset::Xl),
            ..Default::default()
        });
        let lg = px_of(&rules_of(body_cell("x")).padding_left).expect("base cell padding");
        let mut pads = Vec::new();
        // (No premint assertion here: the spanning row switches the grid
        // to explicit placement, which rides an override on every cell.)
        for c in grid_cells(t) {
            let app = cell_app(c);
            let r = runtime_core::resolve_style(&app);
            pads.push((px_of(&r.padding_left).unwrap(), px_of(&r.padding_right).unwrap()));
        }
        let xl = pads[0].0;
        assert!(xl > lg, "the xl inset is wider than the base padding ({xl} vs {lg})");
        assert_eq!(
            pads,
            vec![(xl, lg), (lg, lg), (lg, xl), (xl, lg), (lg, lg), (lg, xl), (xl, xl)],
            "first cell: left inset; last cell: right inset; middle: neither"
        );
    });
}

/// An exact inset rides an override on the edge cells (and the frame
/// slots take the same inset on both sides, so slot content lines up
/// with the first column's text).
#[test]
fn inset_x_px_is_exact_and_reaches_the_slots() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![row_of(vec![body_cell("1"), body_cell("2")])],
            inset_x: Reactive::Static(TableInset::Px(28.0)),
            header_slot: Some(runtime_core::text("note".to_string()).into_element()),
            ..Default::default()
        });
        let mut surface = match peel_owned_keepalive(t) {
            Element::Item { children, .. } => children,
            _ => panic!("surface"),
        };
        let holder = surface.pop().expect("grid holder");
        let slot = surface.pop().expect("header slot");
        let cells = grid_cells(holder);
        let r0 = rules_of(cells.into_iter().next().unwrap());
        assert_eq!(px_of(&r0.padding_left), Some(28.0));
        let slot_rules = match classify(slot) {
            P::View { style, .. } => runtime_core::resolve_style(&style.unwrap().application()),
            _ => panic!("slot is a view"),
        };
        assert_eq!(px_of(&slot_rules.padding_left), Some(28.0));
        assert_eq!(px_of(&slot_rules.padding_right), Some(28.0));
    });
}

/// A token inset is an axis selection, so the edge cells stay
/// preminted (build-time CSS per arm).
#[test]
fn token_inset_keeps_edge_cells_preminted() {
    with_test_world(|| {
        let t = Table(TableProps {
            children: vec![row_of(vec![head_cell("A"), head_cell("B")]), row_of(vec![body_cell("1"), body_cell("2")])],
            inset_x: Reactive::Static(TableInset::Xxl),
            ..Default::default()
        });
        for c in grid_cells(t) {
            assert!(cell_app(c).preminted_class_list().is_some());
        }
    });
}

// =========================================================================
// The column model, the added columns (selection, grip), the empty
// state, live inset, sort headers and cell tones.
// =========================================================================

fn text_of(cell: &str) -> Element {
    body_cell(cell)
}

/// A row with an id (and optional settings), of plain body cells.
fn id_row(id: &str, cells: &[&str], selectable: bool, draggable: bool) -> Element {
    TableRow(TableRowProps {
        children: cells.iter().map(|c| text_of(c)).collect(),
        id: Some(RowId::from(id)),
        selectable,
        draggable,
        ..Default::default()
    })
}

/// The raw view payload of a built element (Owned-peeled, through a
/// `CellMarker`) — for reading a handler `classify` collapses to a bool.
fn view_prim(el: Element) -> runtime_vocabulary::prims::ViewPrim {
    use runtime_vocabulary::prims::{PrimCell, ViewPrim};
    match cell_node(el) {
        Element::Item { data, .. } => data
            .downcast_ref::<PrimCell<ViewPrim>>()
            .expect("a view payload")
            .take(),
        _ => panic!("expected a view item"),
    }
}

/// Every pressable's click under `el`, depth first.
fn pressables(el: Element) -> Vec<Rc<dyn Fn()>> {
    let mut out = Vec::new();
    match classify(el) {
        P::Pressable { on_click, children, .. } => {
            out.push(on_click);
            for c in children {
                out.extend(pressables(c));
            }
        }
        P::View { children, .. } | P::Fragment { children } => {
            for c in children {
                out.extend(pressables(c));
            }
        }
        _ => {}
    }
    out
}

/// Whether a grid child is a row proxy's backdrop (a bound row — the
/// reorder drop target — puts one ahead of its cells on native): it
/// carries raw placement rules, never a cell sheet.
fn is_backdrop(el: &Element) -> bool {
    use runtime_vocabulary::prims::{PrimCell, ViewPrim};
    let mut el = el;
    while let Element::Owned { element, .. } = el {
        el = element;
    }
    match el {
        Element::Item { data, .. } => data.downcast_ref::<PrimCell<ViewPrim>>().is_some_and(|c| {
            let mut raw = false;
            c.with_mut(|p| raw = matches!(p.style, Some(StyleProp::Static(_))));
            raw
        }),
        _ => false,
    }
}

/// The content nodes of a cell.
fn cell_content(cell: Element) -> Vec<Element> {
    match classify(cell_node(cell)) {
        P::View { children, .. } => children,
        _ => panic!("native cell must classify as a View"),
    }
}

/// The grid node's live rules (`display: grid` + tracks).
fn grid_rules_of(t: Element) -> StyleRules {
    let outer = match peel_owned_keepalive(t) {
        Element::Item { children, .. } => children,
        _ => panic!("table lowers to its outer view"),
    };
    match classify(outer.into_iter().next().expect("outer wraps the grid")) {
        P::View { style, .. } => (*style.expect("the grid is styled").resolve()).clone(),
        _ => panic!("grid view"),
    }
}

fn touch(phase: runtime_core::TouchPhase, x: f32, t_ns: u64) -> runtime_core::TouchEvent {
    runtime_core::TouchEvent {
        id: runtime_core::TouchId(1),
        phase,
        position: runtime_core::TouchPoint::new(x, 0.0),
        window_position: runtime_core::TouchPoint::new(x, 0.0),
        timestamp_ns: t_ns,
        force: None,
    }
}

/// CrewForge (proposal item 1): every sized column was rebuilt a layer
/// below idea-ui so its header and body cells could share one width. One
/// table-level spec now sizes the column's header AND body cells, and
/// sets its alignment and truncation on the cell and the text a native
/// backend reads — with nothing on the cells themselves.
#[test]
fn regression_column_model_one_spec_sizes_header_and_body() {
    with_test_world(|| {
        let t = Table(TableProps {
            columns: vec![
                TableColumn::new("hours").width(160.0).align(CellAlign::Right).truncate(),
                TableColumn::new("notes"),
            ],
            children: vec![
                row_of(vec![head_cell("Hours"), head_cell("Notes")]),
                row_of(vec![body_cell("12.5"), body_cell("x")]),
            ],
            ..Default::default()
        });
        let cells = grid_cells(t);
        let mut it = cells.into_iter();
        let (h0, h1, b0) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        for (name, cell) in [("header", h0), ("body", b0)] {
            let app = cell_app(cell);
            let r = runtime_core::resolve_style(&app);
            assert_eq!(px_of(&r.width), Some(160.0), "{name} cell takes the column width");
            assert_eq!(px_of(&r.max_width), Some(160.0), "{name}: an exact column");
            assert_eq!(r.text_align, Some(runtime_core::TextAlign::Right), "{name} aligns");
            assert_eq!(r.max_lines, Some(1), "{name} truncates");
        }
        assert!(rules_of(h1).width.is_none(), "an unsized column keeps its natural width");
        // The text node — what a native text reads its alignment from.
        let t = Table(TableProps {
            columns: vec![TableColumn::new("hours").align(CellAlign::Right).truncate()],
            children: vec![row_of(vec![body_cell("12.5")])],
            ..Default::default()
        });
        let text = cell_text_rules(grid_cells(t).remove(0));
        assert_eq!(text.text_align, Some(runtime_core::TextAlign::Right));
        assert_eq!(text.max_lines, Some(1));
    });
}

/// A `--premint-only` bundle ships no runtime style engine: a node renders
/// only through its preminted classes plus an inline style. A table's
/// continuous geometry — a column width (fixed or following a resize), a
/// frozen column's offset, a pixel edge inset, a grid track placement —
/// rode the override layer, which takes an application off the premint
/// path, so every sized or frozen cell panicked in such a bundle. That
/// geometry is per-instance and belongs on the inline layer: every cell
/// keeps its preminted classes AND resolves to the same rules.
#[test]
fn regression_table_geometry_keeps_cells_premintable() {
    with_test_world(|| {
        let widths = signal(ColumnWidths::new());
        let t = Table(TableProps {
            columns: vec![
                TableColumn::new("name").width(200.0).pinned(ColumnPin::Left).resizable(),
                TableColumn::new("hours").width(120.0).pinned(ColumnPin::Left),
                TableColumn::new("notes").fill(),
            ],
            column_widths: Some(widths),
            on_column_resize: Some(Rc::new(|_, _| {})),
            selection: Some(signal(BTreeSet::new())),
            inset_x: Reactive::Static(TableInset::Px(28.0)),
            children: vec![
                row_of(vec![head_cell("Name"), head_cell("Hours"), head_cell("Notes")]),
                TableRow(TableRowProps {
                    id: Some("a".into()),
                    children: vec![body_cell("Ana"), body_cell("8"), body_cell("x")],
                    ..Default::default()
                }),
            ],
            ..Default::default()
        });
        let cells = grid_cells(t);
        assert!(!cells.is_empty());
        let mut saw_offset = false;
        for (i, cell) in cells.into_iter().enumerate() {
            let app = cell_app(cell);
            assert!(
                app.preminted_class_list().is_some(),
                "cell {i} must stay on the premint path (overrides: {:?})",
                app.overrides
            );
            let r = runtime_core::resolve_style(&app);
            if px_of(&r.left).is_some_and(|l| l > 0.0) {
                saw_offset = true;
            }
        }
        assert!(saw_offset, "the second frozen column still resolves its offset");

        // The scroll-y surface's `max_height` cap rides the same layer.
        use runtime_vocabulary::prims::{PrimCell, ViewPrim};
        let t = Table(TableProps {
            children: vec![row_of(vec![head_cell("Name")]), row_of(vec![body_cell("Ana")])],
            scroll_y: true,
            max_height: Some(300.0),
            ..Default::default()
        });
        let surface = match peel_owned_keepalive(t) {
            Element::Item { data, .. } => data.downcast_ref::<PrimCell<ViewPrim>>().expect("surface view").take(),
            _ => panic!("a scrolling table lowers to its surface view"),
        };
        match surface.style.expect("surface carries the Table sheet") {
            runtime_vocabulary::StyleProp::Sheet(app) => {
                assert!(app.preminted_class_list().is_some(), "the capped surface stays premintable");
                assert_eq!(px_of(&runtime_core::resolve_style(&app).max_height), Some(300.0));
            }
            _ => panic!("the Table sheet is a static application"),
        }
    });
}


/// A `fill` column takes the width the others leave: the table lays out
/// as grid tracks, the fixed column exact and the fill column `1fr`.
#[test]
fn column_model_fill_column_lays_the_table_out_as_tracks() {
    use runtime_core::TrackSize;
    with_test_world(|| {
        let t = Table(TableProps {
            columns: vec![TableColumn::new("name").width(160.0), TableColumn::new("notes").fill().min_width(80.0)],
            children: vec![row_of(vec![head_cell("Name"), head_cell("Notes")])],
            ..Default::default()
        });
        let rules = grid_rules_of(t);
        assert_eq!(
            rules.grid_template_columns,
            Some(vec![
                TrackSize::Px(160.0),
                TrackSize::Minmax(Box::new(TrackSize::Px(80.0)), Box::new(TrackSize::Fr(1.0))),
            ])
        );
    });
}

/// A resizable column's header carries the drag handle (revealed by the
/// header's hover, so the cell reports hover and anchors the handle);
/// its cells follow the width map live — header and body alike — and
/// nothing else in the table re-styles.
#[test]
fn resizable_column_follows_the_width_map_across_header_and_body() {
    with_test_world(|| {
        let widths = signal(ColumnWidths::new());
        let t = Table(TableProps {
            columns: vec![TableColumn::new("name").width(120.0).resizable(), TableColumn::new("notes")],
            column_widths: Some(widths),
            children: vec![
                row_of(vec![head_cell("Name"), head_cell("Notes")]),
                row_of(vec![body_cell("Ana"), body_cell("x")]),
            ],
            ..Default::default()
        });
        let mut cells = grid_cells(t).into_iter();
        let head = cells.next().unwrap();
        let notes_head = cells.next().unwrap();
        let body = cells.next().unwrap();
        let head_view = view_prim(head);
        assert!(head_view.on_hover.is_some(), "the header reports hover to reveal its handle");
        let head_style = match head_view.style {
            Some(StyleProp::SheetDynamic(f)) => f,
            _ => panic!("a resizable column's header follows the width map"),
        };
        let head_app = head_style();
        assert_eq!(head_app.variants.0.get("anchor").map(String::as_str), Some("on"));
        assert_eq!(
            runtime_core::resolve_style(&head_app).position,
            Some(runtime_core::Position::Relative),
            "the header cell anchors the absolutely-placed handle"
        );
        let body_style = match classify(cell_node(body)) {
            P::View { style, .. } => style.expect("styled"),
            _ => panic!("view"),
        };
        let w = |app: StyleApplication| px_of(&runtime_core::resolve_style(&app).width);
        assert_eq!(w(head_style()), Some(120.0), "the spec width until something resizes it");
        assert_eq!(w(body_style.application()), Some(120.0));
        let mut next = ColumnWidths::new();
        next.insert(ColumnId::from("name"), 200.0);
        widths.set(next);
        idea_theme::testing::commit();
        assert_eq!(w(head_style()), Some(200.0), "the header follows the drag");
        assert_eq!(w(body_style.application()), Some(200.0), "and so does every body cell");
        let notes = match classify(cell_node(notes_head)) {
            P::View { style, .. } => style.expect("styled"),
            _ => panic!("view"),
        };
        assert!(!notes.is_reactive(), "a column that is not resizable stays static");
    });
}

/// The handle's gesture: a drag writes the column's entry on every move
/// (clamped to its bounds) and reports the width on release; a press
/// without movement reports nothing; a double-click resets the column to
/// its spec; a cancelled drag restores what it found.
#[test]
fn resize_handle_drags_reports_resets_and_cancels() {
    use runtime_core::TouchPhase::{Began, Cancelled, Ended, Moved};
    with_test_world(|| {
        let widths = signal(ColumnWidths::new());
        let heard: Rc<std::cell::RefCell<Vec<Option<f32>>>> = Default::default();
        let sink = heard.clone();
        let t = Table(TableProps {
            columns: vec![TableColumn::new("name").width(120.0).min_width(60.0).max_width(300.0).resizable()],
            column_widths: Some(widths),
            on_column_resize: Some(Rc::new(move |id: ColumnId, w| {
                assert_eq!(id.as_str(), "name");
                sink.borrow_mut().push(w);
            })),
            children: vec![row_of(vec![head_cell("Name")])],
            ..Default::default()
        });
        let head = grid_cells(t).remove(0);
        let handle = cell_content(head).pop().expect("the handle is the header's last child");
        let on_touch = view_prim(handle).on_touch.expect("the handle takes the drag");
        let entry = || widths.with_untracked(|w| w.get(&ColumnId::from("name")).copied());

        // A press without movement: nothing written, nothing reported.
        let send = |phase, x, t| {
            on_touch(&touch(phase, x, t));
            idea_theme::testing::commit();
        };
        send(Began, 100.0, 1_000_000_000);
        send(Ended, 100.0, 1_010_000_000);
        assert_eq!(entry(), None);
        assert!(heard.borrow().is_empty());

        // A drag from the spec width (nothing laid out to measure here).
        send(Began, 100.0, 2_000_000_000);
        send(Moved, 140.0, 2_010_000_000);
        assert_eq!(entry(), Some(160.0), "live: the entry moves with the pointer");
        send(Moved, 900.0, 2_020_000_000);
        assert_eq!(entry(), Some(300.0), "clamped to the column's max_width");
        send(Ended, 900.0, 2_030_000_000);
        assert_eq!(*heard.borrow(), vec![Some(300.0)], "released: the width is reported");

        // A cancelled drag puts the entry back.
        send(Began, 0.0, 5_000_000_000);
        send(Moved, -500.0, 5_010_000_000);
        assert_eq!(entry(), Some(60.0), "clamped to the column's floor");
        send(Cancelled, -500.0, 5_020_000_000);
        assert_eq!(entry(), Some(300.0), "cancel restores");

        // A double-click resets to the spec width and reports `None`.
        send(Began, 0.0, 9_000_000_000);
        send(Ended, 0.0, 9_050_000_000);
        send(Began, 0.0, 9_200_000_000);
        assert_eq!(entry(), None, "reset: the entry is gone, so the spec width applies");
        assert_eq!(heard.borrow().last(), Some(&None));
    });
}

/// Frozen columns in the model inset by the widths of the frozen columns
/// between them and their edge, and only the innermost draws the frozen
/// block's edge. The actions preset is frozen right, compact, fixed.
#[test]
fn frozen_columns_inset_by_the_widths_before_them() {
    with_test_world(|| {
        let t = Table(TableProps {
            scroll_x: true,
            columns: vec![
                TableColumn::new("a").width(120.0).pinned(ColumnPin::Left),
                TableColumn::new("b").width(80.0).pinned(ColumnPin::Left),
                TableColumn::new("c"),
                TableColumn::actions("menu"),
            ],
            children: vec![row_of(vec![body_cell("1"), body_cell("2"), body_cell("3"), body_cell("⋮")])],
            ..Default::default()
        });
        let cells: Vec<_> = any_grid_cells(t, true, false, true).into_iter().map(cell_app).collect();
        let r: Vec<_> = cells.iter().map(runtime_core::resolve_style).collect();
        assert_eq!(px_of(&r[0].left), Some(0.0));
        assert_eq!(px_of(&r[1].left), Some(120.0), "b clears a");
        assert_eq!(cells[0].variants.0.get("pin_edge").map(String::as_str), Some("interior"));
        assert!(is_transparent(&r[0].border_right_color), "a's edge is inside the frozen block");
        assert!(!is_transparent(&r[1].border_right_color), "b draws the frozen block's edge");
        assert_eq!(r[2].position, None);
        assert_eq!(r[3].position, Some(runtime_core::Position::Sticky));
        assert_eq!(px_of(&r[3].right), Some(0.0), "the actions column is frozen right");
        assert_eq!(px_of(&r[3].width), Some(ACTIONS_COLUMN_WIDTH_PX));
        let lg = px_of(&rules_of(body_cell("x")).padding_left).unwrap();
        assert!(px_of(&r[3].padding_left).unwrap() < lg, "compact icon padding");
        assert_eq!(r[3].text_align, Some(runtime_core::TextAlign::Center));
    });
}

/// CrewForge (item 4): the row menu was a hand-built cell on the raw SDK.
/// `TableCell(padding = CellPadding::Icon, pinned = Right, width = …)` is
/// the same column without a column model.
#[test]
fn regression_actions_cell_is_a_table_cell() {
    with_test_world(|| {
        let r = rules_of(TableCell(TableCellProps {
            pinned: Some(ColumnPin::Right),
            width: Reactive::Static(Some(ACTIONS_COLUMN_WIDTH_PX)),
            padding: CellPadding::Icon,
            align: CellAlign::Center,
            ..Default::default()
        }));
        assert_eq!(r.position, Some(runtime_core::Position::Sticky));
        assert_eq!(px_of(&r.width), Some(ACTIONS_COLUMN_WIDTH_PX));
        let lg = px_of(&rules_of(body_cell("x")).padding_left).unwrap();
        assert!(px_of(&r.padding_left).unwrap() < lg);
        assert!(cell_app(TableCell(TableCellProps { padding: CellPadding::Icon, ..Default::default() }))
            .preminted_class_list()
            .is_some(), "the padding arm premints");
    });
}

/// The selection and grip columns freeze with a pinned first column, at
/// their fixed widths, and the author's frozen column clears them both.
#[test]
fn added_columns_freeze_with_a_pinned_first_column() {
    with_test_world(|| {
        idea_theme::theme::install_idea_theme(crate::light_theme());
        let t = Table(TableProps {
            scroll_x: true,
            columns: vec![TableColumn::new("name").width(160.0).pinned(ColumnPin::Left), TableColumn::new("x")],
            selection: Some(signal(BTreeSet::new())),
            on_reorder: Some(Rc::new(|_, _| {})),
            children: vec![
                row_of(vec![head_cell("Name"), head_cell("X")]),
                id_row("a", &["Ana", "1"], true, true),
            ],
            ..Default::default()
        });
        let cells: Vec<_> =
            any_grid_cells(t, true, false, true).into_iter().filter(|c| !is_backdrop(c)).collect();
        assert_eq!(cells.len(), 8, "grip + select + 2 columns, two rows");
        let r: Vec<_> = cells.into_iter().map(|c| runtime_core::resolve_style(&cell_app(c))).collect();
        for row in [0, 4] {
            assert_eq!(px_of(&r[row].left), Some(0.0), "the grip pins first");
            assert_eq!(px_of(&r[row].width), Some(GRIP_COLUMN_WIDTH_PX));
            assert_eq!(px_of(&r[row + 1].left), Some(GRIP_COLUMN_WIDTH_PX), "then the selection");
            assert_eq!(px_of(&r[row + 1].width), Some(SELECT_COLUMN_WIDTH_PX));
            assert_eq!(
                px_of(&r[row + 2].left),
                Some(GRIP_COLUMN_WIDTH_PX + SELECT_COLUMN_WIDTH_PX),
                "the author's frozen column clears both"
            );
            assert!(is_transparent(&r[row].border_right_color), "no edge inside the frozen block");
            assert!(is_transparent(&r[row + 1].border_right_color));
        }
    });
}

/// Without a column model the added columns still freeze with a pinned
/// first cell, and the author's own `pin_offset`s are moved past them.
#[test]
fn added_columns_shift_author_pin_offsets() {
    with_test_world(|| {
        idea_theme::theme::install_idea_theme(crate::light_theme());
        let pinned = |text: &str, offset: Option<f32>| {
            TableCell(TableCellProps {
                text: Reactive::Static(Some(text.into())),
                pinned: Some(ColumnPin::Left),
                pin_offset: offset,
                width: Reactive::Static(Some(100.0)),
                ..Default::default()
            })
        };
        let t = Table(TableProps {
            scroll_x: true,
            selection: Some(signal(BTreeSet::new())),
            children: vec![TableRow(TableRowProps {
                children: vec![pinned("a", None), pinned("b", Some(100.0))],
                id: Some(RowId::from("r")),
                ..Default::default()
            })],
            ..Default::default()
        });
        let r: Vec<_> = any_grid_cells(t, true, false, true).into_iter().map(rules_of).collect();
        assert_eq!(px_of(&r[0].left), Some(0.0));
        assert_eq!(px_of(&r[1].left), Some(SELECT_COLUMN_WIDTH_PX));
        assert_eq!(px_of(&r[2].left), Some(SELECT_COLUMN_WIDTH_PX + 100.0));
    });
}

/// CrewForge (item 2): a selection column of hand-built cells. The table
/// adds it: a box in the header and in each row with an id that is
/// selectable; other rows get an empty cell, so every row keeps its
/// columns.
#[test]
fn regression_selection_column_boxes_only_selectable_rows() {
    with_test_world(|| {
        idea_theme::theme::install_idea_theme(crate::light_theme());
        let selection = signal(BTreeSet::new());
        let t = Table(TableProps {
            selection: Some(selection),
            children: vec![
                row_of(vec![head_cell("Name")]),
                id_row("a", &["Ana"], true, true),
                id_row("b", &["Bo"], false, true),
                row_of(vec![body_cell("no id")]),
            ],
            ..Default::default()
        });
        let cells = grid_cells(t);
        assert_eq!(cells.len(), 8);
        let boxes: Vec<usize> = cells
            .into_iter()
            .step_by(2)
            .map(|c| cell_content(c).into_iter().map(pressables).map(|p| p.len()).sum())
            .collect();
        assert_eq!(boxes, vec![1, 1, 0, 0], "header and the selectable row");
    });
}

/// The header box is tri-state over the visible selectable rows and
/// toggles exactly those, keeping ids from elsewhere (another page).
#[test]
fn select_all_box_toggles_the_visible_rows() {
    with_test_world(|| {
        idea_theme::theme::install_idea_theme(crate::light_theme());
        let selection = signal(BTreeSet::from([RowId::from("elsewhere")]));
        let t = Table(TableProps {
            selection: Some(selection),
            children: vec![
                row_of(vec![head_cell("Name")]),
                id_row("a", &["Ana"], true, true),
                id_row("b", &["Bo"], true, true),
                id_row("c", &["Cy"], false, true),
            ],
            ..Default::default()
        });
        let mut cells = grid_cells(t).into_iter();
        let header_box = cell_content(cells.next().unwrap()).into_iter().flat_map(pressables).next().unwrap();
        cells.next();
        let a_box = cell_content(cells.next().unwrap()).into_iter().flat_map(pressables).next().unwrap();
        a_box();
        idea_theme::testing::commit();
        assert_eq!(selection.get(), BTreeSet::from([RowId::from("a"), RowId::from("elsewhere")]));
        header_box();
        idea_theme::testing::commit();
        assert_eq!(
            selection.get(),
            BTreeSet::from([RowId::from("a"), RowId::from("b"), RowId::from("elsewhere")]),
            "some → all of the visible selectable rows (not c)"
        );
        header_box();
        idea_theme::testing::commit();
        assert_eq!(selection.get(), BTreeSet::from([RowId::from("elsewhere")]), "all → none of them");
    });
}

/// A cell the table adds takes its row's tone and, on a clickable row,
/// the row's hover tint and tap — the whole row lights and answers.
#[test]
fn added_cells_take_the_row_tone_and_click() {
    with_test_world(|| {
        idea_theme::theme::install_idea_theme(crate::light_theme());
        let t = Table(TableProps {
            selection: Some(signal(BTreeSet::new())),
            children: vec![TableRow(TableRowProps {
                children: vec![body_cell("Ana")],
                id: Some(RowId::from("a")),
                tone: Reactive::Static(RowTone::Warning),
                on_row_click: Some(Rc::new(|| {})),
                ..Default::default()
            })],
            ..Default::default()
        });
        let select_cell = grid_cells(t).remove(0);
        match classify(cell_node(select_cell)) {
            P::View { on_touch, on_hover, style, .. } => {
                assert!(on_touch && on_hover, "the row's tap and hover");
                let r = runtime_core::resolve_style(&style.expect("styled").application());
                assert_eq!(bg_token(&r).as_deref(), Some("color-table-row-warning"));
                assert_eq!(r.cursor, Some(runtime_core::Cursor::Pointer));
            }
            _ => panic!("view"),
        }
    });
}

/// CrewForge (item 6): the blank row counted the columns by hand (select,
/// grip and actions included) for its colspan. With no body rows the
/// table adds one row: empty cells under its added columns, then the
/// message spanning every author column.
#[test]
fn regression_empty_state_spans_past_the_added_columns() {
    with_test_world(|| {
        idea_theme::theme::install_idea_theme(crate::light_theme());
        let t = Table(TableProps {
            selection: Some(signal(BTreeSet::new())),
            empty: Some("Nobody yet".into()),
            children: vec![row_of(vec![head_cell("A"), head_cell("B"), head_cell("C")])],
            ..Default::default()
        });
        let cells = grid_cells(t);
        assert_eq!(cells.len(), 6, "4 header cells, then the blank select cell and the message");
        let mut it = cells.into_iter().skip(4);
        assert!(cell_content(it.next().unwrap()).is_empty());
        let message = it.next().unwrap();
        assert_eq!(rules_of(message).grid_column, Some(runtime_core::GridPlacement::Lines(2, 5)));
        let t = Table(TableProps {
            empty: Some("Nobody yet".into()),
            children: vec![row_of(vec![head_cell("A")])],
            ..Default::default()
        });
        let text = cell_text_rules(grid_cells(t).pop().unwrap());
        let muted = crate::light_theme().colors.text_muted.value().0.to_ascii_lowercase();
        assert_eq!(text.color.as_ref().map(|c| c.resolve().0.to_ascii_lowercase()), Some(muted));
    });
}

#[test]
fn empty_state_stays_out_of_a_table_with_rows() {
    with_test_world(|| {
        let t = Table(TableProps {
            empty: Some("Nobody yet".into()),
            children: vec![row_of(vec![head_cell("A")]), row_of(vec![body_cell("Ana")])],
            ..Default::default()
        });
        assert_eq!(grid_cells(t).len(), 2);
    });
}

/// CrewForge (item 7): a breakpoint change in the page margin rebuilt the
/// whole table, because `inset_x` was static. A live inset re-pads the
/// edge cells in place; the cells between keep their static style.
#[test]
fn regression_live_inset_repads_edge_cells_without_a_rebuild() {
    with_test_world(|| {
        let inset = signal(TableInset::Px(16.0));
        let t = Table(TableProps {
            inset_x: Reactive::Dynamic(Rc::new(move || inset.get())),
            header_slot: Some(runtime_core::text("note".to_string()).into_element()),
            children: vec![row_of(vec![body_cell("1"), body_cell("2"), body_cell("3")])],
            ..Default::default()
        });
        let mut surface = match peel_owned_keepalive(t) {
            Element::Item { children, .. } => children,
            _ => panic!("surface"),
        };
        let holder = surface.pop().expect("grid holder");
        let slot = surface.pop().expect("header slot");
        let mut cells = grid_cells(holder).into_iter();
        let first = match classify(cell_node(cells.next().unwrap())) {
            P::View { style, .. } => style.unwrap(),
            _ => panic!("view"),
        };
        let middle = match classify(cell_node(cells.next().unwrap())) {
            P::View { style, .. } => style.unwrap(),
            _ => panic!("view"),
        };
        let slot = match classify(slot) {
            P::View { style, .. } => style.unwrap(),
            _ => panic!("view"),
        };
        let pad = |s: &crate::test_support::TStyle| px_of(&s.resolve().padding_left);
        assert!(first.is_reactive() && !middle.is_reactive());
        assert_eq!(pad(&first), Some(16.0));
        assert_eq!(pad(&slot), Some(16.0));
        inset.set(TableInset::Px(28.0));
        idea_theme::testing::commit();
        assert_eq!(pad(&first), Some(28.0), "the edge cell re-pads in place");
        assert_eq!(pad(&slot), Some(28.0), "and so does the slot");
        inset.set(TableInset::Xl);
        idea_theme::testing::commit();
        assert_eq!(pad(&first), Some(crate::light_theme().spacing.xl), "token arms too");
    });
}

/// CrewForge (item 5): sort headers were hand-built. A header cell with
/// `sort` draws the direction beside its label, is a press target, and
/// asks for the next sort.
#[test]
fn regression_sort_header_draws_direction_and_asks_for_the_next() {
    use runtime_core::TouchPhase::{Began, Ended};
    with_test_world(|| {
        let asked: Rc<std::cell::Cell<Option<SortState>>> = Default::default();
        let sink = asked.clone();
        let sort = signal(Some(SortState::Asc));
        let cell = TableCell(TableCellProps {
            header: true,
            text: Reactive::Static(Some("Name".into())),
            sort: Reactive::Dynamic(Rc::new(move || sort.get())),
            on_sort: Some(Rc::new(move |s| sink.set(Some(s)))),
            ..Default::default()
        });
        let view = view_prim(cell);
        let app = match view.style {
            Some(StyleProp::Sheet(app)) => *app,
            _ => panic!("a static sheet"),
        };
        assert_eq!(runtime_core::resolve_style(&app).cursor, Some(runtime_core::Cursor::Pointer));
        let on_touch = view.on_touch.expect("the header is a press target");
        on_touch(&touch(Began, 5.0, 1));
        on_touch(&touch(Ended, 5.0, 2));
        assert_eq!(asked.get(), Some(SortState::Desc), "ascending asks for descending");
        sort.set(Some(SortState::Desc));
        idea_theme::testing::commit();
        on_touch(&touch(Began, 5.0, 3));
        on_touch(&touch(Ended, 5.0, 4));
        assert_eq!(asked.get(), Some(SortState::Asc));
    });
}

#[test]
fn sort_state_cycles_asc_desc() {
    assert_eq!(SortState::None.next(), SortState::Asc);
    assert_eq!(SortState::Asc.next(), SortState::Desc);
    assert_eq!(SortState::Desc.next(), SortState::Asc);
}

/// CrewForge (item 9): an overdue date needed app CSS. `TableCell(tone =
/// …)` colours one value — on the cell and its text — and leaves the row
/// alone.
#[test]
fn regression_cell_tone_colours_one_value() {
    with_test_world(|| {
        let make = || {
            TableCell(TableCellProps {
                text: Reactive::Static(Some("Overdue".into())),
                tone: Reactive::Static(CellTone::Danger),
                ..Default::default()
            })
        };
        let danger = crate::light_theme().intents.danger.fg.value().0.to_ascii_lowercase();
        let colour = |r: &StyleRules| r.color.as_ref().map(|c| c.resolve().0.to_ascii_lowercase());
        assert_eq!(colour(&cell_text_rules(make())), Some(danger.clone()), "the text node");
        let app = cell_app(make());
        assert!(app.preminted_class_list().is_some(), "a static tone premints");
        assert_eq!(colour(&runtime_core::resolve_style(&app)), Some(danger), "the cell (rich children inherit it on web)");
        assert!(is_transparent(&runtime_core::resolve_style(&app).background), "no row tint");
    });
}

/// A landed (deep-linked) row is a `RowTone::Highlight`, and it re-tints
/// in place when the landing moves — no app CSS.
#[test]
fn landed_row_is_a_reactive_highlight_tone() {
    with_test_world(|| {
        let landed = signal(false);
        let cells = row_cells(TableRow(TableRowProps {
            children: vec![body_cell("x")],
            tone: Reactive::Dynamic(Rc::new(move || if landed.get() { RowTone::Highlight } else { RowTone::None })),
            ..Default::default()
        }));
        let style = match classify(cells.into_iter().next().unwrap()) {
            P::View { style, .. } => style.unwrap(),
            _ => panic!("view"),
        };
        landed.set(true);
        idea_theme::testing::commit();
        assert_eq!(
            bg_token(&runtime_core::resolve_style(&style.application())).as_deref(),
            Some("color-table-row-highlight")
        );
    });
}

/// Hover across a frozen segment on a plain table: a frozen cell takes
/// the row's hover tint like its neighbours, and stays opaque at rest.
#[test]
fn plain_table_hover_reaches_frozen_cells() {
    with_test_world(|| {
        let base = TableStyles::body_cell().into_style_application().with("variant", "plain").with("pinned", "left");
        let rest = runtime_core::resolve_style(&base.clone().with("row_hovered", "off"));
        let hover = runtime_core::resolve_style(&base.with("row_hovered", "on"));
        assert_eq!(bg_token(&rest).as_deref(), Some("color-surface"), "opaque at rest");
        assert_eq!(bg_token(&hover).as_deref(), Some("color-surface-alt"), "tinted with its row");
        let head = TableStyles::head_cell().into_style_application().with("variant", "plain").with("pinned", "left");
        let head_hover = runtime_core::resolve_style(&head.with("row_hovered", "on"));
        assert_eq!(bg_token(&head_hover).as_deref(), Some("color-surface-alt"));
    });
}

/// CrewForge (item 3): the grip was a hand-built cell. A reorderable
/// table adds a grip column: a grip on each draggable body row, an empty
/// cell elsewhere, and every body cell follows the drag.
#[test]
fn regression_reorder_adds_a_grip_to_draggable_rows() {
    with_test_world(|| {
        let t = Table(TableProps {
            on_reorder: Some(Rc::new(|_, _| {})),
            children: vec![
                row_of(vec![head_cell("Name")]),
                id_row("a", &["Ana"], true, true),
                id_row("b", &["Group"], true, false),
                id_row("c", &["Cy"], true, true),
            ],
            ..Default::default()
        });
        // A draggable row binds its proxy surface (the drop target), which
        // on native is a row-spanning backdrop view ahead of its cells:
        // skip those — they carry raw placement rules, not a cell sheet.
        let cells: Vec<_> = grid_cells(t)
            .into_iter()
            .filter_map(|c| match classify(cell_node(c)) {
                P::View { style: Some(crate::test_support::TStyle::Rules(_)), .. } => None,
                P::View { children, on_touch, .. } => Some((children.len(), on_touch)),
                _ => panic!("view"),
            })
            .collect();
        let grips: Vec<_> = cells.into_iter().step_by(2).collect();
        assert_eq!(grips[0], (0, false), "the header's grip cell is empty");
        assert_eq!(grips[1], (1, true), "a draggable row's grip holds the glyph and the drag");
        assert_eq!(grips[2], (0, false), "a row that does not drag has no grip");
        assert_eq!(grips[3], (1, true));
    });
}

/// The table-level axes are open: an app defines its own density, row
/// tone and variant (`TableDensity::define`, …) and gives them a look with
/// a restyle arm on the cell slots. `Table` / `TableRow` select the arm
/// on cells that are already built, and the restyled cells still premint.
/// A head and a body cell are separate slots of one `TableStyles`.
#[test]
fn app_defined_table_axes_reach_built_cells_and_premint() {
    const SPACIOUS: TableDensity = TableDensity::define("spacious");
    const INFO: TableTone = TableTone::define("info");
    with_test_world(|| {
        runtime_core::component_styles::__reset::<TableStyles>();
        idea_theme::theme::install_idea_theme(crate::light_theme());
        TableStyles::restyle(|s| {
            s.head_cell().arm(SPACIOUS, |t| StyleRules {
                padding_top: Some(t.spacing.xl()),
                padding_bottom: Some(t.spacing.xl()),
                ..Default::default()
            });
            s.body_cell().arm(SPACIOUS, |t| StyleRules {
                padding_top: Some(t.spacing.xl()),
                padding_bottom: Some(t.spacing.xl()),
                ..Default::default()
            });
            s.body_cell().arm(INFO, |t| StyleRules {
                background: Some(t.intent.info.soft_bg()),
                ..Default::default()
            });
        });
        let head = TableRow(TableRowProps {
            children: vec![TableCell(TableCellProps {
                header: true,
                text: Reactive::Static(Some("h".into())),
                ..Default::default()
            })],
            ..Default::default()
        });
        let body = TableRow(TableRowProps {
            children: vec![body_cell("b")],
            tone: Reactive::Static(INFO),
            ..Default::default()
        });
        let cells = grid_cells(Table(TableProps {
            children: vec![head, body],
            density: SPACIOUS,
            ..Default::default()
        }));
        let xl = crate::light_theme().spacing.xl;
        let mut apps = cells.into_iter().map(cell_app);
        let head_app = apps.next().expect("a head cell");
        let body_app = apps.next().expect("a body cell");
        for app in [&head_app, &body_app] {
            assert!(app.preminted_class_list().is_some(), "a restyled cell premints");
            assert_eq!(px_of(&runtime_core::resolve_style(app).padding_top), Some(xl));
        }
        assert_eq!(
            bg_token(&runtime_core::resolve_style(&body_app)).as_deref(),
            Some("intent-info-soft-bg"),
            "the app's row tone paints the body cell"
        );
        runtime_core::component_styles::__reset::<TableStyles>();
    });
}
