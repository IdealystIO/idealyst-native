//! `qr-demo` — both halves of the `qr` SDK on one screen.
//!
//! **Generate**: type text and a `QrCode` component draws it as a crisp vector
//! square (a `canvas`, so it renders the same on every target).
//!
//! **Scan**: one camera stream, two consumers, one canvas. The `camera` SDK
//! opens a single `MediaStream`; a `canvas` composites it as a texture layer,
//! and the scanner reads codes out of the very same stream. The code's outline
//! is stroked over the frame in that same canvas (`Scene::texture(0)` first,
//! the outline after it), so the frame and the outline are one rendered image —
//! on the GPU wherever `canvas-vello` runs. There is no scanner-owned camera or
//! preview, and no per-platform code.
//!
//! Press **Start scanning** → the camera opens and a `spawn_then` loop
//! (`start_scan` / `pump`) awaits `QrScanner::next_frame()`, storing every
//! decoded frame's codes (an empty frame clears the outline). **Stop** clears
//! the signals: the scanner's last clone drops, its pending `next_frame()`
//! resolves `Stopped` (ending the loop), and the stream's last clone drops,
//! releasing the camera.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

use camera::{Camera, CameraConfig, CameraError, MediaStream};
use canvas::{Canvas, CanvasProps, Color, Fit, Path, Stroke, TextureLayer};
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use qr::{QrCode, QrScanner, Scan, ScanConfig, ScanError};
use runtime_core::{
    component, effect, signal, spawn_then, ui, Element, IntoElement, Length, ReadSignal, Signal,
    StyleRules, StyleSheet,
};

/// Registration seam. `canvas-native` is the baseline renderer on every
/// target; `canvas-vello` (GPU) registers second and wins only where the GPU
/// can run it (it self-gates: `navigator.gpu` on web, shader support on
/// native). `camera` and `qr` render nothing of their own (`QrCode` is a canvas).
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::caps::GraphicsOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    canvas_native::register(registry);
    canvas_vello::register(registry);
}

/// Runtime-server (sidecar) recorder seam.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(registry: &mut dev_server::newcore::SceneRegistry) {
    canvas_native::register(registry);
}

/// Android entry.
pub fn scene_app() -> Element {
    app()
}

/// The outline color and width, in canvas logical units.
const OUTLINE: Color = Color::new(0, 220, 90, 255);
const OUTLINE_WIDTH: f32 = 4.0;
/// Height of the preview box; the width follows the screen.
const PREVIEW_HEIGHT: f32 = 300.0;
/// Side of the generated code, in logical px.
const GENERATED_SIZE: f32 = 180.0;

/// Start scanning `stream`: build the scanner, store it (and the stream) in
/// their signals, and start the read loop.
///
/// The FIRST `next_frame()` comes from the local scanner, not from
/// `scanner_sig`. `set` only stages a write until the driver flushes, so
/// reading `scanner_sig` in the same turn that set it returns the old `None` —
/// the loop would end before it began. That was this demo's first bug: the
/// camera opened, the preview ran, and no code was ever read
/// (`tests/scan_loop.rs` pins it).
pub fn start_scan(
    stream: MediaStream,
    stream_sig: Signal<Option<MediaStream>>,
    scanner_sig: Signal<Option<QrScanner>>,
    scan_sig: Signal<Option<Scan>>,
    last: Signal<String>,
) {
    let scanner = QrScanner::new(&stream, ScanConfig::default());
    let first = scanner.next_frame();
    scanner_sig.set(Some(scanner));
    stream_sig.set(Some(stream));
    pump(first, scanner_sig, scan_sig, last);
}

/// Await one decoded frame, publish it, then re-arm. Every signal access
/// happens in `spawn_then`'s callback, which runs inside a turn or not at all,
/// so a teardown mid-decode can't write a freed signal.
///
/// Every frame is published — `scan_sig` goes back to `None` on the first
/// frame without a code, which is what removes the outline when the code
/// leaves the view. `last` keeps the most recent text.
///
/// The re-arm reads `scanner_sig` in a LATER turn than the one that set it, so
/// it sees the committed scanner. The callback never holds a scanner clone
/// itself: if it did, the pending future would keep the scanner (and the
/// camera) alive after Stop. When the scanner is dropped the future resolves
/// `Stopped` and the loop just doesn't re-arm.
fn pump(
    next: impl Future<Output = Result<Scan, ScanError>> + 'static,
    scanner_sig: Signal<Option<QrScanner>>,
    scan_sig: Signal<Option<Scan>>,
    last: Signal<String>,
) {
    spawn_then(next, move |result| match result {
        Ok(scan) => {
            if let Some(code) = scan.codes.first() {
                last.set(match code.text() {
                    Some(text) => text.to_string(),
                    None => format!("<{} bytes of binary data>", code.bytes.len()),
                });
            }
            scan_sig.set((!scan.codes.is_empty()).then_some(scan));
            if let Some(scanner) = scanner_sig.get() {
                pump(scanner.next_frame(), scanner_sig, scan_sig, last);
            }
        }
        Err(ScanError::Stopped) => scan_sig.set(None),
        Err(e) => {
            scan_sig.set(None);
            last.set(format!("Scanner error: {e}"));
        }
    });
}

/// The camera frame and the outline of every code in view, drawn into one
/// canvas: the frame as texture layer 0, filling the canvas (letterboxed), and
/// the outlines stroked after it so they land on top. Everything is placed
/// from `Scene::size()` — the canvas reports its own size, so there is no
/// measuring here.
#[component]
pub fn ScanPreview(stream: ReadSignal<Option<MediaStream>>, scan: ReadSignal<Option<Scan>>) -> Element {
    // The texture's rect is read at composite time; the painter publishes the
    // current canvas size for it.
    let size: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));

    // A live texture needs the canvas to repaint every frame while the camera
    // is on (a canvas otherwise repaints only when its draw dependencies
    // change). `tick` is read by the painter; the loop runs only while there
    // is a stream, and stops when this component unmounts (the effect, and
    // the `RafLoop` it holds, are owned by its scope).
    let tick: Signal<u64> = signal(0);
    let raf: Rc<RefCell<Option<runtime_core::scheduling::RafLoop>>> = Rc::new(RefCell::new(None));
    effect!({
        if stream.get().is_some() {
            if raf.borrow().is_none() {
                *raf.borrow_mut() = Some(runtime_core::scheduling::raf_loop(move || {
                    tick.set(tick.get().wrapping_add(1));
                }));
            }
        } else {
            *raf.borrow_mut() = None;
        }
    });

    // Layer 0: the camera, letterboxed (Contain) into the whole canvas.
    let camera = TextureLayer::new(Rc::new(move || stream.get()), {
        let size = size.clone();
        Rc::new(move || {
            let (w, h) = size.get();
            (0.0, 0.0, w, h)
        })
    })
    .fit(Fit::Contain);
    let mapping = camera.clone();

    let preview_rules = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::Px(PREVIEW_HEIGHT).into()),
        ..Default::default()
    };
    Canvas(CanvasProps {
        draw: canvas::draw(move |s| {
            let _ = tick.get();
            size.set(s.size());
            s.texture(0);
            let Some(scan) = scan.get() else { return };
            // Frame pixels → canvas units, exactly as layer 0 is drawn.
            let m = mapping.source_to_canvas(scan.width as f32, scan.height as f32);
            for code in &scan.codes {
                let [a, rest @ ..] = code.corners.map(|p| m.apply(p.x, p.y));
                let mut outline = Path::new().move_to(a.0, a.1);
                for (x, y) in rest {
                    outline = outline.line_to(x, y);
                }
                s.stroke_path(outline.close(), OUTLINE, Stroke::width(OUTLINE_WIDTH));
            }
        }),
        layers: vec![camera],
        ..Default::default()
    })
    .with_style(Rc::new(StyleSheet::r#static(preview_rules)))
    .into_element()
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // The stream keeps capture alive; the scanner taps it. Both compare by
    // pointer identity, so `Option<_>` of each is a legal signal payload.
    let stream_sig: Signal<Option<MediaStream>> = signal(None);
    let scanner_sig: Signal<Option<QrScanner>> = signal(None);
    let scan_sig: Signal<Option<Scan>> = signal(None);
    let status: Signal<String> = signal("Idle — press Start scanning".to_string());
    let last: Signal<String> = signal("—".to_string());
    let to_encode: Signal<String> = signal("https://idealyst.io".to_string());

    let on_start = move || {
        if scanner_sig.get().is_some() {
            return;
        }
        status.set("Requesting camera…".to_string());
        spawn_then(
            async move { Camera::new().open(CameraConfig::default().back()).await },
            move |opened| match opened {
                Ok(stream) => {
                    status.set("Scanning — point the camera at a QR code".to_string());
                    start_scan(stream, stream_sig, scanner_sig, scan_sig, last);
                }
                Err(e) => status.set(match e {
                    CameraError::PermissionDenied => "Camera permission denied".to_string(),
                    CameraError::NoCamera => "No camera found".to_string(),
                    other => format!("Error: {other}"),
                }),
            },
        );
    };

    let on_stop = move || {
        if scanner_sig.get().is_some() {
            scanner_sig.set(None);
            stream_sig.set(None);
            scan_sig.set(None);
            status.set("Stopped — camera released".to_string());
        }
    };

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "QR".to_string(), kind = idea_ui::typography_kind::H1)
            Typography(content = "Generate".to_string(), kind = idea_ui::typography_kind::H2)
            text_input(value = to_encode, on_change = move |t| to_encode.set(t), placeholder = "Text to encode")
            QrCode(data = to_encode, size = Some(GENERATED_SIZE))
            Typography(content = "Scan".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(
                content = "One camera stream: a canvas shows it, the scanner reads it, \
                    and the code's outline is drawn over the frame in the same canvas."
                    .to_string(),
                muted = true,
            )
            text { move || status.get() }
            ScanPreview(stream = stream_sig.read_only(), scan = scan_sig.read_only())
            text { move || format!("Last code: {}", last.get()) }
            button(label = "Start scanning".to_string(), on_click = on_start)
            button(label = "Stop".to_string(), on_click = on_stop)
        }
    }
}
