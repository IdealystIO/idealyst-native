//! One module per documentation page. Each exposes a single
//! `pub fn page() -> Primitive` builder; the navigator wires those
//! to their corresponding routes.

pub mod cli;
pub mod components;
pub mod navigation;
pub mod overview;
pub mod platforms;
pub mod primitives;
pub mod quickstart;
pub mod reactivity;
pub mod styles;
pub mod ui_dsl;

// "Macros" page is named with an underscore to avoid clashing with
// any future `macros` module Rust might suggest at this path.
pub mod macros_page;
