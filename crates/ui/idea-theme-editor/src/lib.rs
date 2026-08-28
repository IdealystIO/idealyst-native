//! `idea-theme-editor` — a live theme editor for idea-ui.
//!
//! Every token in the installed theme becomes an editable control.
//! Editing one commits it immediately, so the app re-tints as you type;
//! the result saves to a flat JSON file, loads back, and exports as
//! Rust you paste into the app's theme setup.
//!
//! ```ignore
//! use idea_theme_editor::{ThemeDraft, ThemeEditor};
//!
//! #[component]
//! fn DevPanel() -> Element {
//!     let draft = ThemeDraft::from_live();
//!     ui! { ThemeEditor(draft = draft) }
//! }
//! ```
//!
//! # Why this works at all
//!
//! `IdeaThemeRef` canonicalizes every color field when a theme installs,
//! and the length accessors emit token references built from the theme
//! struct's numbers. So every color and every scale value a component
//! paints resolves through a NAMED token at paint time — no component
//! can be holding a baked literal. That is what makes a name-keyed
//! editor complete: there is no tail of components that need their own
//! wiring. `runtime_core::update_tokens` then re-runs only the effects
//! that read the tokens you changed.
//!
//! # What it does not reach
//!
//! Only tokenized values move. Component stylesheets also carry
//! hand-written literals — a `1.0` border width, a fixed `opacity` —
//! and those are not tokens, so no editor can touch them. Editing a
//! theme is not editing every rule. If a literal should be editable it
//! wants a token, not a bigger editor.
//!
//! # It ships opt-in
//!
//! This is a separate crate rather than an idea-ui feature so an app
//! that doesn't want a control panel in its bundle simply doesn't
//! depend on it.
//!
//! # Getting the result out
//!
//! The panel renders controls only — no save/load buttons and no file
//! dialogs, because reading and writing files belongs to the app (and
//! an SDK), not to a control panel. [`ThemeDraft`] carries the whole
//! import/export surface as plain methods:
//!
//! - [`ThemeDraft::to_json`] / [`ThemeDraft::load_json`] — the save
//!   format, a flat `name → text` object covering every token.
//! - [`ThemeDraft::to_rust`] — the EDITS as source, ready to paste.
//! - [`ThemeDraft::revert`] — back to the values the draft opened with.

#![deny(missing_docs)]

// The framework author surface under its historical name — the `ui!` /
// `#[component]` expansions and this crate's own paths both spell
// `runtime_core::…`.
extern crate runtime_core as runtime_core;

mod draft;
mod editor;
mod format;
mod styles;

pub use draft::{DraftEntry, LoadReport, ThemeDraft, EXTENSION_NAMESPACE};
pub use editor::{ThemeEditor, ThemeEditorProps};
pub use format::{
    format_length, format_value, parse_length, parse_value, DraftKind, ParseError,
};

// The vocabulary this editor renders, re-exported so a caller wiring a
// custom panel doesn't need a second dependency.
pub use idea_theme::{token_descriptors, TokenDescriptor, TokenKind};
