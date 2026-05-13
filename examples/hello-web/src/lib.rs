use backend_web::WebBackend;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

thread_local! {
    /// The render call returns an Owner that must outlive the page. Storing
    /// it in a thread-local keeps it alive for the lifetime of the WASM
    /// instance (i.e. until the user navigates away).
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    let owner = framework_core::render(backend, hello::app());
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}
