//! Render-elsewhere builders: `portal()`, plus the `overlay()` /
//! `anchored_overlay()` compositions.
//!
//! `portal()` is the raw primitive; the two overlay builders are
//! build-time compositions that lower to a portal item with the
//! backdrop / content wiring added around the caller's children — they
//! have no payload or handler of their own.

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

    /// Receive the imperative-ref handle at mount.
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

/// Start a viewport-anchored overlay (modal, drawer, sheet). Defaults:
/// `Center` placement, `Dismiss` backdrop, focus-trap ON.
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

    /// Where the content sits in the viewport — the same on every backend:
    /// `Top` / `Bottom` / `FullScreen` give it the full width at its natural
    /// height (top / bottom / top edge); `Left` / `Right` keep its natural
    /// size in the top corner; `Center` keeps its natural size (up to the
    /// viewport width) in the middle. The table is on
    /// [`viewport_container_rules`](runtime_shared::primitives::portal::viewport_container_rules).
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
        // into the backdrop's on_press). No `cycle` wrap needed —
        // writes stage until the driver flush by design.
        self.on_dismiss = Some(Rc::new(f));
        self
    }

    pub fn trap_focus(mut self, t: bool) -> Self {
        self.trap_focus = t;
        self
    }

    /// Make the overlay's own layer transparent to pointer events so
    /// clicks in the empty area pass through to the page beneath (the
    /// ToastHost strip case).
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
/// context menu). Defaults: side `Below`, align
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

// NOTE: `anchored_overlay` has no `click_through` — popovers /
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
// Lowering — shared between both compositions
// ===========================================================================
//
// What an overlay takes hits on, the same on every backend (the
// `BackdropMode` contract):
//
// - The portal ROOT never does: it is `pointer-events: none`. A native
//   container spans the viewport and web's root spans it for some
//   placements, so an interactive root swallowed the page around the
//   content on some backends and not others.
// - The content wrapper does (`pointer-events: auto`) — unless the overlay
//   is `click_through`, when it inherits `none` and only descendants that
//   opt back in take hits (ToastHost's cards).
// - A `Dismiss` / `Opaque` backdrop covers the VIEWPORT, whatever the
//   placement: it is its own `FullScreen` portal mounted before the
//   content's, holding a pressable that fills it. Inside the content's
//   portal it could only cover the content's box on web (a `Center` root
//   is sized to its content), and nothing at all on iOS.

/// Rules filling the parent: `position: absolute; inset: 0`.
fn fill_rules() -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
        top: Some(Length::Px(0.0).into()),
        right: Some(Length::Px(0.0).into()),
        bottom: Some(Length::Px(0.0).into()),
        left: Some(Length::Px(0.0).into()),
        ..Default::default()
    }
}

fn pointer_events_rules(pe: PointerEvents) -> StyleProp {
    StyleProp::Static(Rc::new(StyleRules { pointer_events: Some(pe), ..Default::default() }))
}

/// `style` with `pointer_events: pe` layered on (an unset field only, for
/// the rule-carrying variants; the override layer for sheets), or `Err`
/// with `style` back when the variant can't carry an extra rule.
fn with_pointer_events(style: StyleProp, pe: PointerEvents) -> Result<StyleProp, StyleProp> {
    let add = move |r: &StyleRules| {
        let mut r = r.clone();
        r.pointer_events.get_or_insert(pe);
        Rc::new(r)
    };
    let only = StyleRules { pointer_events: Some(pe), ..Default::default() };
    Ok(match style {
        StyleProp::Static(r) => StyleProp::Static(add(&r)),
        StyleProp::Dynamic(f) => StyleProp::Dynamic(Box::new(move || add(&f()))),
        StyleProp::Sheet(app) => StyleProp::Sheet(Box::new(app.with_overrides(only))),
        StyleProp::SheetDynamic(f) => {
            StyleProp::SheetDynamic(Box::new(move || f().with_overrides(only.clone())))
        }
        StyleProp::Preminted { class, overrides, inline } => StyleProp::Preminted {
            class,
            overrides,
            inline: Some(add(inline.as_deref().unwrap_or(&StyleRules::default()))),
        },
        StyleProp::PremintedDynamic { class_of, overrides } => StyleProp::PremintedDynamic {
            class_of,
            overrides: Some(add(overrides.as_deref().unwrap_or(&StyleRules::default()))),
        },
        other @ StyleProp::SignalClass(_) => return Err(other),
    })
}

/// The content wrapper: the caller's children, its `content_style`, and
/// `pointer_events` (`None` = inherit the root's `none`). The rule rides
/// on the caller's own style node, so the content box is the one it
/// styled; only a signal-class style, which carries no rules, gets an
/// outer wrapper for it.
fn content_wrapper(children: Vec<Element>, content_style: Option<StyleProp>, pe: Option<PointerEvents>) -> Element {
    let content = super::view().children(children);
    match (content_style, pe) {
        (None, None) => content.build(),
        (Some(cs), None) => content.style(cs).build(),
        (None, Some(pe)) => content.style(pointer_events_rules(pe)).build(),
        (Some(cs), Some(pe)) => match with_pointer_events(cs, pe) {
            Ok(cs) => content.style(cs).build(),
            Err(cs) => super::view().style(pointer_events_rules(pe)).child(content.style(cs).build()).build(),
        },
    }
}

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
    let content_pe = (!click_through).then_some(PointerEvents::Auto);
    let content = item(
        PrimCell::new(PortalPrim {
            target,
            on_dismiss: on_dismiss.clone(),
            trap_focus,
            style: Some(pointer_events_rules(PointerEvents::None)),
            a11y: AccessibilityProps::default(),
            ref_fill,
        }),
        vec![content_wrapper(children, content_style, content_pe)],
    );
    if matches!(backdrop, BackdropMode::None) {
        return content;
    }
    let backdrop_portal = item(
        PrimCell::new(PortalPrim {
            target: PortalTarget::Viewport(ViewportPlacement::FullScreen),
            // Platform dismissal (Escape, back) belongs to the content's
            // portal; a second listener here would dismiss twice.
            on_dismiss: None,
            trap_focus: false,
            style: Some(pointer_events_rules(PointerEvents::None)),
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        }),
        vec![backdrop_layer(backdrop, backdrop_style, on_dismiss)],
    );
    Element::Fragment(vec![backdrop_portal, content])
}

/// A `Dismiss` / `Opaque` backdrop: a pressable filling its portal.
/// `Dismiss` presses call `on_dismiss`; `Opaque` presses do nothing, which
/// is the point — they don't reach the page. `backdrop_style` paints a
/// layer inside it (a scrim colour, a fade), so to cover the backdrop that
/// style fills its parent too (`position: absolute; inset: 0`, as every
/// in-tree backdrop sheet does).
fn backdrop_layer(backdrop: BackdropMode, backdrop_style: Option<StyleProp>, on_dismiss: Option<Rc<dyn Fn()>>) -> Element {
    let on_press: Rc<dyn Fn()> = match (&backdrop, &on_dismiss) {
        (BackdropMode::Dismiss, Some(d)) => d.clone(),
        _ => Rc::new(|| {}),
    };
    let mut layers: Vec<Element> = Vec::with_capacity(2);
    if let Some(style) = backdrop_style {
        layers.push(super::view().style(style).build());
    }
    // A `Dismiss` backdrop dismisses on a press of ANY button. The
    // pressable's own activation is the platform "click", which is
    // primary-only on every backend (DOM `click`, AppKit/UIKit tap,
    // Android `OnClickListener`), so without the secondary layer a
    // right-click on the backdrop falls through and the popover stays
    // open.
    if let (BackdropMode::Dismiss, Some(d)) = (&backdrop, &on_dismiss) {
        layers.push(secondary_press_dismiss_layer(d.clone()));
    }
    let mut fill = fill_rules();
    fill.pointer_events = Some(PointerEvents::Auto);
    // PressablePrim constructed directly (not via `pressable()`) so the
    // already-built `Rc<dyn Fn>` installs without a wrapper closure.
    item(
        PrimCell::new(PressablePrim {
            test_id: None,
            on_press,
            disabled: None,
            preserves_focus: false,
            style: Some(StyleProp::Static(Rc::new(fill))),
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        }),
        layers,
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
/// - `position: absolute; inset: 0` makes the layer cover exactly the
///   backdrop box (the pressable is itself positioned — Taffy positions
///   absolutely against the parent on native, CSS against the positioned
///   backdrop on web).
///
/// A separate `on_touch` node rather than a touch handler on the
/// pressable itself because the pressable's click IS its touch handler
/// on AppKit (`make_tap_handler` in the single `on_touch` slot); a
/// second install there would replace the primary click.
fn secondary_press_dismiss_layer(on_dismiss: Rc<dyn Fn()>) -> Element {
    super::view()
        .style(StyleProp::Static(Rc::new(fill_rules())))
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
                .expect("an overlay portal is a PortalPrim item"),
            _ => panic!("expected a portal Item"),
        }
    }

    /// `(backdrop portal, content portal)`; `None` for the backdrop when
    /// the overlay has none.
    fn portals(el: &Element) -> (Option<&Element>, &Element) {
        match el {
            Element::Fragment(parts) => {
                assert_eq!(parts.len(), 2, "a backdrop overlay is [backdrop portal, content portal]");
                (Some(&parts[0]), &parts[1])
            }
            item @ Element::Item { .. } => (None, item),
            _ => panic!("unexpected overlay lowering"),
        }
    }

    fn children_of(el: &Element) -> &Vec<Element> {
        match el {
            Element::Item { children, .. } => children,
            _ => panic!("expected an Item"),
        }
    }

    fn static_rules(style: &Option<StyleProp>) -> Rc<StyleRules> {
        match style {
            Some(StyleProp::Static(r)) => r.clone(),
            _ => panic!("expected a static style"),
        }
    }

    fn view_rules(el: &Element) -> Option<Rc<StyleRules>> {
        match el {
            Element::Item { data, .. } => {
                let v = data.downcast_ref::<PrimCell<crate::prims::ViewPrim>>().expect("a view").take();
                match v.style {
                    Some(StyleProp::Static(r)) => Some(r),
                    None => None,
                    _ => panic!("expected a static view style"),
                }
            }
            _ => panic!("expected an Item"),
        }
    }

    /// Every overlay's portal root is `pointer-events: none`, so the
    /// container never takes a hit; the content wrapper takes them
    /// (`auto`). Before, the root was left interactive: a native container
    /// spans the viewport (macOS swallowed every click around a `Center`
    /// overlay with no backdrop), while web's root is the content box.
    #[test]
    fn regression_the_portal_root_never_takes_hits_and_the_content_does() {
        for el in [overlay().backdrop(BackdropMode::None).build(), overlay().build()] {
            let (backdrop, content) = portals(&el);
            for portal in backdrop.into_iter().chain([content]) {
                let prim = portal_prim_of(portal).take();
                assert_eq!(static_rules(&prim.style).pointer_events, Some(PointerEvents::None));
            }
            let wrapper = &children_of(content)[0];
            assert_eq!(view_rules(wrapper).unwrap().pointer_events, Some(PointerEvents::Auto));
        }
    }

    // Regression (empty ToastHost swallowed clicks): a `click_through`
    // overlay's content inherits the root's `none`, so only descendants
    // that opt back in take hits.
    #[test]
    fn click_through_content_inherits_the_root_s_none() {
        let el = overlay().backdrop(BackdropMode::None).click_through(true).build();
        let (_, content) = portals(&el);
        assert!(view_rules(&children_of(content)[0]).is_none(), "no pointer-events on the wrapper");
    }

    /// A `Dismiss` / `Opaque` backdrop covers the VIEWPORT whatever the
    /// placement: its own `FullScreen` portal, mounted before the
    /// content's, holding a pressable that fills it. Inside the content's
    /// portal it covered only the content's box on web (a `Center` root is
    /// content-sized) and nothing on iOS, so a tap outside a default
    /// overlay neither dismissed it nor stayed off the page.
    #[test]
    fn regression_a_backdrop_is_a_full_screen_portal_that_fills_it() {
        for mode in [BackdropMode::Dismiss, BackdropMode::Opaque] {
            let el = overlay().placement(ViewportPlacement::Center).backdrop(mode).build();
            let (backdrop, content) = portals(&el);
            let backdrop = backdrop.expect("a backdrop portal");
            let prim = portal_prim_of(backdrop).take();
            assert!(matches!(prim.target, PortalTarget::Viewport(ViewportPlacement::FullScreen)));
            assert!(prim.on_dismiss.is_none(), "platform dismissal is the content portal's");
            let pressable = match &children_of(backdrop)[0] {
                Element::Item { data, .. } => data.downcast_ref::<PrimCell<PressablePrim>>().expect("a pressable").take(),
                _ => panic!(),
            };
            let fill = static_rules(&pressable.style);
            assert_eq!(fill.position, Some(Position::Absolute));
            assert!(fill.top.is_some() && fill.left.is_some() && fill.right.is_some() && fill.bottom.is_some());
            assert_eq!(fill.pointer_events, Some(PointerEvents::Auto));
            assert!(matches!(portal_prim_of(content).take().target, PortalTarget::Viewport(ViewportPlacement::Center)));
        }
        assert!(portals(&overlay().backdrop(BackdropMode::None).build()).0.is_none());
    }

    /// The backdrop pressable's children.
    fn backdrop_children(el: &Element) -> &Vec<Element> {
        let backdrop = portals(el).0.expect("a backdrop portal");
        match &children_of(backdrop)[0] {
            Element::Item { data, children } => {
                assert!(
                    data.downcast_ref::<PrimCell<PressablePrim>>().is_some(),
                    "the backdrop portal holds the backdrop pressable"
                );
                children
            }
            _ => panic!("backdrop must be an Item"),
        }
    }

    /// `backdrop_style` paints a layer inside the backdrop, under the
    /// secondary-press layer.
    #[test]
    fn backdrop_style_paints_a_layer_inside_the_backdrop() {
        let el = overlay()
            .backdrop_style(StyleProp::Static(Rc::new(StyleRules { opacity: Some(0.5.into()), ..Default::default() })))
            .on_dismiss(|| {})
            .build();
        let kids = backdrop_children(&el);
        assert_eq!(kids.len(), 2);
        assert_eq!(view_rules(&kids[0]).unwrap().opacity, Some(0.5.into()));
    }

    /// A sheet content style keeps its own node; an outer wrapper carries
    /// the `pointer-events`. A static one takes it directly.
    #[test]
    fn a_content_style_keeps_its_rules_and_gains_pointer_events() {
        let el = overlay()
            .backdrop(BackdropMode::None)
            .style(StyleProp::Static(Rc::new(StyleRules { opacity: Some(0.2.into()), ..Default::default() })))
            .build();
        let rules = view_rules(&children_of(portals(&el).1)[0]).unwrap();
        assert_eq!((rules.opacity, rules.pointer_events), (Some(0.2.into()), Some(PointerEvents::Auto)));
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

    /// Regression: a right-click on a `Dismiss`
    /// backdrop did nothing — the backdrop's only activation was the
    /// pressable's primary-only click, so the popover/menu stayed open.
    /// The lowering now puts a secondary-press layer inside the backdrop
    /// whose `on_touch` dismisses on ANY non-primary `Began` (consuming
    /// it, which is what suppresses the native context menu) and ignores
    /// primary so the pressable's click path is untouched. Fails if the
    /// backdrop pressable has no secondary-press child.
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
