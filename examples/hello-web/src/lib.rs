use backend_web::WebBackend;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

// Replace the default allocator with `lol_alloc`'s much smaller
// implementation on the WASM target. The default Rust allocator
// (`dlmalloc`) pulls in significant code; `lol_alloc::FreeListAllocator`
// is a few KB. Only active on wasm32 — other targets keep the default.
#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

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
