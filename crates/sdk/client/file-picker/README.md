# file-picker

Pick file(s) from the user's filesystem through the platform's native "open"
UI. The counterpart to [`file-export`](../file-export) (which *saves* a file to
a user-chosen location), `file-picker` *opens* one (or many) the user chooses.

```rust
use file_picker::{FilePicker, PickRequest, PickOutcome, MediaKind};

// A document picker (PDFs + plain text), single selection.
let outcome = FilePicker::new()
    .pick(PickRequest::documents(["application/pdf", "text/plain"]))
    .await?;

if let PickOutcome::Picked(files) = outcome {
    for file in &files {
        println!("{} ({} bytes)", file.name(), file.size().unwrap_or(0));
        // Stream to the app sandbox — never buffers the whole file:
        file.copy_to(format!("/tmp/{}", file.name())).await?;
    }
}

// Or the dedicated media picker (photos + videos), multi-select.
let photos = FilePicker::new()
    .pick(PickRequest::media(MediaKind::ImagesAndVideos).multiple())
    .await?;
```

## Documents vs. media

`PickRequest` has one entry point with a `kind`:

- `PickRequest::documents(mimes)` — the general file picker, filtered by MIME
  type (empty = any file).
- `PickRequest::media(MediaKind)` — photos/videos. On **mobile** this routes to
  the dedicated photo picker (iOS `PHPickerViewController`, Android Photo
  Picker), which has a better UX and needs **no** photo-library permission. On
  desktop/web there is no separate media surface, so it's the document picker
  pre-filtered to images/videos.

Chain `.multiple()` to allow selecting more than one file; the result is always
a `Vec<PickedFile>`.

## Reading without blowing up memory

A `PickedFile` is a **lazy handle** — picking a 10 GB video does not read 10 GB
into RAM. In order of preference:

- `path()` — a real filesystem path (desktop + iOS documents); stream it
  yourself.
- `open()` — a chunked `FileStream` that works on **every** platform (a real
  file on desktop, a file descriptor on Android, a `Blob` stream on web). The
  universal, RAM-safe accessor.
- `copy_to(dest)` — stream the file to a path (built on `open`; never buffers).
- `read_all()` — a convenience that *does* buffer the whole file; small files
  only.

On **web** there is no filesystem path, which is exactly why the streaming
reader exists: `open()` / `read_all()` are the way in there.

## Drag-and-drop (`FileDropZone`)

The other way OS files get into an app: the user **drags** them from Finder /
the desktop / another browser tab and **drops** them onto a view. A dropped file
surfaces as the same lazy `PickedFile` a picker returns, so your upload code is
identical however the file arrived.

```rust
use file_picker::{FileDropZone, PickedFile};

let dz = FileDropZone::new().on_drop(|files: Vec<PickedFile>| {
    for f in files {
        // identical to a picked file — stream it to your uploader
        let _ = f.name();
    }
});
let active = dz.active(); // reactive Signal<bool>: true while a drag hovers

// Make any view a drop target and highlight it while a drag is over it:
// ui! {
//     view(style = move || if active.get() { Highlighted() } else { Idle() }) {
//         text("Drop files here")
//     }
//     .on_file_drop(dz.handler())
// }
```

`handler()` accepts the drag (web `preventDefault`, macOS copy operation),
tracks the `active` signal, and converts dropped files to `PickedFile`s for
`on_drop`. Optional `on_enter` / `on_leave` fire on the hover transitions.

Drag-*in* is a desktop/web affordance: it's delivered on **web** (HTML5
`DataTransfer`) and **macOS** (`NSDraggingDestination`). On iOS / Android (no OS
file-drag concept) and the scaffold Windows / Linux backends the `on_file_drop`
view slot is a no-op, so the zone renders inertly — exactly like a wheel handler
on a phone. Dropping app content *out* to Finder (a native drag **source**) is a
separate, not-yet-built feature.

`FileDropZone` lives behind the default-on `drop` cargo feature (it pulls in
`runtime-core` for the view handler + reactive signal); disable it if you only
want the imperative `pick()`.

See [`examples/file-drop-demo`](../../../examples/file-drop-demo) for a runnable
drop-zone demo.

## No permission required

Every backend is user-initiated UI — the act of picking is what grants access
to the chosen file(s). So this SDK needs no storage or photo-library permission
on any platform.

## Backends

One cfg-gated backend compiles per target:

| Platform | Documents | Media | `path()` | Reader |
|----------|-----------|-------|----------|--------|
| iOS | `UIDocumentPickerViewController` (open, security-scoped) | `PHPickerViewController` | yes | file on disk |
| macOS | `NSOpenPanel` | `NSOpenPanel` (image/movie filter) | yes | file on disk |
| Android | `ACTION_OPEN_DOCUMENT` | Photo Picker (`ACTION_PICK_IMAGES`) | no | detached fd |
| Windows | `IFileOpenDialog` | `IFileOpenDialog` (ext filter) | yes | file on disk |
| Linux | portal `FileChooser.OpenFile` | same (mime/glob filter) | yes | file on disk |
| web | `showOpenFilePicker()` / `<input type=file>` | same (`accept` filter) | no | `Blob` stream |

The Android Kotlin shim (`runtime/kotlin/.../RustFilePicker.kt`) is
auto-discovered and compiled by `idealyst run android` via the
`[package.metadata.idealyst.android]` block — no `MainActivity` edits needed.

See [`examples/file-picker-demo`](../../../examples/file-picker-demo) for a
runnable demo.

## Testing checklist

Manual verification per backend — every backend is **interactive OS UI**; an
unchecked **native** box means the code compiles for that target but isn't
confirmed on real hardware yet (macOS is host-verified). Tick each item as you
exercise it.

**Automated**
- [ ] `cargo test -p file-picker` — portable request/outcome types, `PickedFile` handle plumbing
- [ ] `cargo build -p file-picker --target wasm32-unknown-unknown` — web (`showOpenFilePicker` / `<input type=file>`) target compiles

**Behavior**

For each platform: the native picker appears; a picked file streams in chunks
via `open()` / `copy_to()` (**never** the whole file in RAM — pick a large
video and confirm memory stays flat); `.multiple()` returns a `Vec<PickedFile>`;
on mobile, `PickRequest::media` routes to the dedicated photo picker (no
photo-library permission), `documents` to the general file picker.

- [ ] **Web** — `showOpenFilePicker()` / `<input type=file>`; no `path()`, `open()`/`read_all()` stream a `Blob`
- [ ] **iOS** — `UIDocumentPickerViewController` (documents, security-scoped) vs `PHPickerViewController` (media); `path()` available
- [ ] **Android** — `ACTION_OPEN_DOCUMENT` vs Photo Picker (`ACTION_PICK_IMAGES`); no `path()`, reads via detached fd (Kotlin shim auto-discovered — no `MainActivity` edits)
- [ ] **macOS** — `NSOpenPanel` (documents) vs image/movie-filtered panel (media); `path()` available (host-verified)
- [ ] **Windows / Linux** — `IFileOpenDialog` / portal `FileChooser.OpenFile`; `path()` available

**Security / Permissions**
- [ ] No storage or photo-library permission is requested on any platform — the user picking the file is the grant
