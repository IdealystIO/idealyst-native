//! `file-picker-demo` — picks file(s) from the local filesystem with the
//! `file-picker` SDK.
//!
//! **Pick files** opens the document picker; **Pick photos/videos** opens the
//! media picker (the dedicated photo picker on iOS/Android, a filtered file
//! dialog on desktop/web). For each picked file the app shows its name, MIME,
//! and size, and reads **just the first chunk** through the streaming reader —
//! demonstrating that a large file is never buffered whole.
//!
//! No permission is requested on any platform: picking is user-initiated UI
//! that grants access to exactly the chosen file(s).

use file_picker::{FilePicker, MediaKind, PickOutcome, PickRequest};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use runtime_core::{signal, text, ui, Element, Signal};

/// No `Element::External` SDKs to register — `file-picker` is a capability
/// crate, not a rendered primitive.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

/// Run one pick and report the result, streaming only the first chunk of each
/// picked file (so picking a huge file stays cheap).
async fn run_pick(request: PickRequest, status: Signal<String>, info: Signal<String>) {
    match FilePicker::new().pick(request).await {
        Ok(PickOutcome::Picked(files)) => {
            if files.is_empty() {
                status.set("Nothing picked".to_string());
                return;
            }
            let mut lines = Vec::new();
            for file in &files {
                let size = file
                    .size()
                    .map(|s| format!("{s} bytes"))
                    .unwrap_or_else(|| "size unknown".to_string());
                // Read only the first chunk — never the whole (possibly huge)
                // file — to prove the streaming reader works on this platform.
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
            status.set(format!("Picked {} file(s)", files.len()));
            info.set(lines.join("\n"));
        }
        Ok(PickOutcome::Cancelled) => status.set("Cancelled".to_string()),
        // `PickOutcome` is `#[non_exhaustive]`.
        Ok(_) => {}
        Err(e) => status.set(format!("Error: {e}")),
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let status: Signal<String> = signal!("Pick a file to begin".to_string());
    let info: Signal<String> = signal!(String::new());

    let on_docs = move || {
        status.set("Opening file picker…".to_string());
        info.set(String::new());
        runtime_core::driver::spawn_async(async move {
            // Empty filter = any file; multi-select on.
            run_pick(
                PickRequest::documents(Vec::<String>::new()).multiple(),
                status,
                info,
            )
            .await;
        });
    };

    let on_photos = move || {
        status.set("Opening photo picker…".to_string());
        info.set(String::new());
        runtime_core::driver::spawn_async(async move {
            run_pick(
                PickRequest::media(MediaKind::ImagesAndVideos).multiple(),
                status,
                info,
            )
            .await;
        });
    };

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "File Picker".to_string(), kind = idea_ui::typography_kind::H1)
            Typography(
                content = "Pick file(s) from your device. 'Pick files' opens the document \
                    picker; 'Pick photos/videos' opens the media picker (a dedicated photo \
                    picker on mobile). The app reads only the first chunk of each file, so a \
                    multi-GB pick is never loaded into memory. No permission is requested."
                    .to_string(),
                muted = true,
            )
            text(move || status.get())
            text(move || {
                let i = info.get();
                if i.is_empty() {
                    "—".to_string()
                } else {
                    i
                }
            })
            button(label = "Pick files".to_string(), on_click = on_docs)
            button(label = "Pick photos/videos".to_string(), on_click = on_photos)
        }
    }
}
