//! One module per screen. Each module exports a `page() -> Primitive`
//! that the Navigator mounts for the matching route.

pub mod agentic;
pub mod backends;
pub mod concepts;
pub mod demo_animations;
pub mod demo_components;
pub mod demo_counter;
pub mod demo_navigation;
pub mod further_reading;
pub mod home;
pub mod install;
pub mod quickstart;
pub mod targets;
pub mod why_rust;

pub(crate) mod common;
