//! Render-elsewhere builders: `portal()`, plus the `overlay()` /
//! `anchored_overlay()` compositions.
//!
//! Ports `runtime_shared::primitives::{portal,overlay}`'s authoring
//! surface: `portal()` is the raw primitive; the two overlay builders
//! are build-time compositions that lower to a portal item with the
//! backdrop / content wiring added around the caller's children — they
//! have no payload or handler of their own, exactly like the old core.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::overlay::BackdropMode;
use runtime_shared::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, PortalHandle, PortalTarget, ViewportPlacement,
};
use runtime_shared::{
    pointer_button, Length, PointerEvents, Position, StyleRules, TouchEvent,
    TouchHandler, TouchPhase, TouchResponse,
};
use runtime_scene::{item, Element};

use crate::prims::{PortalPrim, PressablePrim, PrimCell};
use crate::style_attach::{IntoStyleProp, StyleProp};

use super::SceneChild;

/// Start a raw `portal` mounting its children at `target`. No backdrop
/// defaults — that's a caller concern; the common cases (modal, popover,
/// tooltip) are the [`overlay`] / [`anchored_overlay`] compositions.
pub fn portal(target: PortalTarget) -> PortalBuilder {
    PortalBuilder {
        prim: PortalPrim {
            target,
            on_dismiss: None,
            trap_focus: false,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
        children: Vec::new(),
    }
}

pub struct PortalBuilder {
    prim: PortalPrim,
    children: Vec<Element>,
}

impl PortalBuilder {
    pub fn child(mut self, child: impl SceneChild) -> Self {
        self.children.push(child.into_child());
        self
    }

    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.children.extend(children);
        self
    }

    /// Fires when the platform requests dismissal (Android back, web
    /// Escape, iOS swipe-down). The host flips its open-state signal in
    /// response — the framework doesn't auto-unmount. Backdrop-tap
    /// dismissal is composition-level (a backdrop `pressable` child).
    pub fn on_dismiss(mut self, f: impl Fn() + 'static) -> Self {
        self.prim.on_dismiss = Some(Rc::new(f));
        self
    }

    /// Confine keyboard / accessibility focus to the portal subtree
    /// until it closes. Default `false`.
    pub fn trap_focus(mut self, trap: bool) -> Self {
        self.prim.trap_focus = trap;
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Receive the imperative-ref handle at mount (P2 form of `.bind`).
    pub fn on_handle(mut self, fill: impl FnOnce(PortalHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), self.children)
    }
}

// ===========================================================================
// overlay() — viewport-anchored composition
// ===========================================================================

/// Start a viewport-anchored overlay (modal, drawer, sheet). Defaults
/// mirror the old core: `Center` placement, `Dismiss` backdrop,
/// focus-trap ON.
pub fn overlay() -> OverlayBuilder {
    OverlayBuilder {
        children: Vec::new(),
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

pub struct OverlayBuilder {
    children: Vec<Element>,
    placement: ViewportPlacement,
    backdrop: BackdropMode,
    backdrop_style: Option<StyleProp>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    content_style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
    click_through: bool,
}

impl OverlayBuilder {
    pub fn child(mut self, child: impl SceneChild) -> Self {
        self.children.push(child.into_child());
        self
    }

    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.children.extend(children);
        self
    }

    pub fn placement(mut self, p: ViewportPlacement) -> Self {
        self.placement = p;
        self
    }

    pub fn backdrop(mut self, b: BackdropMode) -> Self {
        self.backdrop = b;
        self
    }

    pub fn backdrop_style(mut self, s: impl IntoStyleProp) -> Self {
        self.backdrop_style = Some(s.into_style_prop());
        self
    }

    pub fn on_dismiss(mut self, f: impl Fn() + 'static) -> Self {
        // Covers the backdrop-tap path too (the lowering clones this
        // into the backdrop's on_press). No `cycle` wrap needed on the
        // new core — writes stage until the driver flush by design.
        self.on_dismiss = Some(Rc::new(f));
        self
    }

    pub fn trap_focus(mut self, t: bool) -> Self {
        self.trap_focus = t;
        self
    }

    /// Make the overlay's own layer transparent to pointer events so
    /// clicks in the empty area pass through to the page beneath (the
    /// ToastHost strip case — see the old core's `click_through` docs).
    /// Orthogonal to `backdrop(None)`: that controls whether a scrim
    /// CHILD renders; this controls whether the portal ROOT hit-tests.
    pub fn click_through(mut self, t: bool) -> Self {
        self.click_through = t;
        self
    }

    /// Style for the content wrapper view (the layer above the
    /// backdrop that hosts the caller's children).
    pub fn style(mut self, s: impl IntoStyleProp) -> Self {
        self.content_style = Some(s.into_style_prop());
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(PortalHandle) + 'static) -> Self {
        self.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        lower_overlay_portal(
            PortalTarget::Viewport(self.placement),
            self.children,
            self.backdrop,
            self.backdrop_style,
            self.on_dismiss,
            self.trap_focus,
            self.content_style,
            self.ref_fill,
            self.click_through,
        )
    }
}

// ===========================================================================
// anchored_overlay() — element-anchored composition
// ===========================================================================

/// Start an element-anchored overlay (popover, tooltip, dropdown,
/// context menu). Defaults mirror the old core: side `Below`, align
/// `Start`, offset `0`, backdrop `None` (page stays interactive),
/// focus-trap OFF.
pub fn anchored_overlay(target: AnchorTarget) -> AnchoredOverlayBuilder {
    AnchoredOverlayBuilder {
        children: Vec::new(),
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

// NOTE (ported): `anchored_overlay` has no `click_through` — popovers /
// tooltips / menus are content-sized, so their root only covers what
// they render; the shared lowering always receives `false` for it.

pub struct AnchoredOverlayBuilder {
    children: Vec<Element>,
    target: AnchorTarget,
    side: ElementSide,
    align: ElementAlign,
    offset: f32,
    backdrop: BackdropMode,
    backdrop_style: Option<StyleProp>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    content_style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
}

impl AnchoredOverlayBuilder {
    pub fn child(mut self, child: impl SceneChild) -> Self {
        self.children.push(child.into_child());
        self
    }

    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.children.extend(children);
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

    pub fn backdrop_style(mut self, s: impl IntoStyleProp) -> Self {
        self.backdrop_style = Some(s.into_style_prop());
        self
    }

    pub fn on_dismiss(mut self, f: impl Fn() + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(f));
        self
    }

    pub fn trap_focus(mut self, t: bool) -> Self {
        self.trap_focus = t;
        self
    }

    /// Style for the content wrapper view.
    pub fn style(mut self, s: impl IntoStyleProp) -> Self {
        self.content_style = Some(s.into_style_prop());
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(PortalHandle) + 'static) -> Self {
        self.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        lower_overlay_portal(
            PortalTarget::Anchor {
                target: self.target,
                side: self.side,
                align: self.align,
                offset: self.offset,
            },
            self.children,
            self.backdrop,
            self.backdrop_style,
            self.on_dismiss,
            self.trap_focus,
            self.content_style,
            self.ref_fill,
            false,
        )
    }
}

// ===========================================================================
// Lowering — shared between both compositions (port of
// `primitives/overlay.rs::build_overlay_portal`, structure preserved:
// backdrop pressable first, content view second).
// ===========================================================================

#[allow(clippy::too_many_arguments)]
fn lower_overlay_portal(
    target: PortalTarget,
    children: Vec<Element>,
    backdrop: BackdropMode,
    backdrop_style: Option<StyleProp>,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
    content_style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(PortalHandle)>>,
    click_through: bool,
) -> Element {
    let mut portal_children: Vec<Element> = Vec::with_capacity(2);

    // Backdrop layer (first child = behind content). Skipped for
    // `BackdropMode::None`. `Dismiss` wires the tap to `on_dismiss`;
    // `Opaque` swallows the tap so it doesn't reach content behind.
    if !matches!(backdrop, BackdropMode::None) {
        let dismiss_for_backdrop = match backdrop {
            BackdropMode::Dismiss => on_dismiss.clone(),
            BackdropMode::Opaque => Some(Rc::new(|| {}) as Rc<dyn Fn()>),
            BackdropMode::None => None,
        };
        // A `Dismiss` backdrop dismisses on a press of ANY button
        // (FRAMEWORK-NOTES #61). The pressable's own activation is the
        // platform "click", which is primary-only on every backend (DOM
        // `click`, AppKit/UIKit tap, Android `OnClickListener`), so a
        // right-click on the backdrop used to fall through: the popover
        // stayed open. The secondary layer carries that half.
        let secondary_layer = match (&backdrop, &on_dismiss) {
            (BackdropMode::Dismiss, Some(d)) => vec![secondary_press_dismiss_layer(d.clone())],
            _ => Vec::new(),
        };
        let on_press: Rc<dyn Fn()> = dismiss_for_backdrop.unwrap_or_else(|| Rc::new(|| {}));
        // PressablePrim constructed directly (not via `pressable()`)
        // so the already-built `Rc<dyn Fn>` and `StyleProp` install
        // without a wrapper closure — same move as the old lowering.
        portal_children.push(item(
            PrimCell::new(PressablePrim {
                test_id: None,
                on_press,
                disabled: None,
                preserves_focus: false,
                style: backdrop_style,
                a11y: AccessibilityProps::default(),
                ref_fill: None,
            }),
            secondary_layer,
        ));
    }

    // Content layer (second child = above backdrop): the caller's
    // children live inside a styleable wrapper view.
    let mut content = super::view().children(children);
    if let Some(cs) = content_style {
        content = content.style(cs);
    }
    portal_children.push(content.build());

    // A click-through overlay marks its portal root
    // `pointer-events: none` (the empty-strip-swallows-clicks fix);
    // interactive descendants opt back in with `PointerEvents::Auto`.
    // Everything else leaves `style` as `None` (portal root defaults
    // interactive). Static resolved rules, not a sheet — pointer-events
    // has no tokens, so there's nothing for the theme engine to do.
    let style = if click_through {
        Some(StyleProp::Static(Rc::new(StyleRules {
            pointer_events: Some(PointerEvents::None),
            ..Default::default()
        })))
    } else {
        None
    };

    item(
        PrimCell::new(PortalPrim {
            target,
            on_dismiss,
            trap_focus,
            style,
            a11y: AccessibilityProps::default(),
            ref_fill,
        }),
        portal_children,
    )
}

/// The touch handler behind a `Dismiss` backdrop's non-primary press:
/// a `Began` from any button other than
/// [`PointerButton::Primary`](runtime_shared::PointerButton::Primary)
/// fires `on_dismiss` and is consumed; everything else is IGNORED.
///
/// Primary is deliberately left alone — it bubbles to the enclosing
/// backdrop `pressable`, whose click/tap semantics (release-inside,
/// keyboard activation, robot `click`) are unchanged. A non-primary
/// press is contractually `Began`-only (see
/// [`PointerButton`](runtime_shared::PointerButton)), so the
/// `Began` IS the whole click. Consuming it is also what suppresses the
/// native context menu on web (the touch listener's `contextmenu`
/// `preventDefault`) and stops the press reaching anything beneath.
fn secondary_press_dismiss_handler(on_dismiss: Rc<dyn Fn()>) -> TouchHandler {
    Rc::new(move |ev: &TouchEvent| {
        if ev.phase == TouchPhase::Began && !pointer_button().is_primary() {
            (on_dismiss)();
            TouchResponse::CONSUMED
        } else {
            TouchResponse::IGNORED
        }
    })
}

/// Transparent fill layer inside a `Dismiss` backdrop that carries
/// [`secondary_press_dismiss_handler`]. It is a CHILD of the backdrop
/// pressable (not a parent or sibling) for two reasons:
///
/// - hit-testing reaches the deepest node first, so the layer sees the
///   press before the pressable — and a primary press it ignores still
///   bubbles up to the pressable's click unchanged;
/// - the backdrop keeps `backdrop_style` on the same node as before, so
///   its geometry does not move. `position: absolute; inset: 0` makes
///   the layer cover exactly the backdrop box (every in-tree backdrop
///   sheet is itself positioned — Taffy positions absolutely against
///   the parent on native, CSS against the positioned backdrop on web).
///
/// A separate `on_touch` node rather than a touch handler on the
/// pressable itself because the pressable's click IS its touch handler
/// on AppKit (`make_tap_handler` in the single `on_touch` slot); a
/// second install there would replace the primary click.
fn secondary_press_dismiss_layer(on_dismiss: Rc<dyn Fn()>) -> Element {
    let fill = StyleRules {
        position: Some(Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        right: Some(Length::Px(0.0).into()),
        bottom: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        ..Default::default()
    };
    super::view()
        .style(StyleProp::Static(Rc::new(fill)))
        .on_touch(secondary_press_dismiss_handler(on_dismiss))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::PointerButton;

    fn portal_prim_of(el: &Element) -> &PrimCell<PortalPrim> {
        match el {
            Element::Item { data, .. } => data
                .downcast_ref::<PrimCell<PortalPrim>>()
                .expect("overlay lowers to a PortalPrim item"),
            _ => panic!("overlay lowering must produce an Item"),
        }
    }

    // Regression (empty ToastHost swallowed clicks): a `click_through`
    // overlay must lower to a portal root marked `pointer-events: none`.
    #[test]
    fn click_through_marks_portal_root_pointer_events_none() {
        let el = overlay()
            .backdrop(BackdropMode::None)
            .click_through(true)
            .build();
        let prim = portal_prim_of(&el).take();
        match prim.style {
            Some(StyleProp::Static(rules)) => assert_eq!(
                rules.pointer_events,
                Some(PointerEvents::None),
                "click-through overlay must mark its portal root pointer-events:none",
            ),
            _ => panic!("click-through overlay must carry a static portal style"),
        }
    }

    // The default (modal) overlay leaves the portal style unset — its
    // root stays interactive so backdrop taps and content clicks land.
    #[test]
    fn default_overlay_leaves_portal_interactive() {
        let el = overlay().build();
        let prim = portal_prim_of(&el).take();
        assert!(
            prim.style.is_none(),
            "a modal overlay must not mark its portal click-through"
        );
    }

    // Composition shape: Dismiss backdrop → [pressable, view]; None →
    // [view] only (the old lowering's child order, which the portal
    // handler mounts in order → backdrop paints behind content).
    #[test]
    fn backdrop_mode_controls_child_layers() {
        let with_backdrop = overlay().build();
        match &with_backdrop {
            Element::Item { children, .. } => assert_eq!(children.len(), 2),
            _ => panic!("expected Item"),
        }
        let without = overlay().backdrop(BackdropMode::None).build();
        match &without {
            Element::Item { children, .. } => assert_eq!(children.len(), 1),
            _ => panic!("expected Item"),
        }
    }

    /// The backdrop pressable's children (the portal's FIRST child).
    fn backdrop_children(el: &Element) -> &Vec<Element> {
        match el {
            Element::Item { children, .. } => match &children[0] {
                Element::Item { data, children } => {
                    assert!(
                        data.downcast_ref::<PrimCell<PressablePrim>>().is_some(),
                        "the portal's first child is the backdrop pressable"
                    );
                    children
                }
                _ => panic!("backdrop must be an Item"),
            },
            _ => panic!("overlay lowering must produce an Item"),
        }
    }

    fn touch_began() -> TouchEvent {
        TouchEvent {
            id: runtime_shared::TouchId(1),
            phase: TouchPhase::Began,
            position: runtime_shared::TouchPoint::new(5.0, 5.0),
            window_position: runtime_shared::TouchPoint::new(5.0, 5.0),
            timestamp_ns: 0,
            force: None,
        }
    }

    /// Regression (FRAMEWORK-NOTES #61): a right-click on a `Dismiss`
    /// backdrop did nothing — the backdrop's only activation was the
    /// pressable's primary-only click, so the popover/menu stayed open.
    /// The lowering now puts a secondary-press layer inside the backdrop
    /// whose `on_touch` dismisses on ANY non-primary `Began` (consuming
    /// it, which is what suppresses the native context menu) and ignores
    /// primary so the pressable's click path is untouched. Fails on the
    /// old lowering: the backdrop pressable had no children at all.
    #[test]
    fn regression_dismiss_backdrop_right_click_dismisses() {
        let fired = Rc::new(std::cell::Cell::new(0u32));
        let f = fired.clone();
        let el = overlay().on_dismiss(move || f.set(f.get() + 1)).build();
        let kids = backdrop_children(&el);
        assert_eq!(kids.len(), 1, "Dismiss backdrop carries the secondary-press layer");
        let layer = match &kids[0] {
            Element::Item { data, .. } => data
                .downcast_ref::<PrimCell<crate::prims::ViewPrim>>()
                .expect("secondary-press layer is a view")
                .take(),
            _ => panic!("secondary-press layer must be an Item"),
        };
        let on_touch = layer.on_touch.expect("layer carries an on_touch handler");

        // Primary press: ignored (bubbles to the pressable's click).
        runtime_shared::set_pointer_button(PointerButton::Primary);
        let r = on_touch(&touch_began());
        assert!(!r.consumed, "primary Began must bubble to the backdrop pressable");
        assert_eq!(fired.get(), 0, "primary Began must not dismiss by itself");

        // Every non-primary button dismisses and is consumed.
        for b in [
            PointerButton::Secondary,
            PointerButton::Middle,
            PointerButton::Other(3),
        ] {
            runtime_shared::set_pointer_button(b);
            let r = on_touch(&touch_began());
            assert!(r.consumed, "{b:?} Began on a Dismiss backdrop is consumed");
        }
        runtime_shared::set_pointer_button(PointerButton::Primary);
        assert_eq!(fired.get(), 3, "each non-primary press dismissed once");
    }

    /// The secondary layer exists only where there is something to
    /// dismiss: an `Opaque` backdrop (modal swallow) and a `Dismiss`
    /// backdrop with no `on_dismiss` keep the bare pressable.
    #[test]
    fn secondary_press_layer_only_on_wired_dismiss_backdrops() {
        let opaque = overlay()
            .backdrop(BackdropMode::Opaque)
            .on_dismiss(|| {})
            .build();
        assert!(backdrop_children(&opaque).is_empty());
        let unwired = overlay().build();
        assert!(backdrop_children(&unwired).is_empty());
    }
}
