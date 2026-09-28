//! The Inspector's entry point (see the `idealyst` crate docs). It runs
//! on macOS only: its bridge client needs raw TCP and threads, which a
//! browser build doesn't have.
idealyst::entry!(inspector);
