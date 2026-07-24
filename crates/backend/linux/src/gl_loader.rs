//! Resolving GL entry points inside a GTK process.
//!
//! GTK links libepoxy for its own GL renderer, so by the time a
//! `GtkGLArea` has a context the library is already mapped and
//! `dlopen`ing it just bumps a refcount. What it does NOT do is export
//! GL functions as ordinary function symbols: `glClear` is not in
//! libepoxy's dynamic symbol table at all. What *is* exported is
//! `epoxy_glClear` — a **data** symbol holding a lazily-resolved
//! dispatch pointer. So every lookup here is `dlsym` for the address of
//! a variable followed by one deref to read the pointer out of it.
//!
//! Two lookup strategies, in order:
//!
//! 1. **`eglGetProcAddress`** (via the `epoxy_eglGetProcAddress`
//!    dispatch pointer). This is the platform's own loader and returns
//!    the driver's real entry points. Preferred because the pointers it
//!    returns are final — see why that matters below. Mesa implements
//!    `EGL_KHR_get_all_proc_addresses`, so it resolves core GL
//!    functions (`glClear`, `glBindFramebuffer`, …), not just
//!    extensions, which older EGL was allowed to refuse.
//! 2. **`epoxy_<name>`**, the dispatch pointer itself. A correct
//!    fallback, but the value read before the first call is epoxy's
//!    *resolver stub*: it resolves the real function, rewrites epoxy's
//!    global, then tail-calls. A caller that cached the stub (glow does
//!    — it stores whatever the loader returned) keeps entering the
//!    resolver on every call, so it stays correct but pays a lookup per
//!    call forever. Hence strategy 1 first.
//!
//! # `eglGetProcAddress` does NOT return null for unknown GL functions
//!
//! Loaders are conventionally expected to return null for a symbol the
//! driver lacks, and callers (`glow::Context::from_loader_function`)
//! branch on that. EGL does not honor it: Mesa routes `gl*`/`glX*`
//! names through `_glapi_get_proc_address`, which **synthesizes a
//! dispatch stub for any well-formed GL name**, so a nonexistent
//! `glThisFunctionDoesNotExist` resolves to a live pointer while a
//! non-GL name (`totallyNotAGlName`) correctly resolves to null.
//! Measured on this host — see `unknown_gl_names_resolve_to_a_stub`.
//!
//! That is not a reason to prefer strategy 2. wgpu's own native GLES
//! backend loads through `khronos_egl`'s `get_proc_address` and so has
//! exactly these semantics already; it gates capabilities on the GL
//! version and extension strings, not on null function pointers.
//! Matching the loader wgpu is built and tested against is the safer
//! choice than handing it a table with different null-ness than it has
//! ever seen.

use std::ffi::{c_void, CString};
use std::sync::OnceLock;

/// `eglGetProcAddress`, read out of libepoxy once.
type GetProcAddress = unsafe extern "C" fn(*const std::ffi::c_char) -> *const c_void;

struct Loader {
    /// libepoxy's `dlopen` handle. Never `dlclose`d — the dispatch
    /// pointers we hand out must stay valid for the process lifetime.
    handle: *mut c_void,
    egl_get_proc_address: Option<GetProcAddress>,
}

// Raw `fn` pointers are `Send + Sync`; the `dlopen` handle is only ever
// passed back to `dlsym`, which is thread-safe. GTK itself is
// single-threaded, so in practice this is only ever touched from the
// main thread — the bounds exist to satisfy `OnceLock`.
unsafe impl Send for Loader {}
unsafe impl Sync for Loader {}

static LOADER: OnceLock<Option<Loader>> = OnceLock::new();

/// Read the pointer *stored in* the data symbol `name`, or null.
unsafe fn read_dispatch_ptr(handle: *mut c_void, name: &std::ffi::CStr) -> *const c_void {
    let addr = unsafe { libc::dlsym(handle, name.as_ptr()) };
    if addr.is_null() {
        return std::ptr::null();
    }
    // `dlsym` on a data symbol yields the ADDRESS OF the variable; the
    // function pointer is the value stored there, so deref once.
    unsafe { *(addr as *const *const c_void) }
}

fn loader() -> Option<&'static Loader> {
    LOADER
        .get_or_init(|| unsafe {
            // RTLD_LAZY without NOLOAD: GTK has already mapped libepoxy,
            // so this reuses the loaded image (dlopen refcounts by soname).
            let mut handle = libc::dlopen(c"libepoxy.so.0".as_ptr(), libc::RTLD_LAZY);
            if handle.is_null() {
                handle = libc::dlopen(c"libepoxy.so".as_ptr(), libc::RTLD_LAZY);
            }
            if handle.is_null() {
                return None;
            }
            // GTK uses EGL on Wayland and on modern X11; GLX only on
            // legacy X11 setups. Try EGL, then GLX, then give up on
            // strategy 1 and let every lookup fall through to the
            // `epoxy_<name>` dispatch pointers.
            let egl = [
                c"epoxy_eglGetProcAddress",
                c"epoxy_glXGetProcAddressARB",
                c"epoxy_glXGetProcAddress",
            ]
            .into_iter()
            .map(|s| read_dispatch_ptr(handle, s))
            .find(|p| !p.is_null())
            .map(|p| std::mem::transmute::<*const c_void, GetProcAddress>(p));

            Some(Loader { handle, egl_get_proc_address: egl })
        })
        .as_ref()
}

/// Resolve a GL entry point by name. Null when unavailable.
///
/// Sound to call at any time — resolution doesn't require a current
/// context — but the pointers it returns are only sound to *call* while
/// the owning GL context is current.
pub fn proc_address(symbol: &str) -> *const c_void {
    let Some(loader) = loader() else {
        return std::ptr::null();
    };
    let Ok(cname) = CString::new(symbol) else {
        return std::ptr::null();
    };
    if let Some(get) = loader.egl_get_proc_address {
        let p = unsafe { get(cname.as_ptr()) };
        if !p.is_null() {
            return p;
        }
    }
    let Ok(prefixed) = CString::new(format!("epoxy_{symbol}")) else {
        return std::ptr::null();
    };
    unsafe { read_dispatch_ptr(loader.handle, &prefixed) }
}

#[cfg(test)]
mod tests {
    use super::proc_address;

    /// The loader must resolve plain core GL functions, not just
    /// extensions. libepoxy exports NO `glClear` symbol — only
    /// `epoxy_glClear` — so a naive `dlsym(handle, "glClear")` returns
    /// null and every GPU library built on this loader comes up with an
    /// empty function table. Both strategies in this module exist to
    /// avoid exactly that.
    #[test]
    fn resolves_core_gl_entry_points() {
        for sym in ["glClear", "glClearColor", "glBindFramebuffer", "glGetIntegerv"] {
            assert!(
                !proc_address(sym).is_null(),
                "core GL entry point {sym} did not resolve — a GPU library \
                 built on this loader would have no {sym}",
            );
        }
    }

    /// Pins the surprising half of this loader's contract (see the
    /// module docs): a *GL-shaped* name always resolves, even when the
    /// driver has no such function, because Mesa synthesizes a dispatch
    /// stub for it. Anything reading this loader's output must gate on
    /// GL version/extension strings, never on a null function pointer.
    ///
    /// If this ever starts returning null, the driver stopped
    /// synthesizing stubs — good news, but it means capability probing
    /// through this loader silently changed shape, so fail loudly.
    #[test]
    fn unknown_gl_names_resolve_to_a_stub() {
        assert!(
            !proc_address("glThisFunctionDoesNotExist").is_null(),
            "expected Mesa's synthesized stub for a GL-shaped name",
        );
        // Non-GL names are the one case that does resolve to null.
        assert!(proc_address("totallyNotAGlName").is_null());
    }

    /// An embedded NUL can't name a C symbol; must return null rather
    /// than panic on the `CString` conversion.
    #[test]
    fn embedded_nul_is_null_not_a_panic() {
        assert!(proc_address("gl\0Clear").is_null());
    }
}
