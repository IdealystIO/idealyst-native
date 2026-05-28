//! `reactive-loops` — a small demo that flexes the framework's reactive
//! iteration, with the loop bodies being real `#[component]`s (not
//! helper fns). Every list is a `for … in …` written directly inside
//! `ui!`; the iterable's TYPE decides reactivity:
//!
//!   * `for row in items` — `items: Signal<Vec<Row>>` → REACTIVE. Each
//!     iteration mounts an `ItemRow` component; add/remove re-renders the
//!     list (no manual diffing, no `.get()`). Each `Row` owns its own
//!     `count: Signal<i32>`, so a row's `+` updates ONLY that row, and
//!     the count SURVIVES add/remove (state lives in the data, not the
//!     render scope).
//!   * `for i in 0..count.get()` — reactive COUNT. Each iteration mounts
//!     a `GridCell` component; the grid grows/shrinks as `count` changes.
//!   * `for label in LEGEND` — a plain `&[&str]` → STATIC (built once).
//!     Same syntax, different type → different lowering.
//!
//! Components are `#[component]` fns invoked PascalCase in `ui!`
//! (`ItemRow(...)`, `GridCell(...)`, …). Defined leaf-first because the
//! per-component invocation macro is textually scoped — `app()` is last.

use idea_ui::{
    install_idea_theme, light_theme, typography_kind, Card, CardPadding, Stack, StackAxis,
    StackGap, StackPadding, Typography,
};
use runtime_core::{component, memo, signal, ui, Primitive, Signal};

// ---------------------------------------------------------------------------
// Per-target SDK-registration hook the CLI-generated wrappers call before
// mount. The wrappers pass `&mut backend.borrow_mut()` (a `RefMut`), which
// deref-coerces to the concrete `&mut <Backend>` here — a generic
// `<B: Backend>` can't accept that (it'd infer `B = RefMut<…>`), so the
// signature must be per-backend concrete. No third-party SDKs here, so each
// body is empty.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios::IosBackend) {}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android::AndroidBackend) {}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}

/// Static data — a plain array. `for label in LEGEND` lowers to a
/// built-once list (the type isn't a signal): the SAME `for` syntax is
/// static when the iterable is static.
const LEGEND: &[&str] = &["type-driven", "no .get() heuristic", "flat siblings"];

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
fn Header() -> Primitive {
    ui! {
        Stack(gap = StackGap::Sm) {
            Typography(content = "Reactive loops".to_string(), kind = typography_kind::H1.into())
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
fn ItemRow(props: &ItemRowProps) -> Primitive {
    let id = props.row.id;
    let count = props.row.count;
    let items = props.items;
    let label = props.row.label.clone();

    let inc = move || count.update(|n| *n += 1);
    let remove = move || items.update(|l| l.retain(|r| r.id != id));

    // The count is an inline primitive `Text`, NOT `Typography`: `ui!`
    // wraps a `.get()`-bearing `Text(content = …)` in a reactive
    // `Derived<String>` so only THIS row's count re-renders. User
    // components like `Typography` take their props as a one-time
    // snapshot, so they wouldn't update.
    ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
            Typography(content = label)
            Text(content = format!("clicked {}×", count.get()))
            Button(label = "+".to_string(), on_click = inc)
            Button(label = "Remove".to_string(), on_click = remove)
        }
    }
}

struct GridCellProps {
    index: usize,
}

#[component]
fn GridCell(props: &GridCellProps) -> Primitive {
    let i = props.index;
    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = format!("#{}", i))
        }
    }
}

#[component]
fn Legend() -> Primitive {
    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = "Static — for label in LEGEND (a plain array, built once)".to_string(), kind = typography_kind::H3.into())
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
fn DynamicList(props: &DynamicListProps) -> Primitive {
    let items = props.items;
    let next_id = props.next_id;

    // Append a fresh row with its OWN count signal. The `for row in
    // items` region re-runs; existing rows' count signals (in the Vec)
    // survive untouched.
    let add = move || {
        let id = next_id.get();
        next_id.set(id + 1);
        items.update(|l| {
            l.push(Row { id, label: format!("Item {}", id), count: signal!(0) });
        });
    };
    let clear = move || items.set(Vec::new());

    // Derived aggregate over every row's count. `memo` isn't touched by
    // the reactivity rewriter, so the inner `r.count.get()` is fine.
    let total = memo(move || items.get().iter().map(|r| r.count.get()).sum::<i32>());

    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = "Dynamic list — for row in items → ItemRow(...)".to_string(), kind = typography_kind::H3.into())
            // Inline reactive `Text` — reads `items` + the `total` memo,
            // so it re-renders on add/remove/per-row clicks.
            Text(content = format!("{} row(s), {} total clicks", items.get().len(), total.get()))
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
    count: Signal<usize>,
}

/// Section 2 — the reactive count grid: `for i in 0..count.get()`.
#[component]
fn CountGrid(props: &CountGridProps) -> Primitive {
    let count = props.count;

    let inc = move || count.update(|n| *n += 1);
    let dec = move || {
        count.update(|n| {
            if *n > 0 {
                *n -= 1;
            }
        });
    };
    ui! {
        Card(padding = CardPadding::Md) {
            Typography(content = "Reactive count — for i in 0..count.get() → GridCell(...)".to_string(), kind = typography_kind::H3.into())
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                Button(label = "−".to_string(), on_click = dec)
                Text(content = format!("count = {}", count.get()))
                Button(label = "+".to_string(), on_click = inc)
            }
            // Reactive COUNT: a range whose bound reads a signal. Each cell
            // is a `GridCell` component; the grid grows/shrinks reactively.
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                for i in 0..count.get() {
                    GridCell(index = i)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// App entry — last, so every component's invocation macro is in scope.
// ---------------------------------------------------------------------------

#[component]
pub fn app() -> Primitive {
    install_idea_theme(light_theme());

    let next_id: Signal<u32> = signal!(3);
    let items: Signal<Vec<Row>> = signal!(vec![
        Row { id: 0, label: "Reactive".to_string(), count: signal!(0) },
        Row { id: 1, label: "for".to_string(), count: signal!(0) },
        Row { id: 2, label: "loops".to_string(), count: signal!(0) },
    ]);
    let count: Signal<usize> = signal!(4);

    ui! {
        Stack(gap = StackGap::Xl, padding = StackPadding::Lg) {
            Header()
            DynamicList(items = items, next_id = next_id)
            CountGrid(count = count)
            Legend()
        }
    }
}
