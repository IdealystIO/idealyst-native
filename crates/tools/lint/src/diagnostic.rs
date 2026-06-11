//! The diagnostic types that flow out of the engine.
//!
//! [`RawDiag`] is what a rule emits: a rule id, a message, an optional
//! help line, and the proc-macro2 span it was found at — no severity, no
//! resolved location yet. The engine turns each `RawDiag` into a fully
//! resolved [`Diagnostic`] by applying the configured severity, dropping
//! anything the config or an inline directive suppresses, and pinning the
//! line/column/byte location via the source map.

use std::path::PathBuf;

use proc_macro2::Span;

/// Severity of a *resolved* diagnostic. `Off` is never carried here — a
/// rule configured `off` is dropped before a `Diagnostic` is built — so
/// the only states a reported problem can be in are warn or error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Error,
}

impl Severity {
    /// The rustc/cargo JSON `level` string rust-analyzer expects.
    pub fn as_rustc_level(self) -> &'static str {
        match self {
            Severity::Warn => "warning",
            Severity::Error => "error",
        }
    }
}

/// A rule finding before severity/suppression/location resolution.
pub(crate) struct RawDiag {
    pub(crate) rule: &'static str,
    pub(crate) message: String,
    pub(crate) help: Option<String>,
    pub(crate) span: Span,
}

impl RawDiag {
    pub(crate) fn new(rule: &'static str, message: impl Into<String>, span: Span) -> Self {
        RawDiag { rule, message: message.into(), help: None, span }
    }

    pub(crate) fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// A fully resolved problem ready to render or serialize. Locations are
/// 1-based line and 1-based column (rustc convention); byte offsets are
/// 0-based half-open `[start, end)`.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub rule: &'static str,
    pub severity: Severity,
    pub message: String,
    pub help: Option<String>,
    pub file: PathBuf,
    pub line_start: usize,
    pub col_start: usize,
    pub line_end: usize,
    pub col_end: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    /// The full text of `line_start`, used to draw a caret in human output
    /// and to populate the rustc `text` span field.
    pub source_line: String,
}
