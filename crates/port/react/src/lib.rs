//! React → idealyst-native source porter.
//!
//! The pipeline is split across two crates:
//!
//! - `port_core` owns the porter IR, the idealyst Rust emitter,
//!   the `Parser` trait, and the hole machinery. Everything in
//!   there is source-framework-agnostic.
//! - This crate (`port_react`) owns the React-specific bits: the
//!   hook taxonomy (`hooks`) and a stub parser that hand-lowers
//!   bundled TSX fixtures (`parser`). A real swc/oxc backend is
//!   the obvious next step behind the same `Parser` trait.
//!
//! See `../README.md` for the broader design and the AI-final-pass
//! philosophy.

pub mod hooks;
pub mod parser;

pub use port_core::ir;
pub use port_core::{ParseError, Parser};

/// Parse + lift only — produces the porter IR (no Rust emission).
/// Used by the project-level driver so it can resolve cross-file
/// references before emitting.
pub fn lift(source: &str) -> Result<(ir::Module, ir::PortReport), ParseError> {
    let p = parser::ReactParser::new();
    p.parse(source)
}

/// Convenience: parse + emit, return the rendered Rust source
/// alongside the port report. Used by single-file CLIs.
pub fn port(source: &str) -> Result<(String, ir::PortReport), ParseError> {
    let (module, report) = lift(source)?;
    Ok((port_core::emit::emit_module(&module), report))
}
