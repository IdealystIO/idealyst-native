//! Declares the build-pipeline cfgs the CLI sets via `RUSTFLAGS`, so app
//! crates compile warning-free under `unexpected_cfgs`.
//!
//! `idealyst_premint_only` reaches this crate because the style engine's
//! DATA lives here: under that flag a `StyleSheet` carries no rule
//! closures at all (see `StyleSheet::variant`), which is what drops the
//! per-arm `StyleRules` bodies from the wasm.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint)");
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_only)");
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_dump)");
    println!("cargo:rerun-if-changed=build.rs");
}
