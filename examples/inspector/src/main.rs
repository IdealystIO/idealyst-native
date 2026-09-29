//! The Inspector's entry point (see the `idealyst` crate docs). The web
//! build is the one the CLI embeds and serves from `idealyst inspect`;
//! the macOS build connects to the same server.
idealyst::entry!(inspector);
