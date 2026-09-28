//! Live cells: a `#[component]`'s literal props, made writable.
//!
//! A primitive's literal prop has a setter on the backend seam. A
//! component's does not — its props were consumed and its body already
//! ran — so before this, `Typography(content = "Hello")` and
//! `Button(label = "Save")` (most of the text in an idea-ui app) were
//! either REBUILT from a `Clone` copy of the props, or, for the common
//! non-`Clone` props type, left "waiting for the next render".
//!
//! # The mechanism
//!
//! A data prop of a `#[component]` is `Reactive<T>` by default, and a
//! component that renders one through a binding (`text(content)`,
//! `.bind(…)`, a style closure) already updates when a `Dynamic` value
//! changes. So under `ui-overlay`, a prop the CALL SITE wrote as a literal
//! is handed to the component as `Reactive::Dynamic` reading a signal
//! seeded with that literal ([`liven`]), and the signal is registered
//! here under `(site, node, prop)`. A patch then writes the signal
//! ([`set`]) and the component's own bindings do the rest: no rebuild,
//! so its local state survives, and no dependence on `Clone`.
//!
//! Which props get a cell is decided at COMPILE time, by two parties
//! that each know half:
//!
//! - `ui!` knows which props the call site wrote as literals (the same
//!   classification the build-time descriptor uses) and passes their
//!   names to [`super::enter_live`];
//! - `#[component]` / `#[props]` know each field's type, and generate a
//!   `liven` arm only for a field that is `Reactive<T>` with a `T` a
//!   literal can build. A `#[prop(static)]` field, or any non-`Reactive`
//!   one, has no arm, so it keeps today's path (rebuild when the props
//!   are `Clone`, else the next render).
//!
//! # "Registered" is not "live": the subscriber check
//!
//! A body that reads the prop ONCE while building — `props.label.get()`
//! into a `format!`, a structural `if` — bakes the value in, and writing
//! the signal changes nothing on screen. Reporting that as applied is the
//! failure this overlay exists to avoid. So [`set`] asks the kernel how
//! many effects read the cell on their latest run, and a cell nothing
//! reads is reported NOT live; the caller then falls back to the rebuild
//! path, or reports the prop as waiting. A body that reads a prop both
//! ways (bound AND baked) is counted live — a known limit, bounded by the
//! next render, which the `Element` path still guarantees.
//!
//! # Lifetimes
//!
//! A cell's signal is created in the CALLER's reactive scope (the props
//! are built before the component's own scope opens), exempt from the
//! hot-reload state carrier so it cannot shift the caller's state
//! ordinals ([`runtime_world::hot_state_exempt`]). It is freed with that
//! scope, and a freed cell is pruned here on the next visit. The
//! `Dynamic` read is guarded by `is_alive` for the one shape where the
//! component could outlive its caller's scope (an `Element` built in one
//! scope and mounted by a handler into longer-lived storage): it keeps
//! showing the last value rather than aborting on a stale handle — a
//! dev-only cell the author never wrote must not be what crashes their
//! app.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_template::LiteralValue;
use runtime_world::Signal;

use crate::glue::Reactive;

/// What writing one cell did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wrote {
    /// The cell's scope is gone; prune it.
    Dead,
    /// Written, and at least one effect reads it: the screen follows.
    Live,
    /// Written (or not convertible), but nothing reads it in a binding.
    Unread,
}

struct Cell {
    site: u64,
    node: u32,
    name: &'static str,
    write: Box<dyn Fn(&LiteralValue) -> Wrote>,
    alive: Box<dyn Fn() -> bool>,
}

thread_local! {
    /// Every registered cell. Thread-local for the same reason the scene's
    /// live-instance registry is: a scene is single-threaded, and in the
    /// dev sidecar one thread is one session.
    static CELLS: RefCell<Vec<Cell>> = const { RefCell::new(Vec::new()) };
}

/// Turn a literal prop into a live cell, in place.
///
/// Called from a props type's generated `__overlay_liven`, once per prop
/// the call site wrote as a literal. `convert` is the generated per-field
/// literal conversion (the same one `__apply_literal` uses), as a plain
/// `fn` so it costs no closure type per field.
///
/// A value that is already `Dynamic` is left alone: the call site passed
/// code, not a literal, and there is nothing for a patch to address.
pub fn liven<T: Clone + PartialEq + 'static>(
    slot: &mut Reactive<T>,
    site: u64,
    node: u32,
    name: &'static str,
    convert: fn(&LiteralValue) -> Option<Reactive<T>>,
) {
    let Reactive::Static(initial) = slot else { return };
    let initial = initial.clone();
    let sig: Signal<T> = runtime_world::hot_state_exempt(|| runtime_world::signal(initial.clone()));
    // `Dynamic` over an `Rc<dyn Fn>`: the same shape a `Signal` prop
    // coerces to, so a component cannot tell a live cell from an author's
    // signal — which is the point.
    *slot = Reactive::Dynamic(Rc::new(move || {
        if sig.is_alive() { sig.get() } else { initial.clone() }
    }));
    let write = Box::new(move |literal: &LiteralValue| {
        if !sig.is_alive() {
            return Wrote::Dead;
        }
        let Some(Reactive::Static(value)) = convert(literal) else { return Wrote::Unread };
        let read = sig.subscriber_count() > 0;
        sig.set(value);
        if read { Wrote::Live } else { Wrote::Unread }
    });
    let alive = Box::new(move || sig.is_alive());
    CELLS.with(|c| {
        let mut c = c.borrow_mut();
        // Amortized prune, as the scene's live registry does: a cell dies
        // with its caller's scope and nothing else ever visits it unless
        // its prop is patched.
        if c.len() % 64 == 0 {
            c.retain(|cell| (cell.alive)());
        }
        c.push(Cell { site, node, name, write, alive });
    });
}

/// Write `value` into every cell registered for `(site, node, name)`.
///
/// `Some(n)` when there were cells and EVERY one of them is read by a
/// binding — `n` instances now show the value (a `for` builds several).
/// `None` when there was no cell, or when any cell is unread: the caller
/// must take its other path for this prop, since at least one instance on
/// screen would not change. Dead cells are pruned on the way.
pub fn set(site: u64, node: u32, name: &str, value: &LiteralValue) -> Option<usize> {
    // Collect the writers first and run them with the registry released:
    // a write can run effects synchronously, and an effect that builds a
    // component registers a NEW cell — re-entering `CELLS`.
    let matching: Vec<usize> = CELLS.with(|c| {
        c.borrow()
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell.site == site && cell.node == node && cell.name == name)
            .map(|(i, _)| i)
            .collect()
    });
    if matching.is_empty() {
        return None;
    }
    let taken: Vec<(usize, Cell)> = CELLS.with(|c| {
        let mut c = c.borrow_mut();
        // Highest index first so earlier indices stay valid.
        matching.iter().rev().map(|&i| (i, c.swap_remove(i))).collect()
    });
    let mut live = 0usize;
    let mut unread = false;
    let mut keep = Vec::with_capacity(taken.len());
    for (_, cell) in taken {
        match (cell.write)(value) {
            Wrote::Dead => {}
            Wrote::Live => {
                live += 1;
                keep.push(cell);
            }
            Wrote::Unread => {
                unread = true;
                keep.push(cell);
            }
        }
    }
    CELLS.with(|c| c.borrow_mut().extend(keep));
    (live > 0 && !unread).then_some(live)
}

/// How many registered cells are still alive. Diagnostics and tests.
pub fn count() -> usize {
    CELLS.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|cell| (cell.alive)());
        c.len()
    })
}

/// Forget every cell. Test support; see [`super::reset`].
pub(crate) fn clear() {
    CELLS.with(|c| c.borrow_mut().clear());
}
