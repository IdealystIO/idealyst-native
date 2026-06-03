//! Save a file to a user-chosen location through the platform's own "save" UI.
//!
//! `file-export` is the counterpart to [`files`](https://docs.rs/files): where
//! `files` writes to **app-private** storage the user never sees,
//! `file-export` hands a file to the **user's** file system at a destination
//! *they* pick — the OS "Save to Files" sheet / save dialog / document creator.
//! The two compose: produce a file in the app sandbox (e.g. with
//! `media-writer`), then offer to export it.
//!
//! ```no_run
//! use file_export::{FileExport, SaveRequest, SaveOutcome};
//! # async fn demo(recording_path: std::path::PathBuf)
//! #     -> Result<(), file_export::ExportError> {
//! let outcome = FileExport::new()
//!     .save(SaveRequest::path("clip.mp4", "video/mp4", recording_path))
//!     .await?;
//!
//! match outcome {
//!     SaveOutcome::Saved { location } => { /* user saved it (location may be known) */ }
//!     SaveOutcome::Cancelled => { /* user dismissed the picker */ }
//! }
//! # let _ = SaveOutcome::Cancelled;
//! # Ok(())
//! # }
//! ```
//!
//! # No permission required
//!
//! Every backend is **user-initiated UI** — the act of picking a location is
//! what grants access to it. So this SDK needs no storage permission on any
//! platform, unlike broad filesystem access.
//!
//! # Architecture
//!
//! The platform-agnostic surface ([`FileExport`], [`SaveRequest`], [`Source`],
//! [`SaveOutcome`], [`ExportError`]) lives here; one cfg-gated backend compiles
//! per target:
//!
//! - **iOS** — `UIDocumentPickerViewController` (export).
//! - **macOS** — `NSSavePanel`.
//! - **Android** — Storage Access Framework (`ACTION_CREATE_DOCUMENT`).
//! - **Windows** — `IFileSaveDialog`.
//! - **Linux** — `xdg-desktop-portal` `FileChooser.SaveFile`.
//! - **web** — `showSaveFilePicker()`, with an `<a download>` Blob fallback.

#![deny(missing_docs)]

use std::path::PathBuf;

mod error;
pub use error::ExportError;

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an `imp`
// module with `async fn save(SaveRequest) -> Result<SaveOutcome, ExportError>`.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
#[path = "android.rs"]
mod imp;

#[cfg(all(any(target_os = "ios", target_os = "macos"), not(target_arch = "wasm32")))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
#[path = "windows.rs"]
mod imp;

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
#[path = "linux.rs"]
mod imp;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "linux"
)))]
#[path = "stub.rs"]
mod imp;

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// Where the bytes to save come from.
///
/// Prefer [`Path`](Source::Path) for anything already on disk (a recording,
/// a downloaded file) — the backend copies/streams it without loading it into
/// memory. Use [`Bytes`](Source::Bytes) for generated or in-memory content,
/// and on **web**, where there is no real filesystem path (only `Bytes` is
/// supported there).
pub enum Source {
    /// Copy an existing on-disk file to the chosen destination.
    Path(PathBuf),
    /// Write these bytes to the chosen destination.
    Bytes(Vec<u8>),
}

/// A request to save one file: what to write, what to call it, and its type.
pub struct SaveRequest {
    /// The filename to pre-fill in the picker (e.g. `"clip.mp4"`). The user
    /// may change it; the extension informs the OS's type handling.
    pub suggested_name: String,
    /// The MIME type of the content (e.g. `"video/mp4"`, `"application/pdf"`).
    /// Drives the picker's type filter / default extension where the platform
    /// supports it.
    pub mime: String,
    /// The bytes to save — see [`Source`].
    pub source: Source,
}

impl SaveRequest {
    /// Save an existing on-disk file (the common case — e.g. a recording from
    /// `media-writer`). Not supported on web (no filesystem path); use
    /// [`bytes`](Self::bytes) there.
    pub fn path(
        suggested_name: impl Into<String>,
        mime: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            suggested_name: suggested_name.into(),
            mime: mime.into(),
            source: Source::Path(path.into()),
        }
    }

    /// Save in-memory bytes (generated content, or the web path).
    pub fn bytes(
        suggested_name: impl Into<String>,
        mime: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            suggested_name: suggested_name.into(),
            mime: mime.into(),
            source: Source::Bytes(bytes.into()),
        }
    }
}

/// The result of a [`save`](FileExport::save): the user either completed the
/// save or dismissed the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SaveOutcome {
    /// The user chose a destination and the file was written.
    Saved {
        /// The destination path / URI, when the platform reports one. `None`
        /// where the platform withholds it (e.g. a sandboxed picker that only
        /// grants a security-scoped handle).
        location: Option<String>,
    },
    /// The user dismissed the picker without saving. Not an error.
    Cancelled,
}

/// Presents the platform save UI. Cheap to construct and clone; holds no
/// resources until you [`save`](Self::save).
#[derive(Clone, Default)]
pub struct FileExport {
    _private: (),
}

impl FileExport {
    /// Create an exporter handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Present the platform's save UI for `request` and, if the user picks a
    /// destination, write the file there.
    ///
    /// Resolves to [`SaveOutcome::Saved`] on success or
    /// [`SaveOutcome::Cancelled`] if the user dismisses the picker; errors only
    /// on a genuine failure (no UI surface, unreadable source, write failure,
    /// unsupported target).
    pub async fn save(&self, request: SaveRequest) -> Result<SaveOutcome, ExportError> {
        imp::save(request).await
    }
}
