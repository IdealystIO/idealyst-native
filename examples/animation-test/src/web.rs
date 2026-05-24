//! Web entry — wasm-bindgen `start()` that mounts `super::app()`
//! into the `#app` DOM element. Same shape as `touch-test`'s
//! transitional `web.rs`.
//!
//! Also exposes a thread-local handle to the active `WebBackend` so
//! the demo's animated cards can write per-frame property updates
//! via `Backend::set_animated_f32` directly — bypassing the
//! class-minting reactive-style path which is fast enough for
//! one-off mutations but too costly per frame for animation.

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use runtime_core::animation::AnimProp;
use runtime_core::Backend;
use wasm_bindgen::prelude::*;

#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    static OWNER: RefCell<Option<runtime_core::Owner>> = const { RefCell::new(None) };
    /// Active WebBackend handle. Installed by [`start`] before the
    /// first render; the demo's per-frame property writes
    /// ([`set_animated_f32`]) read this back to call into the
    /// backend without taking a generic backend parameter at every
    /// author-facing helper. Single-threaded by virtue of wasm32
    /// being single-threaded.
    static WEB_BACKEND: RefCell<Option<Rc<RefCell<WebBackend>>>> =
        const { RefCell::new(None) };
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
        // Stash the backend handle so author-facing helpers
        // (`set_animated_f32` below) can dispatch into it.
        WEB_BACKEND.with(|s| *s.borrow_mut() = Some(backend.clone()));
        let owner = runtime_core::render(backend, super::app());
        OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
    }
}

/// Write an animated scalar property to a node on the active
/// `WebBackend`. The demo's [`AnimatedValue`](runtime_core::animation::AnimatedValue)
/// subscribers call this each frame to push the new value as
/// inline CSS (`element.style.scale = "<x> <y>"`,
/// `element.style.translate = "<x>px <y>px"`, etc.) — much cheaper
/// than the reactive-style path's class minting.
///
/// No-op if the backend hasn't been installed yet (pre-`start()`)
/// or if the listener fires after teardown.
pub fn set_animated_f32(node: &web_sys::Node, prop: AnimProp, value: f32) {
    WEB_BACKEND.with(|s| {
        if let Some(backend) = s.borrow().as_ref() {
            backend
                .borrow_mut()
                .set_animated_f32(node, prop, value);
        }
    });
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
