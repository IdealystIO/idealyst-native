//! The app's entry point — every platform, one line.
//!
//! `entry!` reads `[package.metadata.idealyst.app]` from this crate's
//! Cargo.toml, lifts `docs_app::register_scene_extensions` into the
//! `SceneExtensions` impl the boot seam needs, and emits a `main` that
//! hands both to `idealyst::boot::run`. The shell is picked by the
//! target triple, so this file names no platform. `idealyst docs` does
//! not use this binary: it generates its own project that depends on
//! this crate as a library and carries its own entry point.
idealyst::entry!(docs_app);
