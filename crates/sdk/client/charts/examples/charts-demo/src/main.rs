//! The app's entry point — every platform, one line.
//!
//! `entry!` reads `[package.metadata.idealyst.app]` from Cargo.toml, emits
//! the `SceneExtensions` impl carrying `charts_demo::register_scene_extensions`
//! across the generic boundary, and emits a `main` that hands both to
//! `idealyst::boot::run`. The target triple picks the shell, so this file
//! names no platform.
idealyst::entry!(charts_demo);
