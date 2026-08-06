//! Web boot — what the generated `web/wrapper/src/lib.rs` used to be.
//!
//! Two paths, picked by the `runtime-server` feature:
//!
//! - **Default:** mount the app locally. The browser runs the framework
//!   runtime directly.
//! - **`runtime-server`:** read `window.IDEALYST_RUNTIME_SERVER_URL`
//!   (the dev HTTP server injects it into every served page) and
//!   connect a caps-driven replay client over WebSocket. The browser
//!   becomes a thin replayer and the runtime lives in the dev sidecar.

use runtime_scene::Element;

use super::{AppConfig, SceneExtensions};

/// Smaller WASM allocator — a few cycles per allocation in exchange for
/// a few KB off the bundle.
///
/// Lives here rather than in the app's `main.rs` because a
/// `#[global_allocator]` may be declared anywhere in the crate graph and
/// applies process-wide. Keeping it in the framework means `entry!`'s
/// expansion carries no `unsafe`.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

/// Boot the app in the browser.
///
/// Called from the binary's `main`. For a `wasm32-unknown-unknown` bin
/// target, wasm-bindgen's generated `init()` calls `__wbindgen_start`,
/// which runs `main` — so no `#[wasm_bindgen(start)]` shim is needed
/// (the old wrapper was a `cdylib` and did need one).
pub fn run<E: SceneExtensions, S: runtime_vocabulary::BuiltinSet>(
    app: impl FnOnce() -> Element,
    config: AppConfig,
) {
    // This same wasm is also imported and initialized inside Web Workers
    // (by the `offload` SDK) purely to make `#[offload::job]` exports
    // callable there. A Worker has no `Window` and no DOM, so boot must
    // NOT install the UI backend or mount — just return, leaving the
    // module instantiated and the job exports reachable.
    if web_sys::window().is_none() {
        return;
    }

    console_error_panic_hook::set_once();

    // Host services, before anything can reach for them.
    //
    // `start_in_with` installs the scheduler, time source and wall
    // clock itself, but NOT these three — they were the wrapper's job,
    // and dropping them cost a boot panic ("no AsyncExecutor installed"
    // the first time a server fn was called). All are idempotent (first
    // install wins), so the overlap with `start_in_with` is free, and
    // installing here also covers the runtime-server path, which never
    // goes through it.
    //
    // Each is load-bearing:
    // * async executor — `runtime_core::driver::spawn_async`, i.e. every
    //   resource fetcher, server-fn call and mutation trigger. Without
    //   it the app renders and then panics at the first async work.
    // * logger — routes `log_info!`/`log_error!` to `console.*`; without
    //   it Rust-side logs hit the wasm stderr no-op sink and vanish.
    // * render loop — ticks `runtime_core::driver::render_loop`; without
    //   it host-driven paint surfaces (canvas, wgpu preview) mount but
    //   never paint.
    backend_web::install_scheduler();
    backend_web::install_time_source();
    backend_web::install_logger();
    backend_web::install_async_executor();
    backend_web::install_render_loop();
    // NOTE: the viewport observer is deliberately NOT installed here.
    // Its timing differs per path — after mount when hydrating (so the
    // first render uses the SSR-assumed viewport) vs before mount on a
    // fresh boot — so each start fn owns it.

    #[cfg(feature = "runtime-server")]
    {
        start_runtime_server::<E, S>(app, config);
    }
    #[cfg(not(feature = "runtime-server"))]
    {
        start_local::<E, S>(app, config);
    }
}

/// Runtime-server mode: the framework runtime lives in a native sidecar
/// on the dev host and this browser is a thin client that replays wire
/// commands and forwards events back.
///
/// Unlike the native platforms — where the runtime-server end is a
/// separate binary that must not link the app at all — the web build is
/// one bundle that decides at compile time, so the app is still linked
/// here. It is used only by the defensive local-mount fallback below.
#[cfg(feature = "runtime-server")]
fn start_runtime_server<E: SceneExtensions, S: runtime_vocabulary::BuiltinSet>(
    app: impl FnOnce() -> Element,
    config: AppConfig,
) {
    use std::cell::RefCell;
    use std::rc::Rc;

    thread_local! {
        /// The `WebClientHandle` owns the WebSocket, the event closures
        /// and the raf pump; dropping it tears the connection down, so
        /// it has to outlive this function.
        static HANDLE: RefCell<Option<backend_web::WebClientHandle>> =
            const { RefCell::new(None) };
        /// Behind `Rc<RefCell<…>>` because both the `connect_web` raf
        /// pump and the on-disconnect closure retarget it.
        static WIRE: RefCell<
            Option<Rc<RefCell<dev_client::newcore::NewCoreReplayClient<backend_web::WebBackend>>>>,
        > = const { RefCell::new(None) };
    }

    // Runtime-server mode never hydrates (the host renders), so read the
    // real viewport up front rather than after mount.
    backend_web::install_viewport_observer();

    let Some(url) = read_global_string("IDEALYST_RUNTIME_SERVER_URL") else {
        web_sys::console::error_1(
            &"[dev-client] runtime-server mode enabled but window.IDEALYST_RUNTIME_SERVER_URL \
              is missing — did the dev HTTP server fail to inject it? Falling back to local \
              mount."
                .into(),
        );
        // Don't leave the page blank over a dev-server misconfiguration.
        backend_web::newcore::start_in(config.mount_selector, E::register, app);
        return;
    };

    web_sys::console::log_1(&format!("[dev-client] runtime-server mode: connecting to {url}").into());

    let backend = backend_web::WebBackend::new(config.mount_selector);
    // `new_newcore` drives the backend's CAPABILITY surface (the
    // `runtime_vocabulary::caps` traits) rather than a `Backend` impl:
    // wire commands in, capability calls out. SDK web modules submit
    // their handlers into backend-web's link-time inventory registrar,
    // so the chrome the replayer drives registers without a call here.
    let wire = Rc::new(RefCell::new(dev_client::WireBackend::new_newcore(
        backend,
        dev_client::OutboundSender::new(),
    )));
    WIRE.with(|slot| *slot.borrow_mut() = Some(wire.clone()));

    // A disconnect usually means the dev server is restarting the
    // sidecar (hot-patch fallback). Retry once; a second failure leaves
    // the page inert until reload rather than spinning.
    let on_disconnect: Rc<dyn Fn()> = {
        let wire = wire.clone();
        let url = url.clone();
        Rc::new(move || {
            let retry_url = url.clone();
            let give_up: Rc<dyn Fn()> = Rc::new(move || {
                web_sys::console::warn_1(
                    &format!("[dev-client] reconnect to {retry_url} failed; will retry on next disconnect")
                        .into(),
                );
            });
            match backend_web::connect_web(&url, wire.clone(), give_up) {
                Ok(h) => HANDLE.with(|slot| *slot.borrow_mut() = Some(h)),
                Err(e) => {
                    web_sys::console::error_2(&"[dev-client] reconnect failed:".into(), &e)
                }
            }
        })
    };

    match backend_web::connect_web(&url, wire, on_disconnect) {
        Ok(h) => HANDLE.with(|slot| *slot.borrow_mut() = Some(h)),
        Err(e) => web_sys::console::error_2(
            &"[dev-client] initial runtime-server connect failed:".into(),
            &e,
        ),
    }
}

/// Local mode: `backend_web::newcore::{start_in_with, hydrate_in_with}`
/// own the backend, scene registry and `World` lifecycle, and install
/// the scheduler, time source, logger, async executor, render loop,
/// microtask flush driver and viewport source themselves.
///
/// A prerendered `#app` (SSG/SSR output) takes the adoption path;
/// `hydrate_in_with` falls back to a fresh mount when there is no server
/// DOM.
#[cfg(not(feature = "runtime-server"))]
fn start_local<E: SceneExtensions, S: runtime_vocabulary::BuiltinSet>(
    app: impl FnOnce() -> Element,
    config: AppConfig,
) {
    let selector = config.mount_selector;
    // BOTH boot paths must instantiate the SAME `S`. The bundle
    // compiles in `start_in_with` and `hydrate_in_with` and picks at
    // runtime, so if either named the full vocabulary the whole thing
    // would stay reachable and neither path would shrink (measured:
    // 0.4% instead of 35%).
    if backend_web::page_is_prerendered(selector) {
        backend_web::newcore::hydrate_in_with::<S>(selector, E::register, app);
    } else {
        backend_web::newcore::start_in_with::<S>(selector, E::register, app);
    }

    // Robot-on-web (local/static builds only): dial the relay so this
    // browser-hosted app exposes its Robot bridge to the MCP server. The
    // robot driver env is installed by the boot itself. In
    // runtime-server mode the framework runs in the native sidecar
    // (robot is native there), so this belongs to the local path only.
    #[cfg(feature = "robot")]
    if let Some(url) = read_global_string("IDEALYST_ROBOT_RELAY_URL") {
        if let Err(e) = backend_web::install_robot_relay_client(&url) {
            web_sys::console::error_2(&"[robot] relay connect failed:".into(), &e);
        }
    }
}

/// Read a string off `window`, for the dev server's injected globals.
#[cfg(any(feature = "robot", feature = "runtime-server"))]
fn read_global_string(key: &str) -> Option<String> {
    let window = web_sys::window()?;
    js_sys::Reflect::get(&window, &key.into()).ok()?.as_string()
}
