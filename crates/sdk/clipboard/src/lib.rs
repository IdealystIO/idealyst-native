//! Cross-platform **system clipboard** access — copy and paste plain text.
//!
//! Two small async free functions over the OS clipboard. The API is the
//! same on every target; the platforms diverge only in *mechanism*, never
//! in the surface you call:
//!
//! - [`set_text`] — copy a string onto the system clipboard.
//! - [`text`] — read the clipboard's text, or `None` when it holds no text
//!   (it's empty or holds a non-text representation like an image).
//!
//! The functions are `async` for a uniform surface: the web backend
//! (`navigator.clipboard`) is genuinely Promise-based, while the native
//! backends do their work synchronously inside the returned future.
//!
//! # Plain text only
//!
//! This SDK is deliberately scoped to plain text. Images, rich text, and
//! multiple simultaneous representations are left to a later, higher-level
//! SDK — see the crate README's *Scope* section. The clean extension seam
//! is "more representations alongside text", not a different shape.
//!
//! # Permissions
//!
//! None — no OS manifest permission on any platform. On **web**, reading
//! the clipboard ([`text`]) requires a user gesture and may prompt for the
//! `clipboard-read` permission at runtime; a denial surfaces as
//! [`ClipboardError::Backend`]. That's a runtime browser concern, not a
//! build-time manifest entry, so this crate declares no capability.
//!
//! ```ignore
//! use clipboard::{set_text, text};
//!
//! # async fn demo() -> Result<(), clipboard::ClipboardError> {
//! set_text("hello").await?;
//! assert_eq!(text().await?, Some("hello".to_string()));
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

// Compile-checked usage recipes (catalog feature only).
pub mod recipes;

// Platform-native backends. Exactly one of `web`/`apple`/`android` is
// compiled per target; every other native target (Windows, Linux, …)
// falls back to the `NotSupported`-returning `unsupported` module.
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
mod apple;
#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
mod android;
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
mod unsupported;

// Re-route the public functions to whichever backend this target compiles.
#[cfg(target_arch = "wasm32")]
use web as backend;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
use apple as backend;
#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
use android as backend;
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
use unsupported as backend;

/// A clipboard operation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardError {
    /// The underlying platform clipboard API failed — e.g. a web
    /// `readText` permission denial, a missing window, or an Obj-C / JNI
    /// error. The string carries the platform's detail for logging.
    Backend(String),
    /// The clipboard isn't available on this platform. Returned by the
    /// desktop (Windows / Linux / other native) fallback, which has no
    /// in-scope backend.
    NotSupported,
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardError::Backend(msg) => write!(f, "clipboard backend error: {msg}"),
            ClipboardError::NotSupported => {
                write!(f, "clipboard not supported on this platform")
            }
        }
    }
}

impl std::error::Error for ClipboardError {}

/// Copy `text` onto the system clipboard, replacing its current contents.
///
/// On every platform this overwrites whatever was on the clipboard with
/// the given plain-text string.
///
/// # Errors
///
/// Returns [`ClipboardError::Backend`] if the platform clipboard API
/// fails, or [`ClipboardError::NotSupported`] on a target with no backend
/// (desktop Windows / Linux).
pub async fn set_text(text: &str) -> Result<(), ClipboardError> {
    backend::set_text(text).await
}

/// Read the system clipboard's text.
///
/// Returns `Ok(Some(s))` with the clipboard's text, or `Ok(None)` when the
/// clipboard is empty or holds only a non-text representation (e.g. an
/// image).
///
/// # Errors
///
/// Returns [`ClipboardError::Backend`] if the platform clipboard API fails
/// — on **web** this includes a `clipboard-read` permission denial or a
/// call made without a user gesture — or [`ClipboardError::NotSupported`]
/// on a target with no backend (desktop Windows / Linux).
pub async fn text() -> Result<Option<String>, ClipboardError> {
    backend::text().await
}
