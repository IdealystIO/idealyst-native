//! `pdf-demo` — load a PDF from disk and render it on the GPU.
//!
//! **Open PDF…** opens the native file picker (`file-picker` SDK); the chosen
//! file's bytes are stashed in a signal that a reactive [`PdfReactive`] viewer
//! reads, so the page re-renders with no remount. The PDF is interpreted once
//! per file (hayro) into a renderer-agnostic [`canvas::Scene`] — text as glyph
//! runs, vectors as fills, images as blits — and drawn by `canvas-vello` on the
//! GPU (vello/Metal here), falling back to `canvas-native` (CPU) where the GPU
//! can't run vello. A small built-in document shows until you load your own.
//!
//! **Run with `--local`** (`idealyst dev --macos --local`). The canvas carries a
//! `draw` closure in its `Element::External` payload, which can't be serialized
//! across the default dev-server wire — so canvas-based SDKs (this one,
//! `whiteboard-demo`, `canvas-demo`) need single-process local-render mode, or
//! the client shows "Component not available: canvas_core::CanvasProps".

use std::rc::Rc;

// Link anchor: `canvas-native` self-registers its renderer via `inventory`; the
// `as _` keeps it linked so it's the fallback when vello self-gates off.
use canvas_native as _;
use file_picker::{FilePicker, PickOutcome, PickRequest};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use pdf::PdfReactive;
use runtime_core::{driver::spawn_async, signal, text, ui, Element, IntoElement, Signal};

/// `canvas-vello` needs an explicit `register` (it self-gates on GPU
/// capability); registering it last makes it win over `canvas-native` where the
/// GPU can run vello, and step aside (leaving the native fallback) otherwise.
pub fn register_extensions<B: runtime_core::RegisterExternal>(backend: &mut B) {
    #[cfg(any(
        target_arch = "wasm32",
        all(
            any(target_os = "macos", target_os = "ios", target_os = "android"),
            not(target_arch = "wasm32")
        )
    ))]
    canvas_vello::register(backend);
    let _ = backend;
}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // The document being shown. `Rc` so the reactive viewer can identity-compare
    // (re-interpret only on a genuinely new file). Starts with a built-in sample.
    let doc: Signal<Option<Rc<Vec<u8>>>> = signal!(Some(Rc::new(sample_pdf())));
    let status: Signal<String> = signal!("Showing the built-in sample.".to_string());

    let on_open = move || {
        status.set("Opening file picker…".to_string());
        spawn_async(async move {
            match FilePicker::new().pick(PickRequest::documents(["application/pdf"])).await {
                Ok(PickOutcome::Picked(files)) => {
                    let Some(file) = files.into_iter().next() else {
                        status.set("Nothing picked.".to_string());
                        return;
                    };
                    let name = file.name().to_string();
                    match file.read_all().await {
                        Ok(bytes) => {
                            status.set(format!("Loaded {name} ({} bytes).", bytes.len()));
                            doc.set(Some(Rc::new(bytes)));
                        }
                        Err(e) => status.set(format!("Read error: {e}")),
                    }
                }
                Ok(PickOutcome::Cancelled) => status.set("Cancelled.".to_string()),
                // `PickOutcome` is `#[non_exhaustive]`.
                Ok(_) => {}
                Err(e) => status.set(format!("Picker error: {e}")),
            }
        });
    };

    // Body assembled as a Vec so the pre-built reactive `viewer` Element splats
    // in alongside the macro-authored children (the canvas-demo idiom).
    let body: Vec<Element> = vec![
        ui! { Typography(content = "PDF on the GPU".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Open a PDF from your device — it's interpreted into a canvas \
                    Scene and rendered by vello on the GPU. Text becomes glyph runs, \
                    vectors become fills."
                    .to_string(),
                muted = true,
            )
        },
        ui! { button(label = "Open PDF…".to_string(), on_click = on_open) },
        text(move || status.get()).into_element(),
        PdfReactive(move || doc.get(), 0, 520.0, 680.0).into_element(),
    ];

    ui! {
        scroll_view {
            Stack(gap = StackGap::Md, padding = StackPadding::Lg) { body }
        }
    }
}

/// A small, self-contained PDF (a titled box + body text in Helvetica) built at
/// runtime with correct xref offsets, so the demo shows something before you
/// load your own file.
fn sample_pdf() -> Vec<u8> {
    let content = "\
0.20 0.45 0.85 rg
60 540 372 160 re
f
BT /F1 28 Tf 1 1 1 rg 90 640 Td (Idealyst PDF) Tj ET
BT /F1 13 Tf 0 0 0 rg 60 500 Td (Rendered on the GPU via vello.) Tj ET
BT /F1 13 Tf 60 470 Td (Open a file to view your own PDF.) Tj ET";

    let objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 480 720] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];

    let mut pdf = String::from("%PDF-1.7\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }
    let xref_offset = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for off in &offsets {
        pdf.push_str(&format!("{off:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF",
        objects.len() + 1
    ));
    pdf.into_bytes()
}
