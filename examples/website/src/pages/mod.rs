//! One module per screen. Each module exports a `page() -> Element`
//! that the Navigator mounts for the matching route.

pub mod agentic;
pub mod architecture;
pub mod backends;
pub mod code_splitting;
pub mod comparisons;
pub mod concepts;
pub mod cross_platform;
pub mod demo;
pub mod features;
pub mod further_reading;
pub mod home;
pub mod install;
pub mod navigation;
pub mod performance;
pub mod quickstart;
pub mod reactivity;
pub mod roadmap;
pub mod server_functions;
pub mod styling;
pub mod ssr;
pub mod targets;
pub mod type_safety;
pub mod why_rust;

pub(crate) mod common;
