# `qr`

QR codes on every target: **show** them, **export** them as SVG, and **scan**
them out of a live camera (or any other) stream.

| Feature | What it adds | Pulls in |
| --- | --- | --- |
| `component` | The `QrCode` component and `QrMatrix::draw` | `canvas` |
| `generate` | `QrMatrix::encode`, `QrMatrix::to_svg` — pure Rust, no UI, works server-side | `qrcode` |
| `scan` | `QrScanner` over a `MediaStream`, `decode_rgba8` / `decode_luma8` for stills | `rqrr`, `offload`, `media-stream` |

All three are on by default. An app that only shows codes can use
`default-features = false, features = ["component"]` and skip the decoder.

## Showing a code

```rust
use qr::{ErrorCorrection, QrCode};

ui! {
    QrCode(data = "https://example.com".to_string())        // fills the width, square
    QrCode(data = invite_url, size = Some(160.0))           // fixed 160×160
    QrCode(data = label, error_correction = ErrorCorrection::High, dark = brand, light = paper)
}
```

`QrCode` draws the code into a [`canvas`](../canvas) as vector rectangles, so it
is crisp at any size and looks the same on every canvas renderer (web
Canvas2D, CoreGraphics on iOS/macOS, `android.graphics`, Cairo on Linux, and
vello on the GPU).

- **Size.** Without `size` it is a square as wide as its container
  (`width: 100%`, `aspect-ratio: 1`). The canvas reports its size to the
  painter (`Scene::size`), and the modules are fitted into it, snapped to whole
  logical pixels so module edges land on pixel boundaries.
- **Reactive.** Every prop can be a signal. A change of `data` or
  `error_correction` re-encodes; a resize or a color change just redraws.
- **Contrast.** Defaults are black on white with the standard 4-module quiet
  zone. Most scanners can't read light-on-dark codes, so keep a light
  background.
- **Too much data** (about 2.3 KB at `Medium`) draws only the background. If
  the data's length isn't under your control, check it first with
  `QrMatrix::encode`, which returns `GenerateError::DataTooLong`.

To draw a code inside a canvas of your own (a printable card, an overlay),
encode it once and call `QrMatrix::draw(scene, (x, y, side), quiet_zone, color)`
from your painter.

## Exporting an SVG

```rust
use qr::{ErrorCorrection, QrMatrix, SvgOptions};

let matrix = QrMatrix::encode("https://example.com", ErrorCorrection::Medium)?;
let svg: String = matrix.to_svg(&SvgOptions::default());
```

A standalone `.svg` document for saving, sharing, printing or `file-export`:
one `<path>` of module runs over a background `<rect>`, with a `viewBox` in
whole modules (quiet zone included) and `shape-rendering="crispEdges"`.
`SvgOptions` sets the quiet zone, the dark and light colors (any SVG color,
`light: None` for transparent) and optional fixed `width`/`height`. This half
has no UI dependency, so it works in server code too.

## Scanning

The scanner **consumes** a stream. It does not open a camera. You open the
camera once with [`camera`](../camera), show it however you like (a `video`, or
a `canvas` texture so you can draw over it), and pass the same stream to
`QrScanner::new`. There is no second capture session and no preview owned by
the scanner. Any producer works: a `screen-recorder` stream scans the same way.

### Usage

```rust
use qr::{QrScanner, ScanConfig};

let scanner = QrScanner::new(&stream, ScanConfig::default());
let scan = scanner.next().await?;          // the next frame with ≥1 code
for code in &scan.codes {
    println!("{:?}", code.text());         // Option<&str>; raw bytes in `code.bytes`
    // code.corners: [top-left, top-right, bottom-right, bottom-left] in the
    // frame's own pixels (scan.width × scan.height) — map onto your preview.
}
```

#### In an idealyst app

Keep the scanner in a signal next to the stream, and drive the loop with
`spawn_then`, so every signal write runs inside a turn:

```rust
fn pump(
    next: impl Future<Output = Result<Scan, ScanError>> + 'static,
    scanner: Signal<Option<QrScanner>>,
    last: Signal<String>,
) {
    spawn_then(next, move |result| {
        if let Ok(scan) = result {
            last.set(scan.codes[0].text().unwrap_or("<binary>").to_string());
            if let Some(s) = scanner.get() {
                pump(s.next(), scanner, last);    // re-arm
            }
        }
        // Err(Stopped): the scanner was dropped — the loop simply ends.
    });
}

// after Camera::open resolves:
let scanner = QrScanner::new(&stream, ScanConfig::default());
let first = scanner.next();          // BEFORE storing it — see below
scanner_sig.set(Some(scanner));
stream_sig.set(Some(stream));
pump(first, scanner_sig, last);

// stop: clearing both signals ends the loop and releases the camera.
scanner_sig.set(None);
stream_sig.set(None);
```

Take the first `next()` from the local scanner. `set` only stages a write
until the driver flushes, so `scanner_sig.get()` in the same turn that set it
still returns `None`, and a loop started that way never runs. The re-arm inside
the callback is fine, because it runs in a later turn. Don't move a scanner
clone into the callback instead: the pending `next()` would then keep the
scanner, and the camera, alive after Stop.

[`examples/qr-demo`](examples/qr-demo) is the runnable version: one
canvas shows the camera with each code's outline drawn over it.

#### Drawing an outline over the camera

Use `next_frame()` instead of `next()` when you track what's in view: it
resolves for **every** decoded frame, with `scan.codes` empty when no code was
found, so an outline can disappear on the first frame the code is gone.

Draw the camera and the outline into one [`canvas`](../canvas). The camera is a
texture layer placed with `s.texture(0)`, and the outline is stroked after it so
it lands on top. `TextureLayer::source_to_canvas` maps the code's corners from
frame pixels to exactly where the layer draws them, whatever the fit:

```rust
// The texture fills the canvas; the painter publishes the canvas size for it.
let size = Rc::new(Cell::new((0.0, 0.0)));
let camera = TextureLayer::new(Rc::new(move || stream.get()), {
    let size = size.clone();
    Rc::new(move || { let (w, h) = size.get(); (0.0, 0.0, w, h) })
})
.fit(Fit::Contain);
let mapping = camera.clone();
Canvas(CanvasProps {
    draw: canvas::draw(move |s| {
        let _ = tick.get();                 // a raf loop keeps the frames live
        size.set(s.size());
        s.texture(0);
        let Some(scan) = scan.get() else { return };
        let m = mapping.source_to_canvas(scan.width as f32, scan.height as f32);
        for code in &scan.codes {
            let [a, rest @ ..] = code.corners.map(|p| m.apply(p.x, p.y));
            let mut outline = Path::new().move_to(a.0, a.1);
            for (x, y) in rest { outline = outline.line_to(x, y); }
            s.stroke_path(outline.close(), GREEN, Stroke::width(4.0));
        }
    }),
    layers: vec![camera],
    ..Default::default()
})
```

The outline trails a moving code by the decode time (a frame or two). The
corners are the code's real corners, so a tilted code gets a tilted outline.
`examples/qr-demo` is the complete version.

#### Still images

`decode_rgba8(width, height, &rgba)` and `decode_luma8(width, height, &grey)`
decode one image on the calling thread. Use them for a photo from
`file-picker`, a screenshot, or a test fixture. A large image is CPU-heavy, so
when it comes from user input on the main thread, wrap the call in your own
`#[offload::job]`.

### How it works

1. While a `next()` is waiting, the stream's CPU tap (`MediaStream::subscribe`)
   converts the **next** frame to greyscale and parks it. If the frame's
   longer edge is above `ScanConfig::max_dimension` (default 960), it is
   box-filtered down first. Frames that arrive while nothing is waiting, or
   while a decode is running, are skipped without being read. The scanner
   always decodes the freshest frame and never builds up a queue.
2. The frame is decoded **off the main thread** through
   [`offload`](../offload), which uses a Web Worker on web and a `std::thread`
   on native. The decoder is [`rqrr`](https://crates.io/crates/rqrr), which is
   pure Rust and the same on every target.
3. A frame with no readable code goes back to step 1. The first frame that
   reads resolves the future with a `Scan`.

Scanning uses CPU only while a `next()` is pending. To scan less often, wait
between calls. The same code is reported again on every call for as long as it
stays in view, so de-duplicate in the caller if you only want changes.

#### Lifetime

`QrScanner` is a cloneable handle, like `MediaStream`, and its clones compare
equal by pointer. It holds a clone of the stream, so capture keeps running
while any scanner clone is alive. Dropping the **last** clone does three
things:

- removes the frame tap,
- releases the stream (if nothing else holds the stream, the camera stops),
- resolves every pending `next()` with `ScanError::Stopped`.

A `next()` future holds only the frame mailbox, never the stream. A future that
was spawned and then forgotten therefore cannot keep the camera on.

#### Why not the platform detectors?

Vision / AVFoundation, ML Kit and the web `BarcodeDetector` were considered and
not used:

- AVFoundation's metadata output only works on a capture session it is attached
  to, so it would need the scanner to own its own camera.
- `BarcodeDetector` is missing from Firefox and desktop Safari.
- Three different engines would disagree on edge cases.

One pure-Rust decoder on a stream the app already has means a code that scans
on one platform scans on all of them.

## Permissions

None. This crate never touches a camera. The producer declares its own
permission (`camera` declares `capabilities = ["camera"]`).

## Tests

- `cargo test -p qr`:
  - generation unit tests: smallest version, too-long data, runs covering
    exactly the dark modules, SVG viewBox/escaping;
  - **round trips** (`tests/roundtrip.rs`): every error-correction level,
    binary and large payloads, and the exported SVG are rasterized and decoded
    by this crate's own scanner. The `QrCode` component is mounted, given a
    size the way a renderer reports it, painted, rasterized on the GPU by
    vello's headless compositor, and decoded. The GPU tests skip without an
    adapter;
  - scanning: the greyscale box filter, still decode, a live `MediaStream` scan
    fed from a producer thread, corners on downscaled frames, `next_frame`, and
    the stop/release lifecycle.
- `cargo test -p qr --target wasm32-unknown-unknown --test web_scanner` (with
  `CHROMEDRIVER` set to a driver that matches your Chrome) runs the live scan in
  a browser, with the decode inside a real Web Worker.
- `examples/qr-demo` (`idealyst dev --web --local`): type text to generate a
  code; start the camera to scan, with the outline drawn over the frame.
