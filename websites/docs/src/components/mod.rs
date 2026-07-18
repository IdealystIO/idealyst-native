//! Docs-specific composite components. Each file exports a single
//! `#[component]` that the docs pages invoke through the `ui!`
//! macro. Pure composition over idea-ui primitives — anything that
//! would be reusable outside the docs site belongs in idea-ui.

// `#[macro_use]` lifts the `CardTabs!` invocation macro (generated
// by `#[component]` on `card_tabs::CardTabs`) up into this module,
// from which the matching `#[macro_use] mod components;` in lib.rs
// promotes it to crate-root scope where `ui!` can find it.
#[macro_use]
pub mod card_tabs;

// Same pattern for `Simulator!` — embedded live preview that runs
// the docs' example trees through the wgpu render backend.
#[macro_use]
pub mod simulator;
