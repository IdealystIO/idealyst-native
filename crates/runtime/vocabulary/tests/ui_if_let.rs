//! `if let` inside `ui!` is reactive like any `match`: `if let Some(n) =
//! sig.get() { … }` re-binds and rebuilds when the signal changes (as a
//! plain Rust `if let` it would run once and freeze at the signal's first
//! value, with nothing to say why). Its patterns bind by value, as a
//! plain `if let` does.

use host_mock::Harness;
use runtime_macros::ui;
use runtime_vocabulary::glue::*;

fn texts(log: &[String]) -> Vec<String> {
    log.iter().filter(|l| l.contains(" text ")).cloned().collect()
}

#[test]
fn regression_if_let_on_a_signal_rebuilds_when_it_changes() {
    let h = Harness::new();
    let sig = h.world.enter(|| runtime_world::signal(None::<i32>));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if let Some(n) = sig.get() { text { "n={n}" } } else { text { "none" } }
            }
        }
    }));
    assert_eq!(texts(&h.take_log()), ["create n2 text \"none\""]);
    h.world.enter(|| sig.set(Some(3)));
    h.flush();
    assert_eq!(texts(&h.take_log()), ["create n3 text \"n=3\""], "the branch re-bound");
    h.world.enter(|| sig.set(None));
    h.flush();
    assert_eq!(texts(&h.take_log()).len(), 1, "and back");
}

#[test]
fn if_let_on_a_plain_value_stays_static() {
    let h = Harness::new();
    let value = Some(7);
    let _r = h.mount(h.world.enter(|| ui! { view() { if let Some(n) = value { text { "n={n}" } } } }));
    let log = h.take_log();
    assert!(!log.iter().any(|l| l.contains("anchor")), "no reactive region for a static scrutinee: {log:?}");
    assert_eq!(texts(&log), ["create n1 text \"n=7\""]);
}

fn takes_u64(id: u64) -> u64 {
    id
}

/// Regression: once reactive, `if let Some(id) = pending.get()` bound
/// `id: &u64` (the arm matched the `switch`'s `&S`), so code written as a
/// plain `if let` stopped compiling — the shipped `confirm_dialog_overlay`
/// recipe among it (`i.id != id`). This test does not compile unless the
/// binding is owned.
#[test]
fn regression_reactive_if_let_binds_by_value() {
    let h = Harness::new();
    let pending = h.world.enter(|| runtime_world::signal(Some(4u64)));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if let Some(id) = pending.get() {
                    text { format!("id={}", takes_u64(id)) }
                }
            }
        }
    }));
    assert_eq!(texts(&h.take_log()), ["create n2 text \"id=4\""]);
}
