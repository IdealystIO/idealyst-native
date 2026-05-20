//! Web entry — wasm-bindgen `start()` that mounts `super::app()` on
//! `#app`.
//!
//! Also stashes the `WebBackend` handle in a thread-local so the
//! per-property animation helper [`set_animated_f32`] can dispatch
//! to it from `AnimatedValue` subscribers running outside the
//! reactive build path.

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use framework_core::animation::AnimProp;
use wasm_bindgen::prelude::*;

#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    /// `render` returns an `Owner` that must outlive the page.
    static OWNER: RefCell<Option<framework_core::Owner>> =
        const { RefCell::new(None) };

    /// Stash of the live `WebBackend`. AnimatedValue subscribers
    /// (created from inside `app()` and living for the page) need
    /// to write inline style properties without going through the
    /// build-walker's backend borrow. They reach the backend
    /// through this slot.
    static WEB_BACKEND: RefCell<Option<Rc<RefCell<WebBackend>>>> =
        const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();

    // Scheduler — `after_ms` (used by the act sequencing) AND the
    // animation clock (raf-driven per-frame ticks for AnimatedValue
    // subscribers) dispatch through `framework_core::scheduling`.
    // Without this neither the timeline advances nor any spring
    // ever ticks.
    backend_web::install_scheduler();
    // Time source — supplies wall-clock readings for the per-frame
    // animation clock's `dt` calculation.
    backend_web::install_time_source();

    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    WEB_BACKEND.with(|s| *s.borrow_mut() = Some(backend.clone()));
    let owner = framework_core::mount(backend, super::app);
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}

/// Push an animated scalar property update to `node` on the active
/// `WebBackend`. Called by `AnimatedValue` subscribers each frame.
///
/// Quietly no-ops if the backend isn't stashed yet (pre-mount) or
/// has been torn down (post-page).
pub fn set_animated_f32(node: &web_sys::Node, prop: AnimProp, value: f32) {
    WEB_BACKEND.with(|s| {
        if let Some(backend) = s.borrow().as_ref() {
            use framework_core::Backend;
            backend.borrow_mut().set_animated_f32(node, prop, value);
        }
    });
}

/// Same shape as `set_animated_f32` but for the color family
/// (`AnimProp::BackgroundColor` / `ForegroundColor`). `value` is
/// `[r, g, b, a]` with channels in `0..=1`.
pub fn set_animated_color(node: &web_sys::Node, prop: AnimProp, value: [f32; 4]) {
    WEB_BACKEND.with(|s| {
        if let Some(backend) = s.borrow().as_ref() {
            use framework_core::Backend;
            backend.borrow_mut().set_animated_color(node, prop, value);
        }
    });
}
