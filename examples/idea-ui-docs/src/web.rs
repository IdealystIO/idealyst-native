//! Web entry — wasm-bindgen `start()` for the docs app. Transitional:
//! lives here until the CLI generates this wrapper into
//! `target/idealyst/web/` from `idea-ui-docs` like every other
//! platform wrapper.
//!
//! Nothing in this module is reachable from `app()` — strictly the
//! host-side glue that mounts the cross-platform tree into the DOM.

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

// Smaller WASM allocator — slightly higher per-alloc cost in
// exchange for a few KB shaved off the bundle.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    /// `render` returns an `Owner` that must outlive the page. Stash
    /// it in a thread-local so it survives `start()` returning.
    static OWNER: RefCell<Option<runtime_core::Owner>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let mut web = WebBackend::new("#app");
    // Register navigator-SDK handlers so the app's
    // `stack_navigator::Navigator` builders dispatch through
    // `Backend::create_navigator`.
    stack_navigator::register(&mut web);
    let backend = Rc::new(RefCell::new(web));
    let owner = runtime_core::render(backend, super::app());
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}
