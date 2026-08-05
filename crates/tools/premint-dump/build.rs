//! Sets BOTH premint cfgs on this crate's own compilation (lib + tests).
//!
//! Why: cargo has no way to set a custom `--cfg` for one test target, so
//! the dual-cfg agreement test — "the class the shipped binary assembles
//! at runtime appears in the CSS the dump emits" — could otherwise only
//! run out-of-tree with hand-set RUSTFLAGS. This crate is a build tool
//! (never linked into an app), so unconditionally compiling it premint-on
//! is safe: the lib itself has no premint cfg branches; only the
//! `stylesheet!` expansions inside its tests react, which is the point.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint)");
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_dump)");
    println!("cargo::rustc-cfg=idealyst_premint");
    println!("cargo::rustc-cfg=idealyst_premint_dump");
}
