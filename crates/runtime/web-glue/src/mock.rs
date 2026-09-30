//! Off-wasm32 stand-in for the JS side, so the Rust half of web-glue is
//! testable with plain `cargo test`.
//!
//! It models only what the Rust logic observes — a slab with the same
//! 0-is-undefined / loud-on-stale rules as `js/runtime.js`, functions that
//! route through the real [`__glue_invoke`](crate::callback::__glue_invoke),
//! promises whose reactions the test fires, and strings copied into
//! buffers from the real [`__glue_alloc`](crate::string::__glue_alloc).
//! It is NOT evidence that the JS runtime behaves the same; the
//! headless-browser E2E is (`crates/tools/build/web/tests/own_glue_e2e.rs`).

#![allow(clippy::missing_safety_doc)]
// The test-driving helpers (`settle`, `take_microtask_requests`, …) are
// used only by `src/tests.rs`.
#![cfg_attr(not(test), allow(dead_code))]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::JsError;

#[derive(Clone)]
pub(crate) enum V {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    Obj(Rc<RefCell<HashMap<String, V>>>),
    Func(Rc<FnState>),
    Promise(Rc<RefCell<Vec<(V, V)>>>),
}

pub(crate) struct FnState {
    id: u32,
    flags: u32,
    dead: Cell<bool>,
}

enum Slot {
    Used(V),
    Free,
}

struct Heap {
    slots: Vec<Slot>,
    free: Vec<u32>,
    live: u32,
    global: Option<V>,
    microtasks_requested: u32,
}

thread_local! {
    static HEAP: RefCell<Heap> = RefCell::new(Heap {
        slots: vec![Slot::Used(V::Undefined)],
        free: Vec::new(),
        live: 0,
        global: None,
        microtasks_requested: 0,
    });
}

pub(crate) fn add(v: V) -> u32 {
    if matches!(v, V::Undefined) {
        return 0;
    }
    HEAP.with(|h| {
        let mut h = h.borrow_mut();
        h.live += 1;
        if let Some(i) = h.free.pop() {
            h.slots[i as usize] = Slot::Used(v);
            i
        } else {
            h.slots.push(Slot::Used(v));
            (h.slots.len() - 1) as u32
        }
    })
}

pub(crate) fn val(i: u32) -> V {
    HEAP.with(|h| match h.borrow().slots.get(i as usize) {
        Some(Slot::Used(v)) => v.clone(),
        _ => panic!("web-glue: handle {i} used after release"),
    })
}

/// How many times Rust asked JS for a microtask since the last call.
pub(crate) fn take_microtask_requests() -> u32 {
    HEAP.with(|h| std::mem::take(&mut h.borrow_mut().microtasks_requested))
}

/// What `G.fn`'s wrapper does when JS calls a minted function.
pub(crate) fn call_js_fn(f: &V, arg: V) -> Result<(), String> {
    let V::Func(st) = f else { return Err("not a function".into()) };
    if st.dead.get() {
        if st.flags & crate::callback::FLAG_SILENT != 0 {
            return Ok(());
        }
        return Err(format!("web-glue: callback #{} called after its Rust owner dropped it", st.id));
    }
    if st.flags & crate::callback::FLAG_ONCE != 0 {
        st.dead.set(true);
    }
    match crate::callback::__glue_invoke(st.id, add(arg)) {
        0 => Ok(()),
        1 => Err(format!("web-glue: callback #{} is no longer registered", st.id)),
        _ => Err(format!("web-glue: callback #{} invoked recursively", st.id)),
    }
}

/// Settle a mock promise: run every reaction registered with `then`.
pub(crate) fn settle(p: u32, result: Result<V, V>) -> Result<(), String> {
    let V::Promise(reactions) = val(p) else { panic!("not a promise") };
    let reactions = std::mem::take(&mut *reactions.borrow_mut());
    for (ok, err) in reactions {
        match &result {
            Ok(v) => call_js_fn(&ok, v.clone())?,
            Err(e) => call_js_fn(&err, e.clone())?,
        }
    }
    Ok(())
}

pub(crate) fn new_promise() -> u32 {
    add(V::Promise(Rc::new(RefCell::new(Vec::new()))))
}

fn ret_str(s: &str, out: usize) {
    let ptr = crate::string::__glue_alloc(s.len());
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, s.len());
        let slot = out as *mut usize;
        *slot = if s.is_empty() { 0 } else { ptr as usize };
        *slot.add(1) = s.len();
    }
}

fn throw<T>(msg: &str) -> Result<T, JsError> {
    crate::error::park(add(V::Str(msg.into())));
    Err(crate::error::take_pending().expect("just parked"))
}

fn js_string(v: &V) -> String {
    match v {
        V::Undefined => "undefined".into(),
        V::Null => "null".into(),
        V::Bool(b) => b.to_string(),
        V::Num(n) => n.to_string(),
        V::Str(s) => s.to_string(),
        V::Obj(_) => "[object Object]".into(),
        V::Func(_) => "function".into(),
        V::Promise(_) => "[object Promise]".into(),
    }
}

unsafe fn read_str<'a>(p: usize, l: usize) -> &'a str {
    if l == 0 {
        return "";
    }
    unsafe { std::str::from_utf8(std::slice::from_raw_parts(p as *const u8, l)).unwrap() }
}

pub(crate) unsafe fn drop_ref(i: u32) {
    if i == 0 {
        return;
    }
    HEAP.with(|h| {
        let mut h = h.borrow_mut();
        match h.slots.get(i as usize) {
            Some(Slot::Used(_)) => {}
            _ => panic!("web-glue: handle {i} released twice"),
        }
        h.slots[i as usize] = Slot::Free;
        h.free.push(i);
        h.live -= 1;
    })
}
pub(crate) unsafe fn clone_ref(i: u32) -> u32 {
    add(val(i))
}
pub(crate) unsafe fn live_js() -> u32 {
    HEAP.with(|h| h.borrow().live)
}
pub(crate) unsafe fn str_new(p: usize, l: usize) -> u32 {
    add(V::Str(unsafe { read_str(p, l) }.into()))
}
pub(crate) unsafe fn num_new(n: f64) -> u32 {
    add(V::Num(n))
}
pub(crate) unsafe fn bool_new(b: u32) -> u32 {
    add(V::Bool(b != 0))
}
pub(crate) unsafe fn null_new() -> u32 {
    add(V::Null)
}
pub(crate) unsafe fn global() -> u32 {
    let g = HEAP.with(|h| {
        h.borrow_mut()
            .global
            .get_or_insert_with(|| V::Obj(Rc::new(RefCell::new(HashMap::new()))))
            .clone()
    });
    add(g)
}
pub(crate) unsafe fn type_of(i: u32) -> u32 {
    match val(i) {
        V::Undefined => 0,
        V::Null => 1,
        V::Bool(_) => 2,
        V::Num(_) => 3,
        V::Str(_) => 4,
        V::Func(_) => 6,
        V::Obj(_) | V::Promise(_) => 5,
    }
}
pub(crate) unsafe fn num_get(i: u32) -> f64 {
    match val(i) {
        V::Num(n) => n,
        _ => f64::NAN,
    }
}
pub(crate) unsafe fn truthy(i: u32) -> u32 {
    match val(i) {
        V::Undefined | V::Null => 0,
        V::Bool(b) => b as u32,
        V::Num(n) => (n != 0.0 && !n.is_nan()) as u32,
        V::Str(s) => (!s.is_empty()) as u32,
        _ => 1,
    }
}
pub(crate) unsafe fn str_get(i: u32, out: usize) -> u32 {
    match val(i) {
        V::Str(s) => {
            ret_str(&s, out);
            1
        }
        _ => 0,
    }
}
pub(crate) unsafe fn strict_eq(a: u32, b: u32) -> u32 {
    let eq = match (val(a), val(b)) {
        (V::Undefined, V::Undefined) | (V::Null, V::Null) => true,
        (V::Bool(x), V::Bool(y)) => x == y,
        (V::Num(x), V::Num(y)) => x == y,
        (V::Str(x), V::Str(y)) => x == y,
        (V::Obj(x), V::Obj(y)) => Rc::ptr_eq(&x, &y),
        (V::Func(x), V::Func(y)) => Rc::ptr_eq(&x, &y),
        (V::Promise(x), V::Promise(y)) => Rc::ptr_eq(&x, &y),
        _ => false,
    };
    eq as u32
}
pub(crate) unsafe fn get(h: u32, p: usize, l: usize) -> Result<u32, JsError> {
    let key = unsafe { read_str(p, l) };
    match val(h) {
        V::Obj(o) => Ok(add(o.borrow().get(key).cloned().unwrap_or(V::Undefined))),
        V::Undefined | V::Null => throw(&format!("TypeError: cannot read properties of null ('{key}')")),
        _ => Ok(0),
    }
}
pub(crate) unsafe fn set(h: u32, p: usize, l: usize, v: u32) -> Result<(), JsError> {
    let key = unsafe { read_str(p, l) };
    match val(h) {
        V::Obj(o) => {
            o.borrow_mut().insert(key.to_string(), val(v));
            Ok(())
        }
        V::Undefined | V::Null => throw(&format!("TypeError: cannot set properties of null ('{key}')")),
        _ => Ok(()),
    }
}
unsafe fn first_arg(args: usize, argc: usize) -> V {
    if argc == 0 { V::Undefined } else { val(unsafe { *(args as *const u32) }) }
}
pub(crate) unsafe fn call_method(h: u32, p: usize, l: usize, args: usize, argc: usize) -> Result<u32, JsError> {
    let key = unsafe { read_str(p, l) };
    let f = match val(h) {
        V::Obj(o) => o.borrow().get(key).cloned().unwrap_or(V::Undefined),
        _ => V::Undefined,
    };
    match f {
        V::Func(_) => match call_js_fn(&f, unsafe { first_arg(args, argc) }) {
            Ok(()) => Ok(0),
            Err(e) => throw(&e),
        },
        _ => throw(&format!("TypeError: o.{key} is not a function")),
    }
}
pub(crate) unsafe fn call(f: u32, _this: u32, args: usize, argc: usize) -> Result<u32, JsError> {
    let f = val(f);
    match call_js_fn(&f, unsafe { first_arg(args, argc) }) {
        Ok(()) => Ok(0),
        Err(e) => throw(&e),
    }
}
pub(crate) unsafe fn construct(_f: u32, _args: usize, _argc: usize) -> Result<u32, JsError> {
    throw("TypeError: not a constructor")
}
pub(crate) unsafe fn to_string(h: u32, out: usize) -> Result<(), JsError> {
    ret_str(&js_string(&val(h)), out);
    Ok(())
}
pub(crate) unsafe fn error_message(h: u32, out: usize) {
    ret_str(&js_string(&val(h)), out);
}
pub(crate) unsafe fn make_fn(id: u32, flags: u32) -> u32 {
    add(V::Func(Rc::new(FnState { id, flags, dead: Cell::new(false) })))
}
pub(crate) unsafe fn revoke_fn(h: u32) {
    if let V::Func(st) = val(h) {
        st.dead.set(true);
    }
    unsafe { drop_ref(h) }
}
// Functions handed to the (mock) garbage collector by `gc_own_fn`.
thread_local! {
    static GC_OWNED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}
pub(crate) unsafe fn gc_own_fn(h: u32) {
    if let V::Func(st) = val(h) {
        GC_OWNED.with(|g| g.borrow_mut().push(st.id));
    }
}
/// What the runtime's `FinalizationRegistry` does once JS collected every
/// function handed over with `gc_own_fn`.
pub(crate) fn collect_garbage() {
    for id in GC_OWNED.with(|g| std::mem::take(&mut *g.borrow_mut())) {
        crate::callback::__glue_release(id);
    }
}
pub(crate) unsafe fn queue_microtask() {
    HEAP.with(|h| h.borrow_mut().microtasks_requested += 1);
}
pub(crate) unsafe fn promise_then(p: u32, ok: u32, err: u32) {
    let V::Promise(reactions) = val(p) else { panic!("mock: then on a non-promise") };
    reactions.borrow_mut().push((val(ok), val(err)));
}
pub(crate) unsafe fn promise_resolve(_v: u32) -> u32 {
    new_promise()
}
