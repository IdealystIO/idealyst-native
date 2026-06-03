# file-export

Save a file to a **user-chosen** location through the platform's own "save" UI.
The counterpart to [`files`](../files): where `files` writes to app-private
storage the user never sees, `file-export` hands a file to the *user's* file
system at a destination they pick.

```rust
use file_export::{FileExport, SaveRequest, SaveOutcome};

// Export an on-disk file (e.g. a recording from media-writer).
let outcome = FileExport::new()
    .save(SaveRequest::path("clip.mp4", "video/mp4", recording_path))
    .await?;

match outcome {
    SaveOutcome::Saved { location } => { /* user saved it */ }
    SaveOutcome::Cancelled => { /* user dismissed the picker */ }
}
```

`SaveRequest::path(name, mime, path)` copies an existing file (best for media —
no load into memory); `SaveRequest::bytes(name, mime, bytes)` writes in-memory
content (and is the only form supported on web).

## No permission required

Every backend is **user-initiated UI** — the act of picking a location is what
grants access to it. So this SDK needs **no storage permission** on any
platform, unlike broad filesystem access.

## Backends

| Target | Mechanism |
| --- | --- |
| iOS | `UIDocumentPickerViewController` (export) — "Save to Files" |
| macOS | `NSSavePanel` |
| Android | Storage Access Framework (`ACTION_CREATE_DOCUMENT`) via a Kotlin shim |
| Windows | `IFileSaveDialog` |
| Linux | `xdg-desktop-portal` `FileChooser.SaveFile` (via `ashpd`) |
| web | `showSaveFilePicker()`, with an `<a download>` Blob fallback |

The user dismissing the picker is `SaveOutcome::Cancelled`, **not** an error.

## Composes with media-writer / files

```rust
// 1. record to the app sandbox
let path = recording.stop().await?;                 // media-writer → files
// 2. offer to export it
let local = store.local_path(&path).unwrap();       // real path on native
FileExport::new().save(SaveRequest::path("clip.mp4", "video/mp4", local)).await?;
```

On web there's no path, so read the bytes from the store and use
`SaveRequest::bytes(...)`.

## Verification status

Every backend is **interactive OS UI**, so it can't be host-tested
automatically (a save dialog needs a human to pick a location). Each backend is
**compile-verified** for its target (Apple on host; web/wasm32;
Android/aarch64; Windows/x86_64; Linux/x86_64) and needs **manual** device /
desktop verification — the same posture as the `camera` / `biometrics`
interactive backends.

## Platform notes

- **iOS/macOS** — link `UIKit` / `AppKit` (done in the backend). Sandboxed
  macOS apps get write access to the panel's chosen location automatically.
- **Android** — ships `runtime/kotlin/.../RustFileExport.kt`; the SAF result is
  routed through the shared `io.idealyst.runtime.RustActivityResult` registry,
  so no `MainActivity` edits are needed.
- **Linux** — talks to a running `xdg-desktop-portal`; falls through to an
  error if no portal is present (headless).
- **web** — `showSaveFilePicker` is Chromium-only and secure-context-only; the
  `<a download>` fallback saves to the browser's default download location and
  reports `Saved { location: None }` (no completion signal is exposed).
