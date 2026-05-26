//! Web-specific components built on top of the framework primitives.
//! Lives separately from `pages/` so it can be reused across the
//! marketing site's screens without each page reaching into the
//! preview-stack types (`host_web`, `ios_sim`, etc.) directly.

// `#[macro_use]` lifts the `simulator!` invocation macro generated
// by `#[component]` on `simulator::simulator` up to this module,
// from which the matching `#[macro_use] mod components;` in lib.rs
// promotes it to crate-root scope where `ui!` can find it.
#[macro_use]
pub mod simulator;
