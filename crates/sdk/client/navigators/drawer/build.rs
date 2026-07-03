//! Declares the custom `idealyst_form` cfg so `#[cfg(idealyst_form =
//! "…")]` in `register_native` doesn't trip the `unexpected_cfgs` lint.
//!
//! `idealyst_form` is the compile-time **form-factor** dimension —
//! orthogonal to `target_os` and to the backend — set by the variant /
//! build tool via `RUSTFLAGS=--cfg idealyst_form="desktop|mobile"`. Unset
//! means desktop (the default for plain `cargo` builds). See
//! `register_native` for the rationale.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(idealyst_form, values(\"desktop\", \"mobile\"))");
}
