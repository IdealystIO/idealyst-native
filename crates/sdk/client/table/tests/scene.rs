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
    bind_row, item_lowering, map_cell_style, set_cell_interaction, set_cell_style, table,
    table_cell, table_row, ColSpan, TableCellPrim, TableCellProps, TableProps, TableRowPrim,
    TableRowProps,
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
        Element::Fragment(children) => children,
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
    set_cell_style(&native_cell, || {
        glue::StyleApplication::new(Rc::new(glue::StyleSheet::new(
            |_vs: &glue::VariantSet| glue::StyleRules::default(),
        )))
    });
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
        ColSpan::default(),
        Vec::new(),
        Some(StyleProp::Sheet(Box::new(glue::StyleApplication::new(
            sheet.clone(),
        )))),
    );
    set_cell_style(&web_cell, move || glue::StyleApplication::new(sheet.clone()));
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
/// The bug: a read-then-replace pair (read the style, write it back with
/// [`set_cell_style`]) only ever sees a static `StyleProp::Sheet`, so it
/// skipped such a cell in SILENCE — a width-pinned column (its width moves
/// under a resize drag, so its style has to be a closure) was the one
/// column in a clickable row that never lit up on hover, with nothing
/// logged. `map_cell_style`
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
        ColSpan::default(),
        Vec::new(),
        Some(StyleProp::SheetDynamic(Box::new(move || pinned.clone()))),
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
        ColSpan::default(),
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
                            ColSpan::default(),
                            vec![glue::text("Prop").into_element()],
                            None,
                        ),
                        item_lowering::cell_item(
                            true,
                            ColSpan::default(),
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
                            ColSpan::default(),
                            vec![glue::text("children").into_element()],
                            None,
                        ),
                        item_lowering::cell_item(
                            false,
                            ColSpan::default(),
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
        let cell = item_lowering::cell_item(false,ColSpan::default(), vec![glue::text("go").into_element()], None);
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
                    ColSpan::default(),
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
                    ColSpan::default(),
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
        ..Default::default()
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
            assert_eq!(
                prim.axis,
                runtime_shared::ScrollAxis::Horizontal,
                "the scroller must be horizontal"
            );
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

/// CrewForge (table 4.0.0): a `scroll_y` plain table in a fixed-height
/// region showed its first rows and cut the rest off. A watched table
/// (`on_scroll_edges`, which idea-ui's plain variant uses to draw frozen
/// edges) wraps its scroller in a viewport-measuring holder; unstyled, the
/// holder's automatic minimum size was its content, so it grew to every
/// row and the scroller inside had nothing to scroll. The holder must pass
/// the surface's bound through: content-sized, shrinkable to 0 on both
/// axes. (`runtime-layout`'s `edge_watch_holder_passes_the_surface_cap`
/// pins the layout consequence.)
#[test]
fn regression_watched_scroll_y_table_holder_passes_the_height_cap_through() {
    use runtime_vocabulary::prims::ScrollViewPrim;
    let t = table(TableProps {
        children: vec![row(2), row(2), row(2)],
        scroll_x: true,
        scroll_y: true,
        ..Default::default()
    })
    .on_scroll_edges(|_| {})
    .into_element();
    let Element::Item { mut children, .. } = t else { panic!("surface view") };
    let Element::Item { data, children: holder_children } = children.pop().expect("one child") else {
        panic!("the surface holds the edge-watch holder");
    };
    let holder = data
        .downcast_ref::<PrimCell<ViewPrim>>()
        .expect("a watched table's scroller sits in a holder view")
        .take();
    let rules = match holder.style {
        Some(StyleProp::Static(r)) => r,
        _ => panic!("the holder carries static rules"),
    };
    let len = |l: &Option<glue::Tokenized<glue::Length>>| l.as_ref().map(|t| t.resolve());
    assert_eq!(len(&rules.min_height), Some(glue::Length::Px(0.0)), "shrinks below its rows");
    assert_eq!(len(&rules.min_width), Some(glue::Length::Px(0.0)), "shrinks below its columns");
    assert_eq!(rules.flex_shrink.as_ref().map(|t| t.resolve()), Some(1.0));
    assert_eq!(rules.flex_grow.as_ref().map(|t| t.resolve()), Some(0.0), "content-sized like the scroller");
    assert_eq!(rules.flex_direction, Some(glue::FlexDirection::Column));
    match &holder_children[..] {
        [Element::Item { data, .. }] => assert!(data.downcast_ref::<PrimCell<ScrollViewPrim>>().is_some()),
        _ => panic!("the holder wraps exactly the scroller"),
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
                    ColSpan::default(),
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
                ColSpan::default(),
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

// ===========================================================================
// Column spans, footer sections, frame slots
// ===========================================================================

fn text_cell(label: &str) -> Element {
    table_cell(TableCellProps {
        children: vec![glue::text(label.to_string()).into_element()],
        ..Default::default()
    })
    .into_element()
}

fn span_cell(span: ColSpan) -> Element {
    table_cell(TableCellProps { span, ..Default::default() }).into_element()
}

fn row_of(cells: Vec<Element>) -> Element {
    table_row(TableRowProps { children: cells }).into_element()
}

/// The grid placement a native cell ended up with.
fn placement(cell: &Element) -> (Option<GridPlacement>, Option<GridPlacement>) {
    let prim = take_view_prim(cell);
    let app = match prim.style {
        Some(StyleProp::Sheet(app)) => *app,
        _ => panic!("cell keeps its sheet (placement rides overrides)"),
    };
    let rules = resolve_style(&app);
    (rules.grid_row, rules.grid_column)
}

use runtime_shared::GridPlacement;

/// Regression (CrewForge 10-09 table gaps #2): a cell could only ever
/// cover one column, so a "No entries for this day." row squeezed into
/// column 1's width and wrapped there. A spanning cell must lower to a
/// multi-track grid placement, and every cell of the table must be
/// explicitly placed — the layout attributes auto-flow children to
/// columns in document order, which a multi-track child breaks.
#[test]
fn regression_spanning_cell_covers_its_columns_on_native() {
    let t = table(TableProps {
        children: vec![row(3), row_of(vec![span_cell(ColSpan::Columns(2)), cell()])],
        ..Default::default()
    })
    .into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let rules = resolve_style(&match take_view_prim(&inner).style {
        Some(StyleProp::Sheet(app)) => *app,
        _ => panic!("grid sheet"),
    });
    assert_eq!(rules.grid_template_columns.as_ref().map(|c| c.len()), Some(3));
    let cells = item_children(inner);
    assert_eq!(cells.len(), 5, "the span marker is unwrapped, not a grid child");
    let expect = [
        (1, GridPlacement::Line(1)),
        (1, GridPlacement::Line(2)),
        (1, GridPlacement::Line(3)),
        (2, GridPlacement::Lines(1, 3)),
        (2, GridPlacement::Line(3)),
    ];
    for (i, (r, c)) in expect.iter().enumerate() {
        assert_eq!(placement(&cells[i]), (Some(GridPlacement::Line(*r)), Some(*c)), "cell {i}");
    }
}

/// `ColSpan::Rest` covers every column to the end of its row, resolved
/// against the widest row — the "blank row" shape needs no column count.
#[test]
fn rest_span_resolves_to_the_remaining_columns() {
    let t = table(TableProps {
        children: vec![row(4), row_of(vec![cell(), span_cell(ColSpan::Rest)])],
        ..Default::default()
    })
    .into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let cells = item_children(inner);
    assert_eq!(
        placement(&cells[5]),
        (Some(GridPlacement::Line(2)), Some(GridPlacement::Lines(2, 5))),
        "Rest after one cell of four covers columns 2-4"
    );
}

/// Regression (gap #4): a footer section's rows land after every body
/// row on native, wherever the section sits among the children — the
/// browser lays out `<tfoot>` that way, so the native grid must too.
#[test]
fn regression_footer_rows_render_after_body_rows_on_native() {
    let foot = table::table_foot(table::TableFootProps {
        children: vec![row_of(vec![text_cell("total"), cell()])],
    });
    let t = table(TableProps {
        children: vec![row(2), foot, row(2)],
        ..Default::default()
    })
    .into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let cells = item_children(inner);
    assert_eq!(cells.len(), 6, "the footer's rows flatten into the grid");
    // Auto-flow (no spans or proxies): order IS position. Only the
    // footer's first cell holds content, and it must open the last row.
    let with_content: Vec<usize> = cells
        .into_iter()
        .enumerate()
        .filter(|(_, c)| !item_children(clone_shallow(c)).is_empty())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(with_content, vec![4], "the footer row is the grid's last row");
}

/// A cheap stand-in carrying `el`'s child count, so a test can count
/// children without taking the (single-use) payload.
fn clone_shallow(el: &Element) -> Element {
    match el {
        Element::Item { children, .. } => {
            Element::Fragment(children.iter().map(|_| Element::Fragment(Vec::new())).collect())
        }
        _ => panic!("expected a cell item"),
    }
}

/// Web: a span becomes a real `colspan` attribute (a `Rest` span
/// resolved to a number — the browser accepts nothing else), and a
/// footer section mounts as a real `<tfoot>`.
#[test]
fn web_handlers_emit_colspan_and_tfoot() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        let head = item_lowering::row_item(
            (0..3)
                .map(|_| item_lowering::cell_item(true, ColSpan::default(), Vec::new(), None))
                .collect(),
            None,
        );
        let blank = item_lowering::row_item(
            vec![item_lowering::cell_item(
                false,
                ColSpan::Rest,
                vec![glue::text("Nobody yet").into_element()],
                None,
            )],
            None,
        );
        let foot = table::table_foot(table::TableFootProps {
            children: vec![item_lowering::row_item(
                vec![
                    item_lowering::cell_item(false, ColSpan::Columns(2), Vec::new(), None),
                    item_lowering::cell_item(false, ColSpan::default(), Vec::new(), None),
                ],
                None,
            )],
        });
        // The web lowering `table()` builds on wasm32 — spans resolve
        // exactly as an app's do.
        item_lowering::web_table(vec![head, blank, foot], None, false, None, None)
    });
    let html = &page.html;
    assert!(html.contains(r#"colspan="3""#), "Rest resolves to 3 columns: {html}");
    assert!(html.contains(r#"colspan="2""#), "explicit span: {html}");
    assert!(html.contains("<tfoot"), "footer section is a real <tfoot>: {html}");
    let tfoot_at = html.find("<tfoot").unwrap();
    assert!(
        html[tfoot_at..].contains(r#"colspan="2""#),
        "the footer row mounts inside the <tfoot>: {html}"
    );
}

/// Regression (gap #5): content that must keep still while a scroll-x
/// table's columns scroll — and share the table's frame — had nowhere
/// to go. The slots sit inside the styled surface, either side of the
/// scroller.
#[test]
fn regression_slots_sit_inside_the_surface_outside_the_scroller() {
    use runtime_vocabulary::prims::ScrollViewPrim;
    let t = table(TableProps {
        children: vec![row(2)],
        scroll_x: true,
        header_slot: Some(glue::text("holiday").into_element()),
        footer_slot: Some(glue::text("add").into_element()),
        ..Default::default()
    })
    .into_element();
    let kids = item_children(t);
    assert_eq!(kids.len(), 3, "surface holds header slot, scroller, footer slot");
    match &kids[1] {
        Element::Item { data, .. } => assert!(
            data.downcast_ref::<PrimCell<ScrollViewPrim>>().is_some(),
            "the scroller sits between the slots"
        ),
        _ => panic!("middle child must be the scroller"),
    }
    for i in [0, 2] {
        if let Element::Item { data, .. } = &kids[i] {
            assert!(
                data.downcast_ref::<PrimCell<ScrollViewPrim>>().is_none(),
                "slot {i} is outside the scroller"
            );
        }
    }
}

/// A plain (non-scroll) table with a slot moves the author style onto a
/// surface wrapper, on web too: the slot then shares the frame instead
/// of floating outside the `<table>`'s border.
#[test]
fn slots_on_a_plain_table_wrap_it_in_the_styled_surface_on_web() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        let style = glue::StyleRules {
            border_top_width: Some(glue::Tokenized::Literal(7.0)),
            ..Default::default()
        };
        item_lowering::web_table(
            vec![item_lowering::row_item(
                vec![item_lowering::cell_item(false, ColSpan::default(), Vec::new(), None)],
                None,
            )],
            Some(StyleProp::Static(Rc::new(style))),
            false,
            Some(glue::text("caption text").into_element()),
            None,
        )
    });
    let html = &page.html;
    let caption = html.find("caption text").expect("slot renders");
    let table_at = html.find("<table").expect("table renders");
    assert!(caption < table_at, "header slot precedes the table: {html}");
    let table_tag_end = table_at + html[table_at..].find('>').unwrap();
    assert!(
        !html[table_at..table_tag_end].contains("border-top-width"),
        "the author style moved off the <table> onto the surface: {html}"
    );
}

/// Regression (idea-ui 5.2.0, CrewForge): a styled plain table — no
/// `scroll_x`, no slot — drew SQUARE corners on web although its style
/// asked for a radius. The style landed on the `<table>`, whose
/// `border-collapse: collapse` reset makes browsers ignore
/// `border-radius`. The style must land on a surface view around the
/// `<table>` instead, as it already did for slotted / scroll-x tables.
/// (The browser's paint is not reachable from a host test; this pins
/// the node the radius lands on, which is the whole bug.)
#[test]
fn regression_styled_plain_table_rounds_on_a_surface_not_the_collapsed_table() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        let style = glue::StyleRules {
            border_top_left_radius: Some(glue::Tokenized::Literal(runtime_shared::Length::Px(9.0))),
            ..Default::default()
        };
        item_lowering::web_table(
            vec![item_lowering::row_item(
                vec![item_lowering::cell_item(false, ColSpan::default(), Vec::new(), None)],
                None,
            )],
            Some(StyleProp::Static(Rc::new(style))),
            false,
            None,
            None,
        )
    });
    let html = &page.html;
    let table_at = html.find("<table").expect("table renders");
    let table_tag_end = table_at + html[table_at..].find('>').unwrap();
    assert!(
        html[table_at..table_tag_end].contains("border-collapse: collapse")
            || html[table_at..table_tag_end].contains("border-collapse:collapse"),
        "the <table> keeps its collapsed-border reset: {html}"
    );
    assert!(
        !html[table_at..table_tag_end].contains("radius"),
        "the radius must not land on the collapsed <table>: {html}"
    );
    // The surface's style mints a class; its rule carries the radius.
    let surface = &html[..table_at];
    let class_at = surface.find("class=\"").expect("surface wrapper carries the style class") + 7;
    let class = &surface[class_at..class_at + surface[class_at..].find('"').unwrap()];
    let css = &page.head_css;
    let rule_at = css.find(&format!(".{class}")).unwrap_or_else(|| panic!("rule for {class}: {css}"));
    let rule = &css[rule_at..rule_at + css[rule_at..].find('}').unwrap()];
    assert!(
        rule.contains("border-top-left-radius"),
        "the radius lands on the surface wrapping the <table>: {rule}"
    );
}

/// An UNSTYLED plain table has no frame to round, so it stays a bare
/// `<table>` — no wrapper view for nothing.
#[test]
fn unstyled_plain_table_stays_a_bare_table_on_web() {
    let el = item_lowering::web_table(
        vec![item_lowering::row_item(Vec::new(), None)],
        None,
        false,
        None,
        None,
    );
    match el {
        Element::Item { data, .. } => assert!(
            data.downcast_ref::<PrimCell<table::TablePrim>>().is_some(),
            "root is the <table> item itself"
        ),
        _ => panic!("expected the <table> item"),
    }
}

/// `map_cell_application` re-selects a STATIC sheet without turning it
/// reactive (row tone / footer / density must not cost every cell a
/// style closure), reaches through a span marker, and composes around a
/// reactive one.
#[test]
fn map_cell_application_keeps_static_sheets_static() {
    let cell = table_cell(TableCellProps { span: ColSpan::Columns(2), ..Default::default() })
        .with_style(StyleProp::Sheet(Box::new(glue::StyleApplication::new(Rc::new(
            glue::StyleSheet::new(|_| glue::StyleRules::default()),
        )))))
        .into_element();
    assert!(table::map_cell_application(&cell, Rc::new(|a| a.with("tone", "warning"))));
    let inner = match &cell {
        Element::Item { children, .. } => &children[0],
        _ => panic!("span marker"),
    };
    match take_view_prim(inner).style {
        Some(StyleProp::Sheet(app)) => assert_eq!(
            format!("{:?}", app.variants),
            format!("{:?}", glue::VariantSet::new().with("tone", "warning")),
        ),
        _ => panic!("a static sheet must stay a static sheet"),
    }
}

/// CrewForge: `cell_application` returned `None` for a cell styled with
/// a reactive sheet, so a table-level pass (idea-ui's head-row
/// detection) could not see what the cell selected. It evaluates the
/// closure once, untracked: reading it must not subscribe the caller.
#[test]
fn regression_cell_application_reads_a_dynamic_sheet_untracked() {
    let world = World::new();
    world.enter(|| {
        let tone = glue::signal("warning");
        let cell = table_cell(TableCellProps::default())
            .with_style(StyleProp::SheetDynamic(Box::new(move || {
                glue::StyleApplication::new(Rc::new(glue::StyleSheet::new(|_| glue::StyleRules::default())))
                    .with("tone", tone.get())
            })))
            .into_element();
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        let _watch = runtime_world::effect(move || {
            r.set(r.get() + 1);
            let app = table::cell_application(&cell).expect("a dynamic sheet has an application");
            assert_eq!(app.variants.0.get("tone").map(String::as_str), Some("warning"));
        });
        assert_eq!(runs.get(), 1);
        tone.set("danger");
        world.flush();
        assert_eq!(runs.get(), 1, "reading the cell's application must not subscribe the reader");
    });
}

// ===========================================================================
// Grid columns (`ColumnTrack`): flex / max_width switch the table to
// explicit grid tracks on both lowerings.
// ===========================================================================

use runtime_shared::TrackSize;
use table::{ColumnSpec, ColumnTrack};

fn sized_cell(header: bool, label: &str, track: ColumnTrack) -> Element {
    table_cell(TableCellProps {
        header,
        column: Some(ColumnSpec::new(track)),
        children: vec![glue::text(label.to_string()).into_element()],
        ..Default::default()
    })
    .into_element()
}

const FLEX: ColumnTrack = ColumnTrack { width: None, min_width: None, max_width: None, flex: Some(1.0) };

/// The Name | Type | Page shape from the report: Name fills and may
/// shrink to nothing, Type and Page hug their content.
fn name_type_page_rows() -> Vec<Element> {
    vec![
        row_of(vec![
            sized_cell(true, "Name", FLEX),
            sized_cell(true, "Type", ColumnTrack::default()),
            sized_cell(true, "Page", ColumnTrack::default()),
        ]),
        row_of(vec![text_cell("A very long field name"), text_cell("Text"), text_cell("3")]),
    ]
}

fn grid_rules(prop: Option<StyleProp>) -> runtime_vocabulary::glue::StyleRules {
    match prop {
        Some(StyleProp::Dynamic(f)) => (*f()).clone(),
        Some(StyleProp::Sheet(app)) => (*resolve_style(&app)).clone(),
        _ => panic!("grid style is a rules closure or a sheet"),
    }
}

#[test]
fn column_track_maps_to_grid_tracks() {
    let px = |v| Box::new(TrackSize::Px(v));
    let t = |width, min_width, max_width, flex| {
        ColumnTrack { width, min_width, max_width, flex }.track()
    };
    assert_eq!(t(Some(120.0), Some(10.0), Some(300.0), Some(1.0)), TrackSize::Px(120.0), "width wins");
    assert_eq!(
        t(None, None, None, Some(2.0)),
        TrackSize::Minmax(px(0.0), Box::new(TrackSize::Fr(2.0))),
        "flex shares the leftover and may shrink to nothing"
    );
    assert_eq!(t(None, Some(80.0), None, Some(1.0)), TrackSize::Minmax(px(80.0), Box::new(TrackSize::Fr(1.0))));
    assert_eq!(t(None, None, Some(300.0), Some(1.0)), TrackSize::Minmax(px(0.0), px(300.0)), "flex up to max");
    assert_eq!(t(None, None, Some(300.0), None), TrackSize::FitContent(300.0), "content, capped");
    assert_eq!(t(None, Some(80.0), None, None), TrackSize::MaxContent, "min floors through the cell style");
    assert_eq!(t(None, None, None, None), TrackSize::MaxContent, "content-sized, never squeezed");
    assert!(!ColumnTrack { width: Some(1.0), min_width: Some(1.0), ..Default::default() }.needs_grid());
    assert!(FLEX.needs_grid());
    assert!(ColumnTrack { max_width: Some(1.0), ..Default::default() }.needs_grid());
}

/// Regression (CrewForge, idea-ui 8.0.0): no column could take "the
/// width the others leave, then truncate". A flex column makes the
/// native grid's tracks explicit — `minmax(0, 1fr)` for Name,
/// `max-content` for the rest — instead of the all-`auto` browser-table
/// sizing.
#[test]
fn regression_flex_column_gives_the_native_grid_explicit_tracks() {
    let t = table(TableProps { children: name_type_page_rows(), ..Default::default() }).into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let rules = grid_rules(take_view_prim(&inner).style);
    assert_eq!(rules.display, Some(DisplayKind::Grid));
    assert_eq!(
        rules.grid_template_columns,
        Some(vec![
            TrackSize::Minmax(Box::new(TrackSize::Px(0.0)), Box::new(TrackSize::Fr(1.0))),
            TrackSize::MaxContent,
            TrackSize::MaxContent,
        ])
    );
    assert_eq!(item_children(inner).len(), 6, "the sized cells unwrap into the grid like any other");
}

/// Width / min-width alone keep today's auto-table sizing (all-`auto`
/// tracks, which `runtime-layout` sizes like a browser table).
#[test]
fn width_only_columns_keep_the_auto_table_tracks() {
    let rows = vec![row_of(vec![
        sized_cell(true, "Name", ColumnTrack { width: Some(160.0), ..Default::default() }),
        sized_cell(true, "Type", ColumnTrack { min_width: Some(40.0), ..Default::default() }),
    ])];
    let t = table(TableProps { children: rows, ..Default::default() }).into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let rules = grid_rules(take_view_prim(&inner).style);
    assert_eq!(rules.grid_template_columns, Some(vec![TrackSize::Auto, TrackSize::Auto]));
}

/// A live column re-reads its sizing each time the grid style is
/// evaluated, so a width that follows a resize drag moves its track.
#[test]
fn live_column_spec_is_re_read_by_the_grid_style() {
    let width = Rc::new(Cell::new(100.0f32));
    let w = width.clone();
    let rows = vec![row_of(vec![
        table_cell(TableCellProps {
            header: true,
            column: Some(ColumnSpec::live(move || ColumnTrack { width: Some(w.get()), ..Default::default() })),
            ..Default::default()
        })
        .into_element(),
        sized_cell(true, "Rest", FLEX),
    ])];
    let t = table(TableProps { children: rows, ..Default::default() }).into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let Some(StyleProp::Dynamic(f)) = take_view_prim(&inner).style else {
        panic!("a grid-column table's style is a live rules closure");
    };
    assert_eq!(f().grid_template_columns.as_ref().unwrap()[0], TrackSize::Px(100.0));
    width.set(180.0);
    assert_eq!(f().grid_template_columns.as_ref().unwrap()[0], TrackSize::Px(180.0));
}

fn web_cell(header: bool, label: &str, track: Option<ColumnTrack>) -> Element {
    let c = item_lowering::cell_item(header, ColSpan::default(), vec![glue::text(label.to_string()).into_element()], None);
    item_lowering::set_cell_column(&c, track.map(ColumnSpec::new));
    c
}

/// Web: a flex column keeps the real `<table>` (and its semantics, said
/// explicitly) but lays it out as a CSS grid — rows `display: contents`
/// so the cells are the grid's items.
#[test]
fn regression_flex_column_lays_the_web_table_out_as_a_grid() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::web_table(
            vec![
                item_lowering::row_item(
                    vec![web_cell(true, "Name", Some(FLEX)), web_cell(true, "Type", None)],
                    None,
                ),
                item_lowering::row_item(
                    vec![web_cell(false, "A very long field name", None), web_cell(false, "Text", None)],
                    None,
                ),
            ],
            None,
            false,
            None,
            None,
        )
    });
    let html = &page.html;
    let css = &page.head_css;
    assert!(html.contains("<table") && html.contains("role=\"table\""), "{html}");
    assert!(css.contains("display: grid") || css.contains("display:grid"), "grid table: {css}");
    assert!(css.contains("minmax(0px, 1fr) max-content"), "the column tracks: {css}");
    assert!(html.contains("role=\"row\""), "explicit row role: {html}");
    assert!(html.contains("display: contents") || html.contains("display:contents"), "contents rows: {html}");
    assert!(html.contains("role=\"columnheader\"") && html.contains("role=\"cell\""), "{html}");
    // Not table cells any more: a `table-cell` whose row is `display:
    // contents` is wrapped with its siblings in an anonymous table, which
    // becomes one grid item.
    assert!(html.contains("<td style=\"display:flex"), "cells are flex boxes: {html}");
}

/// Regression: prerendered `<table><tr>…` is re-parsed by the browser,
/// whose parser inserts a `<tbody>` — an ordinary row-group box that
/// became the grid's only item, every cell stacked in column one
/// (measured in Chrome). Grid mode emits its own `display: contents`
/// `<tbody>` so parsed and live DOM match.
#[test]
fn regression_web_grid_table_emits_its_own_contents_tbody() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::web_table(
            vec![item_lowering::row_item(vec![web_cell(true, "Name", Some(FLEX))], None)],
            None,
            false,
            None,
            None,
        )
    });
    let html = &page.html;
    let tbody = html.find("<tbody").unwrap_or_else(|| panic!("an explicit tbody: {html}"));
    assert!(html[tbody..].starts_with("<tbody style=\"display:contents\""), "{html}");
    assert!(tbody < html.find("<tr").unwrap(), "the rows sit inside it: {html}");
}

/// The same table without a flex / max column is the unchanged auto
/// `<table>`: no grid, no role attributes.
#[test]
fn web_table_without_grid_columns_is_unchanged() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::web_table(
            vec![item_lowering::row_item(
                vec![web_cell(true, "Name", Some(ColumnTrack { width: Some(120.0), ..Default::default() }))],
                None,
            )],
            None,
            false,
            None,
            None,
        )
    });
    let html = &page.html;
    assert!(!html.contains("role=\"table\"") && !html.contains("contents"), "{html}");
}

/// Grid mode: a footer section renders after the body rows even when
/// written first — the browser's `<tfoot>`-last rule does not reach
/// `display: contents` grid items, so `table` moves it.
#[test]
fn web_grid_table_renders_its_footer_last() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::web_table(
            vec![
                table::table_foot(table::TableFootProps {
                    children: vec![item_lowering::row_item(vec![web_cell(false, "TOTALS", None)], None)],
                }),
                item_lowering::row_item(vec![web_cell(true, "HEADING", Some(FLEX))], None),
            ],
            None,
            false,
            None,
            None,
        )
    });
    let html = &page.html;
    assert!(
        html.find("HEADING").unwrap() < html.find("TOTALS").unwrap(),
        "body before footer: {html}"
    );
    assert!(html.contains("role=\"rowgroup\""), "{html}");
}

/// Grid mode: a `display: contents` `<tr>` has no box, so a bound row
/// gets the native lowering's backdrop (and every cell an explicit
/// place) — the row handle still has geometry.
#[test]
fn web_grid_table_bound_row_gets_a_backdrop() {
    let filled = Rc::new(Cell::new(false));
    let filled_in = filled.clone();
    let page = backend_ssr::newcore::render_path_with("/", table::register, move || {
        let r = item_lowering::row_item(vec![web_cell(false, "x", Some(FLEX))], None);
        let filled = filled_in.clone();
        bind_row(&r, move |_handle| filled.set(true));
        item_lowering::web_table(vec![r], None, false, None, None)
    });
    let html = &page.html;
    assert!(filled.get(), "the row handle is filled (from the backdrop): {html}");
    let tr = html.find("<tr").expect("a row");
    let after_tr = &html[tr..];
    let open_end = after_tr.find('>').expect("row tag closes");
    assert!(after_tr[open_end + 1..].starts_with("<div"), "the backdrop leads the row: {html}");
    assert!(page.head_css.contains("grid-row"), "cells and backdrop are placed explicitly: {}", page.head_css);
}

// ===========================================================================
// Scroll-y mode
// ===========================================================================

/// Peel a scrolling table down to (scroller prim, scroller rules,
/// content rules) — the native shape is surface > scroller > content.
fn native_scroller_parts(
    t: Element,
) -> (runtime_vocabulary::prims::ScrollViewPrim, glue::StyleRules, glue::StyleRules) {
    use runtime_vocabulary::prims::ScrollViewPrim;
    let mut surface_children = item_children(t);
    assert_eq!(surface_children.len(), 1, "surface holds just the scroller");
    let (prim, mut scroller_children) = match surface_children.pop().expect("len checked") {
        Element::Item { data, children } => (
            data.downcast_ref::<PrimCell<ScrollViewPrim>>()
                .expect("surface wraps the scroll_view")
                .take(),
            children,
        ),
        _ => panic!("surface must wrap a scroll_view item"),
    };
    let scroller_rules = match prim.style.as_ref() {
        Some(StyleProp::Static(r)) => (**r).clone(),
        _ => panic!("scroller carries static wrapper rules"),
    };
    let content = take_view_prim(&scroller_children.pop().expect("scroll content"));
    let content_rules = match content.style {
        Some(StyleProp::Static(r)) => (*r).clone(),
        _ => panic!("scroll content carries static rules"),
    };
    (prim, scroller_rules, content_rules)
}

/// `scroll_y` alone: the table owns a VERTICAL scroller (so a sticky
/// header pins against it), content-sized and shrinkable inside the
/// surface, and the content keeps its full height (`flex_shrink: 0`) but
/// takes no width floor — columns fill and wrap as in a plain table,
/// with no sideways overflow the scroller could not scroll.
#[test]
fn scroll_y_lowers_to_a_vertical_scroller_whose_content_keeps_its_height() {
    let t = table(TableProps { children: vec![row(2)], scroll_y: true, ..Default::default() })
        .into_element();
    let (prim, scroller, content) = native_scroller_parts(t);
    assert_eq!(prim.axis, runtime_shared::ScrollAxis::Vertical);
    assert_eq!(prim.bounces, Some(false), "a bounded pane inside a page does not spring");
    assert!(scroller.flex_direction.is_none(), "column scroller: rows run on its main axis");
    assert!(
        matches!(scroller.flex_grow.as_ref().map(|t| t.resolve()), Some(g) if g == 0.0),
        "content-sized, so a short table hugs its rows"
    );
    assert!(content.min_width.is_none(), "no width floor without scroll_x");
    assert!(
        matches!(content.flex_shrink.as_ref().map(|t| t.resolve()), Some(s) if s == 0.0),
        "rows keep their height inside the bounded scroller"
    );
}

/// Regression (CrewForge): with `scroll_x` the table's horizontal
/// scroller was the header cells' NEAREST scroll container, so a header
/// could never stick to the page's vertical scroll. `scroll_x` +
/// `scroll_y` must build ONE two-axis scroller — header (top) and
/// frozen columns (left/right) pin against the same container.
#[test]
fn regression_scroll_x_and_y_share_one_two_axis_scroller() {
    let t = table(TableProps {
        children: vec![row(2)],
        scroll_x: true,
        scroll_y: true,
        ..Default::default()
    })
    .into_element();
    let (prim, scroller, content) = native_scroller_parts(t);
    assert_eq!(prim.axis, runtime_shared::ScrollAxis::Both);
    assert_eq!(scroller.flex_direction, Some(glue::FlexDirection::Row));
    assert_eq!(
        scroller.align_items,
        Some(glue::AlignItems::FlexStart),
        "content keeps its own height on the cross axis instead of stretching to the bounded scroller"
    );
    assert!(
        matches!(
            content.min_width.as_ref().map(|t| t.resolve()),
            Some(glue::Length::Percent(p)) if (p - 100.0).abs() < f32::EPSILON
        ),
        "the scroll-x width floor still applies"
    );
}

/// Web: `scroll_y` wraps the `<table>` in the scroller and switches to
/// the separate border model (a pinned header's hairlines travel with
/// it), but keeps the fill-and-wrap width — only `scroll_x` lays the
/// columns out at natural width.
#[test]
fn scroll_y_web_border_model_and_scroller() {
    let page = backend_ssr::newcore::render_path_with("/", table::register, || {
        item_lowering::web_scrolling_table(
            vec![item_lowering::row_item(
                vec![item_lowering::cell_item(
                    true,
                    ColSpan::default(),
                    vec![glue::text("head").into_element()],
                    None,
                )],
                None,
            )],
            None,
            false,
            true,
        )
    });
    // Inline declarations serialize without the space after the colon.
    let html: String = page.html.replace(": ", ":");
    let html = &html;
    assert!(html.contains("overflow:auto"), "the table sits in a scroller: {html}");
    assert!(
        html.contains("border-collapse:separate"),
        "scroll-y table must use the separate border model: {html}"
    );
    assert!(!html.contains("border-collapse:collapse"), "{html}");
    assert!(html.contains("width:100%"), "fill-and-wrap width kept: {html}");
    assert!(!html.contains("max-content"), "natural widths are scroll_x's: {html}");
}

// ===========================================================================
// Table-level column model + the theme-layer hooks (row data, cell
// insertion, cell content, hover-only reporters).
// ===========================================================================

/// A table-level spec (`TableProps::columns`) sizes its column whatever
/// the cells say: column 0's cells declare nothing, column 1's cells
/// declare a flex track that the table-level fixed width overrides. One
/// spec drives the header and every body cell of the column.
#[test]
fn table_level_column_spec_overrides_the_cells() {
    let rows = vec![
        row_of(vec![text_cell("Name"), sized_cell(true, "Notes", FLEX)]),
        row_of(vec![text_cell("Ana"), sized_cell(false, "…", FLEX)]),
    ];
    let t = table(TableProps {
        children: rows,
        columns: vec![
            Some(ColumnSpec::new(ColumnTrack { flex: Some(1.0), ..Default::default() })),
            Some(ColumnSpec::new(ColumnTrack { width: Some(90.0), ..Default::default() })),
        ],
        ..Default::default()
    })
    .into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let rules = grid_rules(take_view_prim(&inner).style);
    assert_eq!(
        rules.grid_template_columns,
        Some(vec![
            TrackSize::Minmax(Box::new(TrackSize::Px(0.0)), Box::new(TrackSize::Fr(1.0))),
            TrackSize::Px(90.0),
        ]),
        "the table's spec wins over the cells' flex"
    );
}

/// A `None` entry — and every column past the list — keeps reading its
/// cells, so a table may size only some columns at the table level.
#[test]
fn table_level_column_gaps_fall_back_to_the_cells() {
    let rows = vec![row_of(vec![
        sized_cell(true, "Name", FLEX),
        sized_cell(true, "Hours", ColumnTrack { max_width: Some(80.0), ..Default::default() }),
    ])];
    let t = table(TableProps { children: rows, columns: vec![None], ..Default::default() })
        .into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let rules = grid_rules(take_view_prim(&inner).style);
    assert_eq!(
        rules.grid_template_columns,
        Some(vec![
            TrackSize::Minmax(Box::new(TrackSize::Px(0.0)), Box::new(TrackSize::Fr(1.0))),
            TrackSize::FitContent(80.0),
        ])
    );
}

/// Row data rides the row payload through a component's `Owned` scope
/// wrapper, on the native marker and the web `<tr>` item alike.
#[test]
fn row_data_round_trips_on_both_lowerings() {
    let world = World::new();
    world.enter(|| {
        let wrapped = component_scope(|| {
            let _state = glue::signal(false);
            row(2)
        });
        assert!(matches!(wrapped, Element::Owned { .. }), "precondition: Owned-wrapped");
        table::set_row_data(&wrapped, Rc::new(7usize));
        let got = table::row_data(&wrapped).expect("data attached");
        assert_eq!(got.downcast_ref::<usize>(), Some(&7));
    });

    let web = item_lowering::row_item(Vec::new(), None);
    assert!(table::row_data(&web).is_none(), "no data until attached");
    table::set_row_data(&web, Rc::new("tag"));
    assert_eq!(table::row_data(&web).unwrap().downcast_ref::<&str>(), Some(&"tag"));
    assert!(table::row_data(&cell()).is_none(), "a cell is not a row");
}

/// A leading cell inserted into a built row becomes column 0 of the
/// native grid — the row's cells shift right, and the column count
/// grows with it.
#[test]
fn a_cell_inserted_into_a_built_row_takes_its_column() {
    let mut rows = vec![row(2), row(2)];
    for r in rows.iter_mut() {
        table::visit_rows_mut(r, |row| {
            table::row_cells_mut(row).expect("a row").insert(0, text_cell("lead"));
        });
    }
    let t = table(TableProps { children: rows, ..Default::default() }).into_element();
    let inner = item_children(t).pop().expect("outer wraps the grid");
    let rules = grid_rules(take_view_prim(&inner).style);
    assert_eq!(rules.grid_template_columns.map(|t| t.len()), Some(3));
    assert_eq!(item_children(inner).len(), 6);
}

/// `visit_rows_mut` reaches a footer section's rows, and `is_footer`
/// tells the section apart from a row.
#[test]
fn visit_rows_mut_reaches_footer_rows() {
    let mut foot = table::table_foot(table::TableFootProps { children: vec![row(1), row(1)] });
    assert!(table::is_footer(&foot));
    assert!(!table::is_footer(&row(1)));
    let mut n = 0;
    table::visit_rows_mut(&mut foot, |r| {
        n += 1;
        assert!(table::row_cells_mut(r).is_some());
    });
    assert_eq!(n, 2);
}

/// Content can be appended to a built cell — through the `CellMarker` a
/// spanning native cell rides in, and on the web `<td>` item — and
/// `visit_cell_content` sees it.
#[test]
fn push_cell_content_reaches_through_the_marker() {
    let mut spanning = span_cell(ColSpan::Columns(2));
    assert!(table::push_cell_content(&mut spanning, text_cell("extra")));
    let mut seen = 0;
    table::visit_cell_content(&spanning, |_| seen += 1);
    assert_eq!(seen, 1, "the pushed child is the cell's only content");

    let mut web = item_lowering::cell_item(false, ColSpan::default(), Vec::new(), None);
    assert!(table::push_cell_content(&mut web, text_cell("x")));
    let mut seen = 0;
    table::visit_cell_content(&web, |_| seen += 1);
    assert_eq!(seen, 1);
    let mut not_a_cell = row(1);
    assert!(!table::push_cell_content(&mut not_a_cell, text_cell("x")));
}

/// A hover-only reporter leaves the touch handler alone and COMPOSES with
/// a hover reporter the cell already carries — a clickable row's shared
/// hover keeps working under a header cell's reveal.
#[test]
fn set_cell_hover_composes_and_leaves_touch_alone() {
    let c = cell();
    let first = Rc::new(Cell::new(0));
    let second = Rc::new(Cell::new(0));
    let (f, s) = (first.clone(), second.clone());
    set_cell_interaction(
        &c,
        Rc::new(|_ev| runtime_shared::TouchResponse::default()),
        Rc::new(move |_| f.set(f.get() + 1)),
    );
    table::set_cell_hover(&c, Rc::new(move |_| s.set(s.get() + 1)));
    let prim = take_view_prim(&c);
    assert!(prim.on_touch.is_some(), "the tap handler survives");
    (prim.on_hover.expect("hover installed"))(true);
    assert_eq!((first.get(), second.get()), (1, 1), "both reporters hear it");

    let bare = cell();
    table::set_cell_hover(&bare, Rc::new(|_| {}));
    let prim = take_view_prim(&bare);
    assert!(prim.on_hover.is_some() && prim.on_touch.is_none());
}
