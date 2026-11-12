//! `virtual-table-reorder-demo` — drag-to-reorder on a virtualized `Table`.
//!
//! A 5,000-task backlog in `Table(virtualized = …, on_reorder = …)`. Each
//! row has a grip in a frozen leading column: drag it and the row follows
//! the pointer while the rows it passes slide aside; hold it near the top
//! (under the header) or the bottom of the table and the table scrolls, so
//! a task can travel the whole backlog. On a touch screen, hold the grip or
//! swipe it sideways to pick the row up — a vertical swipe scrolls.
//!
//! The table only reports the move — `on_reorder(from, to)`, row indices.
//! The app owns the list, so it applies the move itself (`move_in_place`)
//! in the same callback; the cells read the tasks reactively, so they show
//! the new order in the same frame.

use std::collections::BTreeSet;
use std::rc::Rc;

use idea_ui::{
    install_idea_theme, light_theme, move_in_place, CellAlign, ColumnId, ColumnPin, RowId, Table, TableColumn,
    Typography, VirtualRows,
};
use runtime_core::{rx, signal, stylesheet, ui, Element, Memo, SafeAreaSides, Signal};

const TASKS: u32 = 5_000;

const OWNERS: [&str; 6] = ["Ana", "Bo", "Chen", "Dee", "Eli", "Fatima"];
const AREAS: [&str; 5] = ["billing", "search", "onboarding", "reports", "mobile"];
const VERBS: [&str; 4] = ["Fix", "Speed up", "Redesign", "Document"];

stylesheet! {
    pub Screen<()> {
        base(_t) {
            flex_grow: 1.0,
            padding: 24,
            gap: 16,
        }
    }
}

#[derive(Clone, PartialEq)]
struct Task {
    id: u32,
    title: String,
    owner: &'static str,
    points: u32,
}

impl Task {
    fn new(id: u32) -> Self {
        let n = id as usize;
        Task {
            id,
            title: format!("{} {} ({})", VERBS[n % VERBS.len()], AREAS[n % AREAS.len()], id),
            owner: OWNERS[n % OWNERS.len()],
            points: [1, 2, 3, 5, 8][n % 5],
        }
    }
}

fn columns() -> Vec<TableColumn> {
    vec![
        TableColumn::new("rank").label("Rank").width(80.0).align(CellAlign::Right).pinned(ColumnPin::Left),
        TableColumn::new("title").label("Task").min_width(240.0),
        TableColumn::new("owner").label("Owner").width(140.0),
        TableColumn::new("points").label("Points").width(100.0).align(CellAlign::Right),
    ]
}

/// Task `i`'s value for one column, in the current order.
fn value(tasks: Signal<Vec<Task>>, i: usize, col: &str) -> String {
    tasks.with(|t| match (t.get(i), col) {
        (None, _) => String::new(),
        (Some(_), "rank") => format!("{}", i + 1),
        (Some(t), "title") => t.title.clone(),
        (Some(t), "owner") => t.owner.to_string(),
        (Some(t), _) => t.points.to_string(),
    })
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let tasks: Signal<Vec<Task>> = signal((1..=TASKS).map(Task::new).collect());
    let picked: Signal<BTreeSet<RowId>> = signal(BTreeSet::new());
    let last_move = signal(None::<(u32, usize, usize)>);

    // Reusable content: each cell reads its row through `row`, so as the
    // table scrolls — or a drop reorders the tasks — only the text changes.
    let cell = move |row: Memo<usize>, col: &ColumnId| -> Element {
        let col = col.to_string();
        ui! { Typography(content = rx!(value(tasks, row.get(), &col))) }
    };
    let rows = VirtualRows::reusing(
        move || tasks.with(|t| t.len()),
        move |i| RowId::from(format!("task-{}", tasks.with(|t| t[i].id))),
        cell,
    );

    // The table reports the move; the app applies it.
    let on_reorder: Rc<dyn Fn(usize, usize)> = Rc::new(move |from, to| {
        let id = tasks.with(|t| t[from].id);
        tasks.update(|t| {
            let mut t = t.clone();
            move_in_place(&mut t, from, to);
            t
        });
        last_move.set(Some((id, from + 1, to + 1)));
    });

    let status = move || match last_move.get() {
        Some((id, from, to)) => format!("Moved task {id} from rank {from} to rank {to}."),
        None => "Drag a row by its grip. Hold it near the top or bottom edge to scroll the backlog.".to_string(),
    };

    ui! {
        view(style = Screen(), safe_area = SafeAreaSides::TOP) {
            text { "A 5,000-task backlog, virtualized: only the rows in view are built." }
            text { status }
            Table(
                columns = columns(),
                selection = Some(picked),
                on_reorder = Some(on_reorder),
                virtualized = Some(rows),
            )
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
