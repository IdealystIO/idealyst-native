//! `js::WeakSet` in a real browser: identity membership through the
//! `Set` imports it shares (`add` / `has` / `delete`), on a real
//! `WeakSet` (the property that keeps no member alive is the engine's).

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::window;
use web_glue::js::{Object, Set, WeakSet};
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn weak_set_tracks_membership_by_identity() {
    let set = WeakSet::new();
    let doc = window().unwrap().document().unwrap();
    let a = doc.create_element("div").unwrap();
    let b = doc.create_element("div").unwrap();
    let a_again: JsValue = a.clone().into();

    assert!(!set.has(a.as_ref()), "empty");
    set.add(a.as_ref());
    assert!(set.has(&a_again), "the same object, through another handle");
    assert!(!set.has(b.as_ref()), "an equal-looking element is a different member");

    assert!(set.delete(a.as_ref()), "delete reports a member");
    assert!(!set.has(a.as_ref()), "gone");
    assert!(!set.delete(a.as_ref()), "delete reports a non-member");

    let plain = Object::new();
    set.add(plain.as_ref());
    assert!(set.has(plain.as_ref()), "any object can be a member");
}

#[wasm_bindgen_test]
fn weak_set_is_a_js_weak_set() {
    let set: JsValue = WeakSet::new().into();
    assert!(set.clone().dyn_into::<WeakSet>().is_ok(), "instanceof WeakSet");
    assert!(set.dyn_into::<Set>().is_err(), "not a Set: a Set keeps its members alive");
}
