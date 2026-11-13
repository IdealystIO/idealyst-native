//! The stack navigator's runtime is the vocabulary's stack handler, so
//! nothing in this crate needs a desktop backend. Depending on one makes
//! every host build of a UI crate (its unit tests, on a CI box) compile
//! that toolkit: `backend-linux` brings gtk4 and pango-sys, which needs a
//! newer pango than Ubuntu 24.04 ships, so `cargo test` of any app crate
//! that uses a stack failed there.
//!
//! A manifest check, not a `cargo tree` run: the bug is a dependency line,
//! and reading the manifest needs no toolchain or network inside a test.

#[test]
fn regression_stack_navigator_pulls_no_desktop_backend() {
    let manifest = include_str!("../Cargo.toml");
    for backend in ["backend-linux", "backend-macos", "backend-windows"] {
        let declared = manifest
            .lines()
            .map(str::trim_start)
            .any(|l| l.starts_with(backend) && l[backend.len()..].trim_start().starts_with('='));
        assert!(!declared, "stack-navigator declares `{backend}`; nothing in it uses one");
    }
}
