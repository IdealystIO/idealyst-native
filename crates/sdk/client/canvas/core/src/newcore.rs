//! New-core surface for the canvas abstraction — the scene-registry leg
//! (idea-lite migration, External-SDK wave).
//!
//! Everything renderer-agnostic (the [`Scene`](crate::Scene) model,
//! [`CanvasProps`], texture layers, the wire serde) is core-agnostic and
//! lives in the crate root, shared verbatim by both cores. This module
//! owns only the per-core pieces:
//!
//! - [`Canvas`] — the same author call shape as the old core
//!   (`canvas::Canvas(CanvasProps { .. }).with_style(…)` then element
//!   coercion), lowering to a scene item carrying [`CanvasPrim`]
//!   instead of `Element::External`. The unstyled default is the same
//!   fill-parent sheet (`default_fill_style` — shared).
//! - [`CanvasPrim`] — the registry payload. Renderer crates register a
//!   handler for it (`registry.register::<CanvasPrim, _>(…)`), exactly
//!   like the old per-backend `register_external::<CanvasProps>` — the
//!   new core's unified primitive==external contract. The prim exposes
//!   the shared [`CanvasProps`] plus a single-take author-style slot
//!   ([`CanvasPrim::take_style`]) so the renderer's mount handler can
//!   attach it through `runtime_vocabulary::style_attach::attach_style`.
//! - [`register_ssr_scene`] — the renderer-agnostic SSR/hydration host
//!   (the old `register_ssr` twin): emits a bare `<canvas>` +
//!   author style so pre-rendered pages ship the real element.
//!
//! Renderer coverage on the new core: `canvas-native`'s web (`<canvas>`
//! 2D) renderer is ported; the native CoreGraphics/android painters and
//! the GPU `canvas-vello` renderer remain old-core-only — their ports
//! ride the same seam (register a `CanvasPrim` handler; wrap any author
//! callbacks with the backend's `schedule_flush`, the residual named in
//! each backend's `newcore.rs` module docs).

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
    /// `attach_style` — the new-core home of the old walker's External
    /// style attach.
    pub fn take_style(&self) -> Option<StyleProp> {
        self.style.borrow_mut().take()
    }
}

/// Author-side builder returned by [`Canvas`] — mirrors the old-core
/// `Bound<ExternalHandle<CanvasProps>>` call shape (`.with_style(…)`
/// then element coercion; no consumer binds a canvas handle, so there
/// is no `.bind`).
pub struct CanvasBound {
    props: Rc<CanvasProps>,
    style: Option<StyleProp>,
}

/// Construct a `Canvas` primitive — the new-core stand-in for the
/// old-core `Canvas(props)`, same call shape at every author site.
///
/// **Default sizing** matches the old core: an unstyled canvas carries
/// the shared fill-parent sheet; `.with_style(…)` REPLACES it.
/// Registers the wire serde on first construction (idempotent), same
/// as the old constructor — the dev-wire recorder path serializes the
/// scene snapshot identically on both cores.
#[allow(non_snake_case)]
pub fn Canvas(props: CanvasProps) -> CanvasBound {
    crate::ensure_wire_serde();
    CanvasBound {
        props: Rc::new(props),
        style: None,
    }
}

impl CanvasBound {
    /// Attach the author style — REPLACES the fill default, exactly like
    /// `.with_style` on the old-core `Bound`.
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

/// Mirror of the old core's `From<Bound<H>> for Element`.
impl From<CanvasBound> for Element {
    fn from(b: CanvasBound) -> Element {
        b.into_element()
    }
}

/// Register the renderer-agnostic **SSR / hydration host** handler for
/// [`CanvasPrim`] — the new-core twin of the old `register_ssr`: emits a
/// bare `<canvas>` (the real element a hydrating client adopts) plus the
/// author style; the platform renderer attaches the drawing surface
/// client-side. Pass from an app's SSR register seam
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
        // Old walker parity: every External mount installed a cleanup
        // guard calling `release_external` at scope teardown.
        let backend_for_drop = backend.clone();
        let node_for_drop = node.clone();
        on_teardown(move || {
            backend_for_drop.borrow_mut().release_external(&node_for_drop);
        });
        node
    });
}
