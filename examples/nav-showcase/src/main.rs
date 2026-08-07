//! The app's entry point — every platform, one line.
//!
//! `entry!` reads `[package.metadata.idealyst.app]` from this crate's
//! Cargo.toml, lifts `nav_showcase::register_scene_extensions` into the
//! `SceneExtensions` impl the boot seam needs, and emits a `main` that
//! hands both to `idealyst::boot::run`. The shell is picked by the
//! target triple, so this file names no platform.
idealyst::entry!(nav_showcase);
