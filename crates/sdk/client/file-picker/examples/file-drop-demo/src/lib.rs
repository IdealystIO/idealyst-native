//! `file-drop-demo` — drop OS files onto the app with the `file-picker` SDK's
//! [`FileDropZone`].
//!
//! A dashed drop zone highlights while a file drag hovers it (driven by the
//! zone's reactive `active` signal). On drop, the app lists each file's name,
//! MIME, and size, and reads **just the first chunk** through the streaming
//! reader — showing a dropped file is the same lazy [`PickedFile`] a picker
//! returns, and that a multi-GB drop is never buffered whole.
//!
//! Drag-in is a desktop/web affordance: it works on **web** (HTML5
//! `DataTransfer`) and **macOS** (`NSDraggingDestination`). Run it from this
//! directory (or pass the path):
//!
//! ```text
//! idealyst dev --web                              # opens a browser tab
//! idealyst dev --macos --local                    # native AppKit window
//! # or, from the repo root:
//! idealyst dev --web examples/file-drop-demo
//! idealyst dev --macos --local examples/file-drop-demo
//! ```
//!
//! Then drag a file from Finder / the desktop / another tab onto the dashed
//! zone: it turns blue while the drag hovers, and on release the file's name,
//! MIME, and size appear below.

use file_picker::{FileDropZone, PickedFile};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{
    signal, stylesheet, text, ui, Color, Element, FlexDirection, Length, Signal, StyleApplication,
};

/// No `Element::External` SDKs to register — `file-picker` is a capability
/// crate, not a rendered primitive.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

// The drop zone, idle: a dashed neutral box.
stylesheet! {
    DropIdle<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: runtime_core::AlignItems::Center,
            justify_content: runtime_core::JustifyContent::Center,
            padding: 40.0,
            min_height: Length::Px(160.0),
            border_width: 2.0,
            border_color: Color("#94a3b8".into()),
            border_radius: 12.0,
            background: Color("#f8fafc".into()),
        }
    }
}

// The drop zone, active (a file drag is hovering): accented border + tint.
stylesheet! {
    DropActive<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: runtime_core::AlignItems::Center,
            justify_content: runtime_core::JustifyContent::Center,
            padding: 40.0,
            min_height: Length::Px(160.0),
            border_width: 2.0,
            border_color: Color("#2563eb".into()),
            border_radius: 12.0,
            background: Color("#dbeafe".into()),
        }
    }
}

/// Report a batch of dropped files, streaming only the first chunk of each so a
/// huge drop stays cheap. Runs off the reactive turn (the drop handler) via the
/// async driver, exactly like the picker demo.
async fn report_drop(files: Vec<PickedFile>, status: Signal<String>, info: Signal<String>) {
    if files.is_empty() {
        status.set("Nothing dropped".to_string());
        return;
    }
    let mut lines = Vec::new();
    for file in &files {
        let size = file
            .size()
            .map(|s| format!("{s} bytes"))
            .unwrap_or_else(|| "size unknown".to_string());
        // Read only the first chunk — never the whole (possibly huge) file.
        let first = match file.open().await {
            Ok(mut stream) => match stream.chunk().await {
                Ok(Some(chunk)) => format!("first chunk {} bytes", chunk.len()),
                Ok(None) => "empty file".to_string(),
                Err(e) => format!("read error: {e}"),
            },
            Err(e) => format!("open error: {e}"),
        };
        let mime = if file.mime().is_empty() {
            "?".to_string()
        } else {
            file.mime().to_string()
        };
        lines.push(format!("• {} [{}] — {} — {}", file.name(), mime, size, first));
    }
    status.set(format!("Dropped {} file(s)", files.len()));
    info.set(lines.join("\n"));
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let status: Signal<String> = signal("Drag files onto the zone".to_string());
    let info: Signal<String> = signal(String::new());

    // One drop zone for the screen. `on_drop` hands us the same lazy
    // `PickedFile` handles a picker would; `active()` drives the highlight.
    let dz = FileDropZone::new().on_drop(move |files| {
        status.set("Reading dropped file(s)…".to_string());
        info.set(String::new());
        runtime_core::driver::spawn_async(async move {
            report_drop(files, status, info).await;
        });
    });
    let active = dz.active();
    let zone_active = active.clone();

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "File Drop".to_string(), kind = idea_ui::typography_kind::H1)
            Typography(
                content = "Drag file(s) from Finder, the desktop, or another browser tab onto \
                    the dashed zone below and release. Each dropped file is the same lazy, \
                    streamable handle the file picker returns — the app reads only the first \
                    chunk, so a multi-GB drop is never loaded into memory. Works on web and \
                    macOS; on iOS/Android (no OS file-drag) the zone is inert."
                    .to_string(),
                muted = true,
            )
            view(style = move || if zone_active.get() {
                StyleApplication::new(DropActive::sheet())
            } else {
                StyleApplication::new(DropIdle::sheet())
            }) {
                text(move || {
                    if active.get() {
                        "Release to drop".to_string()
                    } else {
                        "Drop files here".to_string()
                    }
                })
            }
            .on_file_drop(dz.handler())
            text(move || status.get())
            text(move || {
                let i = info.get();
                if i.is_empty() {
                    "—".to_string()
                } else {
                    i
                }
            })
        }
    }
}
