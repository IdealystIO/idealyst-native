//! Scene-lowering suite: the authored surface —
//! `table(TableProps { … }).with_style(…)` / `table_row` / `table_cell`
//! — exercised against both lowerings:
//!
//! - the NATIVE grid lowering (host target): rows flatten to cells
//!   under one `display: grid` view item, column tracks = widest row,
//!   `Owned` row scopes survive the flattening;
//! - the WEB `<table>` lowering: the scene-Registry handlers
//!   ([`table::register`]) drive a real caps-complete host
//!   (`backend_ssr::newcore`) and must emit genuine
//!   `<table>`/`<tr>`/`<th>`/`<td>` markup.

use std::cell::Cell;
use std::rc::Rc;

use runtime_scene::{component_scope, Element};
use runtime_vocabulary::glue::{self, resolve_style, DisplayKind, IntoElement};
use runtime_vocabulary::prims::{PrimCell, ViewPrim};
use runtime_vocabulary::style_attach::StyleProp;
use runtime_world::World;
use table::{
    bind_row, cell_base_application, item_lowering, map_cell_style, set_cell_interaction,
    set_cell_style, table, table_cell, table_row, TableCellPrim, TableCellProps, TableProps,
    TableRowPrim, TableRowProps,
};

// ===========================================================================
// Shared builders
// ===========================================================================

fn cell() -> Element {
    table_cell(TableCellProps::default()).into_element()
}

fn row(n: usize) -> Element {
    let cells: Vec<Element> = (0..n).map(|_| cell()).collect();
    table_row(TableRowProps { children: cells }).into_element()
}

/// Downcast an `Element::Item`'s payload as a view prim and read it out
/// (the payload is single-use, so reading = taking; fine in tests).
fn take_view_prim(el: &Element) -> ViewPrim {
    match el {
        Element::Item { data, .. } => data
            .downcast_ref::<PrimCell<ViewPrim>>()
            .expect("native table nodes lower to view prims")
            .take(),
        _ => panic!("expected an Element::Item"),
    }
}

fn item_children(el: Element) -> Vec<Element> {
    match el {
        Element::Item { children, .. } => children,
        _ => panic!("expected an Element::Item"),
    }
}

// ===========================================================================
// Native grid lowering
// ===========================================================================

/// A row produces no layout box of its own — it lowers to a
/// `TableRowPrim` MARKER item whose children are the cells; the parent
/// table consumes the marker and flattens the cells into a single
/// grid. (Until the row-proxy work this was a bare `Fragment`, which
/// could carry no row-level slots.)
#[test]
fn table_row_lowers_to_row_marker_of_cells() {
    match row(3) {
        Element::Item { data, children } => {
            assert!(
                data.downcast_ref::<PrimCell<TableRowPrim>>().is_some(),
                "native row payload is the TableRowPrim marker"
            );
            assert_eq!(children.len(), 3);
        }
        _ => panic!("table_row must lower to a TableRowPrim marker item"),
    }
}

/// The table lowers to an outer passthrough view wrapping an inner grid
/// item: every cell from every row is a direct child of the grid, and
/// the grid's style resolves to `display: grid` with one column track
/// per column — the shared `StyleRules` grid props flowing through the
/// style prop unchanged.
#[test]
fn table_lowers_to_grid_with_flattened_cells() {
    let t = table(TableProps {
        children: vec![row(3), row(3), row(3)],
        ..Default::default()
    })
    .into_element();

    // Outer node is a passthrough view item wrapping exactly the grid.
    let mut outer_children = item_children(t);
    assert_eq!(outer_children.len(), 1, "outer wraps exactly the inner grid");
    let inner = outer_children.pop().expect("len checked");

    let prim = take_view_prim(&inner);
    let app = match prim.style {
        Some(StyleProp::Sheet(app)) => app,
        _ => panic!("the grid sheet is constant → StyleProp::Sheet"),
    };
    let rules = resolve_style(&app);
    assert_eq!(
        rules.display,
        Some(DisplayKind::Grid),
        "the inner node lays its children out as a grid"
    );
    assert_eq!(
        rules.grid_template_columns.as_ref().map(|c| c.len()),
        Some(3),
        "one column track per table column"
    );
    assert_eq!(
        item_children(inner).len(),
        9,
        "all 9 cells become direct children of the grid"
    );
}

/// A ragged table (rows of differing length) sizes its column track
/// list to the widest row, so short rows leave trailing columns empty
/// rather than the grid losing a column.
#[test]
fn table_column_count_is_widest_row() {
    let t = table(TableProps {
        children: vec![row(2), row(4), row(3)],
        ..Default::default()
    })
    .into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let prim = take_view_prim(&inner);
    let app = match prim.style {
        Some(StyleProp::Sheet(app)) => app,
        _ => panic!("Sheet"),
    };
    assert_eq!(
        resolve_style(&app)
            .grid_template_columns
            .as_ref()
            .map(|c| c.len()),
        Some(4),
        "column count tracks the widest row (4)"
    );
}

/// REGRESSION GUARD for the flattening ⇄ `#[component]` interaction: a
/// row whose component body created reactive state arrives as
/// `Owned { Fragment(cells), scope }`. Flattening must peel the wrapper
/// to reach the cells AND keep the scope alive by re-attaching it
/// around the finished table — dropping it would free the row's shared
/// hover signal while the cells' reactive styles still read it.
#[test]
fn owned_row_scope_survives_grid_flattening() {
    let world = World::new();
    world.enter(|| {
        // A component-shaped row: creates a signal (so the collected
        // scope is non-empty), returns the fragment of cells.
        let owned_row = component_scope(|| {
            let hovered = glue::signal(false);
            let _keep = hovered; // the row's shared state
            row(2)
        });
        assert!(
            matches!(owned_row, Element::Owned { .. }),
            "precondition: the component row is Owned-wrapped"
        );

        let t = table(TableProps {
            children: vec![owned_row, row(2)],
            ..Default::default()
        })
        .into_element();

        // The peeled scope is re-attached around the table element.
        let (inner_el, kept_scopes) = peel_owned(t);
        assert_eq!(
            kept_scopes, 1,
            "exactly the one peeled row scope is re-attached (not dropped)"
        );
        let inner = item_children(inner_el).pop().expect("outer wraps grid");
        assert_eq!(
            item_children(inner).len(),
            4,
            "the Owned row's cells still flatten into the grid"
        );
    });
}

/// Unwrap nested `Owned` layers, counting them.
fn peel_owned(mut el: Element) -> (Element, usize) {
    let mut n = 0;
    loop {
        match el {
            Element::Owned { element, .. } => {
                n += 1;
                el = *element;
            }
            other => return (other, n),
        }
    }
}

// ===========================================================================
// Built-cell post-processing helpers (the idea-ui clickable-row seam)
// ===========================================================================

/// The helpers must reach the payload through BOTH lowerings: the
/// native grid cell (a `ViewPrim` item) and the web cell (a
/// [`TableCellPrim`] item). If either downcast arm is dropped, idea-ui's
/// clickable rows silently lose their hover highlight / tap handler on
/// that target.
#[test]
fn cell_helpers_reach_both_lowerings() {
    // --- native shape (host constructor arm) ---
    let native_cell = cell();
    assert!(
        cell_base_application(&native_cell).is_some(),
        "native cell carries its default sheet application"
    );
    let base = cell_base_application(&native_cell).expect("checked");
    set_cell_style(&native_cell, move || base.clone());
    set_cell_interaction(
        &native_cell,
        Rc::new(|_ev| runtime_shared::TouchResponse::default()),
        Rc::new(|_entering| {}),
    );
    let prim = take_view_prim(&native_cell);
    assert!(
        matches!(prim.style, Some(StyleProp::SheetDynamic(_))),
        "set_cell_style upgraded the native cell to a reactive sheet"
    );
    assert!(prim.on_touch.is_some(), "native cell got the tap handler");
    assert!(prim.on_hover.is_some(), "native cell got the hover handler");

    // --- web shape (item_lowering, what the wasm32 arm emits) ---
    let sheet = std::rc::Rc::new(glue::StyleSheet::new(|_vs: &glue::VariantSet| {
        glue::StyleRules::default()
    }));
    let web_cell = item_lowering::cell_item(
        false,
        Vec::new(),
        Some(StyleProp::Sheet(Box::new(glue::StyleApplication::new(sheet)))),
    );
    let base = cell_base_application(&web_cell).expect("web cell base application");
    set_cell_style(&web_cell, move || base.clone());
    set_cell_interaction(
        &web_cell,
        Rc::new(|_ev| runtime_shared::TouchResponse::default()),
        Rc::new(|_entering| {}),
    );
    match &web_cell {
        Element::Item { data, .. } => {
            let prim = data
                .downcast_ref::<PrimCell<TableCellPrim>>()
                .expect("web cell payload")
                .take();
            assert!(matches!(prim.style, Some(StyleProp::SheetDynamic(_))));
            assert!(prim.on_touch.is_some(), "web cell got the tap handler");
            assert!(prim.on_hover.is_some(), "web cell got the hover handler");
        }
        _ => panic!("web cell must be an Item"),
    }
}


/// A cell styled REACTIVELY must still take a row overlay's axes.
///
/// [`cell_base_application`] only ever sees a static `StyleProp::Sheet`,
/// so the read-then-replace pair skipped such a cell in SILENCE — which
/// is how a width-pinned column (its width moves under a resize drag, so
/// its style has to be a closure) became the one column in a clickable
/// row that never lit up on hover, with nothing logged. `map_cell_style`
/// composes over the closure instead of replacing it, so the cell keeps
/// its own reactive width AND takes the row's highlight.
#[test]
fn map_cell_style_composes_over_a_reactive_cell_style() {
    let sheet = Rc::new(glue::StyleSheet::new(|_vs: &glue::VariantSet| {
        glue::StyleRules::default()
    }));
    let pinned = glue::StyleApplication::new(sheet).with("pinned", "left");
    let cell = item_lowering::cell_item(
        false,
        Vec::new(),
        Some(StyleProp::SheetDynamic(Box::new(move || pinned.clone()))),
    );

    assert!(
        cell_base_application(&cell).is_none(),
        "a reactive style is invisible to the read-then-replace pair — the gap this closes"
    );
    assert!(
        map_cell_style(&cell, Rc::new(|app: glue::StyleApplication| {
            app.with("row_hovered", "on")
        })),
        "map_cell_style reports that it wrapped the style"
    );

    match &cell {
        Element::Item { data, .. } => {
            let prim = data
                .downcast_ref::<PrimCell<TableCellPrim>>()
                .expect("web cell payload")
                .take();
            let Some(StyleProp::SheetDynamic(f)) = prim.style else {
                panic!("a composed style stays reactive");
            };
            let app = f();
            assert_eq!(
                app.variants.0.get("pinned").map(String::as_str),
                Some("left"),
                "the cell's own axis survives the composition"
            );
            assert_eq!(
                app.variants.0.get("row_hovered").map(String::as_str),
                Some("on"),
                "the overlay's axis lands on top of it"
            );
        }
        _ => panic!("web cell must be an Item"),
    }
}

/// A static sheet composes the same way — the path every ordinary
/// `TableCell` in a clickable row takes.
#[test]
fn map_cell_style_composes_over_a_static_cell_style() {
    let native_cell = cell();
    assert!(
        map_cell_style(&native_cell, Rc::new(|app: glue::StyleApplication| {
            app.with("row_hovered", "on")
        })),
        "a static sheet is composable too"
    );
    let prim = take_view_prim(&native_cell);
    let Some(StyleProp::SheetDynamic(f)) = prim.style else {
        panic!("composing makes a static sheet reactive");
    };
    assert_eq!(
        f().variants.0.get("row_hovered").map(String::as_str),
        Some("on")
    );
}

/// A style with no application to compose over — a preminted class, a
/// resolved-rules closure — is left EXACTLY as it was, and the caller is
/// told so. Overwriting it would drop styling the author asked for.
#[test]
fn map_cell_style_leaves_a_non_application_style_alone() {
    let cell = item_lowering::cell_item(
        false,
        Vec::new(),
        Some(StyleProp::Static(Rc::new(glue::StyleRules::default()))),
    );
    assert!(
        !map_cell_style(&cell, Rc::new(|app: glue::StyleApplication| {
            app.with("row_hovered", "on")
        })),
        "nothing to compose with, and the caller hears about it"
    );
    match &cell {
        Element::Item { data, .. } => {
            let prim = data
                .downcast_ref::<PrimCell<TableCellPrim>>()
                .expect("web cell payload")
                .take();
            assert!(
                matches!(prim.style, Some(StyleProp::Static(_))),
                "the author's style is untouched"
            );
        }
        _ => panic!("web cell must be an Item"),
    }
}

// ===========================================================================
// Web `<table>` handlers on a real caps-complete host (SSR)
// ===========================================================================

/// The scene-Registry handlers must emit genuine table markup:
/// `<table>` (with the browser-default reset inline),
/// `<tr>` rows, `<th>` for header cells, `<td>` for body cells, with
/// children realized INSIDE their cells.
#[test]
fn web_handlers_emit_real_table_markup() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::table_item(
            vec![
                item_lowering::row_item(
                    vec![
                        item_lowering::cell_item(
                            true,
                            vec![glue::text("Prop").into_element()],
                            None,
                        ),
                        item_lowering::cell_item(
                            true,
                            vec![glue::text("Type").into_element()],
                            None,
                        ),
                    ],
                    None,
                ),
                item_lowering::row_item(
                    vec![
                        item_lowering::cell_item(
                            false,
                            vec![glue::text("children").into_element()],
                            None,
                        ),
                        item_lowering::cell_item(
                            false,
                            vec![glue::text("Vec<Element>").into_element()],
                            None,
                        ),
                    ],
                    None,
                ),
            ],
            None,
            false,
        )
    });

    let html = &page.html;
    assert!(html.contains("<table"), "outer node is a real <table>: {html}");
    assert!(html.contains("<tr"), "rows are real <tr>: {html}");
    assert!(html.contains("<th"), "header cells are <th>: {html}");
    assert!(html.contains("<td"), "body cells are <td>: {html}");
    assert!(
        html.contains("border-collapse"),
        "browser-default reset is applied inline on the <table>: {html}"
    );
    assert!(
        html.contains("Prop") && html.contains("Vec&lt;Element&gt;") || html.contains("Vec<Element>"),
        "cell children are realized inside their cells: {html}"
    );
    // Structural ordering: the <th> content appears after a <th> open
    // tag (children mount INTO the cell, not beside it).
    let th_at = html.find("<th").expect("th present");
    let prop_at = html.find("Prop").expect("content present");
    assert!(th_at < prop_at, "header content renders inside the <th>");
}

/// A cell with interaction attached must still mount (the handlers'
/// install calls hit the host's `InputOps` — a no-op on SSR, real
/// listeners on web) and the row/cell structure must be unaffected.
#[test]
fn interactive_cell_mounts_through_the_registry() {
    let fired = Rc::new(Cell::new(false));
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        let cell = item_lowering::cell_item(false, vec![glue::text("go").into_element()], None);
        let fired = fired.clone();
        set_cell_interaction(
            &cell,
            Rc::new(move |_ev| {
                fired.set(true);
                runtime_shared::TouchResponse::default()
            }),
            Rc::new(|_| {}),
        );
        item_lowering::table_item(vec![item_lowering::row_item(vec![cell], None)], None, false)
    });
    assert!(page.html.contains("<td"), "interactive cell still renders");
    assert!(!fired.get(), "handler install must not invoke the callback");
}

/// The NATIVE grid lowering realized through the scene + vocabulary
/// built-ins must surface its grid props in the emitted CSS —
/// `display: grid` plus a `grid-template-columns` track list. Guards
/// the whole prop path: builder → `StyleProp::Sheet` → `attach_style` →
/// sheet engine → css emission.
#[test]
fn native_grid_lowering_emits_grid_css_through_scene_realization() {
    let page = backend_ssr::newcore::render_path_with("/", |_| {}, || {
        table(TableProps {
            children: vec![row(3), row(3)],
            ..Default::default()
        })
        .into_element()
    });
    let css = &page.head_css;
    let has_display_grid = css.contains("display:grid") || css.contains("display: grid");
    assert!(
        has_display_grid,
        "grid display must flow into the emitted CSS: {css}"
    );
    assert!(
        css.contains("grid-template-columns"),
        "column tracks must flow into the emitted CSS: {css}"
    );
}

// ===========================================================================
// 1.1.0 registration seams: `defer` / `register_from_chunk`
// ===========================================================================

/// `defer` must be safe to call on a NON-WEB target.
///
/// Only web code-splits, so off-web `defer` installs the handlers eagerly
/// rather than declaring the kinds late-bound. Without that arm a host
/// (or native) app calling `defer` would park every table behind a
/// layout-transparent placeholder forever — no panic, no log, just an
/// invisible table, because no chunk is ever coming to drain it.
///
/// This is the regression test for that arm: the same tree rendered
/// through `defer` must produce the same real table markup `register`
/// produces. A `defer` that only declared (the wasm behavior) would
/// render placeholders here and fail every assertion below.
#[test]
fn defer_registers_eagerly_off_web() {
    let page = backend_ssr::newcore::render_path_with("/", table::defer, || {
        item_lowering::table_item(
            vec![item_lowering::row_item(
                vec![item_lowering::cell_item(
                    true,
                    vec![glue::text("Prop").into_element()],
                    None,
                )],
                None,
            )],
            None,
            false,
        )
    });

    let html = &page.html;
    assert!(
        html.contains("<table"),
        "`defer` installed the real <table> handler off-web: {html}"
    );
    assert!(html.contains("<tr"), "row handler installed by `defer`: {html}");
    assert!(
        html.contains("<th"),
        "cell handler installed by `defer`: {html}"
    );
    assert!(
        html.contains("Prop"),
        "cell children realized, so the item mounted rather than parking: {html}"
    );
}

/// The non-web `register_from_chunk` stub must exist and be inert.
///
/// A `#[component(lazy)]` body calls it unconditionally; the call has to
/// compile on every target. Off-web it must do nothing, because `defer`
/// already registered — queueing a late registration for a kind that was
/// never declared deferred would panic in `register_deferred`.
#[test]
fn register_from_chunk_is_inert_off_web() {
    table::register_from_chunk::<backend_ssr::SsrBackend>();

    // Registration still works afterwards, and rendering is unaffected —
    // proving the stub queued nothing that a later realize would trip on.
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::table_item(
            vec![item_lowering::row_item(
                vec![item_lowering::cell_item(
                    false,
                    vec![glue::text("cell").into_element()],
                    None,
                )],
                None,
            )],
            None,
            false,
        )
    });
    assert!(page.html.contains("<table"), "{}", page.html);
    assert!(page.html.contains("cell"), "{}", page.html);
}

// ===========================================================================
// scroll-x mode + row proxies
// ===========================================================================

/// `scroll_x` lowers to styled surface > HORIZONTAL scroll_view >
/// width-floored content > grid. The surface stays OUTSIDE the
/// scroller (its border/radius must not ride along with the scrolled
/// columns — the "card border clips at the viewport edge" regression),
/// the scroller is horizontal with content height (`flex_grow: 0` —
/// the fill-the-parent seed collapsed it inside a content-sized
/// surface), and the content floors at the scroller's width.
/// Sticky-pinned cells (idea-ui's `pinned` axis) register against the
/// scroll view; without it a frozen column has nothing to pin to.
#[test]
fn scroll_x_lowers_to_surface_wrapping_horizontal_scroller() {
    use runtime_vocabulary::prims::ScrollViewPrim;
    let t = table(TableProps {
        children: vec![row(2)],
        scroll_x: true,
    })
    .into_element();
    // Surface (the author-style target — unstyled here, no style passed).
    let mut surface_children = match t {
        Element::Item { data, children } => {
            assert!(
                data.downcast_ref::<PrimCell<ViewPrim>>().is_some(),
                "scroll_x root is the surface view, OUTSIDE the scroller"
            );
            children
        }
        _ => panic!("scroll_x table must lower to a surface view item"),
    };
    assert_eq!(surface_children.len(), 1);
    // Scroller.
    let mut scroller_children = match surface_children.pop().expect("len checked") {
        Element::Item { data, children } => {
            let prim = data
                .downcast_ref::<PrimCell<ScrollViewPrim>>()
                .expect("surface wraps the scroll_view")
                .take();
            assert!(prim.horizontal, "the scroller must be horizontal");
            let rules = match prim.style {
                Some(StyleProp::Static(r)) => r,
                _ => panic!("scroller carries the static wrapper rules"),
            };
            assert!(
                matches!(rules.flex_grow.as_ref().map(|t| t.resolve()), Some(g) if g == 0.0),
                "scroller must override the fill-the-parent seed (content height)"
            );
            children
        }
        _ => panic!("surface must wrap a scroll_view item"),
    };
    assert_eq!(scroller_children.len(), 1);
    // Width-floored content.
    match scroller_children.pop().expect("len checked") {
        Element::Item { data, .. } => {
            let vp = data
                .downcast_ref::<PrimCell<ViewPrim>>()
                .expect("scroll content is a view")
                .take();
            let rules = match vp.style {
                Some(StyleProp::Static(r)) => r,
                _ => panic!("floor lands on the content as Static rules"),
            };
            assert!(
                matches!(
                    rules.min_width.as_ref().map(|t| t.resolve()),
                    Some(glue::Length::Percent(p)) if (p - 100.0).abs() < f32::EPSILON
                ),
                "scroll content floors at min-width: 100%"
            );
        }
        _ => panic!("scroll content must be a view item"),
    }
}

/// Binding a row (the drag-and-drop geometry seam) turns the native
/// lowering into the PROXY shape: a row-spanning backdrop view is
/// inserted before the bound row's cells (carrying the ref_fill), and
/// EVERY cell in the table gains an explicit `(grid_row, grid_column)`
/// — auto-flow and a spanning item cannot mix (auto placement would
/// skip the occupied row and push every cell down).
#[test]
fn bound_row_emits_backdrop_and_places_every_cell() {
    use runtime_shared::GridPlacement;

    let r0 = row(2);
    bind_row(&r0, |_handle| {});
    let t = table(TableProps {
        children: vec![r0, row(2)],
        ..Default::default()
    })
    .into_element();

    let inner = item_children(t).pop().expect("outer wraps the grid");
    let grid_children = item_children(inner);
    // 1 backdrop + 4 cells.
    assert_eq!(
        grid_children.len(),
        5,
        "one backdrop (bound row) + four cells"
    );

    // Backdrop first: spans all columns of row 1 and carries the
    // ref_fill so the mount hands out its handle.
    let backdrop = take_view_prim(&grid_children[0]);
    assert!(backdrop.ref_fill.is_some(), "backdrop carries the row binding");
    let backdrop_rules = match backdrop.style {
        Some(StyleProp::Static(r)) => r,
        _ => panic!("slotless backdrop style is Static rules"),
    };
    assert_eq!(backdrop_rules.grid_row, Some(GridPlacement::Line(1)));
    assert_eq!(backdrop_rules.grid_column, Some(GridPlacement::SPAN_ALL));

    // Every cell is explicitly placed; cells of row 1 come after the
    // backdrop (paint above it), row 2's cells land on line 2.
    let expect = [(1, 1), (1, 2), (2, 1), (2, 2)];
    for (i, (row_line, col_line)) in expect.iter().enumerate() {
        let prim = take_view_prim(&grid_children[i + 1]);
        let app = match prim.style {
            Some(StyleProp::Sheet(app)) => app,
            _ => panic!("cell {i} keeps its sheet (placement rides overrides)"),
        };
        let rules = resolve_style(&app);
        assert_eq!(
            rules.grid_row,
            Some(GridPlacement::Line(*row_line)),
            "cell {i} grid_row"
        );
        assert_eq!(
            rules.grid_column,
            Some(GridPlacement::Line(*col_line)),
            "cell {i} grid_column"
        );
    }
}

/// A table with NO bound/styled rows keeps the pre-proxy fast path:
/// plain auto-flow, no backdrops, no per-cell placement.
#[test]
fn unbound_rows_keep_the_auto_flow_fast_path() {
    let t = table(TableProps {
        children: vec![row(2), row(2)],
        ..Default::default()
    })
    .into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let grid_children = item_children(inner);
    assert_eq!(grid_children.len(), 4, "no backdrops for slotless rows");
    let prim = take_view_prim(&grid_children[0]);
    let app = match prim.style {
        Some(StyleProp::Sheet(app)) => app,
        _ => panic!("Sheet"),
    };
    assert_eq!(
        resolve_style(&app).grid_row,
        None,
        "auto-flow cells carry no explicit placement"
    );
}

/// Web: `scroll_x` flips the `<table>`'s inline width strategy to
/// `min-width: 100%; width: max-content` and wraps it in the scroller
/// element, so columns overflow sideways instead of squeezing.
#[test]
fn scroll_x_web_width_strategy_and_wrapper() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::table_item(
            vec![item_lowering::row_item(
                vec![item_lowering::cell_item(
                    false,
                    vec![glue::text("wide").into_element()],
                    None,
                )],
                None,
            )],
            None,
            true,
        )
    });
    let html = &page.html;
    assert!(
        html.contains("min-width: 100%") || html.contains("min-width:100%"),
        "scroll-x table floors at the scroller's width: {html}"
    );
    assert!(
        html.contains("max-content"),
        "scroll-x table lays columns at natural width: {html}"
    );
    assert!(
        !html.contains("width: 100%;") || html.contains("min-width: 100%"),
        "the fill-and-wrap width strategy must be replaced, not stacked: {html}"
    );
    // The collapse border model keeps cell borders on the table's own
    // (scrolled) paint layer, so a sticky cell's background rode the
    // pin OVER its own hairlines — dividers vanished white-on-white.
    // Scroll-x tables must paint borders per cell.
    assert!(
        html.contains("border-collapse: separate") || html.contains("border-collapse:separate"),
        "scroll-x table must use the separate border model: {html}"
    );
    assert!(
        !html.contains("border-collapse: collapse"),
        "collapse must be replaced in scroll-x mode, not stacked: {html}"
    );
}

/// Web: a bound row hands out the `<tr>`'s handle at mount — the
/// row-proxy geometry seam on the `<table>` lowering.
#[test]
fn bound_row_fills_handle_on_web_mount() {
    let filled = Rc::new(Cell::new(false));
    let filled_in = filled.clone();
    let page = backend_ssr::newcore::render_path_with("/", table::register, move || {
        let r = item_lowering::row_item(
            vec![item_lowering::cell_item(
                false,
                vec![glue::text("x").into_element()],
                None,
            )],
            None,
        );
        let filled = filled_in.clone();
        bind_row(&r, move |_handle| filled.set(true));
        item_lowering::table_item(vec![r], None, false)
    });
    assert!(page.html.contains("<tr"), "{}", page.html);
    assert!(
        filled.get(),
        "mount must hand the <tr> handle to the row binding"
    );
}
