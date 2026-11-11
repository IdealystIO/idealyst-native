//! `virtual-table-demo` — `Table(virtualized = …)` with 100,000 rows.
//!
//! Twelve columns, wider together than any window, so the table scrolls
//! sideways as well as down. The selection column and the ID column are
//! frozen at the left, the menu column at the right, and the header row at
//! the top: scroll either way and the rest slides beneath them. Only the
//! cells in view are built. The p99 header sorts; a row press shows which
//! row was opened.

use std::collections::BTreeSet;

use idea_ui::{
    install_idea_theme, light_theme, tone, variant, Button, CellAlign, ColumnId, ColumnPin, RowId,
    SortState, Table, TableColumn, TableScroller, Typography, VirtualRows,
};
use runtime_core::{signal, stylesheet, ui, Element, SafeAreaSides, Signal};

const ROWS: usize = 100_000;

stylesheet! {
    pub Screen<()> {
        base(_t) {
            flex_grow: 1.0,
            padding: 24,
            gap: 16,
        }
    }
}

const REGIONS: [&str; 5] = ["eu-west-1", "us-east-1", "ap-south-1", "us-west-2", "sa-east-1"];
const OWNERS: [&str; 5] = ["platform", "payments", "search", "identity", "data"];

fn columns() -> Vec<TableColumn> {
    vec![
        TableColumn::new("id").label("ID").width(100.0).pinned(ColumnPin::Left),
        TableColumn::new("service").label("Service").min_width(180.0),
        TableColumn::new("region").label("Region").width(140.0),
        TableColumn::new("version").label("Version").width(120.0),
        TableColumn::new("owner").label("Owner").width(160.0),
        TableColumn::new("replicas").label("Replicas").width(110.0).align(CellAlign::Right),
        TableColumn::new("cpu").label("CPU").width(100.0).align(CellAlign::Right),
        TableColumn::new("memory").label("Memory").width(120.0).align(CellAlign::Right),
        TableColumn::new("uptime").label("Uptime").width(110.0).align(CellAlign::Right),
        TableColumn::new("errors").label("Errors").width(110.0).align(CellAlign::Right),
        TableColumn::new("latency").label("p99").width(110.0).align(CellAlign::Right),
        TableColumn::actions("menu").pinned(ColumnPin::Right),
    ]
}

/// Row `n`'s value for one column.
fn value(n: usize, col: &str) -> String {
    match col {
        "id" => format!("#{n}"),
        "service" => format!("service-{n}"),
        "region" => REGIONS[n % REGIONS.len()].to_string(),
        "version" => format!("v2.{}.{}", n % 9, n % 4),
        "owner" => format!("team-{}", OWNERS[n % OWNERS.len()]),
        "replicas" => format!("{}", 2 + n % 7),
        "cpu" => format!("{}%", 5 + n * 13 % 90),
        "memory" => format!("{} MiB", 128 + n * 37 % 2048),
        "uptime" => format!("{}d", 1 + n % 180),
        "errors" => format!("0.{:02}%", n * 3 % 100),
        _ => format!("{}ms", 8 + n * 7 % 300),
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let picked: Signal<BTreeSet<RowId>> = signal(BTreeSet::new());
    let descending = signal(false);
    let opened = signal(None::<usize>);
    // Row `i` in the current sort order.
    let at = move |i: usize| if descending.get() { ROWS - 1 - i } else { i };
    let cell = move |i: usize, col: &ColumnId| -> Element {
        let n = at(i);
        match col.as_str() {
            "menu" => ui! { Typography(content = "⋮".to_string()) },
            id => ui! { Typography(content = value(n, id)) },
        }
    };
    let rows = VirtualRows::new(move || ROWS, move |i| RowId::from(format!("row-{}", at(i))), cell)
        .sort(
            move |col| {
                (col.as_str() == "latency").then(|| if descending.get() { SortState::Desc } else { SortState::Asc })
            },
            move |_, s| descending.set(s == SortState::Desc),
        )
        .on_row_click(move |i| opened.set(Some(at(i))));

    let scroller = TableScroller::new();
    let jump_target = signal(0usize);
    let jump: std::rc::Rc<dyn Fn()> = {
        let scroller = scroller.clone();
        std::rc::Rc::new(move || {
            // Alternate between two far-apart rows, so every press brings a
            // whole new screen of rows into view.
            let n = if jump_target.get() == 50_000 { 10_000 } else { 50_000 };
            jump_target.set(n);
            scroller.scroll_to_row(&RowId::from(format!("row-{n}")));
        })
    };
    let status = move || match opened.get() {
        Some(n) => format!("Opened row #{n} · {} selected", picked.with(|p| p.len())),
        None => format!("Click a row to open it · {} selected", picked.with(|p| p.len())),
    };

    ui! {
        view(style = Screen(), safe_area = SafeAreaSides::TOP) {
            text { "100,000 rows. Scroll down and sideways: the header, the ID column and the menu column stay put." }
            text { status }
            Button(label = "Jump to row 50,000 / 10,000".to_string(), on_click = jump, tone = tone::Neutral, variant = variant::Soft)
            Table(columns = columns(), selection = Some(picked), scroller = Some(scroller), virtualized = Some(rows))
        }
    }
}

/// The Android wrapper's entry (see `examples/README.md`).
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam. This app registers nothing.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
