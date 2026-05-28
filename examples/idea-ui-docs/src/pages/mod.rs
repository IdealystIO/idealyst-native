//! One module per documented category. Each module exports a
//! `page() -> Element` (or `page(&signals)`) that the Navigator
//! mounts for the matching route.

pub mod actions;
pub mod feedback;
pub mod inputs;
pub mod layout;
pub mod overlays;
pub mod overview;
pub mod stateful;
pub mod themes;
pub mod typography;
