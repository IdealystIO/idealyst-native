//! `Primitive::Virtualizer` — a scrolling `<div>` whose visible-range
//! diff is handled by `runtime/js/virtualizer.js`.
//!
//! The shim defines `window.__idealystVirtualizer` (a JS class).
//! `create` constructs an instance with a callbacks bundle. Each
//! Rust callback is wrapped in a wasm-bindgen `Closure`; the
//! `Closure` handles are *owned* by the backend (not forgotten)
//! so `release` can destroy them when the surrounding scope drops.
//!
//! ## Why owning the closures matters
//!
//! If a user-supplied callback (e.g. the data-source closure inside
//! `item_count`) captures a `Signal<T>`, dropping the surrounding
//! scope frees the signal's arena slot. A queued JS `scroll` /
//! `ResizeObserver` event firing after that point would invoke the
//! closure, which would `Signal::get()` an empty slot and panic
//! "signal used after its scope was dropped". The release path
//! below first asks the JS instance to disconnect every listener
//! (`instance.release()`), THEN drops the closures so any callback
//! the browser has already queued but not yet dispatched fails
//! cleanly (wasm-bindgen sees a destroyed Closure — itself a
//! panic, but a less confusing one than a freed Signal).

use crate::WebBackend;
use runtime_core::VirtualizerCallbacks;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// Per-instance state held in `WebBackend::virtualizer_instances`.
/// The `JsValue` is the JS-side `Virtualizer` instance (which the
/// framework calls back into via `dataChanged()` / `release()`).
/// The `_closures` Vec is the type-erased holder for the six
/// wasm-bindgen `Closure`s passed to the JS shim — they must live
/// at least as long as the JS instance, and dropping them is what
/// destroys their wasm-bindgen state.
pub(crate) struct VirtualizerInstance {
    pub(crate) js: JsValue,
    // Type-erased Vec<Box<dyn Any>> so we can hold heterogeneously-
    // typed `Closure<dyn FnMut(...)>`s in one collection. We never
    // need to call into them from Rust — they're invoked exclusively
    // through the JS instance's bound function references.
    _closures: Vec<Box<dyn std::any::Any>>,
}

pub(crate) fn create(
    b: &mut WebBackend,
    callbacks: VirtualizerCallbacks<Node>,
    overscan: f32,
    horizontal: bool,
) -> Node {
    // 1) Make sure the JS-side recycler shim is in the page.
    b.ensure_virtualizer_shim();

    // 2) Create the outer scrolling container.
    let container = b
        .doc
        .create_element("div")
        .expect("create_element div failed");
    // Stamp a stable virtualizer id as an attribute on the container.
    // Don't rely on `node_id(&container_node)` here because that map
    // gets cleared by `on_node_unstyled` when the FlatList's style
    // effect drops — and the style effect can drop BEFORE our
    // cleanup effect inside the same `Scope::drop` batch (effect
    // drop order = insertion order = style first, cleanup second).
    // If `release_virtualizer` looked up via `node_ids` it would
    // find nothing and silently return, never running the JS-side
    // teardown that flips `_released` and detaches scroll listeners.
    let virt_id = b.next_virtualizer_id;
    b.next_virtualizer_id += 1;
    let _ = container.set_attribute("data-virtualizer-id", &virt_id.to_string());
    let container_node: Node = container.clone().unchecked_into();
    let id = virt_id;

    // 3) Build the JS callbacks object. Each Rust callback is wrapped
    //    in a Closure so JS can invoke it; we keep the closures alive
    //    by stashing them as properties on the Virtualizer instance
    //    (JS holds refs through the instance's lifetime), then `.forget()`
    //    the Rust-side wrappers.
    //
    //    NOTE: wasm-bindgen-typed Closures are FnMut even when the
    //    underlying Rust closure is Fn — that's fine, we just invoke
    //    through the immutable signature.

    let item_count_cb = {
        let f = callbacks.item_count.clone();
        Closure::<dyn FnMut() -> JsValue>::new(move || JsValue::from_f64(f() as f64))
    };
    let item_count_js = item_count_cb.as_ref().clone();

    let item_key_cb = {
        let f = callbacks.item_key.clone();
        Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |idx: JsValue| {
            let i = idx.as_f64().unwrap_or(0.0) as usize;
            // Item key is a u64; JS numbers handle up to 2^53.
            JsValue::from_f64(f(i) as f64)
        })
    };
    let item_key_js = item_key_cb.as_ref().clone();

    let item_size_cb = {
        let f = callbacks.item_size.clone();
        Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |idx: JsValue| {
            let i = idx.as_f64().unwrap_or(0.0) as usize;
            JsValue::from_f64(f(i) as f64)
        })
    };
    let item_size_js = item_size_cb.as_ref().clone();

    let mount_item_cb = {
        let f = callbacks.mount_item.clone();
        Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |idx: JsValue| {
            let i = idx.as_f64().unwrap_or(0.0) as usize;
            let (node, scope_id) = f(i);
            // Return a 2-element array: [node, scopeId].
            let arr = js_sys::Array::new_with_length(2);
            arr.set(0, node.into());
            arr.set(1, JsValue::from_f64(scope_id as f64));
            arr.into()
        })
    };
    let mount_item_js = mount_item_cb.as_ref().clone();

    let release_item_cb = {
        let f = callbacks.release_item.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |scope_id: JsValue| {
            let id = scope_id.as_f64().unwrap_or(0.0) as u64;
            f(id);
        })
    };
    let release_item_js = release_item_cb.as_ref().clone();

    let set_measured_size_cb = {
        let f = callbacks.set_measured_size.clone();
        Closure::<dyn FnMut(JsValue, JsValue)>::new(
            move |scope_id: JsValue, size: JsValue| {
                let id = scope_id.as_f64().unwrap_or(0.0) as u64;
                let sz = size.as_f64().unwrap_or(0.0) as f32;
                f(id, sz);
            },
        )
    };
    let set_measured_size_js = set_measured_size_cb.as_ref().clone();

    // Build the callbacks object.
    let cb_obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("itemCount"), &item_count_js);
    let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("itemKey"), &item_key_js);
    let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("itemSize"), &item_size_js);
    let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("mountItem"), &mount_item_js);
    let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("releaseItem"), &release_item_js);
    let _ = js_sys::Reflect::set(
        &cb_obj,
        &JsValue::from_str("setMeasuredSize"),
        &set_measured_size_js,
    );
    let _ = js_sys::Reflect::set(
        &cb_obj,
        &JsValue::from_str("measureSizes"),
        &JsValue::from_bool(callbacks.measure_sizes),
    );
    let _ = js_sys::Reflect::set(
        &cb_obj,
        &JsValue::from_str("overscan"),
        &JsValue::from_f64(overscan as f64),
    );
    let _ = js_sys::Reflect::set(
        &cb_obj,
        &JsValue::from_str("horizontal"),
        &JsValue::from_bool(horizontal),
    );

    // 4) Construct the Virtualizer JS class.
    let window = web_sys::window().expect("no window");
    let ctor_raw = match js_sys::Reflect::get(&window, &JsValue::from_str("__idealystVirtualizer")) {
        Ok(v) => v,
        Err(e) => {
            web_sys::console::error_2(
                &JsValue::from_str(
                    "[virtualizer] Reflect::get(window, __idealystVirtualizer) failed:",
                ),
                &e,
            );
            panic!("Reflect::get failed");
        }
    };
    if ctor_raw.is_undefined() || ctor_raw.is_null() {
        web_sys::console::error_1(&JsValue::from_str(
            "[virtualizer] window.__idealystVirtualizer is undefined/null — shim never installed",
        ));
        panic!("shim missing");
    }
    if !ctor_raw.is_function() {
        web_sys::console::error_2(
            &JsValue::from_str(
                "[virtualizer] window.__idealystVirtualizer is not a function. Value:",
            ),
            &ctor_raw,
        );
        panic!("shim not a function");
    }
    let ctor: js_sys::Function = ctor_raw.unchecked_into();
    let args = js_sys::Array::new_with_length(2);
    args.set(0, container.clone().into());
    args.set(1, cb_obj.into());
    let instance = match js_sys::Reflect::construct(&ctor, &args) {
        Ok(v) => v,
        Err(e) => {
            web_sys::console::error_2(
                &JsValue::from_str("[virtualizer] Reflect::construct(Virtualizer) failed:"),
                &e,
            );
            panic!("construct failed");
        }
    };

    // 5) Hand each closure to the JS instance as a property so JS
    //    keeps a reference. Crucially, we DO NOT `.forget()` them
    //    — Rust retains ownership in `VirtualizerInstance._closures`
    //    so `release()` can drop them deterministically when the
    //    surrounding scope tears down.
    let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_item_count"), item_count_cb.as_ref());
    let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_item_key"), item_key_cb.as_ref());
    let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_item_size"), item_size_cb.as_ref());
    let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_mount"), mount_item_cb.as_ref());
    let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_release"), release_item_cb.as_ref());
    let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_set_size"), set_measured_size_cb.as_ref());

    // Store the JS instance + the closure handles. Drop order on
    // `release` (Vec drops in reverse insertion order, but order
    // among them doesn't matter — none of them touch each other).
    let closures: Vec<Box<dyn std::any::Any>> = vec![
        Box::new(item_count_cb),
        Box::new(item_key_cb),
        Box::new(item_size_cb),
        Box::new(mount_item_cb),
        Box::new(release_item_cb),
        Box::new(set_measured_size_cb),
    ];
    b.virtualizer_instances.insert(
        id,
        VirtualizerInstance { js: instance, _closures: closures },
    );

    container_node
}

/// Called from `Backend::release_virtualizer` when the virtualizer's
/// surrounding scope drops (a `when` branch flip, a `switch` arm
/// rebuild, `Owner` teardown). Two-phase:
///
/// 1. **Synchronously**, set `_released = true` on the JS instance.
///    No more queued browser events can call back into our Rust
///    closures from this moment.
/// 2. **Microtask-deferred**, do the full `instance.release()` and
///    drop the `VirtualizerInstance` (and its `_closures`). The
///    full release loops through `_unmountEntry` for every mounted
///    item, each of which calls back into Rust via `releaseItem`
///    to drop a per-item scope. Those per-item drops can
///    transitively call `backend.borrow_mut()` (e.g. StyleHandle
///    drops invoking `on_node_unstyled`) — but the caller of this
///    function is `cleanup.drop()` running INSIDE
///    `backend.borrow_mut()`, so a synchronous call would be a
///    re-entrant borrow_mut and panic.
///
/// Deferring to a microtask lets the outer borrow drop first; the
/// microtask runs with the backend unborrowed.
pub(crate) fn release(b: &mut WebBackend, node: &Node) {
    let Some(id) = virtualizer_id_of(node) else { return };
    let Some(instance) = b.virtualizer_instances.remove(&id) else { return };

    // Step 1: flip `_released` on the JS instance synchronously, so
    // any queued scroll/resize events that fire BEFORE the
    // microtask in step 2 drain become no-ops via the JS-side guard
    // (see `runtime/js/virtualizer.js`'s `update()` / `refresh()`).
    set_released_now(&instance.js);

    // Step 2: defer the heavy release work to a microtask. The
    // outer `backend.borrow_mut()` (held by the cleanup Effect's
    // Drop) is released before the microtask runs, so the
    // re-entrant `borrow_mut()` from per-item scope drops is safe.
    runtime_core::schedule_microtask(move || {
        // Best-effort `release()` call on the JS side. If the
        // method is missing (older shim version), we silently
        // proceed — dropping the closures below is the actual
        // safety contract.
        if let Ok(release_fn) =
            js_sys::Reflect::get(&instance.js, &JsValue::from_str("release"))
        {
            if let Ok(release_fn) = release_fn.dyn_into::<js_sys::Function>() {
                let _ = release_fn.call0(&instance.js);
            }
        }
        // `instance` drops here — `_closures` drops with it,
        // destroying the wasm-bindgen Closures and dropping their
        // captured user state (the `data` Signal handle, etc.).
        drop(instance);
    });
}

/// Flip the JS instance's `_released` flag without invoking the full
/// `release()` method. Used as step 1 of teardown — we need the
/// guard up before per-item drops fire, but the full release loops
/// through per-item unmount which can't run under our outer
/// `borrow_mut()`.
fn set_released_now(js_instance: &JsValue) {
    let _ = js_sys::Reflect::set(
        js_instance,
        &JsValue::from_str("_released"),
        &JsValue::from_bool(true),
    );
}

pub(crate) fn data_changed(b: &mut WebBackend, node: &Node) {
    let Some(id) = virtualizer_id_of(node) else { return };
    let Some(instance) = b.virtualizer_instances.get(&id) else { return };
    let _ = js_sys::Reflect::get(&instance.js, &JsValue::from_str("dataChanged"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .map(|f| f.call0(&instance.js));
}

/// Read the `data-virtualizer-id` attribute that `create` stamps on
/// the container. Returns `None` if the attribute is missing or
/// unparseable — both indicate either a non-virtualizer node or a
/// container that hasn't been mounted yet.
fn virtualizer_id_of(node: &Node) -> Option<u32> {
    node.clone()
        .dyn_into::<web_sys::Element>()
        .ok()
        .and_then(|el| el.get_attribute("data-virtualizer-id"))
        .and_then(|s| s.parse::<u32>().ok())
}
