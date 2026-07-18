//! Pull the ESP-IDF link args out of `esp-idf-sys`'s build-script
//! output and re-emit them for THIS binary's link step.
//!
//! ## Why this file exists
//!
//! `esp-idf-sys`'s build script knows everything about how to link
//! against the IDF — the cross-linker (`riscv32-esp-elf-gcc`), the
//! ESP-IDF link scripts (`memory.ld`, `sections.ld`, etc.), every
//! IDF static lib (`libfreertos.a`, `libesp_wifi.a`, …), the
//! `-Wl,--wrap=` shims for `_Unwind_*`, and so on. But instead of
//! emitting those as `cargo:rustc-link-arg=…` directives (which
//! Cargo only applies to the package the build script is in), it
//! stashes them in an environment variable
//! (`DEP_ESP_IDF_EMBUILD_LINK_ARGS` after Cargo's `links =`
//! mangling) for downstream binaries to pull out.
//!
//! Without this `build.rs`, the link step gets a fresh `ldproxy`
//! invocation with NO ARGUMENTS, panics with "Cannot locate
//! argument '--ldproxy-linker <linker>'", and the binary never
//! links.
//!
//! `embuild::espidf::sysenv::output()` does the reading +
//! re-emission in one call — it's the canonical helper the
//! esp-idf-template generates for every esp-idf-svc project.

fn main() {
    embuild::espidf::sysenv::output();
}
