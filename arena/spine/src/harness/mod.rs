//! The live half of the arena: turning a scenario into scored runs by driving
//! real agents. Where [`crate::verify`] and [`crate::score`] are pure and
//! deterministic, this module shells out — to the `idealyst` CLI (scaffold +
//! build), to `claude` headless (the implementation, locator, and feedback
//! agents), and to a static web server (for the locator pass).
//!
//! Layering: [`scaffold`] builds the isolated project, [`agent`] runs the
//! implementation agent and captures its transcript, [`run`] orchestrates one
//! full run through verification + scoring, and [`bench`] repeats it N times
//! and aggregates.
//!
//! Every external dependency is treated as optional: a missing `idealyst`,
//! `claude`, or web server downgrades the affected tier to a *skip* with
//! evidence, never a silent pass or a hard crash. That keeps the spine useful
//! on a machine that only has some of the toolchain.

pub mod agent;
pub mod bench;
pub mod feedback;
pub mod locate;
pub mod run;
pub mod scaffold;
