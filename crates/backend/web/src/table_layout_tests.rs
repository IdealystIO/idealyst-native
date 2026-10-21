//! Real-browser layout of idea-ui tables (the `table` SDK's web
//! lowering): what only a browser's table and grid sizing can show. Run
//! as `tests.rs`'s module docs describe (`cargo test --target
//! wasm32-unknown-unknown` with a CHROMEDRIVER that matches Chrome).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{realize, Registry};
use runtime_vocabulary::view;
use runtime_world::World;
use wasm_bindgen_test::*;
use web_glue::JsCast;

use crate::WebBackend;

wasm_bindgen_test_configure!(run_in_browser);

fn harness() -> (Rc<RefCell<WebBackend>>, Rc<Registry<WebBackend>>, World) {
    let document = web_glue::dom::window().unwrap().document().unwrap();
    if let Some(prior) = document.get_element_by_id("app") {
        prior.remove();
    }
    let app = document.create_element("div").unwrap();
    app.set_id("app");
    document.body().unwrap().append_child(&app).unwrap();
    crate::scheduler::install_scheduler();
    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    crate::install_global_self(&backend);
    let mut registry: Registry<WebBackend> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    table::register(&mut registry);
    (backend, Rc::new(registry), World::new())
}

/// Let layout, `ResizeObserver` deliveries and the work they schedule run.
/// The harness boots no dispatch hook, so the signal writes a layout
/// callback stages are committed here.
async fn settle(world: &World) {
    for _ in 0..6 {
        world.flush();
        let promise = web_glue::js::Promise::new(&mut |resolve, _reject| {
            web_glue::dom::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 20)
                .unwrap();
        });
        let _ = web_glue::JsFuture::new(&promise).await;
    }
}

/// A `width`-px host under `#app` holding the realized tree.
fn mount_in(width: f32, root: web_glue::dom::Node) -> web_glue::dom::Element {
    let doc = web_glue::dom::window().unwrap().document().unwrap();
    let host = doc.create_element("div").unwrap();
    host.set_attribute("style", &format!("width:{width}px;display:flex;flex-direction:column"))
        .unwrap();
    host.append_child(&root).unwrap();
    doc.get_element_by_id("app").unwrap().append_child(&host).unwrap();
    host
}

fn width_of(el: &web_glue::dom::Element) -> f64 {
    el.get_bounding_client_rect().width()
}

const LONG_NAME: &str =
    "Shotcrete crew lead, night shift, north decline, ventilation raise and refuge bay";

fn text_cell(header: bool, text: &str) -> runtime_scene::Element {
    idea_ui::TableCell(idea_ui::TableCellProps {
        header,
        text: runtime_vocabulary::glue::Reactive::Static(Some(text.to_string())),
        ..Default::default()
    })
}

fn crew_table(scroll_x: bool) -> runtime_scene::Element {
    use idea_ui::{Table, TableColumn, TableProps, TableRow, TableRowProps};
    let row = |header: bool, cells: [&str; 4]| {
        TableRow(TableRowProps {
            children: cells.iter().map(|t| text_cell(header, t)).collect(),
            ..Default::default()
        })
    };
    Table(TableProps {
        scroll_x,
        columns: vec![
            TableColumn::new("name").fill().truncate().min_width(120.0),
            TableColumn::new("role").width(180.0),
            TableColumn::new("start").width(168.0),
            TableColumn::new("end").width(168.0),
        ],
        children: vec![
            row(true, ["Name", "Role", "Start", "End"]),
            row(false, [LONG_NAME, "Lead", "06:00", "18:00"]),
        ],
        ..Default::default()
    })
}

/// CrewForge (2026-10-21): in a `scroll_x` table a `fill` column sized to
/// its whole one-line content — the measured grid template was `434.516px
/// 180px 168px 168px 40px` on a 945px parent, so the table overflowed and
/// truncation never happened. The `<table>` took `width: max-content`, and
/// a `1fr` track's max-content contribution is its content. The fill
/// column now takes what the scroller leaves and truncates; with the
/// table wider than its fixed columns plus the fill floor it does not
/// overflow at all.
#[wasm_bindgen_test]
async fn regression_fill_column_in_a_scroll_x_table_takes_the_rest_and_truncates() {
    let (backend, registry, world) = harness();
    let realized = world.enter(|| {
        idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
        realize(&backend, &registry, view().child(crew_table(true)).build())
    });
    let root = realized.collect_nodes().pop().expect("one root");
    let host = mount_in(945.0, root);
    settle(&world).await;
    let table = host.query_selector("table").unwrap().expect("a <table>");
    assert!(
        width_of(&table) <= 945.5,
        "the table fits its 945px scroller: {} wide, columns `{}`",
        width_of(&table),
        web_glue::dom::window()
            .unwrap()
            .get_computed_style(&table)
            .unwrap()
            .unwrap()
            .get_property_value("grid-template-columns")
            .unwrap()
    );
    // The text inside the name cell is clipped to the cell: its full
    // line is wider than the box it is drawn in.
    let name: web_glue::dom::HtmlElement = host
        .query_selector("td")
        .unwrap()
        .expect("the first body cell")
        .first_element_child()
        .expect("the cell's text")
        .unchecked_into();
    assert!(
        name.scroll_width() > name.client_width(),
        "the long name is cut short ({} of {})",
        name.client_width(),
        name.scroll_width()
    );
    drop(realized);
}

/// The phone case: the fixed columns alone overflow a 390px scroller, so
/// the table scrolls, and the fill column sits at its `min_width` floor
/// rather than at its content's width.
#[wasm_bindgen_test]
async fn regression_fill_column_in_a_narrow_scroll_x_table_drops_to_its_floor() {
    let (backend, registry, world) = harness();
    let realized = world.enter(|| {
        idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
        realize(&backend, &registry, view().child(crew_table(true)).build())
    });
    let root = realized.collect_nodes().pop().expect("one root");
    let host = mount_in(390.0, root);
    settle(&world).await;
    let table = host.query_selector("table").unwrap().expect("a <table>");
    let name = host.query_selector("td").unwrap().expect("the first body cell");
    assert!(width_of(&table) > 390.0, "the fixed columns overflow, so it scrolls");
    assert!(
        width_of(&name) <= 120.5,
        "the fill column sits at its floor: {} wide",
        width_of(&name)
    );
    drop(realized);
}

/// A crew table that scrolls on both axes: a frozen name column, three
/// wide ones and a frozen actions column, under a header, in a 300px-tall
/// 600px-wide host.
fn scrolling_crew(scroll_x: bool) -> runtime_scene::Element {
    use idea_ui::{ColumnPin, Table, TableColumn, TableProps, TableRow, TableRowProps};
    let row = |header: bool, cells: [&str; 5]| {
        TableRow(TableRowProps {
            children: cells.iter().map(|t| text_cell(header, t)).collect(),
            ..Default::default()
        })
    };
    let mut rows = vec![row(true, ["Name", "Role", "Shift", "Heading", ""])];
    for i in 0..30 {
        rows.push(row(false, ["Ana Moreno", "Shotcrete lead", "Night", &format!("North decline {i}"), "…"]));
    }
    Table(TableProps {
        scroll_x,
        scroll_y: true,
        max_height: Some(300.0),
        columns: vec![
            TableColumn::new("name").width(160.0).pinned(ColumnPin::Left),
            TableColumn::new("role").width(220.0),
            TableColumn::new("shift"),
            TableColumn::new("heading").width(240.0),
            TableColumn::actions("menu"),
        ],
        children: rows,
        ..Default::default()
    })
}

fn rect(el: &web_glue::dom::Element) -> web_glue::dom::DomRect {
    el.get_bounding_client_rect()
}

/// CrewForge (want_9f817d95): in a `scroll_y` table the vertical
/// scrollbar ran the table's full height beside the pinned header — the
/// header was a sticky row INSIDE the rows' scroller. The header now lays
/// out above the scroller, which starts where the header ends; the header
/// keeps the body's column tracks (every header cell starts at its
/// column's x) and follows the body's horizontal scroll, its frozen
/// columns pinned like the body's.
#[wasm_bindgen_test]
async fn regression_scroll_y_scrollbar_runs_under_the_header() {
    let (backend, registry, world) = harness();
    let realized = world.enter(|| {
        idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
        realize(&backend, &registry, view().child(scrolling_crew(true)).build())
    });
    let root = realized.collect_nodes().pop().expect("one root");
    let host = mount_in(600.0, root);
    settle(&world).await;

    let frame = host.query_selector("[role=table]").unwrap().expect("the split table's frame");
    let tables = frame.query_selector_all("table").unwrap();
    assert_eq!(tables.length(), 2, "a head table and a body table");
    let head_table: web_glue::dom::Element = tables.item(0).unwrap().unchecked_into();
    let body_table: web_glue::dom::Element = tables.item(1).unwrap().unchecked_into();
    assert!(head_table.query_selector("thead").unwrap().is_some(), "the header rows sit in a <thead>");

    // The rows' scroller begins under the header.
    let scroller = body_table.parent_element().expect("the body table's scroller");
    let body_scroll: web_glue::dom::HtmlElement = scroller.clone().unchecked_into();
    assert!(body_scroll.scroll_height() > body_scroll.client_height(), "the rows overflow and scroll");
    let head_bottom = rect(&head_table).bottom();
    assert!(
        rect(&scroller).top() >= head_bottom - 0.5,
        "the scroller (and its scrollbar) starts under the header: scroller top {} vs header bottom {}",
        rect(&scroller).top(),
        head_bottom
    );

    // Same column tracks: each header cell starts where its column's
    // first body cell does.
    let heads = head_table.query_selector_all("th").unwrap();
    let first_row = body_table.query_selector_all("tr:not([aria-hidden]) td").unwrap();
    for i in 0..heads.length() {
        let th: web_glue::dom::Element = heads.item(i).unwrap().unchecked_into();
        let td: web_glue::dom::Element = first_row.item(i).unwrap().unchecked_into();
        assert!(
            (rect(&th).left() - rect(&td).left()).abs() < 1.0 && (rect(&th).width() - rect(&td).width()).abs() < 1.0,
            "column {i}: header {}+{} vs body {}+{}",
            rect(&th).left(),
            rect(&th).width(),
            rect(&td).left(),
            rect(&td).width()
        );
    }

    // Horizontal sync: scroll the rows sideways and the header follows;
    // the frozen name column stays put in both.
    let name_th: web_glue::dom::Element = heads.item(0).unwrap().unchecked_into();
    let role_th: web_glue::dom::Element = heads.item(1).unwrap().unchecked_into();
    let role_td: web_glue::dom::Element = first_row.item(1).unwrap().unchecked_into();
    let name_x = rect(&name_th).left();
    body_scroll.set_scroll_left(120);
    settle(&world).await;
    assert!(
        (rect(&role_th).left() - rect(&role_td).left()).abs() < 1.0,
        "the header followed the rows: {} vs {}",
        rect(&role_th).left(),
        rect(&role_td).left()
    );
    assert!((rect(&name_th).left() - name_x).abs() < 0.5, "the frozen header cell stayed put");
    drop(realized);
}
