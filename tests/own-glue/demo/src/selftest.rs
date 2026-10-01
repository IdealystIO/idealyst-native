//! In-page checks of the invariants that only exist on the JS side of the
//! boundary (the Rust side is unit-tested in `web-glue` itself). Each
//! writes `name=ok` or `name=FAIL(detail)` into `#selftest`; the E2E
//! asserts on the line.

use web_glue::{string, Closure, JsValue};

use crate::dom;

fn check(results: &mut Vec<String>, name: &str, r: Result<(), String>) {
    results.push(match r {
        Ok(()) => format!("{name}=ok"),
        Err(detail) => format!("{name}=FAIL({detail})"),
    });
}

const STRINGS: &[&str] = &[
    "",
    "ascii",
    "é ü ñ — “quotes”",
    "日本語テキスト",
    "emoji 🦀🎉 ZWJ 👩‍👩‍👧",
    "nul\0inside",
];

pub fn run(body: &JsValue) {
    let mut results = Vec::new();

    check(&mut results, "strings", strings(body));
    check(&mut results, "growth", memory_growth());
    check(&mut results, "handles", handles());
    check(&mut results, "callbacks", callbacks(body));
    check(&mut results, "module", module(body));
    check(&mut results, "reflect", reflect());
    check(&mut results, "casts", casts());
    check(&mut results, "listener", listener());

    let out = dom::child(body, "pre", "selftest");
    dom::set_text(&out, &results.join(" "));
}

/// Non-ASCII both ways: through a JS string value, and through the DOM.
fn strings(body: &JsValue) -> Result<(), String> {
    let el = dom::child(body, "span", "strings");
    for s in STRINGS {
        let back = JsValue::from_str(s).as_string();
        if back.as_deref() != Some(*s) {
            return Err(format!("value {s:?} came back {back:?}"));
        }
        dom::set_text(&el, s);
        let back = dom::text(&el);
        if back != *s {
            return Err(format!("dom {s:?} came back {back:?}"));
        }
    }
    let big = "long ✓ ".repeat(50_000);
    if JsValue::from_str(&big).as_string().as_deref() != Some(big.as_str()) {
        return Err("450 KB string".into());
    }
    Ok(())
}

/// A JS → Rust string whose buffer allocation GROWS memory. Growth detaches
/// every existing view of the old buffer; `G.retStr` must take its view
/// after `__glue_alloc`. Verified against a deliberately broken runtime
/// (view taken before the alloc): the E2E then fails with "TypeError:
/// Cannot perform %TypedArray%.prototype.set on a detached or
/// out-of-bounds ArrayBuffer" thrown from `G.retStr`.
fn memory_growth() -> Result<(), String> {
    let s = "growth ✓ ".repeat(64);
    let v = JsValue::from_str(&s);
    for round in 0..3 {
        let pages = core::arch::wasm32::memory_size(0);
        string::debug_grow_on_next_alloc();
        let back = v.as_string();
        let grown = core::arch::wasm32::memory_size(0);
        if grown <= pages {
            return Err(format!("round {round}: memory did not grow ({pages} pages)"));
        }
        if back.as_deref() != Some(s.as_str()) {
            return Err(format!("round {round}: got {back:?}"));
        }
        // And Rust → JS right after growth reads the new buffer too.
        if JsValue::from_str("après ✓").as_string().as_deref() != Some("après ✓") {
            return Err(format!("round {round}: read after growth"));
        }
    }
    Ok(())
}

/// Creating and dropping values leaves both sides of the slab where they
/// started.
fn handles() -> Result<(), String> {
    let (r0, j0) = (JsValue::live_count(), JsValue::js_live_count());
    {
        let vals: Vec<JsValue> = (0..1000).map(|i| JsValue::from_f64(i as f64)).collect();
        let clones = vals.clone();
        let (r1, j1) = (JsValue::live_count(), JsValue::js_live_count());
        if r1 != r0 + 2000 || j1 != j0 + 2000 {
            return Err(format!("expected +2000, got rust {r0}->{r1}, js {j0}->{j1}"));
        }
        if !vals[999].strict_eq(&clones[999]) {
            return Err("clone is not the same value".into());
        }
    }
    let (r2, j2) = (JsValue::live_count(), JsValue::js_live_count());
    if (r2, j2) != (r0, j0) {
        return Err(format!("leak: rust {r0}->{r2}, js {j0}->{j2}"));
    }
    Ok(())
}

/// Listener attach/detach and drop; a stale function is left on `window`
/// for the E2E to call (it must throw), and the registry returns to its
/// baseline.
fn callbacks(body: &JsValue) -> Result<(), String> {
    let before = Closure::live_count();
    let target = dom::child(body, "button", "detach-target");
    let hits = std::rc::Rc::new(std::cell::Cell::new(0));
    let h = hits.clone();
    let c = Closure::new(move |_| h.set(h.get() + 1));
    dom::add_listener(&target, "click", c.as_js());
    target.call_method("click", &[]).map_err(|e| e.message())?;
    dom::remove_listener(&target, "click", c.as_js());
    target.call_method("click", &[]).map_err(|e| e.message())?;
    if hits.get() != 1 {
        return Err(format!("listener fired {} times, expected 1", hits.get()));
    }
    dom::set_global("__staleCallback", c.as_js());
    drop(c);
    // Calling it from Rust must surface JS's error as a JsError, not run it.
    let stale = JsValue::global().get("__staleCallback").map_err(|e| e.message())?;
    match stale.call(&JsValue::undefined(), &[]) {
        Ok(_) => return Err("stale callback ran".into()),
        Err(e) if e.message().contains("after its Rust owner dropped it") => {}
        Err(e) => return Err(format!("unexpected error {}", e.message())),
    }
    if hits.get() != 1 {
        return Err("stale callback reached Rust".into());
    }
    if Closure::live_count() != before {
        return Err(format!("registry {} -> {}", before, Closure::live_count()));
    }
    Ok(())
}

/// The custom-section carrier: a crate JS module reachable as G.m(name).
fn module(body: &JsValue) -> Result<(), String> {
    let list = dom::child(body, "ul", "module-list");
    for _ in 0..3 {
        dom::append(&list, &dom::create("li"));
    }
    match dom::row_count(&list) {
        3 => Ok(()),
        n => Err(format!("rowCount {n}")),
    }
}

/// The generic reflect surface and its error path.
fn reflect() -> Result<(), String> {
    let obj = JsValue::global()
        .get("Object")
        .and_then(|o| o.construct(&[]))
        .map_err(|e| e.message())?;
    obj.set("k", &JsValue::from_str("v ✓")).map_err(|e| e.message())?;
    let v = obj.get("k").map_err(|e| e.message())?.as_string();
    if v.as_deref() != Some("v ✓") {
        return Err(format!("got {v:?}"));
    }
    match JsValue::null().get("x") {
        Err(e) if e.message().starts_with("TypeError") => Ok(()),
        Err(e) => Err(format!("wrong error {}", e.message())),
        Ok(_) => Err("null.x did not throw".into()),
    }
}

/// Typed handles: `instanceof`-checked casts, the deref chain, and a
/// failed cast handing the value back.
fn casts() -> Result<(), String> {
    use web_glue::dom::{self, Element, HtmlElement, HtmlInputElement, Node};
    use web_glue::JsCast;
    let doc = dom::window().ok_or("no window")?.document().ok_or("no document")?;
    let body = doc.body().ok_or("no body")?;
    if body.tag_name() != "BODY" {
        return Err(format!("tag {}", body.tag_name()));
    }
    let as_node: Node = body.clone().into();
    let back = as_node.dyn_into::<HtmlElement>().map_err(|_| "Node -> HtmlElement refused")?;
    if !back.is_same_node(Some(&body)) {
        return Err("round trip is a different node".into());
    }
    let not_input = back.dyn_into::<HtmlInputElement>();
    let Err(returned) = not_input else { return Err("body cast to HtmlInputElement".into()) };
    if returned.dyn_ref::<Element>().is_none() {
        return Err("the refused value came back unusable".into());
    }
    let n = web_glue::JsValue::from_f64(1.0);
    if n.dyn_ref::<Node>().is_some() {
        return Err("a number is a Node".into());
    }
    Ok(())
}

/// `dom::Listener` detaches on drop (so the dropped closure is never
/// reached), and `into_target_owned` keeps firing after its owner is gone.
fn listener() -> Result<(), String> {
    use std::cell::Cell;
    use std::rc::Rc;
    use web_glue::dom::{self, EventTarget, Listener, ListenerOptions};
    use web_glue::JsCast;
    let doc = dom::window().ok_or("no window")?.document().ok_or("no document")?;
    let body = doc.body().ok_or("no body")?;
    let host = crate::dom::child(&web_glue::JsValue::from(body), "button", "listener-target");
    let target: EventTarget = host.clone().unchecked_into();
    let hits = Rc::new(Cell::new(0));
    let kinds = Rc::new(std::cell::RefCell::new(String::new()));
    let (h, k) = (hits.clone(), kinds.clone());
    let owned = Listener::new(target.clone(), "click", ListenerOptions::default(), move |ev| {
        h.set(h.get() + 1);
        *k.borrow_mut() = ev.type_();
    });
    host.call_method("click", &[]).map_err(|e| e.message())?;
    drop(owned);
    // Detached, so this click reaches nothing (a revoked-but-attached
    // function would throw out of `click()`).
    host.call_method("click", &[]).map_err(|e| e.message())?;
    if hits.get() != 1 || *kinds.borrow() != "click" {
        return Err(format!("owned listener fired {} times ({})", hits.get(), kinds.borrow()));
    }
    let before = web_glue::Closure::live_count();
    let h2 = hits.clone();
    Listener::new(target, "click", ListenerOptions::default(), move |_| h2.set(h2.get() + 10))
        .into_target_owned();
    host.call_method("click", &[]).map_err(|e| e.message())?;
    if hits.get() != 11 {
        return Err(format!("target-owned listener: {} hits", hits.get()));
    }
    if web_glue::Closure::live_count() != before + 1 {
        return Err("target-owned closure not registered".into());
    }
    Ok(())
}

/// Own mode's half of `web_glue::worker`: a Worker re-imports
/// `pkg/<lib>.js` (the URL the loader reported through `G.entry`),
/// instantiates from the compiled module the loader kept, runs
/// [`worker_entry`] and posts back what it saw. Writes `#worker`.
pub async fn worker(body: &JsValue) {
    let out = dom::child(body, "div", "worker");
    let text = match worker_round_trip().await {
        Ok(s) => s,
        Err(e) => format!("worker=FAIL({e})"),
    };
    dom::set_text(&out, &text);
}

async fn worker_round_trip() -> Result<String, String> {
    let w = web_glue::worker::spawn(worker_entry).map_err(|e| e.message())?;
    // The worker's first message, or its error event.
    let next = web_glue::js::Function::new_with_args(
        "w",
        "return new Promise((res, rej) => { \
           w.onmessage = (e) => res(e.data); \
           w.onerror = (e) => { e.preventDefault(); rej(new Error(e.message)); }; })",
    );
    let promise = next.call1(&JsValue::undefined(), w.as_js()).map_err(|e| e.message())?;
    let reply = web_glue::JsFuture::from(promise).await.map_err(|e| e.message())?;
    w.terminate();
    reply.as_string().ok_or_else(|| "reply is not a string".into())
}

/// Runs in the worker's instance of this module.
fn worker_entry() {
    let msg = format!(
        "worker=ok in_worker={} window={} ctors={} \u{2713}",
        web_glue::worker::in_worker(),
        web_glue::dom::window().is_some(),
        crate::ctor_runs(),
    );
    web_glue::worker::post_to_parent(&JsValue::from_str(&msg)).expect("postMessage");
}
