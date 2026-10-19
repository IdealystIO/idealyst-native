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

/// The `confirm_dialog_overlay` recipe's shape: a closure inside the branch
/// keeps the binding (`if let Some(id) = pending.get() { button(on_click =
/// move || … id …) }`). As a plain Rust `if let` the binding was owned and
/// this compiled; lowered through the reactive `match` it bound a reference
/// into the matched value, which no `'static` closure may keep
/// ("borrowed data escapes"). A reactive `if let` binds owned values.
#[test]
fn regression_reactive_if_let_binding_moves_into_a_closure() {
    let h = Harness::new();
    let sig = h.world.enter(|| runtime_world::signal(None::<i32>));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if let Some(n) = sig.get() { text { move || format!("next={}", n + 1) } }
            }
        }
    }));
    h.take_log();
    h.world.enter(|| sig.set(Some(41)));
    h.flush();
    // A closure child creates its text node empty, then writes it.
    let log = h.take_log();
    assert!(log.iter().any(|l| l == "update_text n3 \"next=42\""), "{log:?}");
}

/// A scrutinee whose type isn't `Clone` (reachable through the reactive-call
/// shape, `key(state)`) can't be bound by value; it keeps reference bindings,
/// so making `if let` own its bindings adds no `Clone` requirement.
#[test]
fn reactive_if_let_on_a_non_clone_value_still_binds() {
    #[derive(PartialEq)]
    struct Token(i32);
    fn token(v: Option<i32>) -> Option<Token> {
        v.map(Token)
    }
    let h = Harness::new();
    let sig = h.world.enter(|| runtime_world::signal(None::<i32>));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if let Some(_t) = token(sig) { text { "some" } } else { text { "none" } }
            }
        }
    }));
    assert_eq!(texts(&h.take_log()), ["create n2 text \"none\""]);
    h.world.enter(|| sig.set(Some(5)));
    h.flush();
    assert_eq!(texts(&h.take_log()), ["create n3 text \"some\""]);
}

type Callback = Option<std::rc::Rc<dyn Fn() -> i32>>;

fn no_anchor(log: &[String]) -> bool {
    !log.iter().any(|l| l.contains("anchor"))
}

/// Regression: the gate counted every method call as a possible signal
/// read, so the common `if let Some(cb) = props.on_add.clone()` lowered to
/// `switch` and failed on its `PartialEq` bound (E0277 "can't compare `dyn
/// Fn()`"). A `.clone()` reads no more than its receiver; this does not
/// compile unless the scrutinee stays a plain `if let`.
#[test]
fn regression_if_let_on_a_cloned_callback_stays_static() {
    let h = Harness::new();
    let on_add: Callback = Some(std::rc::Rc::new(|| 7));
    let _r = h.mount(h.world.enter(|| {
        ui! { view() { if let Some(add) = on_add.clone() { text { format!("add={}", add()) } } } }
    }));
    let log = h.take_log();
    assert!(no_anchor(&log), "no reactive region for a cloned static value: {log:?}");
    assert_eq!(texts(&log), ["create n1 text \"add=7\""]);
}

/// Regression: inside a `for` over a borrowed slice, `if let Some(band) =
/// entry.1.clone()` went down the reactive path, whose `'static` branch
/// can't hold the borrowed `entry` (E0597 "does not live long enough").
/// This does not compile unless the scrutinee stays a plain `if let`.
#[test]
fn regression_if_let_on_a_cloned_borrowed_field_stays_static() {
    let h = Harness::new();
    let rows: Vec<(u32, Option<String>)> = vec![(1, Some("a".into())), (2, None)];
    let rows_ref = &rows;
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                for entry in rows_ref.iter() {
                    if let Some(band) = entry.1.clone() { text { format!("{}:{band}", entry.0) } }
                }
            }
        }
    }));
    let log = h.take_log();
    assert!(no_anchor(&log), "{log:?}");
    assert_eq!(texts(&log), ["create n1 text \"1:a\""]);
}

/// The same gate fix for `match`: a cloned non-`PartialEq` scrutinee is a
/// plain `match`.
#[test]
fn match_on_a_cloned_callback_stays_static() {
    let h = Harness::new();
    let on_add: Callback = None;
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                match on_add.clone() {
                    Some(_) => { text { "some" } }
                    None => { text { "none" } }
                }
            }
        }
    }));
    let log = h.take_log();
    assert!(no_anchor(&log), "{log:?}");
    assert_eq!(texts(&log), ["create n1 text \"none\""]);
}

/// A clone keeps type-driven dispatch for a bare `if`: `open.clone()` on a
/// `Signal<bool>` is still a signal, so the branch stays live.
#[test]
fn if_on_a_cloned_signal_stays_reactive() {
    let h = Harness::new();
    let open = h.world.enter(|| runtime_world::signal(false));
    let _r = h.mount(h.world.enter(|| ui! { view() { if open.clone() { text { "open" } } } }));
    assert!(texts(&h.take_log()).is_empty());
    h.world.enter(|| open.set(true));
    h.flush();
    assert_eq!(texts(&h.take_log()), ["create n3 text \"open\""]);
}

fn handler_for(n: Option<i32>) -> Callback {
    n.map(|n| std::rc::Rc::new(move || n * 10) as std::rc::Rc<dyn Fn() -> i32>)
}

/// A genuinely reactive scrutinee with no `PartialEq` lowers too (it failed
/// on `switch`'s bound). Nothing can compare two values, so every change
/// rebuilds the arm — including a change inside the same arm, `Some(a)` to
/// `Some(b)`.
#[test]
fn reactive_if_let_on_a_non_partial_eq_value_rebuilds_on_every_change() {
    let h = Harness::new();
    let sig = h.world.enter(|| runtime_world::signal(None::<i32>));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if let Some(f) = handler_for(sig.get()) { text { format!("v={}", f()) } } else { text { "none" } }
            }
        }
    }));
    assert_eq!(texts(&h.take_log()), ["create n2 text \"none\""]);
    h.world.enter(|| sig.set(Some(1)));
    h.flush();
    assert_eq!(texts(&h.take_log()), ["create n3 text \"v=10\""]);
    h.world.enter(|| sig.set(Some(2)));
    h.flush();
    assert_eq!(texts(&h.take_log()), ["create n4 text \"v=20\""], "same arm, new value");
}

/// The `match` counterpart of the test above.
#[test]
fn reactive_match_on_a_non_partial_eq_value_rebuilds() {
    let h = Harness::new();
    let sig = h.world.enter(|| runtime_world::signal(None::<i32>));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                match handler_for(sig.get()) {
                    Some(f) => { text { format!("v={}", f()) } }
                    None => { text { "none" } }
                }
            }
        }
    }));
    assert_eq!(texts(&h.take_log()), ["create n2 text \"none\""]);
    h.world.enter(|| sig.set(Some(3)));
    h.flush();
    assert_eq!(texts(&h.take_log()), ["create n3 text \"v=30\""]);
}

/// A `PartialEq` scrutinee keeps the value dedup: a re-fire that produces
/// an equal value leaves the mounted arm alone.
#[test]
fn reactive_if_let_on_a_partial_eq_value_keeps_its_arm_when_equal() {
    let h = Harness::new();
    let sig = h.world.enter(|| runtime_world::signal(Some(4)));
    let _r = h.mount(h.world.enter(|| {
        ui! { view() { if let Some(half) = sig.get().map(|v| v / 2) { text { "half={half}" } } } }
    }));
    assert_eq!(texts(&h.take_log()), ["create n2 text \"half=2\""]);
    h.world.enter(|| sig.set(Some(5)));
    h.flush();
    assert!(texts(&h.take_log()).is_empty(), "5 / 2 == 4 / 2: no rebuild");
}
