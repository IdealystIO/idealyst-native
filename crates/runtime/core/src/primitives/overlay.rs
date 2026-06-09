//! `overlay()` and `anchored_overlay()` — compositions on top of
//! [`primitives::portal`]. These aren't framework primitives; they're
//! builders that lower to `Element::Portal` at conversion time,
//! adding the backdrop wiring around the caller's children.
//!
//! Defaults baked in here are deliberate UX choices for the common
//! cases (centered modal with dismiss-on-tap backdrop; popover with
//! no backdrop). Authors who want non-default behavior either chain
//! the builder methods or reach for [`portal()`](super::portal::portal)
//! directly and assemble their own backdrop + content children.

use crate::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, PortalHandle, PortalTarget, ViewportPlacement,
};
use crate::sources::{IntoStyleSource, StyleSource};
use crate::{ChildList, IntoElement, Element, Ref};
use std::rc::Rc;

// =============================================================================
// Backdrop
// =============================================================================

/// How an overlay's backdrop layer behaves.
///
/// Backdrop dismissal is composition-level here — we render a
/// fullscreen `pressable()` as the first child inside the portal and
/// wire its `on_click` to the user's `on_dismiss` (for `Dismiss`)
/// or leave it as a passive scrim (for `Opaque`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum BackdropMode {
    /// Semi-transparent scrim. Clicks on the scrim fire the
    /// `on_dismiss` callback.
    #[default]
    Dismiss,
    /// Semi-transparent scrim. Clicks on the scrim do NOT dismiss;
    /// the host must drive open/close itself.
    Opaque,
    /// No scrim at all. The viewport behind stays interactive.
    None,
}

// =============================================================================
// overlay() — viewport-anchored composition
// =============================================================================

/// Build a viewport-anchored overlay (modal, drawer, full-screen
/// sheet). Returns a builder; chain `.placement(...)`,
/// `.backdrop(...)`, `.on_dismiss(...)`, `.trap_focus(...)`,
/// `.with_style(...)`, `.backdrop_style(...)`, `.bind(...)` and the
/// composition lowers to a [`Element::Portal`] when consumed by a
/// child list.
///
/// Defaults: `Center` placement, `Dismiss` backdrop, focus-trap on.
pub fn overlay(children: Vec<Element>) -> OverlayBuilder {
    OverlayBuilder {
        children,
        placement: ViewportPlacement::default(),
        backdrop: BackdropMode::default(),
        backdrop_style: None,
        on_dismiss: None,
        trap_focus: true,
        content_style: None,
        ref_fill: None,
    }
}

/// Builder for the viewport-anchored overlay composition. Lowers to
/// [`Element::Portal`] via `From<OverlayBuilder>`.
pub struct OverlayBuilder {
    children: Vec<Element>,
    placement: ViewportPlacement,
    backdrop: BackdropMode,
    backdrop_style: Option<StyleSource>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    content_style: Option<StyleSource>,
    ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
}

impl OverlayBuilder {
    pub fn placement(mut self, p: ViewportPlacement) -> Self {
        self.placement = p;
        self
    }

    pub fn backdrop(mut self, b: BackdropMode) -> Self {
        self.backdrop = b;
        self
    }

    pub fn backdrop_style<S: IntoStyleSource>(mut self, s: S) -> Self {
        self.backdrop_style = Some(s.into_style_source());
        self
    }

    pub fn on_dismiss<F: Fn() + 'static>(mut self, f: F) -> Self {
        // Born batched — see `reactive::cycle`. Covers the backdrop-tap path too
        // (it clones this stored closure into the backdrop's on_click).
        self.on_dismiss = Some(Rc::new(move || crate::cycle(|| f())));
        self
    }

    pub fn trap_focus(mut self, t: bool) -> Self {
        self.trap_focus = t;
        self
    }

    pub fn with_style<S: IntoStyleSource>(mut self, s: S) -> Self {
        self.content_style = Some(s.into_style_source());
        self
    }

    pub fn bind(mut self, r: Ref<PortalHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |h| r.fill(h)));
        self
    }
}

impl From<OverlayBuilder> for Element {
    fn from(b: OverlayBuilder) -> Element {
        build_overlay_portal(
            PortalTarget::Viewport(b.placement),
            b.children,
            b.backdrop,
            b.backdrop_style,
            b.on_dismiss,
            b.trap_focus,
            b.content_style,
            b.ref_fill,
        )
    }
}

impl IntoElement for OverlayBuilder {
    fn into_element(self) -> Element {
        self.into()
    }
}

impl ChildList for OverlayBuilder {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.into());
    }
}

impl ChildList for Option<OverlayBuilder> {
    fn append_to(self, out: &mut Vec<Element>) {
        if let Some(b) = self {
            out.push(b.into());
        }
    }
}

// =============================================================================
// anchored_overlay() — element-anchored composition
// =============================================================================

/// Build an element-anchored overlay (popover, tooltip, dropdown,
/// context menu). Returns a builder; chain `.target(...)`,
/// `.side(...)`, `.align(...)`, `.offset(...)`, `.backdrop(...)`,
/// `.backdrop_style(...)`, `.on_dismiss(...)`, `.trap_focus(...)`,
/// `.with_style(...)`, `.bind(...)`.
///
/// Defaults: side `Below`, align `Start`, offset `0`, backdrop
/// `None` (page behind stays interactive — the typical popover UX),
/// focus-trap off.
pub fn anchored_overlay(
    target: AnchorTarget,
    children: Vec<Element>,
) -> AnchoredOverlayBuilder {
    AnchoredOverlayBuilder {
        children,
        target,
        side: ElementSide::default(),
        align: ElementAlign::default(),
        offset: 0.0,
        backdrop: BackdropMode::None,
        backdrop_style: None,
        on_dismiss: None,
        trap_focus: false,
        content_style: None,
        ref_fill: None,
    }
}

/// Builder for the element-anchored overlay composition. Lowers to
/// [`Element::Portal`] via `From<AnchoredOverlayBuilder>`.
pub struct AnchoredOverlayBuilder {
    children: Vec<Element>,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    backdrop: BackdropMode,
    backdrop_style: Option<StyleSource>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    content_style: Option<StyleSource>,
    ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
}

impl AnchoredOverlayBuilder {
    pub fn target(mut self, t: AnchorTarget) -> Self {
        self.target = t;
        self
    }

    pub fn side(mut self, s: ElementSide) -> Self {
        self.side = s;
        self
    }

    pub fn align(mut self, a: ElementAlign) -> Self {
        self.align = a;
        self
    }

    pub fn offset(mut self, o: f32) -> Self {
        self.offset = o;
        self
    }

    pub fn backdrop(mut self, b: BackdropMode) -> Self {
        self.backdrop = b;
        self
    }

    pub fn backdrop_style<S: IntoStyleSource>(mut self, s: S) -> Self {
        self.backdrop_style = Some(s.into_style_source());
        self
    }

    pub fn on_dismiss<F: Fn() + 'static>(mut self, f: F) -> Self {
        // Born batched — see `reactive::cycle`. Covers the backdrop-tap path too
        // (it clones this stored closure into the backdrop's on_click).
        self.on_dismiss = Some(Rc::new(move || crate::cycle(|| f())));
        self
    }

    pub fn trap_focus(mut self, t: bool) -> Self {
        self.trap_focus = t;
        self
    }

    pub fn with_style<S: IntoStyleSource>(mut self, s: S) -> Self {
        self.content_style = Some(s.into_style_source());
        self
    }

    pub fn bind(mut self, r: Ref<PortalHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |h| r.fill(h)));
        self
    }
}

impl From<AnchoredOverlayBuilder> for Element {
    fn from(b: AnchoredOverlayBuilder) -> Element {
        build_overlay_portal(
            PortalTarget::Anchor {
                target: b.target,
                side: b.side,
                align: b.align,
                offset: b.offset,
            },
            b.children,
            b.backdrop,
            b.backdrop_style,
            b.on_dismiss,
            b.trap_focus,
            b.content_style,
            b.ref_fill,
        )
    }
}

impl IntoElement for AnchoredOverlayBuilder {
    fn into_element(self) -> Element {
        self.into()
    }
}

impl ChildList for AnchoredOverlayBuilder {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.into());
    }
}

impl ChildList for Option<AnchoredOverlayBuilder> {
    fn append_to(self, out: &mut Vec<Element>) {
        if let Some(b) = self {
            out.push(b.into());
        }
    }
}

// =============================================================================
// Lowering — shared between both compositions
// =============================================================================

fn build_overlay_portal(
    target: PortalTarget,
    children: Vec<Element>,
    backdrop: BackdropMode,
    backdrop_style: Option<StyleSource>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    content_style: Option<StyleSource>,
    ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
) -> Element {
    let mut portal_children: Vec<Element> = Vec::with_capacity(2);

    // Backdrop layer (first child = behind content). Skipped when
    // `BackdropMode::None`. `Dismiss` wires the tap to `on_dismiss`;
    // `Opaque` swallows the tap so it doesn't reach content behind.
    if !matches!(backdrop, BackdropMode::None) {
        let dismiss_for_backdrop = match backdrop {
            BackdropMode::Dismiss => on_dismiss.clone(),
            BackdropMode::Opaque => Some(Rc::new(|| {}) as Rc<dyn Fn()>),
            BackdropMode::None => None,
        };
        let on_click: Rc<dyn Fn()> =
            dismiss_for_backdrop.unwrap_or_else(|| Rc::new(|| {}));

        // Construct the backdrop primitive directly so we can install
        // the already-built `Rc<dyn Fn>` and `StyleSource` without
        // going through `pressable()`'s closure-typed builder.
        let backdrop_primitive = Element::Pressable {
            children: Vec::new(),
            on_click,
            style: backdrop_style,
            ref_fill: None,
            disabled: None,
            accessibility: crate::accessibility::AccessibilityProps::default(),
            #[cfg(feature = "robot")]
            test_id: None,
        };
        portal_children.push(backdrop_primitive);
    }

    // Content layer (second child = above backdrop). Wrap the user's
    // children in a view we can style; their actual children stay as
    // siblings under that view.
    let mut content_view = crate::builder::view(children);
    if let Some(cs) = content_style {
        if let Element::View { style, .. } = content_view.primitive_mut() {
            *style = Some(cs);
        }
    }
    portal_children.push(content_view.into());

    // Build the portal. `ref_fill` goes straight onto the Element
    // since `RefFill::Portal` is the right variant for the composed
    // primitive's handle.
    Element::Portal {
        children: portal_children,
        target,
        on_dismiss,
        trap_focus,
        style: None,
        ref_fill: ref_fill.map(|f| crate::handles::RefFill::Portal(f)),
        accessibility: crate::accessibility::AccessibilityProps::default(),
    }
}
