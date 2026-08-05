//! Declares the build-pipeline cfgs the CLI sets via `RUSTFLAGS`, so app
//! crates compile warning-free under `unexpected_cfgs`.
//!
//! `idealyst_premint_only` reaches this crate because the style engine's
//! DATA lives here: under that flag a `StyleSheet` carries no rule
//! closures at all (see `StyleSheet::variant`), which is what drops the
//! per-arm `StyleRules` bodies from the wasm.
//!
//! `idealyst_premint_report` likewise: the diagnostic's most useful field
//! is WHERE an un-preminted sheet was constructed, and only the
//! constructor can capture that (see `StyleSheet::origin`).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint)");
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_only)");
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_dump)");
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_report)");
    println!("cargo:rerun-if-changed=build.rs");
}
