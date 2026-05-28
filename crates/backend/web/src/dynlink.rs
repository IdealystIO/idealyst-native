//! Web dynamic-split loader bridge.
//!
//! In `idealyst build --web --dynamic-split`, each `lazy!` body is compiled
//! into its own PIC `--shared` side module instead of into the main bundle.
//! The `lazy!` macro's web expansion emits a stub that calls
//! `runtime_core::primitives::lazy::__dynlink_load(hash)`; this module wires
//! that seam to the JS glue that does the actual fetch + dynamic link.
//!
//! The link itself happens in JS (`__idealyst_dynlink.js`, shipped by
//! build-web) because it needs the LIVE main instance's exports to resolve
//! the side's GOT — see the proven loader in `crates/tools/dynlink/loader.mjs`.
//! This Rust side is just the bridge: call JS, get back the raw
//! `*mut Element` the side built on the shared heap, reconstitute it.

use runtime_core::primitives::lazy::LazyFuture;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    // Calls `globalThis.__IDEALYST_DYNLINK.load(hash)`, resolving to the raw
    // `*mut Element` (as a JS number) the side module built — or 0 on
    // failure. The global is installed by the shipped dynlink glue.
    #[wasm_bindgen(js_namespace = __IDEALYST_DYNLINK, js_name = load)]
    fn dynlink_load_js(hash: &str) -> js_sys::Promise;
}

/// Reserve `size` bytes from main's heap so the JS loader can place a side
/// module's memory image there without DLMALLOC later reusing the region.
/// Kept in the main bundle via `--export-all`; the loader reads it off main's
/// exports. (Mirrors the `host_reserve` proven in the dynlink spike.)
#[no_mangle]
pub extern "C" fn host_reserve(size: usize) -> *mut u8 {
    if size == 0 {
        return std::ptr::null_mut();
    }
    match std::alloc::Layout::from_size_align(size, 16) {
        Ok(layout) => unsafe { std::alloc::alloc(layout) },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Install the `runtime_core` dynamic-split loader seam. The generated
/// `--dynamic-split` wrapper calls this once at boot. Wires `lazy!`'s
/// `__dynlink_load(hash)` stub to the JS glue: fetch the side module, link it
/// against the live main instance, invoke its `__idealyst_lazy_body_<hash>`
/// export, and hand back the `Element` for the walker to mount.
pub fn install_dynlink_loader() {
    let loader: Rc<dyn Fn(&'static str) -> LazyFuture> = Rc::new(|hash: &'static str| {
        let hash = hash.to_string();
        Box::pin(async move {
            let promise = dynlink_load_js(&hash);
            match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(v) => {
                    let ptr = v.as_f64().unwrap_or(0.0) as u32 as *mut runtime_core::Element;
                    if ptr.is_null() {
                        empty_placeholder()
                    } else {
                        // SAFETY: the side built this `Box<Element>` on the
                        // SHARED heap (its allocator resolves to main's via the
                        // GOT), so it is a valid main-heap allocation we now own.
                        *unsafe { Box::from_raw(ptr) }
                    }
                }
                Err(e) => {
                    runtime_core::logging::log(
                        runtime_core::logging::LogLevel::Error,
                        &format!("lazy! dynamic-split: load failed for {hash}: {e:?}"),
                    );
                    empty_placeholder()
                }
            }
        })
    });
    runtime_core::primitives::lazy::install_dynlink_loader(loader);
}

fn empty_placeholder() -> runtime_core::Element {
    use runtime_core::IntoElement;
    runtime_core::view(Vec::new()).into_element()
}
