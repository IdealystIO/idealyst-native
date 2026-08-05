//! Live panels for the reactivity lessons.
//!
//! Prose can assert that a write stages and commits at the flush; these
//! panels let a reader watch it. Each one is deliberately small enough to
//! read in full, and each reports a number the reader can predict from the
//! lesson — if the prediction and the panel disagree, the lesson is wrong.
//!
//! Two mechanisms do the reporting:
//!
//! - **A trace signal.** The handler writes a `Signal<String>` describing
//!   what it observed *while it ran* (before the flush). Because that write
//!   stages like every other, what you read after the flush is a faithful
//!   record of the pre-commit snapshot the handler saw.
//! - **A render counter.** A reactive text node IS an effect, so counting
//!   its runs counts effect runs. The counter is a plain `Cell` bumped
//!   inside the node's own `rx!` closure — no signal write from inside an
//!   effect, and the number is displayed by the very node that produced it.
//!
//! These panels are built inside a lesson screen, so their signals and
//! effects belong to that screen's realized scope. Navigating away drops
//! it and every counter starts over: that is drop-as-teardown, visible.

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::{component, memo, rx, signal, ui, Element, Memo, Signal};

use crate::styles::{DemoButton, DemoPanel, DemoReadout, DemoRow, DemoTrace};

fn button_style() -> runtime_core::StyleApplication {
    runtime_core::StyleApplication::new(DemoButton::sheet())
}

fn readout_style() -> runtime_core::StyleApplication {
    runtime_core::StyleApplication::new(DemoReadout::sheet())
}

fn trace_style() -> runtime_core::StyleApplication {
    runtime_core::StyleApplication::new(DemoTrace::sheet())
}

// =============================================================================
// Signals — a handler's reads never see its own staged writes.
// =============================================================================

/// Two buttons that both "add two", one the wrong way and one the right
/// way. `set(get() + 1)` twice lands +1 because the second `get()` still
/// reports the committed value; `update` twice lands +2 because each
/// closure composes on the staged value.
#[component]
pub fn StagedWriteDemo() -> Element {
    let count: Signal<i32> = signal(0);
    let trace: Signal<String> = signal("Press a button.".to_string());

    let set_twice = move || {
        let before = count.get();
        count.set(before + 1);
        let after_first = count.get();
        count.set(count.get() + 1);
        trace.set(format!(
            "set(get() + 1) twice — get() was {before} before, {after_first} between the two writes; \
             both staged {}",
            before + 1
        ));
    };

    let update_twice = move || {
        let before = count.get();
        count.update(|n| n + 1);
        count.update(|n| n + 1);
        trace.set(format!(
            "update(|n| n + 1) twice — started from {before}; the second closure saw {} staged",
            before + 1
        ));
    };

    let reset = move || {
        count.set(0);
        trace.set("Reset.".to_string());
    };

    ui! {
        view(style = DemoPanel()) {
            text(style = readout_style) { rx!(format!("count = {}", count.get())) }
            text(style = trace_style) { rx!(trace.get()) }
            view(style = DemoRow()) {
                button(label = "set(get() + 1) twice".to_string(), on_click = set_twice, style = button_style)
                button(label = "update twice".to_string(), on_click = update_twice, style = button_style)
                button(label = "reset".to_string(), on_click = reset, style = button_style)
            }
        }
    }
}

// =============================================================================
// The flush boundary — many writes, one logical update; guarded vs forced.
// =============================================================================

/// One handler writing two signals wakes the reader once. Re-setting a
/// signal to the value it already holds wakes nobody; `set_always` wakes
/// them anyway.
#[component]
pub fn FlushDemo() -> Element {
    let first: Signal<i32> = signal(0);
    let second: Signal<i32> = signal(0);

    // A plain counter, bumped by the reactive node's own closure. The node
    // is an effect, so this counts effect runs.
    let runs: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let runs_for_node = runs.clone();
    let readout = move || {
        let n = runs_for_node.get() + 1;
        runs_for_node.set(n);
        format!(
            "first = {} · second = {} · this node has run {n}\u{00d7}",
            first.get(),
            second.get()
        )
    };

    let write_both = move || {
        first.update(|n| n + 1);
        second.update(|n| n + 1);
    };
    let set_same = move || first.set(first.get());
    let set_always_same = move || first.set_always(first.get());

    ui! {
        view(style = DemoPanel()) {
            text(style = readout_style) { rx!(readout()) }
            text(style = trace_style) {
                "Both writes happen in one handler, so the flush runs the reader once."
                    .to_string()
            }
            view(style = DemoRow()) {
                button(label = "write both".to_string(), on_click = write_both, style = button_style)
                button(label = "set(first, same value)".to_string(), on_click = set_same, style = button_style)
                button(label = "set_always(first, same value)".to_string(), on_click = set_always_same, style = button_style)
            }
        }
    }
}

// =============================================================================
// Effects — dependencies are whatever this run read.
// =============================================================================

/// While `verbose` is off the reader never reads `count`, so bumping
/// `count` leaves the run counter alone. Turn `verbose` on and the next
/// bump wakes it.
#[component]
pub fn DependencyDemo() -> Element {
    let count: Signal<i32> = signal(0);
    let verbose: Signal<bool> = signal(false);

    let runs: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let runs_for_node = runs.clone();
    let readout = move || {
        let n = runs_for_node.get() + 1;
        runs_for_node.set(n);
        if verbose.get() {
            format!("verbose · count = {} · this node has run {n}\u{00d7}", count.get())
        } else {
            format!("quiet (count not read) · this node has run {n}\u{00d7}")
        }
    };

    let bump = move || count.update(|n| n + 1);
    let toggle = move || verbose.set(!verbose.get());

    ui! {
        view(style = DemoPanel()) {
            text(style = readout_style) { rx!(readout()) }
            text(style = trace_style) {
                "The quiet branch never reads count, so a write to count wakes nothing."
                    .to_string()
            }
            view(style = DemoRow()) {
                button(label = "count + 1".to_string(), on_click = bump, style = button_style)
                button(label = "toggle verbose".to_string(), on_click = toggle, style = button_style)
            }
        }
    }
}

// =============================================================================
// Derived state — the diamond, glitch-free.
// =============================================================================

/// A node reading both a signal and a memo over that signal. Memos settle
/// before reactions run, so the pair is consistent on every single run.
#[component]
pub fn DiamondDemo() -> Element {
    let value: Signal<i32> = signal(1);
    let times_ten: Memo<i32> = memo(move || value.get() * 10);

    let runs: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let runs_for_node = runs.clone();
    let readout = move || {
        let n = runs_for_node.get() + 1;
        runs_for_node.set(n);
        let v = value.get();
        let ten = times_ten.get();
        format!(
            "value = {v} · memo = {ten} · consistent: {} · this node has run {n}\u{00d7}",
            ten == v * 10
        )
    };

    let bump = move || value.update(|n| n + 1);
    let reset = move || value.set(1);

    ui! {
        view(style = DemoPanel()) {
            text(style = readout_style) { rx!(readout()) }
            text(style = trace_style) {
                "One write, one run, and the memo is never a generation behind."
                    .to_string()
            }
            view(style = DemoRow()) {
                button(label = "value + 1".to_string(), on_click = bump, style = button_style)
                button(label = "reset".to_string(), on_click = reset, style = button_style)
            }
        }
    }
}
