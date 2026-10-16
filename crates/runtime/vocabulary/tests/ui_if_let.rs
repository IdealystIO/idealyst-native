//! `if let` inside `ui!` is reactive like any `match`: `if let Some(n) =
//! sig.get() { … }` re-binds and rebuilds when the signal changes. It used
//! to lower to a plain Rust `if let`, which ran once and froze at the
//! signal's first value — no anchor, no effect, nothing to say why (the
//! robot-e2e demo's overlay never opened).

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
