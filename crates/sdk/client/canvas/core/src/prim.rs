//! The `Canvas` primitive: the author-facing constructor, the scene
//! payload renderer crates dispatch on, and the renderer-agnostic
//! SSR/hydration host.
//!
//! Everything renderer-agnostic (the [`Scene`](crate::Scene) model,
//! [`CanvasProps`], texture layers) lives in the crate root; this module
//! owns the runtime-facing surface:
//!
//! - [`Canvas`] — `canvas::Canvas(CanvasProps { .. }).with_style(…)` then
//!   element coercion, lowering to a scene item carrying [`CanvasPrim`].
//!   The unstyled default is the shared fill-parent sheet
//!   (`default_fill_style`).
//! - [`CanvasPrim`] — the registry payload. Renderer crates register a
//!   handler for it (`registry.register::<CanvasPrim, _>(…)`) — the
//!   runtime's unified primitive==external contract. The prim exposes the
//!   shared [`CanvasProps`] plus a single-take author-style slot
//!   ([`CanvasPrim::take_style`]) so the renderer's mount handler can
//!   attach it through `runtime_vocabulary::style_attach::attach_style`.
//! - [`register_ssr`] — the renderer-agnostic SSR/hydration host:
//!   emits a bare `<canvas>` + author style so pre-rendered pages ship
//!   the real element.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::scheduling::{after_ms, ScheduledTask};
use runtime_core::{signal, ScopeAlive, Signal};

use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

use crate::{default_fill_style, paint_scene_sized, CanvasProps, Scene};

/// Scene payload for a `Canvas` item. Registry key type — renderer
/// crates dispatch on it. The style slot is single-take (the vocabulary
/// `PrimCell` discipline, inlined): the scene hands handlers a shared
/// `&Rc<Self>`, but `StyleProp` must move at mount.
pub struct CanvasPrim {
    /// The author's shared, renderer-agnostic props (painter closure,
    /// capture sink, texture layers).
    pub props: Rc<CanvasProps>,
    style: RefCell<Option<StyleProp>>,
    sizing: SizeReporter,
}

impl CanvasPrim {
    /// Run the author's painter for this canvas's current size and normalize
    /// the result ([`paint_scene_sized`]). Renderers call this inside their
    /// repaint effect instead of [`paint_scene`](crate::paint_scene): it reads
    /// the size the canvas last reported, so a resize re-runs the painter.
    pub fn paint(&self) -> Scene {
        let size = self.sizing.inner.size.map(|s| s.get()).unwrap_or_default();
        paint_scene_sized(&self.props, size)
    }

    /// The handle a renderer reports this canvas's laid-out size through.
    /// Clone it into whatever callback learns the size (a resize observer, a
    /// layout pass, a surface resize).
    pub fn size_reporter(&self) -> SizeReporter {
        self.sizing.clone()
    }

    /// Take the author style out of the prim (once, at mount). The
    /// renderer's handler attaches it to the node it returns via
    /// `attach_style`.
    pub fn take_style(&self) -> Option<StyleProp> {
        self.style.borrow_mut().take()
    }
}

/// Author-side builder returned by [`Canvas`]: `.with_style(…)` then
/// element coercion. No consumer binds a canvas handle, so there is no
/// `.bind`.
pub struct CanvasBound {
    props: Rc<CanvasProps>,
    style: Option<StyleProp>,
}

/// Construct a `Canvas` primitive.
///
/// PascalCase intentionally — matches the visual cadence of first-party
/// primitives inside a `ui!` block. Third-party primitives are
/// expression-interpolated (`{ canvas::Canvas(..) }`); the macro only
/// knows the closed first-party set.
///
/// **Default sizing.** An unstyled canvas carries the shared fill-parent
/// sheet (`flex_grow: 1` + `100% × 100%`) so a bare `Canvas(...)` is
/// visible at all; `.with_style(…)` REPLACES it, so a canvas that wants
/// a fixed size just sets its own sheet.
///
/// **Caveat (inherent to flexbox, not a canvas quirk):** `100%` height
/// only resolves against a parent with a *definite* height. A canvas
/// nested under auto-height flex parents needs either a sized ancestor
/// or `flex_grow` on the chain — the same rule every percentage-sized
/// box follows.
#[allow(non_snake_case)]
pub fn Canvas(props: CanvasProps) -> CanvasBound {
    CanvasBound {
        props: Rc::new(props),
        style: None,
    }
}

impl CanvasBound {
    /// Attach the author style — REPLACES the fill default.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

impl IntoElement for CanvasBound {
    fn into_element(self) -> Element {
        let style = self
            .style
            .unwrap_or_else(|| default_fill_style().into_style_prop());
        item(
            CanvasPrim {
                props: self.props,
                style: RefCell::new(Some(style)),
                sizing: SizeReporter::new(),
            },
            Vec::new(),
        )
    }
}

/// A renderer's channel for telling the canvas its laid-out logical size,
/// which the author's painter reads as [`Scene::size`].
///
/// Every renderer already learns its size to size its drawing surface; it
/// calls [`report`](Self::report) from that same place. The rest is here, once,
/// so it behaves identically everywhere:
///
/// - **Deduped.** Reporting the size already reported does nothing, so a
///   renderer may report on every layout pass.
/// - **Committed via the scheduler.** The size is written from a 0 ms
///   scheduler timer rather than directly. Resize notifications arrive from
///   platform callbacks outside the framework's dispatch, where a staged
///   write would sit uncommitted; every backend flushes after a scheduler
///   callback (its post-dispatch hook), so this is the one host-agnostic way
///   to get the write committed. It also keeps a renderer that reports from
///   inside its repaint effect from writing a signal mid-effect. A burst of
///   reports in one tick coalesces into the last one.
/// - **Scoped.** The size signal belongs to the scope the canvas was built in;
///   the deferred write is skipped once that scope is torn down, and a pending
///   write is cancelled when the last reporter clone drops.
///
/// A canvas built outside a world (a test, a wire snapshot) has no size
/// signal; reporting is then a no-op and [`Scene::size`] stays `(0, 0)`.
#[derive(Clone)]
pub struct SizeReporter {
    inner: Rc<SizeState>,
}

struct SizeState {
    size: Option<Signal<(f32, f32)>>,
    alive: Option<ScopeAlive>,
    last: Cell<(f32, f32)>,
    pending: RefCell<Option<ScheduledTask>>,
}

/// Size changes smaller than this (in logical units) are layout noise, not a
/// resize worth re-painting for.
const SIZE_EPSILON: f32 = 0.01;

impl SizeReporter {
    fn new() -> Self {
        let in_world = runtime_world::is_entered();
        SizeReporter {
            inner: Rc::new(SizeState {
                size: in_world.then(|| signal((0.0f32, 0.0f32))),
                alive: in_world.then(ScopeAlive::current),
                last: Cell::new((0.0, 0.0)),
                pending: RefCell::new(None),
            }),
        }
    }

    /// Report the canvas's laid-out logical size (the same units its scene
    /// draws in — CSS px / points / dp, not device pixels).
    pub fn report(&self, width: f32, height: f32) {
        let (Some(size), Some(alive)) = (self.inner.size, self.inner.alive.clone()) else {
            return;
        };
        let next = (width.max(0.0), height.max(0.0));
        let last = self.inner.last.get();
        if (last.0 - next.0).abs() < SIZE_EPSILON && (last.1 - next.1).abs() < SIZE_EPSILON {
            return;
        }
        self.inner.last.set(next);
        let task = after_ms(0, move || {
            if alive.get() {
                size.set(next);
            }
        });
        // Replacing the handle cancels a still-pending earlier report.
        *self.inner.pending.borrow_mut() = Some(task);
    }
}

/// Element coercion for the constructor form.
impl From<CanvasBound> for Element {
    fn from(b: CanvasBound) -> Element {
        b.into_element()
    }
}

// NOTE: canvas deliberately ships NO `defer` seam, unlike the
// single-crate SDKs (`table`, `markdown`, `svg`, …).
//
// Those crates own both their payload and their handler, so their
// `defer` can register eagerly off-web — the arm that keeps a native
// caller from stranding the payload behind a placeholder forever. Canvas
// can't: the payload lives here and the handlers live in `canvas-native`
// / `canvas-vello`, so this crate has nothing to fall back to, and BOTH
// renderer chunk seams are web-only (`canvas_native::register_from_chunk`
// is `cfg(target_arch = "wasm32")`; `canvas_vello`'s lives in
// `render_web`). A `defer` here would therefore be safe on web and a
// silent blank canvas on native.
//
// [`CanvasPrim`] is public, so an app that wants the deferred path spells
// it directly — `registry.defer::<canvas_core::CanvasPrim>()` — which is
// what the `lazy-loading` guide documents.

/// Register the renderer-agnostic **SSR / hydration host** handler for
/// [`CanvasPrim`]: emits a bare `<canvas>` (the real element a hydrating
/// client adopts) plus the author style; the platform renderer attaches
/// the drawing surface client-side.
///
/// A GPU canvas can't paint its CONTENT on the server (no adapter), but
/// its host `<canvas>` element is trivially server-renderable — without
/// this the payload has no handler and realize panics. Pass from an app's
/// SSR register seam
/// (`backend_ssr::newcore::render_path_with` / the
/// `register_ssr_scene_handlers` convention).
pub fn register_ssr<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<CanvasPrim, _>(|cx: &mut MountCx<'_, H>, prim, _children| {
        let backend = cx.backend().clone();
        let node = backend.borrow_mut().create_element("canvas");
        if let Some(style) = prim.take_style() {
            attach_style(&backend, &node, style);
        }
        // Every external mount installs a cleanup guard calling
        // `release_external` at scope teardown.
        let backend_for_drop = backend.clone();
        let node_for_drop = node.clone();
        on_teardown(move || {
            backend_for_drop.borrow_mut().release_external(&node_for_drop);
        });
        node
    });
}
