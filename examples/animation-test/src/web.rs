//! Web entry — wasm-bindgen `start()` that mounts `super::app()`
//! into the `#app` DOM element. Same shape as `touch-test`'s
//! transitional `web.rs`.
//!
//! The demo's per-frame property writes (particle sim + animated
//! cards) go through the framework's backend-agnostic
//! `ViewHandle::set_animated_f32`, so this entry must call
//! `install_global_self` — that's what lets the web backend's
//! `ViewOps::set_animated_f32` dispatch reach this `WebBackend`.
//! Without it those writes silently no-op (see memory note
//! `project_web_install_global_self_for_animation`).

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    static OWNER: RefCell<Option<runtime_core::Owner>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();

    // Scheduler — the animation clock and all touch recognizers
    // dispatch through `runtime_core::scheduling`. Without this
    // install, `AnimatedValue::animate` registers a tick that
    // would never fire because the underlying `raf_loop` returns
    // an inert handle.
    backend_web::install_scheduler();
    // Time source — supplies wall-clock readings for the
    // animation clock's per-frame dt calculation.
    backend_web::install_time_source();

    #[cfg(feature = "dev-hot-reload")]
    {
        dev_hot_reload::start_dev_client();
        return;
    }

    #[cfg(not(feature = "dev-hot-reload"))]
    {
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        // Lets the framework's `ViewHandle::set_animated_f32` dispatch
        // (used by the demo's per-frame writes) reach this backend.
        backend_web::install_global_self(&backend);
        let owner = runtime_core::render(backend, super::app());
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
