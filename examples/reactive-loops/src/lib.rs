//! `reactive-loops` — a small demo that flexes two type-driven reactive
//! mechanisms, with every body being a real `#[component]` (not a helper
//! fn). No `.get()` heuristic anywhere: the TYPE decides reactivity.
//!
//! ## Reactive iteration — `for … in …` directly inside `ui!`
//!
//!   * `for row in items` — `items: Signal<Vec<Row>>` → REACTIVE. Each
//!     iteration mounts an `ItemRow` component; add/remove re-renders the
//!     list (no manual diffing). Each `Row` owns its own
//!     `count: Signal<i32>`, so a row's `+` updates ONLY that row, and
//!     the count SURVIVES add/remove (state lives in the data, not the
//!     render scope).
//!   * `for i in 0..count.get()` — reactive COUNT, where `count` is a
//!     `Reactive<usize>` DERIVED from the list's length
//!     (`rx!(items.get().len())`), NOT a separate signal — so it can't
//!     desync. Each iteration mounts a `GridCell`; add/remove rows above
//!     and the grid grows/shrinks.
//!   * `for label in LEGEND` — a plain `&[&str]` → STATIC (built once).
//!     Same syntax, different type → different lowering.
//!
//! ## Reactive component props — `Typography(content = …)`
//!
//! idea-ui's `Typography.content` is a `Reactive<String>`. A bare value
//! is a static snapshot; `rx!(expr)` wraps a `.get()`-bearing expression
//! as a live `Reactive::Dynamic`, so the text re-paints IN PLACE (no
//! parent rebuild) with full styling. Every reactive label/count below
//! is a styled `Typography(content = rx!(…))` — never a raw primitive.
//!
//! Components are `#[component]` fns invoked PascalCase in `ui!`
//! (`ItemRow(...)`, `GridCell(...)`, …). Defined leaf-first because the
//! per-component invocation macro is textually scoped — `app()` is last.

use idea_ui::{
    install_idea_theme, light_theme, typography_kind, Card, CardPadding, Stack, StackAxis,
    StackGap, StackPadding, Typography,
};
use runtime_core::{component, memo, rx, signal, ui, Element, Reactive, Signal};

// ---------------------------------------------------------------------------
// SDK-registration hook the CLI-generated wrappers call before mount. No
// third-party SDKs here, so it's an empty generic over `Backend` —
// backend-agnostic, no per-target `#[cfg]` and no `backend-*` dep. The
// wrappers pass the concrete backend per platform (web/iOS by value,
// android via `&mut *b`), so `B` resolves to that backend.
// ---------------------------------------------------------------------------

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

/// Static data — a plain array. `for label in LEGEND` lowers to a
/// built-once list (the type isn't a signal): the SAME `for` syntax is
/// static when the iterable is static.
const LEGEND: &[&str] = &["This", "is", "a", "flat", "list", "of", "siblings"];

/// A list row. `count` is the row's OWN reactive state. It lives in the
/// data model (the `Signal<Vec<Row>>`), NOT the row's render scope —
/// that's what makes it survive a full-list rebuild: `Signal` is a
/// `Copy` handle, so cloning a `Row` out of `items.get()` copies the
/// HANDLE (same underlying signal), not a fresh signal.
#[derive(Clone)]
struct Row {
    id: u32,
    label: String,
    count: Signal<i32>,
}

// ---------------------------------------------------------------------------
// Leaf components (defined first so their invocation macros are in scope
// for the composite components below).
// ---------------------------------------------------------------------------

#[component]
fn Header() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            Typography(content = "Reactive loops".to_string(), kind = typography_kind::H1)
            Typography(
                content = "Each list is a `for … in …` in `ui!`, and the loop body is a real component. The iterable's TYPE decides reactivity — a Signal rebuilds, a plain value is static.".to_string(),
                muted = true,
            )
        }
    }
}

struct ItemRowProps {
    row: Row,
    items: Signal<Vec<Row>>,
}

/// One reactive row: label · its own click count · `+` (increments only
/// THIS row — fine-grained) · `Remove` (drops the row by id).
#[component]
fn ItemRow(props: &ItemRowProps) -> Element {
    let id = props.row.id;
    let count = props.row.count;
    let items = props.items;
    let label = props.row.label.clone();

    let inc = move || count.update(|n| *n += 1);
    let remove = move || items.update(|l| l.retain(|r| r.id != id));

    // The count is reactive STYLED text. `Typography.content` is a
    // `Reactive<String>`, and `rx!(expr)` wraps the `.get()`-bearing
    // expression as a live `Reactive::Dynamic` — so only THIS row's
    // count re-renders, with full Typography styling. A bare value
    // (`content = label`) is a static snapshot; a signal or `rx!(…)`
    // is live. Type-driven, no `.get()` heuristic.
    ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
            Typography(content = label)
            Typography(content = rx!(format!("clicked {}×", count.get())), muted = true)
            Button(label = "+".to_string(), on_click = inc)
            Button(label = "Remove".to_string(), on_click = remove)
        }
    }
}

struct GridCellProps {
    index: usize,
}

#[component]
fn GridCell(props: &GridCellProps) -> Element {
    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = format!("#{}", props.index))
        }
    }
}

#[component]
fn Legend() -> Element {
    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = "Static — for label in LEGEND (a plain array, built once)".to_string(), kind = typography_kind::H3)
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                for label in LEGEND {
                    Typography(content = label.to_string())
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Composite sections (use the leaf components above).
// ---------------------------------------------------------------------------

struct DynamicListProps {
    items: Signal<Vec<Row>>,
    next_id: Signal<u32>,
}

/// Section 1 — the reactive list: `for row in items { ItemRow(...) }`.
#[component]
fn DynamicList(props: &DynamicListProps) -> Element {
    let items = props.items;
    let next_id = props.next_id;

    // Append a fresh row with its OWN count signal. The `for row in
    // items` region re-runs; existing rows' count signals (in the Vec)
    // survive untouched.
    let add = move || {
        let id = next_id.get();
        next_id.set(id + 1);
        // Each new row carries its OWN `count` signal, allocated inline.
        // (Creating a signal inside `update` is fine — `update` doesn't
        // hold the arena borrow across its closure.)
        items.update(|l| l.push(Row { id, label: format!("Item {}", id), count: signal!(0) }));
    };
    let clear = move || items.set(Vec::new());

    // Derived aggregate over every row's count. `memo` isn't touched by
    // the reactivity rewriter, so the inner `r.count.get()` is fine.
    let total = memo(move || items.get().iter().map(|r| r.count.get()).sum::<i32>());

    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = "Dynamic list — for row in items → ItemRow(...)".to_string(), kind = typography_kind::H3)
            // Reactive styled text — `rx!` reads `items` + the `total`
            // memo, so it re-renders on add/remove/per-row clicks.
            Typography(content = rx!(format!("{} row(s), {} total clicks", items.get().len(), total.get())), muted = true)
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                Button(label = "Add item".to_string(), on_click = add)
                Button(label = "Clear".to_string(), on_click = clear)
            }
            // THE reactive loop. `items: Signal<Vec<Row>>`, so it's
            // reactive by type — each row is an `ItemRow` component, and
            // add/remove rebuilds the list while per-row counts persist.
            Stack(gap = StackGap::Sm) {
                for row in items {
                    ItemRow(row = row, items = items)
                }
            }
        }
    }
}

struct CountGridProps {
    /// The cell count — a `Reactive<usize>`, not a stored signal. The
    /// caller derives it from the list (`rx!(items.get().len())`), so
    /// there's nothing to keep in sync: the grid IS a view of the list.
    count: Reactive<usize>,
}

/// Section 2 — the reactive count grid: `for i in 0..count.get()`,
/// where `count` is derived from the list above (single source of
/// truth). Add/remove rows and the grid grows/shrinks.
#[component]
fn CountGrid(props: &CountGridProps) -> Element {
    let label_count = props.count.clone();
    let grid_count = props.count.clone();
    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = "Reactive count — for i in 0..count.get() → GridCell(...)".to_string(), kind = typography_kind::H3)
            Typography(content = rx!(format!("count = {} (one cell per row above)", label_count.get())), muted = true)
            // Reactive COUNT: the range bound reads a `Reactive<usize>`
            // that derives from the list. Each cell is a `GridCell`; the
            // grid grows/shrinks reactively when the list changes.
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                for i in 0..grid_count.get() {
                    GridCell(index = i)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Counter-example — state in the RENDER SCOPE instead of the data.
//
// `EphemeralRow` allocates its `count` with `signal!` INSIDE the
// component. Because `for label in labels` is a `Primitive::Each` that
// fully rebuilds the list on any change, every add re-mounts each row
// and re-allocates a FRESH signal — so the count RESETS to 0×. This is
// the opposite of `ItemRow` (count in the `Row` DATA, survives rebuild).
//
// It's not a framework bug — it's the consequence of putting per-item
// state in the ephemeral render scope under a rebuilding list. The fix
// is "state lives in the data" (what `ItemRow` does), or keyed
// reconciliation that preserves unchanged scopes (a future option).
// ---------------------------------------------------------------------------

struct EphemeralRowProps {
    label: String,
}

#[component]
fn EphemeralRow(props: &EphemeralRowProps) -> Element {
    let label = props.label.clone();
    // Allocated in the render scope — re-created on every list rebuild,
    // so this count does NOT survive an add. (Compare `ItemRow`.)
    let count = signal!(0);
    let inc = move || count.update(|n| *n += 1);
    ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
            Typography(content = label)
            Typography(content = rx!(format!("clicked {}×", count.get())), muted = true)
            Button(label = "+".to_string(), on_click = inc)
        }
    }
}

struct EphemeralListProps {
    labels: Signal<Vec<String>>,
}

#[component]
fn EphemeralList(props: &EphemeralListProps) -> Element {
    let labels = props.labels;
    let add = move || labels.update(|l| {
        let n = l.len() + 1;
        l.push(format!("Row {}", n));
    });
    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = "Render-scope state — RESETS on rebuild (anti-pattern)".to_string(), kind = typography_kind::H3)
            Typography(content = "Each row's count lives in the COMPONENT, not the data. Click a few +, then Add row — every count snaps back to 0×, because the list rebuild re-allocates each signal. The Dynamic list above keeps its counts because that state lives in the data.".to_string(), muted = true)
            Button(label = "Add row".to_string(), on_click = add)
            Stack(gap = StackGap::Sm) {
                for label in labels {
                    EphemeralRow(label = label)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// App entry — last, so every component's invocation macro is in scope.
// ---------------------------------------------------------------------------

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let items: Signal<Vec<Row>> = signal!(vec![
        Row { id: 0, label: "Reactive".to_string(), count: signal!(0) },
        Row { id: 1, label: "for".to_string(), count: signal!(0) },
        Row { id: 2, label: "loops".to_string(), count: signal!(0) },
    ]);
    // The id allocator is genuinely a counter (must stay monotonic
    // across removes), so it's its own signal — seeded from the initial
    // row count rather than a magic number.
    let next_id: Signal<u32> = signal!(items.get().len() as u32);

    // Separate list for the render-scope-state counter-example below.
    let labels: Signal<Vec<String>> = signal!(vec!["alpha".to_string(), "beta".to_string()]);

    ui! {
        Stack(gap = StackGap::Xl, padding = StackPadding::Lg) {
            Header()
            DynamicList(items = items, next_id = next_id)
            // count is DERIVED from the list, not a separate signal:
            // `rx!` makes it live so the grid tracks add/remove.
            CountGrid(count = rx!(items.get().len()))
            EphemeralList(labels = labels)
            Legend()
        }
    }
}
