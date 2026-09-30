//! Highlight an element on screen for a debugging tool: the Inspector's
//! hover-to-highlight, driven by the `highlight_element` /
//! `clear_highlight` bridge verbs.
//!
//! # One uniform mechanism
//!
//! The highlight is drawn with the framework's own primitives: an
//! `overlay` (a click-through, backdrop-less portal covering the
//! viewport) holding one absolutely positioned `view`, whose style is
//! bound to the highlighted rect. Every backend already renders portals
//! and views, so there is no per-backend overlay code. It's the same
//! composition idea-ui's `ToastHost` uses to float above the app.
//!
//! The rect is the element's `absolute_frame` (viewport-relative logical
//! px, the same space a `FullScreen` overlay's content is laid out in).
//! While a highlight is up it is re-read on a short timer, so the box
//! follows scrolling, animation and layout changes. A highlight whose
//! element unmounts clears itself.
//!
//! # Where the layer lives
//!
//! Nothing is mounted while nothing is highlighted: a dev build's tree is
//! the release build's tree until a tool asks for a highlight. The first
//! `view` mounted in a world (normally the app root) records how to mount
//! into itself — its backend, registry and node — as the world's
//! highlight host, claimed before its own children mount so the root
//! wins over its descendants. `highlight_element` then realizes the
//! layer and inserts its portal node as the host's last child (a portal
//! escapes layout, so it takes no space there — the same place an
//! idea-ui `ToastHost` sits); `clear_highlight` removes it again. A
//! reactive hole was the obvious alternative and the wrong one: its idle
//! anchor is a real flex item on the backends that still draw anchors as
//! plain views, so dev and release layouts would differ.
//!
//! If the host unmounts (a screen swap below a navigator root), its layer
//! goes with it and the claim is released: the next `view` to mount
//! becomes the host, and a still-active highlight re-mounts there on the
//! next refresh.
//!
//! The layer's own mounts are not registered with the robot registry
//! ([`registration_suppressed`]): it is the tool's drawing, and must not
//! show up in snapshots, element counts or the Inspector's tree.
//!
//! Only in `robot` builds, like every other introspection surface.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_scene::{Element, Host, MountCx};
use runtime_shared::primitives::overlay::BackdropMode;
use runtime_shared::primitives::portal::{ViewportPlacement, ViewportRect};
use runtime_shared::{Color, Length, PointerEvents, Position, StyleRules, Tokenized};
use runtime_world::{inject, provide, signal, unscoped, Signal};

use crate::prims::{PortalPrim, PrimCell, ViewPrim};
use crate::robot::{ElementId, Robot};
use crate::style_attach::{on_teardown, StyleProp};

/// How often a live highlight re-reads its element's frame.
const REFRESH_MS: i32 = 120;
/// The box's border, logical px.
const BORDER_PX: f32 = 2.0;
/// The box's outline and fill: one hue, the fill translucent, so the
/// element stays readable underneath. Not a theme token: the highlight
/// belongs to the debugging tool, not to the app's theme.
const OUTLINE: &str = "#3b82f6";
const FILL: &str = "rgba(59, 130, 246, 0.18)";

/// Mounts the layer into the host view; the returned guard unmounts it.
type MountLayer = Rc<dyn Fn(Signal<Option<ViewportRect>>) -> Box<dyn Any>>;

struct HostSlot {
    /// Which mount this is, so a stale host's teardown can't evict a newer one.
    id: u64,
    mount: MountLayer,
}

/// Per-world highlight state (world-lifetime: it must outlive whichever
/// view first hosted the layer).
#[derive(Clone)]
struct HighlightCtx {
    /// Where the box is. Only meaningful while `layer` is mounted.
    rect: Signal<Option<ViewportRect>>,
    host: Rc<RefCell<Option<HostSlot>>>,
    next_host: Rc<Cell<u64>>,
    /// The mounted layer (its unmount guard), while a highlight is up.
    layer: Rc<RefCell<Option<Box<dyn Any>>>>,
    /// The element being tracked, and a generation that ends a superseded
    /// refresh loop.
    target: Rc<Cell<Option<ElementId>>>,
    generation: Rc<Cell<u64>>,
}

fn ctx() -> HighlightCtx {
    if let Some(c) = inject::<HighlightCtx>() {
        return c;
    }
    let c = HighlightCtx {
        rect: unscoped(|| signal(None)),
        host: Rc::default(),
        next_host: Rc::default(),
        layer: Rc::default(),
        target: Rc::default(),
        generation: Rc::default(),
    };
    unscoped(|| provide(c.clone()));
    c
}

thread_local! {
    static SUPPRESS: Cell<u32> = const { Cell::new(0) };
    /// Set while arming the refresh timer: with no scheduler installed
    /// `after_ms` runs its callback synchronously, and a self-rearming
    /// loop would never return.
    static ARMING: Cell<bool> = const { Cell::new(false) };
}

/// `true` while the highlight layer itself is mounting.
pub(crate) fn registration_suppressed() -> bool {
    SUPPRESS.with(|s| s.get() > 0)
}

/// Offer the view being mounted as the world's highlight host. Takes it
/// only when no view hosts yet and this registry can build the layer (an
/// app whose primitive set leaves out `overlay` gets no highlight rather
/// than a panic). Call before the view's children mount.
pub(crate) fn offer_host<H: Host + 'static>(cx: &MountCx<'_, H>, node: &H::Node) {
    if registration_suppressed() {
        return;
    }
    let registry = cx.registry();
    if !registry.has::<PrimCell<PortalPrim>>() || !registry.has::<PrimCell<ViewPrim>>() {
        return;
    }
    let c = ctx();
    if c.host.borrow().is_some() {
        return;
    }
    let id = c.next_host.get();
    c.next_host.set(id + 1);
    let (backend, registry, host_node) = (cx.backend().clone(), registry.clone(), node.clone());
    let mount: MountLayer = Rc::new(move |rect| {
        struct Unsuppress;
        impl Drop for Unsuppress {
            fn drop(&mut self) {
                SUPPRESS.with(|s| s.set(s.get() - 1));
            }
        }
        SUPPRESS.with(|s| s.set(s.get() + 1));
        let realized = {
            let _unsuppress = Unsuppress;
            runtime_scene::realize(&backend, &registry, layer(rect))
        };
        let nodes = realized.collect_nodes();
        let mut host = host_node.clone();
        for n in &nodes {
            backend.borrow_mut().insert(&mut host, n.clone());
        }
        struct Mounted<H: Host> {
            backend: Rc<RefCell<H>>,
            host: H::Node,
            nodes: Vec<H::Node>,
            realized: Option<runtime_scene::Realized<H::Node>>,
        }
        impl<H: Host> Drop for Mounted<H> {
            fn drop(&mut self) {
                // Detach, then tear down (the portal releases itself).
                if let Ok(mut b) = self.backend.try_borrow_mut() {
                    for n in &self.nodes {
                        b.remove_child(&self.host, n);
                    }
                }
                drop(self.realized.take());
            }
        }
        Box::new(Mounted { backend: backend.clone(), host: host_node.clone(), nodes, realized: Some(realized) })
            as Box<dyn Any>
    });
    *c.host.borrow_mut() = Some(HostSlot { id, mount });
    // The host is going away: its layer goes with it, and the next view
    // to mount takes over.
    let (host, layer) = (c.host.clone(), c.layer.clone());
    on_teardown(move || {
        if host.borrow().as_ref().is_some_and(|h| h.id == id) {
            let old = layer.borrow_mut().take();
            drop(old);
            host.borrow_mut().take();
        }
    });
}

/// Mount the layer if it isn't up (and a host exists).
fn ensure_layer(c: &HighlightCtx) {
    if c.layer.borrow().is_some() {
        return;
    }
    let mount = c.host.borrow().as_ref().map(|h| h.mount.clone());
    if let Some(mount) = mount {
        let guard = mount(c.rect);
        *c.layer.borrow_mut() = Some(guard);
    }
}

fn unmount_layer(c: &HighlightCtx) {
    let old = c.layer.borrow_mut().take();
    drop(old);
}

fn layer(rect: Signal<Option<ViewportRect>>) -> Element {
    let fill = StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(0.0))),
        right: Some(Tokenized::Literal(Length::Px(0.0))),
        bottom: Some(Tokenized::Literal(Length::Px(0.0))),
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    };
    let highlight_box = crate::builders::view()
        .style(StyleProp::Dynamic(Box::new(move || Rc::new(box_rules(rect.get())))))
        .build();
    crate::builders::overlay()
        .placement(ViewportPlacement::FullScreen)
        .backdrop(BackdropMode::None)
        .trap_focus(false)
        .click_through(true)
        .style(StyleProp::Static(Rc::new(fill)))
        .child(highlight_box)
        .build()
}

/// The box at `rect`, or an invisible zero-size box when nothing is
/// highlighted.
fn box_rules(rect: Option<ViewportRect>) -> StyleRules {
    let px = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
    let r = rect.unwrap_or_default();
    let color = |c: &str| Some(Tokenized::Literal(Color(c.to_string())));
    let border = if rect.is_some() { BORDER_PX } else { 0.0 };
    StyleRules {
        position: Some(Position::Absolute),
        left: px(r.x),
        top: px(r.y),
        width: px(r.width),
        height: px(r.height),
        opacity: Some(Tokenized::Literal(if rect.is_some() { 1.0 } else { 0.0 })),
        background: color(FILL),
        border_top_width: Some(Tokenized::Literal(border)),
        border_right_width: Some(Tokenized::Literal(border)),
        border_bottom_width: Some(Tokenized::Literal(border)),
        border_left_width: Some(Tokenized::Literal(border)),
        border_top_color: color(OUTLINE),
        border_right_color: color(OUTLINE),
        border_bottom_color: color(OUTLINE),
        border_left_color: color(OUTLINE),
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    }
}

fn element(id: ElementId) -> crate::robot::Element {
    crate::robot::Element { id, kind: crate::robot::ElementKind::View, test_id: None, label: None }
}

/// The element's current frame, or `None` once it's gone (or has no
/// frame on this backend).
fn frame_of(id: ElementId) -> Option<ViewportRect> {
    Robot::new().absolute_frame(&element(id)).ok().flatten()
}

/// The `highlight_element` verb: show the box over `id` and keep it there.
pub(crate) fn highlight(id: ElementId) -> Result<String, String> {
    let generation = crate::robot::entered(|| {
        let rect = frame_of(id).ok_or_else(|| format!("element {} has no frame to highlight", id.0))?;
        let c = ctx();
        if c.host.borrow().is_none() {
            return Err("no view is mounted to host the highlight".to_string());
        }
        c.target.set(Some(id));
        c.rect.set(Some(rect));
        c.generation.set(c.generation.get() + 1);
        ensure_layer(&c);
        Ok(c.generation.get())
    })?;
    crate::robot::settle();
    arm_refresh(generation);
    Ok("\"ok\"".into())
}

/// The `clear_highlight` verb.
pub(crate) fn clear() {
    crate::robot::entered(|| {
        let c = ctx();
        c.target.set(None);
        c.rect.set(None);
        c.generation.set(c.generation.get() + 1);
        unmount_layer(&c);
    });
    crate::robot::settle();
}

/// Re-read the tracked element's frame every [`REFRESH_MS`] until the
/// highlight is cleared or replaced (`generation` moves on).
fn arm_refresh(generation: u64) {
    if !runtime_shared::scheduling::is_scheduler_installed() {
        return;
    }
    ARMING.with(|a| a.set(true));
    runtime_shared::scheduling::after_ms_detached(REFRESH_MS, move || {
        if ARMING.with(|a| a.get()) {
            return; // ran synchronously: no timer to follow the element with
        }
        let live = crate::robot::entered(|| refresh(generation));
        crate::robot::settle();
        if live {
            arm_refresh(generation);
        }
    });
    ARMING.with(|a| a.set(false));
}

/// One refresh tick: follow the element, or take the box down once it's
/// gone. `false` ends the loop.
fn refresh(generation: u64) -> bool {
    let c = ctx();
    if c.generation.get() != generation {
        return false;
    }
    let Some(id) = c.target.get() else { return false };
    match frame_of(id) {
        Some(rect) => {
            c.rect.set(Some(rect));
            // Re-mount on a new host if the old one unmounted meanwhile.
            ensure_layer(&c);
            true
        }
        None => {
            c.target.set(None);
            c.rect.set(None);
            unmount_layer(&c);
            false
        }
    }
}
