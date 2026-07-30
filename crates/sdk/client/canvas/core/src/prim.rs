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
//! - [`register_ssr_scene`] — the renderer-agnostic SSR/hydration host:
//!   emits a bare `<canvas>` + author style so pre-rendered pages ship
//!   the real element.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

use crate::{default_fill_style, CanvasProps};

/// Scene payload for a `Canvas` item. Registry key type — renderer
/// crates dispatch on it. The style slot is single-take (the vocabulary
/// `PrimCell` discipline, inlined): the scene hands handlers a shared
/// `&Rc<Self>`, but `StyleProp` must move at mount.
pub struct CanvasPrim {
    /// The author's shared, renderer-agnostic props (painter closure,
    /// capture sink, texture layers).
    pub props: Rc<CanvasProps>,
    style: RefCell<Option<StyleProp>>,
}

impl CanvasPrim {
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
            },
            Vec::new(),
        )
    }
}

/// Element coercion for the constructor form.
impl From<CanvasBound> for Element {
    fn from(b: CanvasBound) -> Element {
        b.into_element()
    }
}

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
pub fn register_ssr_scene<H>(registry: &mut Registry<H>)
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
