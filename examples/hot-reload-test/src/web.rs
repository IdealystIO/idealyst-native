//! Web entry — wasm-bindgen `start()` for the hot-reload-test app.
//! Mirrors the structure used by `docs` and `idea-ui-docs`. Not
//! reachable from `app()` — strictly host-side DOM mount glue.

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    let owner = framework_core::render(backend, super::app());
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}
