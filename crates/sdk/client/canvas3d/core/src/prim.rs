//! The `Canvas3d` primitive: the author-facing constructor, the scene payload
//! renderer crates register a handler for, the handle authors pick through,
//! and the renderer-agnostic SSR host.

use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::SizeReporter;
use glam::Vec2;
use runtime_core::{signal, ScopeAlive, Signal};
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::{ChildList, IntoElement};
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

use crate::pick::PickHit;
use crate::scene::Scene3d;
use crate::Canvas3dProps;

/// What a renderer draws for one frame: the 3D scene and, optionally, the 2D
/// overlay composited over it in the same frame.
pub struct Frame3d {
    pub scene: Rc<Scene3d>,
    pub overlay: Option<canvas_core::Scene>,
}

/// A handle onto a mounted `Canvas3d`: the scene it last painted, for
/// picking and projecting from input handlers, and which renderer engaged.
/// Create one in the component body, pass a clone in
/// [`Canvas3dProps::handle`], keep the other.
#[derive(Clone)]
pub struct Canvas3dHandle {
    last: Rc<RefCell<Option<Rc<Scene3d>>>>,
    /// Which GPU path the renderer brought up ("WebGPU", "WebGL2", "Metal" …).
    /// `None` outside a world.
    renderer: Option<Signal<Option<String>>>,
    alive: Option<ScopeAlive>,
}

impl Default for Canvas3dHandle {
    fn default() -> Self {
        Canvas3dHandle::new()
    }
}

impl Canvas3dHandle {
    pub fn new() -> Canvas3dHandle {
        let in_world = runtime_world::is_entered();
        Canvas3dHandle {
            last: Rc::default(),
            renderer: in_world.then(|| signal(None)),
            alive: in_world.then(ScopeAlive::current),
        }
    }

    /// The renderer's description of the GPU path it brought up — a reactive
    /// read (`None` until the view's surface is ready). For diagnostics and
    /// HUDs; not a capability check.
    pub fn renderer_info(&self) -> Option<String> {
        self.renderer.and_then(|s| s.get())
    }

    /// Renderer-facing: record which GPU path engaged. Called from surface
    /// callbacks outside the framework's dispatch, so the write is committed
    /// through the scheduler (like `SizeReporter`), and skipped if the handle's
    /// scope is gone.
    pub fn report_renderer(&self, info: impl Into<String>) {
        let (Some(sig), Some(alive)) = (self.renderer, self.alive.clone()) else { return };
        let info = info.into();
        runtime_core::scheduling::after_ms_detached(0, move || {
            if alive.get() {
                sig.set(Some(info));
            }
        });
    }

    /// The scene most recently painted (`None` before the first paint).
    pub fn scene(&self) -> Option<Rc<Scene3d>> {
        self.last.borrow().clone()
    }

    /// The view's logical size as of the last paint (`(0, 0)` before it).
    pub fn size(&self) -> (f32, f32) {
        self.scene().map(|s| s.size()).unwrap_or_default()
    }

    /// The nearest pickable model under `(x, y)` — logical units, top-left
    /// origin of the view. Touch positions from a view wrapping the canvas
    /// edge-to-edge are already in these coordinates.
    pub fn pick(&self, x: f32, y: f32) -> Option<PickHit> {
        self.scene()?.pick(Vec2::new(x, y))
    }

    fn store(&self, scene: Rc<Scene3d>) {
        *self.last.borrow_mut() = Some(scene);
    }
}

/// Scene payload for a `Canvas3d` item — the registry key renderer crates
/// dispatch on.
pub struct Canvas3dPrim {
    pub props: Rc<Canvas3dProps>,
    style: RefCell<Option<StyleProp>>,
    sizing: SizeReporter,
}

impl Canvas3dPrim {
    /// Run the author's painter (and overlay painter) at the size the view
    /// last reported. Renderers call this inside their repaint effect: it reads
    /// the size reactively, so a resize re-paints.
    pub fn paint(&self) -> Frame3d {
        let (w, h) = self.sizing.size();
        let mut scene = Scene3d::with_size(w, h);
        (self.props.draw)(&mut scene);
        let scene = Rc::new(scene);
        if let Some(handle) = &self.props.handle {
            handle.store(scene.clone());
        }
        let overlay = self.props.overlay.as_ref().map(|draw| {
            let mut s = canvas_core::Scene::with_size(w, h);
            draw(&mut s);
            s
        });
        Frame3d { scene, overlay }
    }

    /// The handle a renderer reports the view's laid-out size through.
    pub fn size_reporter(&self) -> SizeReporter {
        self.sizing.clone()
    }

    /// Take the author style (once, at mount).
    pub fn take_style(&self) -> Option<StyleProp> {
        self.style.borrow_mut().take()
    }
}

/// Author-side builder returned by [`Canvas3d`]: `.with_style(…)` then element
/// coercion.
pub struct Canvas3dBound {
    props: Rc<Canvas3dProps>,
    style: Option<StyleProp>,
}

/// Construct a `Canvas3d` view. Like `canvas::Canvas`, an unstyled view fills
/// its parent; `.with_style(…)` replaces that.
#[allow(non_snake_case)]
pub fn Canvas3d(props: Canvas3dProps) -> Canvas3dBound {
    Canvas3dBound { props: Rc::new(props), style: None }
}

impl Canvas3dBound {
    /// Attach the author style — REPLACES the fill default.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

impl IntoElement for Canvas3dBound {
    fn into_element(self) -> Element {
        let style = self.style.unwrap_or_else(|| canvas_core::default_fill_style().into_style_prop());
        item(
            Canvas3dPrim { props: self.props, style: RefCell::new(Some(style)), sizing: SizeReporter::new() },
            Vec::new(),
        )
    }
}

/// So `{ Canvas3d(..) }` splices straight into a `ui!` child list.
impl ChildList for Canvas3dBound {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.into_element());
    }
}

impl From<Canvas3dBound> for Element {
    fn from(b: Canvas3dBound) -> Element {
        b.into_element()
    }
}

/// Register the renderer-agnostic **SSR / hydration host** handler for
/// [`Canvas3dPrim`]: a bare `<canvas>` plus the author style, which the
/// client renderer adopts. Without it an SSR render of a page containing a
/// `Canvas3d` has no handler and realize panics.
pub fn register_ssr<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<Canvas3dPrim, _>(|cx: &mut MountCx<'_, H>, prim, _children| {
        let backend = cx.backend().clone();
        let node = backend.borrow_mut().create_element("canvas");
        if let Some(style) = prim.take_style() {
            attach_style(&backend, &node, style);
        }
        let backend_for_drop = backend.clone();
        let node_for_drop = node.clone();
        on_teardown(move || {
            backend_for_drop.borrow_mut().release_external(&node_for_drop);
        });
        node
    });
}
