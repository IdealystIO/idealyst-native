//! `canvas-core` — the renderer-agnostic 2D drawing abstraction for the
//! idealyst framework.
//!
//! This crate owns the **abstraction**: the retained [`Scene`] model
//! (paths, paint, strokes, transforms) and the [`Canvas`] primitive that
//! carries an author's draw closure into an [`Element::External`]
//! payload. It contains **no rendering code**. Two interchangeable
//! renderer crates register a handler for [`CanvasProps`]:
//!
//! - `canvas-native` — replays the scene with each platform's native 2D
//!   engine (web Canvas2D, iOS CoreGraphics, Android `android.graphics`).
//! - `canvas-vello` — renders the scene on a GPU surface via `vello`,
//!   for backends with no native 2D API (winit/wgpu desktop, etc.).
//!
//! An app picks a renderer at bootstrap by calling exactly one
//! `register(&mut backend)` (the registry is `TypeId`-keyed, last-wins).
//! Because both renderers consume the identical [`Scene`], swapping the
//! `register` call swaps renderers with zero changes to screen code —
//! which also makes benchmarking native-vs-vello apples-to-apples.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap — one renderer:
//! canvas_native::register(&mut backend);   // or canvas_vello::register
//!
//! // On a screen — the "type in tag" SDK convention, small namespace:
//! use canvas::prelude::*;
//! ui! {
//!     View {
//!         { canvas::Canvas(CanvasProps {
//!             draw: canvas::draw(move |s: &mut Scene| {
//!                 s.path()
//!                  .move_to(10.0, 10.0)
//!                  .line_to(120.0, 10.0)
//!                  .cubic_to(140.0, 40.0, 90.0, 80.0, 10.0, 60.0)
//!                  .close();
//!                 s.fill(Paint::solid(Color::new(40, 120, 255, 255)));
//!                 s.stroke(Color::new(20, 20, 20, 255), Stroke::width(2.0));
//!             }),
//!             ..Default::default()
//!         }) }
//!     }
//! }
//! ```
//!
//! The `draw` closure runs inside the active backend handler's reactive
//! `Effect`, so any `Signal` read inside it re-renders the canvas on
//! change — the same reactive-source convention as `video`/`svg`.
#![deny(missing_docs)]

mod scene;
pub use scene::*;

use runtime_core::{external, Bound, ExternalHandle, IdealystSchema};
use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

/// Author-supplied props for a [`Canvas`] instance. Type-erased into an
/// [`Element::External`](runtime_core::Element) payload at build time;
/// the active renderer's registered handler reads the typed
/// `Rc<CanvasProps>` back out and replays the [`Scene`] the `draw`
/// closure produces.
#[derive(IdealystSchema)]
pub struct CanvasProps {
    /// The scene painter. Called by the renderer (inside a reactive
    /// `Effect`) with a fresh, empty [`Scene`] to populate. Build it
    /// with [`draw`] from a closure — `&str`-style coercion isn't
    /// applicable here, the value is always a `Fn(&mut Scene)`.
    ///
    /// Reactive: signals read inside the closure re-run it and
    /// re-render the canvas. `Fn` (not `FnMut`) because the renderer
    /// may invoke it on every frame / dependency change.
    #[schema(constraint = "a Fn(&mut Scene) painter — build with canvas::draw(...)")]
    pub draw: DrawFn,

    /// Optional self-capture sink. When set, a GPU renderer publishes each
    /// rendered frame into this `FrameWriter` (the producer half of a
    /// [`media_stream::MediaStream`] the app holds), so the canvas's OWN output
    /// can be recorded: `let (stream, writer) = MediaStream::new();
    /// Canvas { capture: Some(writer), .. }`, then record `stream`. The renderer
    /// only does the (GPU→CPU) read-back while a consumer is actually tapping
    /// frames (`writer.wants_cpu_frames()`), so an idle canvas pays nothing.
    /// `None` = no capture (the default). Renderer support is GPU-only
    /// (canvas-vello) for now; the CPU renderers ignore it.
    #[schema(constraint = "optional media_stream::FrameWriter to record the canvas output")]
    pub capture: Option<media_stream::FrameWriter>,

    /// Texture layers composited ON TOP of the painted scene, in order — each a
    /// live `MediaStream` (a camera, screen share, …) drawn as a positioned,
    /// rounded, opacity-blended rectangle. They become part of the rendered
    /// output, so both the on-screen canvas AND the self-capture recording show
    /// them (WYSIWYG). A GPU renderer imports each stream's native surface (an
    /// IOSurface on macOS) and composites it every frame — zero CPU copy. Empty
    /// by default. GPU-only (canvas-vello); the CPU renderers ignore it (an app
    /// overlays a `video` widget there instead).
    #[schema(constraint = "texture layers (e.g. a camera) composited over the scene")]
    pub layers: Vec<TextureLayer>,
}

/// How a [`TextureLayer`]'s source maps into its destination rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Fit {
    /// Stretch to fill the rect exactly (may distort).
    Fill,
    /// Scale to fill the rect preserving aspect; crop the overflow (centered).
    #[default]
    Cover,
    /// Scale to fit inside the rect preserving aspect; letterbox the remainder.
    Contain,
}

/// A `MediaStream` composited into the canvas at a reactive rectangle, with a
/// fit mode, rounded corners, and opacity. See [`CanvasProps::layers`].
#[derive(Clone)]
pub struct TextureLayer {
    /// The stream to draw, resolved every frame so a source that opens/closes
    /// (or swaps) after the canvas is built is picked up reactively — read a
    /// `Signal<Option<MediaStream>>` here. `None` → nothing drawn this frame. On
    /// macOS the stream's `native_source` (an IOSurface) is imported as a GPU
    /// texture; no CPU frame is touched.
    pub source: Rc<dyn Fn() -> Option<media_stream::MediaStream>>,
    /// The destination rectangle `(x, y, w, h)` in the canvas's LOGICAL
    /// coordinate space (the same points the author's `Scene` uses). Read every
    /// frame, so a reactive drag position follows live. The renderer scales it
    /// by the device pixel ratio to hit the physical-pixel target.
    pub rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
    /// How the source maps into [`rect`](Self::rect).
    pub fit: Fit,
    /// Corner radius in LOGICAL points (0 = square). Scaled to physical pixels.
    pub corner_radius: f32,
    /// Layer opacity `0.0..=1.0` (1 = opaque).
    pub opacity: f32,
}

impl TextureLayer {
    /// A full-opacity, square, cover-fit layer from a reactive source + rect.
    pub fn new(
        source: Rc<dyn Fn() -> Option<media_stream::MediaStream>>,
        rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
    ) -> Self {
        Self { source, rect, fit: Fit::Cover, corner_radius: 0.0, opacity: 1.0 }
    }

    /// Set the corner radius (logical points).
    pub fn corner_radius(mut self, r: f32) -> Self {
        self.corner_radius = r;
        self
    }

    /// Set the fit mode.
    pub fn fit(mut self, fit: Fit) -> Self {
        self.fit = fit;
        self
    }

    /// Set the opacity (`0.0..=1.0`).
    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = o;
        self
    }
}

/// The boxed scene-painter closure [`CanvasProps::draw`] holds. Build
/// one with [`draw`].
pub type DrawFn = Box<dyn Fn(&mut Scene)>;

impl Default for CanvasProps {
    fn default() -> Self {
        // A no-op painter renders an empty canvas rather than panicking
        // on an unset field.
        Self { draw: Box::new(|_| {}), capture: None, camera: None }
    }
}

/// Coerce a `Fn(&mut Scene)` closure into the [`DrawFn`] boxed shape
/// [`CanvasProps::draw`] expects. Mirrors `video::url` / `svg::markup`
/// — the small adapter that keeps call sites from writing `Box::new`.
///
/// ```ignore
/// CanvasProps { draw: canvas::draw(|s| { s.path()...; s.fill(paint); }), ..Default::default() }
/// ```
pub fn draw<F: Fn(&mut Scene) + 'static>(f: F) -> DrawFn {
    Box::new(f)
}

/// Render the props' painter into a fresh [`Scene`] snapshot. Renderer
/// handlers call this (inside their reactive effect) to obtain the
/// scene to replay; the wire serializer calls it to capture a static
/// snapshot for transport.
pub fn paint_scene(props: &CanvasProps) -> Scene {
    let mut scene = Scene::new();
    (props.draw)(&mut scene);
    scene
}

/// Construct a `Canvas` primitive. Returns a typed
/// `Bound<ExternalHandle<CanvasProps>>` so `.bind(...)` is type-checked
/// against a call-site `Ref<ExternalHandle<CanvasProps>>`.
///
/// PascalCase intentionally — matches the visual cadence of first-party
/// primitives inside a `ui!` block. Third-party primitives are
/// expression-interpolated (`{ canvas::Canvas(..) }`); the macro only
/// knows the closed first-party set.
///
/// Registers the wire serde for [`CanvasProps`] on first construction
/// (idempotent) so a canvas can render across the runtime-server wire.
#[allow(non_snake_case)]
pub fn Canvas(props: CanvasProps) -> Bound<ExternalHandle<CanvasProps>> {
    ensure_wire_serde();
    external(props)
}

/// Register the wire (serialize, deserialize) pair for [`CanvasProps`]
/// so an `Element::External<CanvasProps>` can cross the runtime-server
/// wire. A draw closure can't be serialized, so we ship a **`Scene`
/// snapshot**: the serializer runs the painter once and encodes the
/// resulting ops; the deserializer rebuilds a `CanvasProps` whose
/// painter replays that snapshot. Server-side reactivity still works —
/// a dependency change rebuilds the element tree, which re-serializes a
/// fresh snapshot.
///
/// Idempotent (guarded by a thread-local flag) so the per-construction
/// call in [`Canvas`] only registers once.
pub fn ensure_wire_serde() {
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    runtime_core::register_external_serde(
        std::any::type_name::<CanvasProps>(),
        |any: &dyn Any| {
            let props = any.downcast_ref::<CanvasProps>()?;
            let scene = paint_scene(props);
            serde_json::to_vec(&scene).ok()
        },
        |bytes: &[u8]| {
            let scene: Scene = serde_json::from_slice(bytes).ok()?;
            // Replay the decoded snapshot verbatim into the renderer's
            // scene. `Rc` so the closure is `Fn` (clonable into effects).
            let scene = Rc::new(scene);
            let draw: DrawFn = Box::new(move |s: &mut Scene| *s = (*scene).clone());
            // `capture` is a runtime-only sink (a live `FrameWriter`); it never
            // crosses the wire, so a wire-adopted canvas has no self-capture.
            Some(Rc::new(CanvasProps { draw, capture: None, camera: None }) as Rc<dyn Any>)
        },
    );
}

/// One-stop import for typical screen code: brings in the [`Canvas`]
/// constructor, [`CanvasProps`], the [`draw`] coercion helper, and the
/// scene-model types ([`Scene`], [`Path`], [`Paint`], [`Stroke`],
/// [`Color`], …).
pub mod prelude {
    pub use super::{draw, Canvas, CameraLayer, CanvasProps};
    pub use crate::scene::{
        color, Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PaintKind,
        Path, PathSeg, RadialGradient, Scene, Stroke, Transform,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_props_paint_to_empty_scene() {
        let props = CanvasProps::default();
        assert!(paint_scene(&props).is_empty());
    }

    #[test]
    fn draw_helper_produces_scene_ops() {
        let props = CanvasProps {
            draw: draw(|s| {
                s.path().add_path(Path::rect(0.0, 0.0, 10.0, 10.0));
                s.fill(Color::new(255, 0, 0, 255));
            }),
            ..Default::default()
        };
        assert_eq!(paint_scene(&props).ops().len(), 1);
    }

    #[test]
    fn wire_serde_round_trips_a_painted_scene() {
        ensure_wire_serde();
        let props = CanvasProps {
            draw: draw(|s| {
                s.path().add_path(Path::circle(20.0, 20.0, 15.0));
                s.fill(Paint::solid(Color::new(10, 20, 30, 255)));
                s.stroke(Color::new(0, 0, 0, 255), Stroke::width(3.0));
            }),
            ..Default::default()
        };

        let type_name = std::any::type_name::<CanvasProps>();
        let bytes = runtime_core::serialize_external_payload(type_name, &props as &dyn Any)
            .expect("serialize");
        let decoded =
            runtime_core::deserialize_external_payload(type_name, &bytes).expect("deserialize");
        let decoded = decoded.downcast_ref::<CanvasProps>().expect("downcast");

        // The decoded painter replays the snapshot — same ops as the
        // original painter produced.
        assert_eq!(paint_scene(&props).ops(), paint_scene(decoded).ops());
    }
}
