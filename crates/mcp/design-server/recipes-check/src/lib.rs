//! Every recipe in `../content/recipes/`, compiled as a module of this crate.
//!
//! A recipe is a whole app module — it names `idea_ui`, `idea_ui_nav`,
//! `swap_navigator`, … exactly as an app crate would, and exposes
//! `pub fn app() -> Element`. Compiling them here is what lets the design
//! MCP call its recipes "checked against idea-ui"; `tests/render.rs` then
//! server-renders each one and asserts its `//! render:` expectations.

#![allow(dead_code)]

include!(concat!(env!("OUT_DIR"), "/recipes_generated.rs"));
