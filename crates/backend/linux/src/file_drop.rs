//! OS file drag-and-drop delivery for `on_file_drop` views — the GTK4
//! counterpart of macOS's `NSDraggingDestination` and web's
//! `dragenter`/`dragover`/`dragleave`/`drop`.
//!
//! `Backend::install_file_drop_handler` has a **no-op default body**, so a
//! backend that never implements it still compiles and renders — every
//! `.on_file_drop()` view just silently receives nothing. That is what left
//! the `file-picker` SDK's `FileDropZone` inert on Linux: the SDK-side
//! `picked_from_dropped` was ready, but the raw channel that feeds it never
//! fired. This module sources that channel.
//!
//! # Mapping
//!
//! GTK4 delivers drop input through a [`gtk4::DropTarget`] event controller
//! rather than per-widget virtual methods. One controller covers the whole
//! drag lifecycle:
//!
//! | Framework                 | GTK signal            |
//! |---------------------------|-----------------------|
//! | [`FileDropPhase::Entered`]| `enter` / `motion`    |
//! | [`FileDropPhase::Exited`] | `leave`               |
//! | [`FileDropPhase::Dropped`]| `drop`                |
//!
//! The target advertises the two file-drag GTypes a file manager offers:
//! [`gdk::FileList`] (a multi-file drag, the common case) and a bare
//! [`gio::File`] (a single-file drag). On `drop` we read whichever matched off
//! the `glib::Value` and turn each `gio::File` into a neutral
//! [`DroppedFile`] carrying a real filesystem `path` — like macOS (and unlike
//! web) Linux hands us a path, so the SDK streams it with `std::fs` and needs
//! no opaque backend `source` handle.
//!
//! `enter`/`motion` return [`gdk::DragAction::COPY`] only when the framework
//! handler *accepts* the drag (returns [`runtime_core::TouchResponse::CONSUMED`]) — the GTK
//! analogue of web's `preventDefault` and macOS's returning a copy operation;
//! otherwise GTK shows the no-drop cursor and rejects the drop. A drop of a
//! non-`file://` source (a remote URI with no local path) contributes no
//! `DroppedFile` rather than failing the whole drop.

use gtk4::prelude::*;
use gtk4::{gdk, gio};

use runtime_core::{DroppedFile, FileDropEvent, FileDropHandler, FileDropPhase, TouchPoint};

/// Turn one dropped [`gio::File`] into a neutral [`DroppedFile`], or `None`
/// when it has no local filesystem path (e.g. a remote `smb://`/`http://`
/// URI a file manager can also offer — we can't hand the SDK a `std::fs`
/// path for those, and the SDK's `picked_from_dropped` requires one).
///
/// The name is the basename, the size comes from `fs::metadata` (best-effort
/// — absent for a path we can't stat), and the MIME type is guessed from the
/// filename via GLib's content-type database (already MIME-typed on Unix),
/// falling back to `application/octet-stream` — the honest default, matching
/// the macOS backend's behaviour.
pub(crate) fn dropped_from_file(file: &gio::File) -> Option<DroppedFile> {
    let path = file.path()?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(&path).ok().map(|m| m.len());
    Some(DroppedFile {
        name,
        mime: mime_for_path(&path),
        size,
        path: Some(path),
        source: None,
    })
}

/// Best-effort MIME from a dropped file's extension. Deliberately the SAME
/// small table the macOS backend (`mime_for_path`) uses rather than GLib's
/// `g_content_type_guess`: uniform output across backends (CLAUDE.md §7) — a
/// dropped `.png` must report `image/png` on Linux exactly as on macOS — and
/// name-only guessing (we must not sniff bytes: the file may be multi-GB, and
/// GLib's guess trips its "zero-size" detection when handed empty content).
/// Falls back to `application/octet-stream`, the honest default.
fn mime_for_path(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let mime = match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("heic") => "image/heic",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("html") | Some("htm") => "text/html",
        Some("zip") => "application/zip",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

/// Read the file(s) off a dropped `glib::Value`. A file manager offers either
/// a [`gdk::FileList`] (multi-file drag) or a single [`gio::File`]; we accept
/// both GTypes on the target, so handle both here. Unknown value types (a
/// text/other drag that slipped through) yield an empty list.
pub(crate) fn dropped_from_value(value: &gtk4::glib::Value) -> Vec<DroppedFile> {
    if let Ok(list) = value.get::<gdk::FileList>() {
        return list.files().iter().filter_map(dropped_from_file).collect();
    }
    if let Ok(file) = value.get::<gio::File>() {
        return dropped_from_file(&file).into_iter().collect();
    }
    Vec::new()
}

/// Attach a [`gtk4::DropTarget`] to `widget` that drives `handler` through the
/// Entered → Exited/Dropped lifecycle for OS file drags.
pub(crate) fn install(widget: &gtk4::Widget, handler: FileDropHandler) {
    // Advertise both file-drag GTypes. `DropTarget::new` seeds one; `set_types`
    // installs the full accepted set (FileList for multi-file, File for single).
    let target = gtk4::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    target.set_types(&[gdk::FileList::static_type(), gio::File::static_type()]);

    // Fire `Entered` and gate acceptance on the handler's response — return
    // COPY (GTK then shows the copy cursor and permits the drop) only when the
    // handler consumes, else the empty action (GTK rejects the drag). This is
    // the GTK equivalent of web `preventDefault` / macOS returning a copy op.
    let accept = {
        let handler = handler.clone();
        move |x: f64, y: f64| -> gdk::DragAction {
            let ev = FileDropEvent {
                phase: FileDropPhase::Entered,
                position: TouchPoint::new(x as f32, y as f32),
            };
            if handler(&ev).consumed {
                gdk::DragAction::COPY
            } else {
                gdk::DragAction::empty()
            }
        }
    };
    {
        let accept = accept.clone();
        target.connect_enter(move |_t, x, y| accept(x, y));
    }
    {
        // GTK fires `motion` continuously as the pointer moves within the
        // widget — mirror web firing `dragover` repeatedly and keep the accept
        // decision live. The handler is cheap (it toggles a signal).
        target.connect_motion(move |_t, x, y| accept(x, y));
    }
    {
        let handler = handler.clone();
        target.connect_leave(move |_t| {
            let ev = FileDropEvent {
                phase: FileDropPhase::Exited,
                position: TouchPoint::new(0.0, 0.0),
            };
            let _ = handler(&ev);
        });
    }
    {
        let handler = handler.clone();
        target.connect_drop(move |_t, value, x, y| {
            let files = dropped_from_value(value);
            // Nothing usable in the payload (e.g. a remote-only URI): decline
            // the drop so GTK reports failure rather than a silent success.
            if files.is_empty() {
                return false;
            }
            let ev = FileDropEvent {
                phase: FileDropPhase::Dropped(files),
                position: TouchPoint::new(x as f32, y as f32),
            };
            handler(&ev).consumed
        });
    }

    widget.add_controller(target);
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real drag-drop gesture cannot be driven headlessly (it needs a live
    // compositor delivering GDK drag events and a display), so the end-to-end
    // `install`/DropTarget path can't be exercised in a unit test. What we CAN
    // and do test is the pure conversion the drop handler relies on — turning a
    // dropped `gio::File` / value into the SDK's `DroppedFile` payload — since
    // that is where the parsing logic (path, name, size, mime) lives and where
    // a regression would bite. `gio::File` construction needs only GLib, not a
    // display, so these run anywhere.

    #[test]
    fn dropped_file_carries_path_name_size_and_mime() {
        let dir = std::env::temp_dir();
        let path = dir.join("idealyst_file_drop_unit_test.txt");
        std::fs::write(&path, b"hello drop").unwrap();

        let file = gio::File::for_path(&path);
        let dropped = dropped_from_file(&file).expect("a local path yields a DroppedFile");

        assert_eq!(dropped.name, "idealyst_file_drop_unit_test.txt");
        assert_eq!(dropped.path.as_deref(), Some(path.as_path()));
        assert_eq!(dropped.size, Some(10), "size comes from fs::metadata");
        assert_eq!(
            dropped.mime, "text/plain",
            "MIME guessed from the .txt extension via GLib content types",
        );
        assert!(dropped.source.is_none(), "path is the byte source, no opaque handle");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remote_uri_without_local_path_yields_none() {
        // A file manager can offer a non-`file://` source (remote share, http).
        // GIO reports no local `path()` for it, so we must skip it rather than
        // hand the SDK a bogus path.
        let file = gio::File::for_uri("http://example.com/report.pdf");
        assert!(
            dropped_from_file(&file).is_none(),
            "a remote URI has no std::fs path and must not become a DroppedFile",
        );
    }

    #[test]
    fn value_with_single_file_reads_it() {
        let dir = std::env::temp_dir();
        let path = dir.join("idealyst_file_drop_value_test.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();

        // A single-file drag arrives as a `gio::File` GValue.
        let file = gio::File::for_path(&path);
        let value = file.to_value();
        let files = dropped_from_value(&value);

        assert_eq!(files.len(), 1, "one file in the value → one DroppedFile");
        assert_eq!(files[0].name, "idealyst_file_drop_value_test.png");
        assert_eq!(
            files[0].mime, "image/png",
            "MIME guessed from the .png extension",
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn value_with_no_file_type_is_empty() {
        // A non-file drag (a plain string) must contribute no DroppedFiles so
        // the drop handler declines it.
        let value = "just some text".to_value();
        assert!(
            dropped_from_value(&value).is_empty(),
            "a non-file value yields no files",
        );
    }
}
