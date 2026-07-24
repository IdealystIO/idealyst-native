//! OS file drag-and-drop pipeline — the "drop a Finder file onto a view"
//! channel. It is the drag-*in* counterpart to the imperative file-picker
//! dialog: instead of the app popping an open-panel, the user drags files
//! from *outside* the app (Finder, the desktop, another browser tab) and
//! releases them over a view.
//!
//! Like [`wheel`](crate::wheel) and [`hover`](crate::hover) it is a backend
//! event channel, not a composable primitive: only the backend can source it
//! (it needs the platform's drag machinery — the web `DataTransfer`, macOS's
//! `NSDraggingDestination`), so an app cannot build it out of touch/press.
//! The backend installs a [`FileDropHandler`] on a view via
//! [`Backend::install_file_drop_handler`](crate::Backend::install_file_drop_handler)
//! and fires it for the drag lifecycle over that view.
//!
//! Only the desktop-class backends source these: **web** (`dragenter` /
//! `dragover` / `dragleave` / `drop`), **macOS** (`draggingEntered:` /
//! `draggingExited:` / `performDragOperation:`), and **Linux** (GTK4
//! `DropTarget` `enter`/`motion`/`leave`/`drop`). iOS / Android leave the
//! trait method at its no-op default — there is no OS file-drag concept on a
//! phone — and the scaffold Windows backend will fill it in (IDropTarget) when
//! it matures. This is genuine input availability, not a per-platform hack.
//!
//! The `file-picker` SDK wraps this raw channel in a friendly `FileDropZone`
//! that turns each [`DroppedFile`] into the same `PickedFile` handle a picker
//! dialog returns, so upload code is identical however the file arrived.

use std::any::Any;
use std::path::PathBuf;
use std::rc::Rc;

use crate::touch::{TouchPoint, TouchResponse};

/// One file delivered by an OS drop. It carries enough metadata to display the
/// file without reading it, plus one of two ways to get at the bytes:
///
/// - [`path`](DroppedFile::path) is `Some` on desktop backends (macOS), where a
///   real filesystem path is available — the SDK streams it with `std::fs`.
/// - [`source`](DroppedFile::source) carries an **opaque backend handle** used
///   when there is no path (web, where a dropped item is a `File`/`Blob` with no
///   filesystem location). The SDK downcasts it to the backend's concrete type.
///
/// Exactly one of the two is the authoritative byte source for a given backend.
pub struct DroppedFile {
    /// The file's display name (basename), e.g. `"photo.jpg"`.
    pub name: String,
    /// The MIME type as reported by the platform, or a best-effort guess from
    /// the extension. `"application/octet-stream"` when unknown.
    pub mime: String,
    /// The file size in bytes when the platform reports it up front.
    pub size: Option<u64>,
    /// Real filesystem path — `Some` on desktop (macOS), `None` on web.
    pub path: Option<PathBuf>,
    /// Opaque backend handle for streaming when [`path`](DroppedFile::path) is
    /// `None` (on web this holds the raw `web_sys::File`). The SDK downcasts it;
    /// `None` when `path` is the authoritative source.
    pub source: Option<Rc<dyn Any>>,
}

impl std::fmt::Debug for DroppedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DroppedFile")
            .field("name", &self.name)
            .field("mime", &self.mime)
            .field("size", &self.size)
            .field("path", &self.path)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

/// The phase of an OS file drag over a subscribed view. A drag delivers
/// `Entered` (possibly repeated as it moves) then exactly one terminal
/// `Exited` (left without dropping) or `Dropped` (released here).
#[non_exhaustive]
pub enum FileDropPhase {
    /// A file drag has entered (or is moving within) the view. Return
    /// [`TouchResponse::CONSUMED`] from the handler to **accept** the drag —
    /// on web this calls `preventDefault` so the browser allows the drop
    /// instead of navigating to the file; on macOS it returns a copy drag
    /// operation from `draggingEntered:`.
    Entered,
    /// The drag left the view without dropping.
    Exited,
    /// Files were released over the view.
    Dropped(Vec<DroppedFile>),
}

/// One delivery to a subscribed file-drop handler.
pub struct FileDropEvent {
    /// What stage of the drag this is. See [`FileDropPhase`].
    pub phase: FileDropPhase,
    /// Pointer position in the subscribed view's local coordinates.
    pub position: TouchPoint,
}

/// Boxed file-drop handler installed on a view. The framework holds one per
/// subscribed node and invokes it across the drag lifecycle. Returning
/// [`TouchResponse::CONSUMED`] for a [`FileDropPhase::Entered`] event accepts
/// the drag (web `preventDefault`, macOS drag-operation); the return value is
/// ignored for the other phases.
pub type FileDropHandler = Rc<dyn Fn(&FileDropEvent) -> TouchResponse>;
