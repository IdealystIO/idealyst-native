//! Per-category page modules. Each module exports one
//! `pub fn xyz() -> Element` per page in its category. The route
//! wiring lives in `lib.rs::app`.

pub mod overview;
pub mod install;
pub mod hello;
pub mod theming;
pub mod layout;
pub mod typography;
pub mod actions;
pub mod inputs;
pub mod feedback;
pub mod overlays;
pub mod stateful;
pub mod extending;
pub mod controls;
pub mod patterns;
