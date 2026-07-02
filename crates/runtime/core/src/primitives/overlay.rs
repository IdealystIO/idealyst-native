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
use crate::style::{PointerEvents, StyleRules, StyleSheet};
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
        click_through: false,
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
    click_through: bool,
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

    /// Make the overlay's own layer transparent to pointer events, so
    /// clicks in the empty area pass through to the page beneath. Only
    /// interactive descendants that opt back in (via
    /// `pointer_events: PointerEvents::Auto` in their style) receive
    /// clicks.
    ///
    /// This is the non-modal case: an overlay that fills a region of
    /// the viewport (e.g. a toast host pinned to a full-width strip) but
    /// must not swallow clicks where it renders nothing. It is orthogonal
    /// to `backdrop(BackdropMode::None)` — that controls whether a *scrim
    /// child* is rendered, whereas `click_through` controls whether the
    /// overlay's *own root* hit-tests. A click-through overlay always
    /// wants `BackdropMode::None` (a scrim would defeat the purpose).
    ///
    /// Web-only in effect: it lowers to `pointer-events: none` on the
    /// portal root. Native backends don't intercept clicks the way a
    /// stacked DOM does, so it's a no-op there (see [`PointerEvents`]).
    pub fn click_through(mut self, t: bool) -> Self {
        self.click_through = t;
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
            b.click_through,
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

// NOTE: `anchored_overlay` has no `click_through` — popovers, tooltips,
// dropdowns and context menus are content-sized (not full-strip), so their
// root only covers what they render; there's no empty band to pass through.
// The shared lowering always receives `click_through: false` for it below.

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
            false,
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

#[allow(clippy::too_many_arguments)]
fn build_overlay_portal(
    target: PortalTarget,
    children: Vec<Element>,
    backdrop: BackdropMode,
    backdrop_style: Option<StyleSource>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    content_style: Option<StyleSource>,
    ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
    click_through: bool,
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

    // A click-through overlay marks its portal root
    // `pointer-events: none` so the empty area passes clicks to the page;
    // interactive descendants (e.g. toast cards) re-enable hit-testing
    // with `pointer_events: Auto`. Modal/anchored overlays leave `style`
    // as `None` (portal root defaults to `pointer-events: auto`).
    let style = if click_through {
        Some(click_through_portal_style())
    } else {
        None
    };

    // Build the portal. `ref_fill` goes straight onto the Element
    // since `RefFill::Portal` is the right variant for the composed
    // primitive's handle.
    Element::Portal {
        children: portal_children,
        target,
        on_dismiss,
        trap_focus,
        style,
        ref_fill: ref_fill.map(|f| crate::handles::RefFill::Portal(f)),
        accessibility: crate::accessibility::AccessibilityProps::default(),
    }
}

/// The portal-root style for a click-through overlay: `pointer-events:
/// none` and nothing else, so it layers on top of the backend's inline
/// positioning without disturbing it.
fn click_through_portal_style() -> StyleSource {
    let sheet: Rc<StyleSheet> = Rc::new(StyleSheet::r#static(StyleRules {
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    }));
    sheet.into_style_source()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::resolve as resolve_style;

    /// Resolve a portal's `style` field to its `pointer_events` value, if any.
    fn portal_pointer_events(el: &Element) -> Option<Option<PointerEvents>> {
        match el {
            Element::Portal { style, .. } => match style {
                Some(StyleSource::Static(app)) => Some(resolve_style(app).pointer_events),
                Some(_) => None,
                None => Some(None),
            },
            _ => None,
        }
    }

    // Regression (empty ToastHost swallowed clicks): a `click_through` overlay
    // must lower to a portal root marked `pointer-events: none`, so the strip
    // it fills doesn't hit-test where it renders nothing. Without the flag the
    // portal carries no such style and defaults to interactive (modals/drawers).
    #[test]
    fn click_through_marks_portal_root_pointer_events_none() {
        let el: Element = overlay(vec![])
            .backdrop(BackdropMode::None)
            .click_through(true)
            .into();
        assert_eq!(
            portal_pointer_events(&el),
            Some(Some(PointerEvents::None)),
            "click-through overlay must mark its portal root pointer-events:none",
        );
    }

    // The default (modal) overlay leaves the portal style unset — its root
    // stays interactive so backdrop taps and content clicks land. Anchored
    // overlays (popovers/tooltips/menus) always lower with `false` too, so
    // this "no style unless asked" invariant is the whole click-through gate.
    #[test]
    fn default_overlay_leaves_portal_interactive() {
        let el: Element = overlay(vec![]).into();
        assert_eq!(
            portal_pointer_events(&el),
            Some(None),
            "a modal overlay must not mark its portal click-through",
        );
    }
}
