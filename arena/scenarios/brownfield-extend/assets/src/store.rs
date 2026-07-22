//! `TaskStore` — the app's single source of truth.
//!
//! The store owns the task list (`tasks`) and the active filter
//! (`priority_filter`) as signals, and derives the `visible` view from
//! them with a [`memo`]. Every consumer — the Header's counts, the
//! Toolbar's controls, the TaskList's rows — reads this one store, so a
//! filter change recomputes `visible` once and every reader reflects it.
//! Extend the derivation in `new()` (and add the matching signal +
//! setter) to introduce a new filter; readers pick it up for free.

use runtime_core::{memo, signal, ReadSignal, Signal};

/// A task's importance. Drives the Toolbar's priority filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    Low,
    Normal,
    High,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

impl Priority {
    /// Human-readable label shown in the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Priority::Low => "Low",
            Priority::Normal => "Normal",
            Priority::High => "High",
        }
    }
}

/// One row of the dashboard: a titled unit of work with a done flag and
/// a priority.
#[derive(Clone, PartialEq, Default)]
pub struct Task {
    pub id: u32,
    pub title: String,
    pub done: bool,
    pub priority: Priority,
}

/// Reactive store shared by every dashboard component.
///
/// `Copy` (it's a bundle of signal handles), so it threads through props
/// by value with no lifetime juggling — the same pattern the framework's
/// own `Ref` bundles use. The `Default` impl yields *detached* handles
/// for `ui!` dispatch; the real store always comes from [`TaskStore::new`]
/// and is passed down as the `store` prop.
#[derive(Clone, Copy, Default)]
pub struct TaskStore {
    /// Every task, in insertion order.
    pub tasks: Signal<Vec<Task>>,
    /// When `Some`, only tasks of that priority are visible.
    pub priority_filter: Signal<Option<Priority>>,
    /// Derived view: `tasks` with the active filters applied. Readers
    /// bind to this rather than filtering themselves, so the visible set
    /// stays consistent everywhere.
    pub visible: ReadSignal<Vec<Task>>,
}

impl TaskStore {
    /// Build the store with seed data and wire the `visible` derivation.
    /// Call once from inside the mounted reactive scope (i.e. from
    /// `app()`), so the memo and its subscriptions belong to the app's
    /// owner.
    pub fn new() -> Self {
        let tasks = signal(seed());
        let priority_filter: Signal<Option<Priority>> = signal(None);

        // The one place filtering happens. Every reader observes the
        // result via `visible`; a new filter is a new clause here plus a
        // signal to gate it.
        let visible = memo(move || {
            let pf = priority_filter.get();
            tasks
                .get()
                .into_iter()
                .filter(|t| match pf {
                    Some(p) => t.priority == p,
                    None => true,
                })
                .collect::<Vec<Task>>()
        });

        Self {
            tasks,
            priority_filter,
            visible,
        }
    }

    /// Flip a task's done state.
    pub fn toggle(&self, id: u32) {
        self.tasks.update(|v| {
            if let Some(t) = v.iter_mut().find(|t| t.id == id) {
                t.done = !t.done;
            }
        });
    }

    /// Set (or clear, with `None`) the active priority filter.
    pub fn set_priority_filter(&self, p: Option<Priority>) {
        self.priority_filter.set(p);
    }

    /// Total number of tasks, ignoring any filter.
    pub fn total(&self) -> usize {
        self.tasks.get().len()
    }

    /// Number of tasks marked done, ignoring any filter.
    pub fn completed(&self) -> usize {
        self.tasks.get().iter().filter(|t| t.done).count()
    }

    /// Number of tasks currently visible under the active filters.
    pub fn visible_count(&self) -> usize {
        self.visible.get().len()
    }
}

/// Initial dashboard contents.
fn seed() -> Vec<Task> {
    vec![
        Task {
            id: 1,
            title: "Draft the launch post".into(),
            done: false,
            priority: Priority::High,
        },
        Task {
            id: 2,
            title: "Review pull requests".into(),
            done: true,
            priority: Priority::Normal,
        },
        Task {
            id: 3,
            title: "Water the office plants".into(),
            done: false,
            priority: Priority::Low,
        },
        Task {
            id: 4,
            title: "Prepare demo script".into(),
            done: true,
            priority: Priority::High,
        },
        Task {
            id: 5,
            title: "Archive old tickets".into(),
            done: true,
            priority: Priority::Low,
        },
    ]
}
