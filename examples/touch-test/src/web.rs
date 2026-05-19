//! Web entry — wasm-bindgen `start()` that mounts `super::app()`
//! into the `#app` DOM element. Same shape as `hello-world`'s
//! transitional `web.rs`.

use std::cell::RefCell;
#[cfg(not(feature = "dev-hot-reload"))]
use std::rc::Rc;

#[cfg(not(feature = "dev-hot-reload"))]
use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

// Smaller WASM allocator — same trade-off as the other examples.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    /// `render` returns an `Owner` that must outlive the page.
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();

    // Web backend scheduler — needed by the long-press recognizer
    // (uses `after_ms`) and any framework primitive that schedules
    // microtasks.
    backend_web::install_scheduler();

    #[cfg(feature = "dev-hot-reload")]
    {
        dev_hot_reload::start_dev_client();
        return;
    }

    #[cfg(not(feature = "dev-hot-reload"))]
    {
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        let owner = framework_core::render(backend, super::app());
        OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
    }
}

#[cfg(feature = "dev-hot-reload")]
mod dev_hot_reload {
    use std::cell::RefCell;

    thread_local! {
        static CLIENT: RefCell<Option<backend_web::WebClientHandle>> =
            const { RefCell::new(None) };
    }

    pub(super) fn start_dev_client() {
        let handle = backend_web::connect_web("ws://localhost:9001", "#app");
        CLIENT.with(|slot| *slot.borrow_mut() = Some(handle));
    }
}
