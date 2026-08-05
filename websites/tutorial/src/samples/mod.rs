//! Compiled lesson samples.
//!
//! Every Rust snippet the lessons display is a real module here,
//! `include_str!`-ed into its `CodePanel`. The point is that the teaching
//! material sits inside the build graph: a sample that drifts from the
//! framework's actual surface stops compiling, so `cargo check -p
//! tutorial` is the gate on the lessons, not just on the chrome.
//! Whole-file inclusion is deliberate — there is no marker syntax to
//! strip, which means what a reader copies is exactly what the compiler
//! accepted.
//!
//! Three panels in the tutorial are NOT sourced from here, and each is
//! non-Rust or deliberately schematic: the shell transcripts on Quick
//! start, the CSS custom-property listing on "Under the hood: the flush",
//! and the `recipe!` sketch on "Catalog, docs & MCP" (which stands in for
//! a call site in a component library this crate doesn't depend on).
//!
//! The samples are compile-gated; the *runtime* behaviour they describe
//! is exercised live by the panels in [`crate::demo`], which run the
//! same mechanisms against the real flush driver on the page.
//!
//! Rules for adding one: no scaffolding comments (they ship to the
//! reader), no `#[allow]` attributes inside the file (they live on the
//! `mod` lines below), and one idea per file.

#[allow(dead_code, unused_variables)]
pub mod a11y_props;
#[allow(dead_code, unused_variables)]
pub mod a11y_setters;
#[allow(dead_code, unused_variables, unused_mut)]
pub mod fnd_engine;
#[allow(dead_code, unused_variables)]
pub mod fnd_tokens;
#[allow(dead_code, unused_variables)]
pub mod mq_breakpoints;
#[allow(dead_code, unused_variables)]
pub mod mq_container;
#[allow(dead_code, unused_variables)]
pub mod mq_install;
#[allow(dead_code, unused_variables)]
pub mod mq_signal;
#[allow(dead_code, unused_variables)]
pub mod rx_derived;
#[allow(dead_code, unused_variables)]
pub mod rx_effects;
#[allow(dead_code, unused_variables)]
pub mod rx_effects_cleanup;
#[allow(dead_code, unused_variables)]
pub mod rx_flush;
#[allow(dead_code, unused_variables)]
pub mod rx_guarded;
#[allow(dead_code, unused_variables)]
pub mod rx_handler;
#[allow(dead_code, unused_variables)]
pub mod rx_signals;
#[allow(dead_code, unused_variables)]
pub mod rx_staged;
#[allow(dead_code, unused_variables)]
pub mod rx_teardown;
#[allow(dead_code, unused_variables)]
pub mod rx_untrack;
#[allow(dead_code, unused_variables)]
pub mod st_overrides;
#[allow(dead_code, unused_variables)]
pub mod st_sheet;
#[allow(dead_code, unused_variables)]
pub mod st_tokens;
#[allow(dead_code, unused_variables)]
pub mod st_variants;
