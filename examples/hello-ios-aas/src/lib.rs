//! Xcode-side glue staticlib for the iOS AAS demo.
//!
//! All the real iOS AAS-client code now lives in `backend-ios`
//! behind its `aas-shell` feature. Activating that feature in this
//! crate's `Cargo.toml` is what compiles in the C-exported
//! `ios_main` / `ios_teardown` symbols Xcode links against — there
//! would be nothing else for this `lib.rs` to do, except that Rust
//! DCEs the symbols out of the final `.a` if the consuming staticlib
//! never references them. The anchor below is the minimum needed
//! to keep them alive.
//!
//! Future iOS AAS apps can do the same: a thin staticlib crate that
//! depends on `backend-ios = { features = ["aas-shell"] }` plus a
//! single-statement linker anchor pointing at
//! `backend_ios::{ios_main, ios_teardown}`.

#![cfg(target_os = "ios")]

/// Linker anchor. Forces the two AAS-shell entry points from
/// `backend-ios` to be retained in this staticlib's archive.
///
/// Rust's dead-code elimination at staticlib link time only keeps
/// symbols that are reachable from the crate's own items.
/// `#[no_mangle] pub extern "C"` exposes the symbol *if it's
/// emitted*, but doesn't by itself prevent stripping when the
/// containing object file isn't referenced. Taking the addresses
/// here gives the compiler a use site it can't optimize away —
/// `xor`ing them returns a value that depends on both function
/// addresses at runtime.
///
/// The function itself is never called; we export it under a
/// `_idealyst_*` prefix so it's obvious in `nm` what its purpose is.
#[no_mangle]
pub extern "C" fn _idealyst_aas_link_anchor() -> usize {
    let main = backend_ios::ios_main as *const () as usize;
    let teardown = backend_ios::ios_teardown as *const () as usize;
    main ^ teardown
}
