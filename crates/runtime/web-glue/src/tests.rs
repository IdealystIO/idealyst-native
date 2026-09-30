//! Rust-side unit tests against the mock host (`mock.rs`). Each test runs
//! on its own thread, and every piece of state (slab, registry, queues,
//! error slot) is thread-local, so tests are isolated.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::mock::{self, V};
use crate::{spawn_local, Closure, JsFuture, JsType, JsValue};

fn baseline() -> (usize, u32) {
    (JsValue::live_count(), JsValue::js_live_count())
}

// ---- handles --------------------------------------------------------------

#[test]
fn every_clone_and_drop_is_accounted_on_both_sides_of_the_slab() {
    let before = baseline();
    {
        let a = JsValue::from_str("a");
        let b = a.clone();
        let c = JsValue::from_f64(1.5);
        assert_eq!(JsValue::live_count(), before.0 + 3);
        assert_eq!(JsValue::js_live_count(), before.1 + 3);
        assert!(a.strict_eq(&b), "a clone is the same JS value");
        assert_ne!(a.raw(), b.raw(), "…in its own slot");
        drop(b);
        assert_eq!(JsValue::js_live_count(), before.1 + 2);
        drop((a, c));
    }
    assert_eq!(baseline(), before);
}

#[test]
fn undefined_owns_no_slot_and_its_drop_never_crosses_the_boundary() {
    let before = baseline();
    let u = JsValue::undefined();
    let u2 = u.clone();
    assert_eq!(u.raw(), 0);
    assert!(u2.is_undefined());
    assert_eq!(u.js_type(), JsType::Undefined);
    drop((u, u2));
    assert_eq!(baseline(), before);
}

#[test]
fn into_raw_and_from_raw_transfer_ownership_without_leaking_a_slot() {
    let before = baseline();
    let v = JsValue::from_str("moved");
    let raw = v.into_raw();
    assert_eq!(JsValue::live_count(), before.0, "Rust no longer owns it");
    assert_eq!(JsValue::js_live_count(), before.1 + 1, "…but the slot is still held");
    let back = unsafe { JsValue::from_raw(raw) };
    assert_eq!(back.as_string().as_deref(), Some("moved"));
    drop(back);
    assert_eq!(baseline(), before);
}

#[test]
#[should_panic(expected = "released twice")]
fn regression_double_release_of_a_raw_handle_is_loud() {
    let raw = JsValue::from_str("x").into_raw();
    unsafe {
        drop(JsValue::from_raw(raw));
        drop(JsValue::from_raw(raw));
    }
}

// ---- strings --------------------------------------------------------------

#[test]
fn strings_round_trip_byte_exact_including_non_ascii() {
    let before = baseline();
    for s in [
        "",
        "plain ascii",
        "é ü ñ — “quotes”",
        "日本語テキスト",
        "emoji 🦀🎉 and ZWJ 👩‍👩‍👧",
        "nul\0inside",
        &"x".repeat(100_000),
    ] {
        let v = JsValue::from_str(s);
        assert_eq!(v.js_type(), JsType::String);
        assert_eq!(v.as_string().as_deref(), Some(s));
        assert_eq!(v.to_js_string().unwrap(), s);
    }
    assert_eq!(baseline(), before);
}

#[test]
fn a_non_string_is_none_not_an_empty_string() {
    assert_eq!(JsValue::from_f64(3.0).as_string(), None);
    assert_eq!(JsValue::undefined().as_string(), None);
    assert_eq!(JsValue::from_f64(3.0).as_f64(), Some(3.0));
    assert_eq!(JsValue::from_bool(true).as_bool(), Some(true));
}

#[test]
fn glue_alloc_hands_out_buffers_string_can_adopt() {
    // The contract `string::receive` relies on: capacity == len, align 1.
    let p = crate::string::__glue_alloc(5);
    unsafe {
        std::ptr::copy_nonoverlapping(b"hello".as_ptr(), p, 5);
        let s = String::from_raw_parts(p, 5, 5);
        assert_eq!(s, "hello");
    }
    assert!(!crate::string::__glue_alloc(0).is_null(), "zero-length is dangling, never null");
}

// ---- exceptions -----------------------------------------------------------

#[test]
fn a_throw_inside_a_catching_import_is_an_err_and_leaves_no_pending_error() {
    let before = baseline();
    let err = JsValue::null().get("x").unwrap_err();
    assert!(err.message().contains("TypeError"), "{}", err.message());
    assert!(crate::error::take_pending().is_none(), "the wrapper consumed the slot");
    drop(err);
    assert_eq!(baseline(), before, "the thrown value's slot is released with the JsError");
}

#[test]
fn reflect_get_and_set_round_trip() {
    let g = JsValue::global();
    g.set("answer", &JsValue::from_f64(42.0)).unwrap();
    assert_eq!(g.get("answer").unwrap().as_f64(), Some(42.0));
    assert!(g.get("missing").unwrap().is_undefined());
}

// ---- callbacks ------------------------------------------------------------

fn fire(c: &Closure, arg: V) -> Result<(), String> {
    mock::call_js_fn(&mock::val(c.as_js().raw()), arg)
}

#[test]
fn a_closure_runs_every_time_js_calls_it_and_owns_its_argument() {
    let before = baseline();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    let c = Closure::new(move |v: JsValue| s.borrow_mut().push(v.as_string().unwrap()));
    fire(&c, V::Str("one".into())).unwrap();
    fire(&c, V::Str("two".into())).unwrap();
    assert_eq!(*seen.borrow(), ["one", "two"]);
    drop(c);
    assert_eq!(baseline(), before, "argument slots released after each call");
    assert_eq!(Closure::live_count(), 0);
}

#[test]
fn regression_callback_invoked_after_drop_is_a_loud_error_not_a_call() {
    let ran = Rc::new(Cell::new(0));
    let r = ran.clone();
    let c = Closure::new(move |_| r.set(r.get() + 1));
    // JS keeps its own reference to the function (as an event target does).
    let js_fn = mock::val(c.as_js().raw());
    let id = c.id();
    drop(c);
    let err = mock::call_js_fn(&js_fn, V::Undefined).unwrap_err();
    assert!(err.contains("after its Rust owner dropped it"), "{err}");
    assert_eq!(ran.get(), 0);
    // And if the JS-side revocation were bypassed, Rust still refuses:
    let arg = mock::add(V::Num(1.0));
    assert_eq!(crate::callback::__glue_invoke(id, arg), crate::callback::INVOKE_UNKNOWN);
    assert_eq!(ran.get(), 0);
}

#[test]
fn a_once_callback_runs_once_then_refuses() {
    let ran = Rc::new(Cell::new(0));
    let r = ran.clone();
    let c = Closure::once(move |_| r.set(r.get() + 1));
    fire(&c, V::Undefined).unwrap();
    assert!(fire(&c, V::Undefined).is_err(), "the JS side is dead after one call");
    assert_eq!(ran.get(), 1);
    assert_eq!(Closure::live_count(), 0, "spent once-callbacks leave the registry");
}

#[test]
fn once_into_js_frees_its_entry_after_the_single_call() {
    let ran = Rc::new(Cell::new(false));
    let r = ran.clone();
    let before = Closure::live_count();
    let f = Closure::once_into_js(move |_| r.set(true));
    assert_eq!(Closure::live_count(), before + 1);
    mock::call_js_fn(&mock::val(f.raw()), V::Undefined).unwrap();
    assert!(ran.get());
    assert_eq!(Closure::live_count(), before);
}

#[test]
fn regression_a_gc_owned_closure_keeps_running_then_is_released_when_js_collects_it() {
    // The leak `into_js_value` exists to prevent: an element-lifetime
    // listener's Rust closure pinned for the life of the page after the
    // element (and so the function) was discarded.
    struct Captured(Rc<Cell<bool>>);
    impl Drop for Captured {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let dropped = Rc::new(Cell::new(false));
    let calls = Rc::new(Cell::new(0));
    let (c2, cap) = (calls.clone(), Captured(dropped.clone()));
    let before = Closure::live_count();
    let js = Closure::new(move |_| {
        let _ = &cap;
        c2.set(c2.get() + 1);
    })
    .into_js_value();
    let func = mock::val(js.raw());
    drop(js); // the slab slot, not the function's lifetime
    mock::call_js_fn(&func, V::Undefined).unwrap();
    mock::call_js_fn(&func, V::Undefined).unwrap();
    assert_eq!(calls.get(), 2, "still callable after its handle dropped");
    assert_eq!(Closure::live_count(), before + 1);
    assert!(!dropped.get());
    mock::collect_garbage();
    assert_eq!(Closure::live_count(), before, "released once JS collected it");
    assert!(dropped.get(), "the closure's captures dropped with it");
}

#[test]
fn a_closure_may_drop_itself_while_running() {
    let holder: Rc<RefCell<Option<Closure>>> = Rc::new(RefCell::new(None));
    let h = holder.clone();
    let c = Closure::new(move |_| {
        h.borrow_mut().take(); // drops the Closure that is running
    });
    let js_fn = mock::val(c.as_js().raw());
    *holder.borrow_mut() = Some(c);
    mock::call_js_fn(&js_fn, V::Undefined).unwrap();
    assert!(holder.borrow().is_none());
    assert_eq!(Closure::live_count(), 0, "discarded when the call returned");
    assert!(mock::call_js_fn(&js_fn, V::Undefined).is_err());
}

#[test]
fn a_closure_may_create_and_drop_other_closures_while_running() {
    let made: Rc<RefCell<Vec<Closure>>> = Rc::new(RefCell::new(Vec::new()));
    let m = made.clone();
    let c = Closure::new(move |_| {
        m.borrow_mut().push(Closure::new(|_| {}));
        if m.borrow().len() > 1 {
            m.borrow_mut().remove(0);
        }
    });
    for _ in 0..3 {
        fire(&c, V::Undefined).unwrap();
    }
    assert_eq!(made.borrow().len(), 1);
    assert_eq!(Closure::live_count(), 2);
}

#[test]
fn regression_recursive_invocation_is_refused_not_aliased() {
    // A handler that synchronously re-triggers itself (dispatchEvent inside
    // the listener). The inner call must be refused: the closure is out of
    // the registry while it runs, and running it twice would alias its
    // `&mut` state.
    let slot: Rc<RefCell<Option<V>>> = Rc::new(RefCell::new(None));
    let inner_result = Rc::new(RefCell::new(None));
    let (s, ir) = (slot.clone(), inner_result.clone());
    let c = Closure::new(move |_| {
        let me = s.borrow().clone().unwrap();
        *ir.borrow_mut() = Some(mock::call_js_fn(&me, V::Undefined));
    });
    *slot.borrow_mut() = Some(mock::val(c.as_js().raw()));
    fire(&c, V::Undefined).unwrap();
    let inner = inner_result.borrow_mut().take().unwrap();
    assert!(inner.unwrap_err().contains("recursively"));
    // …and the closure is still usable afterwards.
    assert_eq!(Closure::live_count(), 1);
}

// ---- futures / microtasks -------------------------------------------------

fn drain() {
    while mock::take_microtask_requests() > 0 {
        crate::task::__glue_microtask();
    }
}

#[test]
fn a_spawned_task_first_runs_on_a_microtask_not_inline() {
    let ran = Rc::new(Cell::new(false));
    let r = ran.clone();
    spawn_local(async move { r.set(true) });
    assert!(!ran.get());
    drain();
    assert!(ran.get());
    assert_eq!(crate::task::pending_work(), 0);
}

#[test]
fn a_burst_of_microtasks_costs_one_js_crossing() {
    let order = Rc::new(RefCell::new(Vec::new()));
    for i in 0..5 {
        let o = order.clone();
        crate::queue_microtask(move || o.borrow_mut().push(i));
    }
    assert_eq!(mock::take_microtask_requests(), 1);
    crate::task::__glue_microtask();
    assert_eq!(*order.borrow(), [0, 1, 2, 3, 4]);
}

#[test]
fn a_js_future_resolves_with_the_fulfilled_value() {
    let before = baseline();
    let p = unsafe { JsValue::from_raw(mock::new_promise()) };
    let got = Rc::new(RefCell::new(None));
    let g = got.clone();
    let fut = JsFuture::new(&p);
    spawn_local(async move {
        *g.borrow_mut() = Some(fut.await.map(|v| v.as_string().unwrap()).map_err(|e| e.message()));
    });
    drain();
    assert!(got.borrow().is_none(), "pending until settled");
    mock::settle(p.raw(), Ok(V::Str("done ✓".into()))).unwrap();
    drain();
    assert_eq!(got.borrow_mut().take(), Some(Ok("done ✓".to_string())));
    drop(p);
    assert_eq!(Closure::live_count(), 0, "both reactions released with the future");
    assert_eq!(baseline(), before);
}

#[test]
fn a_rejected_promise_is_an_err() {
    let p = unsafe { JsValue::from_raw(mock::new_promise()) };
    let got = Rc::new(RefCell::new(None));
    let g = got.clone();
    let fut = JsFuture::new(&p);
    spawn_local(async move { *g.borrow_mut() = Some(fut.await.map_err(|e| e.message())) });
    drain();
    mock::settle(p.raw(), Err(V::Str("nope".into()))).unwrap();
    drain();
    assert_eq!(got.borrow_mut().take().unwrap().unwrap_err(), "nope");
}

#[test]
fn regression_dropping_a_js_future_before_settle_makes_the_settle_silent() {
    let p = unsafe { JsValue::from_raw(mock::new_promise()) };
    drop(JsFuture::new(&p));
    assert_eq!(Closure::live_count(), 0);
    // The promise still holds both reaction functions; settling must not
    // throw "called after drop" into the page.
    mock::settle(p.raw(), Ok(V::Num(1.0))).unwrap();
}
