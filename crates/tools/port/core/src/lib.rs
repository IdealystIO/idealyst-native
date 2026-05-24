//! Shared porter substrate.
//!
//! See `crates/port/README.md` for the design rationale. This
//! crate exports:
//!
//! - the porter IR (`ir`),
//! - the idealyst Rust emitter (`emit`),
//! - the `Parser` trait every per-framework crate implements,
//! - small helpers (e.g. `render_inline_hole`) used by both the
//!   emitter and the various per-framework stub parsers.
//!
//! No source-language code lives here. `port-react`, `port-solid`,
//! `port-vue`, and `port-svelte` are the frontends; this crate is
//! the lowering target they all agree on.

pub mod cli;
pub mod emit;
pub mod ir;

use ir::{Hole, Module, PortReport};

/// Source-framework-agnostic parser interface. Each per-framework
/// crate implements at least one of these (a stub backed by
/// hand-curated fixtures, plus eventually a real parser).
///
/// Implementations are intentionally not async / not streaming —
/// porting is a CLI-driven, file-at-a-time activity. If that
/// changes (LSP integration, watch mode) the trait can grow; it
/// hasn't earned that complexity yet.
pub trait Parser {
    fn parse(&self, source: &str) -> Result<(Module, PortReport), ParseError>;
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// Render a hole as a single-line `todo!(...);` statement suitable
/// for placement *inside* an effect/handler body. The emitter's
/// own hole rendering wraps the same payload in slightly different
/// contexts (expression vs. statement position) — this helper
/// exists so the stub parsers can pre-bake hole markers into
/// snippet bodies (e.g. the body of an `Effect`) without
/// duplicating the message format.
pub fn render_inline_hole(h: &Hole) -> String {
    let line = h.original.line.map(|l| format!(" (line {})", l)).unwrap_or_default();
    format!(
        "todo!(\"port {kind}{line}: {reason} — {orig}\");",
        kind = h.kind,
        line = line,
        reason = h.reason.replace('"', "\\\""),
        orig = h.original.text.replace('"', "\\\""),
    )
}
