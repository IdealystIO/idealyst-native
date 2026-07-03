//! Let the user pick file(s) from their own filesystem through the platform's
//! native "open" UI — the counterpart to [`file-export`](https://docs.rs/file-export).
//! Where `file-export` *saves* a file to a user-chosen location, `file-picker`
//! *opens* one (or many) the user chooses.
//!
//! ```no_run
//! use file_picker::{FilePicker, PickRequest, PickOutcome, MediaKind};
//! # async fn demo() -> Result<(), file_picker::PickError> {
//! // A basic document picker (PDFs + plain text), single selection.
//! let outcome = FilePicker::new()
//!     .pick(PickRequest::documents(["application/pdf", "text/plain"]))
//!     .await?;
//!
//! if let PickOutcome::Picked(files) = outcome {
//!     for file in &files {
//!         println!("picked {} ({} bytes)", file.name(), file.size().unwrap_or(0));
//!         // Stream it to the app sandbox without loading it all into memory:
//!         file.copy_to(std::env::temp_dir().join(file.name())).await?;
//!     }
//! }
//!
//! // Or the dedicated media picker (photos + videos), multi-select.
//! let _photos = FilePicker::new()
//!     .pick(PickRequest::media(MediaKind::ImagesAndVideos).multiple())
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # No permission required
//!
//! Every backend is **user-initiated UI** — the act of picking a file is what
//! grants access to it. So this SDK needs no storage or photo-library
//! permission on any platform. The media picker in particular (iOS
//! `PHPickerViewController`, Android Photo Picker) is designed to hand back only
//! the chosen items without the broad "all photos" grant.
//!
//! # Reading without blowing up memory
//!
//! A picked file is a **lazy handle**, never eagerly buffered — picking a 10 GB
//! video does not read 10 GB into RAM. Reach for, in order of preference:
//!
//! - [`PickedFile::path`] — a real filesystem path, present on desktop and the
//!   iOS document picker; stream from it yourself however you like.
//! - [`PickedFile::open`] — a chunked [`FileStream`] that works on *every*
//!   platform (a real file on desktop, a file descriptor on Android, a `Blob`
//!   stream on web). This is the universal, RAM-safe accessor.
//! - [`PickedFile::copy_to`] — stream the file to a destination path (built on
//!   `open`; never fully buffers).
//! - [`PickedFile::read_all`] — a convenience that *does* buffer the whole file;
//!   use it only for files you know are small.
//!
//! On **web** there is no filesystem path at all, which is exactly why the
//! streaming reader exists: [`open`](PickedFile::open) / [`read_all`](PickedFile::read_all)
//! are the way in there.
//!
//! # Architecture
//!
//! The platform-agnostic surface ([`FilePicker`], [`PickRequest`], [`PickKind`],
//! [`MediaKind`], [`PickOutcome`], [`PickedFile`], [`FileStream`], [`PickError`])
//! lives here; one cfg-gated backend compiles per target, each supplying a
//! `pick` plus a backend `PickedFile`/`FileStream`:
//!
//! - **iOS** — `UIDocumentPickerViewController` (open) for documents,
//!   `PHPickerViewController` for media.
//! - **macOS** — `NSOpenPanel`.
//! - **Android** — `ACTION_OPEN_DOCUMENT` for documents, the Photo Picker for
//!   media.
//! - **Windows** — `IFileOpenDialog`.
//! - **Linux** — `xdg-desktop-portal` `FileChooser.OpenFile`.
//! - **web** — `showOpenFilePicker()`, with an `<input type=file>` fallback.

#![deny(missing_docs)]

use std::path::Path;

mod error;
pub use error::PickError;

mod mime;

// Shared 1-MiB-chunk path reader for the backends that read from a real file
// path (macOS/iOS/Windows/Linux). Android (fd) and web (Blob) bring their own.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "linux"
    )
))]
mod fsread;

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an `imp`
// module with:
//   async fn pick(&PickRequest) -> Result<Option<Vec<imp::PickedFile>>, PickError>
//      (Ok(None) == the user cancelled)
//   struct PickedFile  { name/mime/size/path + async fn open() -> imp::FileStream }
//   struct FileStream  { async fn chunk() -> Result<Option<Vec<u8>>, PickError> }
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

/// How much to read per [`FileStream::chunk`] — 1 MiB. Big enough that the
/// per-chunk overhead (an FFI hop on Android, a promise on web) is amortized,
/// small enough that a multi-GB file never lands in memory at once.
///
/// Used by the path reader ([`fsread`]) and Android; the web backend's
/// `ReadableStream` chooses its own chunk size, so this is dead there.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) const READ_CHUNK: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// Which kind of picker to present.
///
/// The split is real, not cosmetic: on mobile, [`Media`](PickKind::Media) routes
/// to a *dedicated* photo picker (iOS `PHPickerViewController`, Android Photo
/// Picker) with better UX and no library-wide permission, while
/// [`Documents`](PickKind::Documents) opens the general file browser. On desktop
/// and web there is no separate media surface, so `Media` is the document picker
/// pre-filtered to images/videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickKind {
    /// The general document picker, filtered to these MIME types. An empty list
    /// means "any file".
    Documents(Vec<String>),
    /// The photo/video picker (dedicated where the platform has one).
    Media(MediaKind),
}

/// Which media a [`PickKind::Media`] request accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Photos only.
    Images,
    /// Videos only.
    Videos,
    /// Photos and videos.
    ImagesAndVideos,
}

/// A request to pick one or more files: what kind of picker, and whether the
/// user may select more than one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRequest {
    /// Documents (MIME-filtered) or media — see [`PickKind`].
    pub kind: PickKind,
    /// Allow selecting multiple files. When `false`, the result holds at most
    /// one [`PickedFile`].
    pub allow_multiple: bool,
}

impl PickRequest {
    /// A document picker filtered to `mimes` (e.g. `["application/pdf"]`). Pass
    /// an empty iterator to accept any file. Single-selection by default; chain
    /// [`multiple`](Self::multiple) to allow more.
    pub fn documents(mimes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            kind: PickKind::Documents(mimes.into_iter().map(Into::into).collect()),
            allow_multiple: false,
        }
    }

    /// The media (photo/video) picker. Single-selection by default; chain
    /// [`multiple`](Self::multiple) to allow more.
    pub fn media(kind: MediaKind) -> Self {
        Self {
            kind: PickKind::Media(kind),
            allow_multiple: false,
        }
    }

    /// Allow the user to select multiple files.
    pub fn multiple(mut self) -> Self {
        self.allow_multiple = true;
        self
    }
}

/// The result of a [`pick`](FilePicker::pick): the user either chose file(s) or
/// dismissed the picker.
#[non_exhaustive]
pub enum PickOutcome {
    /// The user chose one or more files. Always a `Vec` (length 1 for a
    /// single-selection request); empty only if the platform reports a
    /// completed-but-empty selection.
    Picked(Vec<PickedFile>),
    /// The user dismissed the picker without choosing. Not an error.
    Cancelled,
}

/// A file the user picked. A lazy handle — see the [crate docs](crate#reading-without-blowing-up-memory)
/// for how to read it without buffering the whole thing.
pub struct PickedFile {
    inner: imp::PickedFile,
}

impl PickedFile {
    /// The file's display name (e.g. `"report.pdf"`).
    pub fn name(&self) -> &str {
        self.inner.name()
    }

    /// The file's MIME type, best-effort (e.g. `"application/pdf"`). May be an
    /// empty string or `"application/octet-stream"` when the platform doesn't
    /// report one.
    pub fn mime(&self) -> &str {
        self.inner.mime()
    }

    /// The file's size in bytes, when the platform reports it.
    pub fn size(&self) -> Option<u64> {
        self.inner.size()
    }

    /// A real filesystem path to the file, when one exists.
    ///
    /// `Some` on desktop (macOS/Windows/Linux) and the iOS document picker —
    /// stream from it directly. `None` on web (no filesystem) and Android
    /// (`content://` URIs have no path), and for media-library assets; use
    /// [`open`](Self::open) / [`copy_to`](Self::copy_to) there.
    pub fn path(&self) -> Option<&Path> {
        self.inner.path()
    }

    /// Open a chunked, RAM-safe reader over the file's contents. Works on every
    /// platform. Pull successive chunks with [`FileStream::chunk`] until it
    /// yields `None` (EOF).
    pub async fn open(&self) -> Result<FileStream, PickError> {
        Ok(FileStream {
            inner: self.inner.open().await?,
        })
    }

    /// Stream the file to `dest`, returning the number of bytes written. Never
    /// loads the whole file into memory.
    ///
    /// When a native [`path`](Self::path) exists this uses the OS's own file
    /// copy; otherwise it streams [`open`](Self::open) chunk-by-chunk to a new
    /// file at `dest`. (On web there is no local filesystem to copy *to* — use
    /// [`open`](Self::open) / [`read_all`](Self::read_all) instead.)
    pub async fn copy_to(&self, dest: impl AsRef<Path>) -> Result<u64, PickError> {
        let dest = dest.as_ref();
        if let Some(src) = self.path() {
            return std::fs::copy(src, dest).map_err(|e| PickError::Io(e.to_string()));
        }
        use std::io::Write;
        let mut stream = self.open().await?;
        let mut out = std::fs::File::create(dest).map_err(|e| PickError::Io(e.to_string()))?;
        let mut total: u64 = 0;
        while let Some(chunk) = stream.chunk().await? {
            out.write_all(&chunk).map_err(|e| PickError::Io(e.to_string()))?;
            total += chunk.len() as u64;
        }
        out.flush().map_err(|e| PickError::Io(e.to_string()))?;
        Ok(total)
    }

    /// Read the **entire** file into memory.
    ///
    /// Convenience for files you know are small (a config, an avatar). For
    /// anything potentially large, prefer [`open`](Self::open) or
    /// [`copy_to`](Self::copy_to) so you never hold the whole file in RAM.
    pub async fn read_all(&self) -> Result<Vec<u8>, PickError> {
        let mut stream = self.open().await?;
        let mut buf = Vec::new();
        while let Some(chunk) = stream.chunk().await? {
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }
}

/// A chunked, forward-only reader over a [`PickedFile`]'s contents. Obtained
/// from [`PickedFile::open`]. Drives the RAM-safe read path on every backend.
pub struct FileStream {
    inner: imp::FileStream,
}

impl FileStream {
    /// Read the next chunk (up to ~1 MiB). Returns `Ok(None)` at end of file.
    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, PickError> {
        self.inner.chunk().await
    }
}

/// Presents the platform's file-open UI. Cheap to construct and clone; holds no
/// resources until you [`pick`](Self::pick).
#[derive(Clone, Default)]
pub struct FilePicker {
    _private: (),
}

impl FilePicker {
    /// Create a picker handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Present the platform's file-open UI for `request`.
    ///
    /// Resolves to [`PickOutcome::Picked`] with the chosen file(s), or
    /// [`PickOutcome::Cancelled`] if the user dismisses the picker; errors only
    /// on a genuine failure (no UI surface, backend error, unsupported target).
    pub async fn pick(&self, request: PickRequest) -> Result<PickOutcome, PickError> {
        match imp::pick(&request).await? {
            Some(files) => Ok(PickOutcome::Picked(
                files.into_iter().map(|inner| PickedFile { inner }).collect(),
            )),
            None => Ok(PickOutcome::Cancelled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_request_collects_mimes_single_select() {
        let req = PickRequest::documents(["application/pdf", "text/plain"]);
        assert_eq!(
            req.kind,
            PickKind::Documents(vec!["application/pdf".into(), "text/plain".into()])
        );
        assert!(!req.allow_multiple);
    }

    #[test]
    fn empty_documents_means_any_file() {
        let req = PickRequest::documents(Vec::<String>::new());
        assert_eq!(req.kind, PickKind::Documents(vec![]));
    }

    #[test]
    fn media_request_and_multiple_flag() {
        let req = PickRequest::media(MediaKind::Images).multiple();
        assert_eq!(req.kind, PickKind::Media(MediaKind::Images));
        assert!(req.allow_multiple);
    }
}
