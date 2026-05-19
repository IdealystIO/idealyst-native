//! Web entry — wasm-bindgen `start()` plus the optional hot-reload
//! dev-client wiring. Transitional: lives here until the CLI
//! generates this wrapper into `target/idealyst/web/` instead.
//!
//! Nothing in this module is reachable from the cross-platform
//! `app()` tree — it's strictly the host-side glue that mounts that
//! tree into the DOM (or replays a wire stream from `idealyst dev`).

use std::cell::RefCell;
#[cfg(not(feature = "dev-hot-reload"))]
use std::rc::Rc;

#[cfg(not(feature = "dev-hot-reload"))]
use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

// Smaller WASM allocator — same trade-off as `hello-web`. Lives in
// this module because it's strictly the web wrapper's concern.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    /// `render` returns an `Owner` that must outlive the page. Stash
    /// it in a thread-local so it survives `start()` returning.
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();

    // Register the web scheduler so framework_core::scheduling
    // (microtasks, after_animation_frame, etc.) has a backend.
    // Navigator/Drawer/Tab primitives invoke schedule_microtask
    // during mount; without this the framework panics on first
    // dispatch.
    backend_web::install_scheduler();

    // The `Simulator` component drives wgpu init through
    // `spawn_async` and the per-frame render through `render_loop`.
    // Both look up backend-supplied drivers from framework_core;
    // without installing them here the embedded preview never wakes.
    backend_web::install_async_executor();
    backend_web::install_render_loop();

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

// --- Hot-reload dev client integration -------------------------------------
//
// Gated behind `dev-hot-reload`. With the feature off, neither this
// module nor the `dev-client` dep is in the build. Lifted from
// `hello-web` so the two stay in sync until the CLI takes over.

#[cfg(feature = "dev-hot-reload")]
mod dev_hot_reload {
    use std::cell::RefCell;
    use std::rc::Rc;

    use backend_web::{connect_web, WebBackend, WebClientHandle};
    use dev_client::{OutboundSender, WireBackend};
    use wasm_bindgen::{JsCast, JsValue};

    const HOST_SELECTOR: &str = "#app";

    /// How long to wait between attempts when the dev server URL
    /// hasn't been injected yet (mDNS discovery is still running on
    /// the CLI side, or the page loaded before the server was up).
    const URL_POLL_MS: i32 = 250;

    type AppWire = Rc<RefCell<WireBackend<WebBackend>>>;

    thread_local! {
        /// Persistent wire — built once on first connect, kept alive
        /// across reconnects so the mounted DOM survives.
        static WIRE: RefCell<Option<AppWire>> = const { RefCell::new(None) };
        /// Current WebSocket. Drop = disconnect. Replaced on each
        /// reconnect (not torn down with `WIRE`).
        static CLIENT: RefCell<Option<WebClientHandle>> = const { RefCell::new(None) };
    }

    pub fn start_dev_client() {
        connect_attempt();
    }

    /// Read `window.IDEALYST_AAS_URL` — the dev HTTP server injects
    /// this into served HTML once mDNS discovery resolves the AAS
    /// host's randomly-bound port. Returns `None` while the value is
    /// still `null` / `undefined` (page loaded before discovery
    /// completed); caller polls.
    fn aas_url() -> Option<String> {
        let window = web_sys::window()?;
        let key = JsValue::from_str("IDEALYST_AAS_URL");
        let v = js_sys::Reflect::get(&window, &key).ok()?;
        v.as_string()
    }

    fn connect_attempt() {
        WIRE.with(|slot| {
            if slot.borrow().is_none() {
                let real_backend = WebBackend::new(HOST_SELECTOR);
                let outbound = OutboundSender::new();
                let wire = Rc::new(RefCell::new(WireBackend::new(real_backend, outbound)));
                *slot.borrow_mut() = Some(wire);
            }
        });

        let Some(url) = aas_url() else {
            web_sys::console::log_1(
                &"[docs] waiting for window.IDEALYST_AAS_URL (mDNS discovery in progress)…"
                    .into(),
            );
            schedule_retry();
            return;
        };

        let wire = WIRE.with(|slot| slot.borrow().as_ref().unwrap().clone());

        let on_disconnect: Rc<dyn Fn()> = Rc::new(|| {
            connect_attempt();
        });

        match connect_web(&url, wire, on_disconnect) {
            Ok(handle) => {
                CLIENT.with(|slot| *slot.borrow_mut() = Some(handle));
                web_sys::console::log_1(
                    &format!("[docs] hot-reload connected to {}", url).into(),
                );
            }
            Err(e) => {
                web_sys::console::warn_2(
                    &"[docs] hot-reload connect failed; retrying:".into(),
                    &e,
                );
                schedule_retry();
            }
        }
    }

    fn schedule_retry() {
        if let Some(window) = web_sys::window() {
            let cb = wasm_bindgen::closure::Closure::once_into_js(|| {
                connect_attempt();
            });
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                URL_POLL_MS,
            );
        }
    }
}
