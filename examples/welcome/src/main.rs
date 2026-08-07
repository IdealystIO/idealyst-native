//! The app's entry point — every platform, one line.
//!
//! `entry!` reads `[package.metadata.idealyst.app]` from this crate's
//! Cargo.toml, emits the `SceneExtensions` impl that carries
//! `welcome::register_scene_extensions` across the generic boundary,
//! and emits a `main` that hands both to `idealyst::boot::run`. Which
//! shell that resolves to is settled by the target triple (plus a
//! feature for the shells that share one), so this file names no
//! platform.
idealyst::entry!(welcome);
