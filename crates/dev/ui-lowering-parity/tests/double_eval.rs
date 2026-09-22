//! Regressions for prop expressions the emitter used to splice TWICE.
//!
//! Both were invisible for a literal — `.gap(4.0).gap(4.0)` and
//! `.draw_in((300, Linear).0, (300, Linear).1)` build the right thing —
//! and wrong for anything with a side effect or a cost.
//!
//! Only ONE of them is observable from here, and the asymmetry is worth
//! knowing:
//!
//! - `icon(draw_in = …)` WAS a double evaluation, and this file's test
//!   fails against the old emission. It could not be masked by the slot
//!   hoist, because the double splice is exactly why `draw_in` was
//!   `Construct`-placed in `ui_split` (a hoisted local would have
//!   silently turned two evaluations into one). The fix binds the tuple
//!   to a local in the emitter, which also let the `Construct` exemption
//!   go away — so `draw_in` now hoists like every other prop.
//! - `flat_list(gap = …)` is a double CALL, not a double evaluation, and
//!   only since phase 1: the slot hoist already reduced it to
//!   `.gap(__ui_s0).gap(__ui_s0)`. Two `.gap(f32)` calls store the same
//!   value, so nothing here can observe it —
//!   `runtime-macros`' `regression_flat_list_gap_lowers_to_one_call`
//!   pins the emission instead. The test below still earns its place: it
//!   is what proves the evaluation stays single as the emitter changes.

use std::cell::Cell;

use runtime_macros::ui;
use runtime_shared::primitives::icon::{FillRule, IconData};
use runtime_vocabulary::glue::primitives::flat_list::fixed_size;
use runtime_vocabulary::glue::{signal, Easing, Element};
use runtime_world::World;

thread_local! {
    static CALLS: Cell<u32> = const { Cell::new(0) };
}

fn calls() -> u32 {
    CALLS.with(|c| c.get())
}

fn reset() {
    CALLS.with(|c| c.set(0));
}

fn bump<T>(value: T) -> T {
    CALLS.with(|c| c.set(c.get() + 1));
    value
}

fn an_icon() -> IconData {
    IconData {
        view_box: (24, 24),
        paths: &["M0 0"],
        fill_rule: FillRule::NonZero,
        filled: false,
    }
}

/// `icon(draw_in = expr)` used to splice `expr` twice, so the duration
/// and the easing came from two separate evaluations.
#[test]
fn icon_draw_in_evaluates_its_tuple_once() {
    reset();
    let _el: Element = ui! {
        icon(data = an_icon(), draw_in = bump((300u32, Easing::Linear)))
    };
    assert_eq!(calls(), 1, "`draw_in`'s expression must be evaluated exactly once");
}

/// `flat_list(gap = expr)` used to lower to `.gap(v).gap(v)` — `gap` was
/// in the by-name builder table AND claimed by `spacing_call`.
#[test]
fn flat_list_gap_evaluates_once() {
    reset();
    let world = World::new();
    world.enter(|| {
    let rows = signal(::std::vec::Vec::<u8>::new());
    let _el: Element = ui! {
        flat_list(
            data = rows,
            key = |i, _item: &u8| i as u64,
            size = fixed_size(24.0),
            render = |_i, _item: &u8| ui! { text { "row" } }.into(),
            gap = bump(4.0f32)
        )
    };
    assert_eq!(calls(), 1, "`gap`'s expression must be evaluated exactly once");
    });
}

/// The sibling spelling must stay single-evaluation too: `main_spacing`
/// / `cross_spacing` lower to one `.spacing(m, c)`.
#[test]
fn flat_list_spacing_pair_evaluates_each_side_once() {
    reset();
    let world = World::new();
    world.enter(|| {
    let rows = signal(::std::vec::Vec::<u8>::new());
    let _el: Element = ui! {
        flat_list(
            data = rows,
            key = |i, _item: &u8| i as u64,
            size = fixed_size(24.0),
            render = |_i, _item: &u8| ui! { text { "row" } }.into(),
            main_spacing = bump(4.0f32),
            cross_spacing = bump(8.0f32)
        )
    };
    assert_eq!(calls(), 2, "one evaluation per spacing axis");
    });
}
