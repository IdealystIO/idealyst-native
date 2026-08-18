//! `Element::Portal` — render a subtree in a window-level overlay above
//! the app content (modals, sheets, dropdowns, tooltips) rather than in
//! the normal layout flow.
//!
//! ## GTK mechanism
//!
//! The framework root is the window's single child. On the first portal
//! we wrap that child in a `gtk::Overlay`: the root stays the overlay's
//! main child (still driven by its own `size_allocate` layout pass) and
//! each portal's container is added via `Overlay::add_overlay`, so it
//! paints on top and escapes the app's flex flow + clipping. Subsequent
//! portals reuse the same overlay. `Overlay` stacks its overlay children
//! in add order — matching the portal primitive's mount-order stacking.
//!
//! ## Container = a full-viewport flex box
//!
//! The portal container is an [`IdealystView`] sized to the whole overlay
//! (`hexpand`/`vexpand` + `halign`/`valign` Fill). Its own flex style
//! places the single content child, mapped from [`PortalTarget`]:
//! `Center`/`Top`/`Bottom`/`Left`/`Right`/`FullScreen` become
//! justify/align pairs (see [`placement_style`]). This is the same
//! "container flex positions the content" model the macOS viewport-portal
//! path uses. An author/composition style on the portal element overrides
//! this default via `apply_style`.
//!
//! Because the container is a Taffy orphan (the walker never inserts a
//! portal node into its logical parent — see `walker/portal.rs`), it runs
//! its own detached layout pass: the container installs the SAME layout
//! callback the framework root uses, calling
//! [`super::LinuxBackend::layout_detached_root`] from its `size_allocate`.
//!
//! ## Dismissal / focus
//!
//! `on_dismiss` fires on Escape (the desktop platform-dismiss gesture) via
//! a key controller on the container. Backdrop-tap dismissal is
//! composition-level (a `pressable` backdrop child), per the portal
//! contract. `trap_focus` is best-effort: the container is made focusable
//! and grabs focus on map, but GTK modal-style focus confinement (a real
//! grab that blocks the app behind) would need a `gtk::Window`-hosted
//! popup — a documented gap for the v1 overlay path.
//!
//! ## `PortalTarget::Anchor`
//!
//! Anchored portals (tooltips/popovers/dropdowns) ARE element-tracked.
//! The container keeps a neutral top-left flex — it exists only to supply
//! the viewport coordinate space — and the content child's frame is
//! overridden every layout pass from
//! [`runtime_shared::primitives::portal::resolve_anchored_placement`], the
//! ONE placement algorithm every backend shares (collision flip + viewport
//! clamp included). See [`AnchorSpec`] and
//! [`LinuxBackend::anchor_override`].
//!
//! The re-pin rides the existing frame beat rather than a per-portal
//! `raf_loop`: [`LinuxBackend::pump`] queue-allocates every live portal
//! container each tick, which re-runs
//! [`LinuxBackend::layout_detached_root`], which re-resolves the
//! placement. So a popover follows its trigger through scrolling and
//! resizing without any extra scheduler handle to own or leak.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk4::glib;
use gtk4::prelude::*;

use runtime_shared::primitives::portal::{
    AnchorTarget, ElementAlign, ElementSide, PortalTarget, ViewportPlacement,
};
use runtime_shared::{AlignItems, FlexDirection, JustifyContent, Length, StyleRules, Tokenized};

use crate::{IdealystView, LinuxBackend};

/// Minimum gutter kept between an anchored overlay and every viewport
/// edge. Matches the value the other backends pass to
/// `resolve_anchored_placement`, so a popover pinned near a window edge
/// lands in the same place on GTK as it does on web / AppKit.
pub(crate) const ANCHOR_EDGE_GAP: f32 = 8.0;

/// What an anchored portal needs to re-pin itself: the trigger it tracks
/// plus the author's placement intent. Recorded by `create_portal` and
/// consumed by `LinuxBackend::anchor_override` on every layout pass.
pub(crate) struct AnchorSpec {
    pub target: AnchorTarget,
    pub side: ElementSide,
    pub align: ElementAlign,
    pub offset: f32,
}

/// Base Taffy style for a portal container: a full-viewport column flex
/// box whose justify/align place the content child per the target's
/// placement. Anchored targets use a neutral top-left placement (see the
/// module doc's anchor gap).
pub(crate) fn placement_style(target: &PortalTarget) -> StyleRules {
    let full = || Some(Tokenized::Literal(Length::Percent(100.0)));
    let (justify, align) = match target {
        PortalTarget::Viewport(p) => match p {
            ViewportPlacement::Center => (JustifyContent::Center, AlignItems::Center),
            ViewportPlacement::Top => (JustifyContent::FlexStart, AlignItems::Center),
            ViewportPlacement::Bottom => (JustifyContent::FlexEnd, AlignItems::Center),
            ViewportPlacement::Left => (JustifyContent::Center, AlignItems::FlexStart),
            ViewportPlacement::Right => (JustifyContent::Center, AlignItems::FlexEnd),
            // Full-screen: stretch the child across both axes.
            ViewportPlacement::FullScreen => (JustifyContent::Center, AlignItems::Stretch),
        },
        // Anchored: neutral top-left (see module doc).
        PortalTarget::Anchor { .. } | PortalTarget::Named(_) => {
            (JustifyContent::FlexStart, AlignItems::FlexStart)
        }
    };
    StyleRules {
        flex_direction: Some(FlexDirection::Column),
        justify_content: Some(justify),
        align_items: Some(align),
        width: full(),
        height: full(),
        ..Default::default()
    }
}

/// Wire the container widget: fill the overlay, install the detached
/// layout pass, the Escape→dismiss controller, optional focus grab, and
/// schedule attachment into the window overlay.
pub(crate) fn configure(
    view: &IdealystView,
    node_id: u64,
    anchored: bool,
    backend: Weak<RefCell<LinuxBackend>>,
    host_window: gtk4::Window,
    on_dismiss: Option<Rc<dyn Fn()>>,
    trap_focus: bool,
) {
    // Fill the overlay so the container's flex has the whole viewport to
    // place content in.
    view.set_hexpand(true);
    view.set_vexpand(true);
    view.set_halign(gtk4::Align::Fill);
    view.set_valign(gtk4::Align::Fill);

    // An ANCHORED container is a coordinate space, not a surface: it is
    // full-viewport only so the resolved placement has somewhere to live, and
    // the framework lowers it expecting a root that "only covers what it
    // renders". Filling the viewport without this made it swallow every click
    // outside the panel — the dismiss-catcher overlay beneath it never saw
    // them, so click-outside did nothing while Escape still worked.
    if anchored {
        view.set_input_transparent(true);
    }

    // Detached layout pass — same mechanism the framework root uses, but
    // scoped to this orphan subtree.
    {
        let backend = backend.clone();
        view.set_layout_callback(Rc::new(move |w, h| {
            let Some(b) = backend.upgrade() else { return };
            // Resolve the trigger rect FIRST, under an immutable borrow: it
            // reaches back through the author's handle into this same
            // `RefCell`, so doing it inside the mutable borrow below fails and
            // the handle falls back to the zero rect — which the placement
            // algorithm reads as a real 0x0 target at the window origin, and
            // every popover lands in the top-left corner.
            let trigger = match b.try_borrow() {
                Ok(bref) => bref.anchor_trigger_rect(node_id),
                Err(_) => None,
            };
            let Ok(mut bmut) = b.try_borrow_mut() else { return };
            bmut.layout_detached_root(node_id, w as f32, h as f32, trigger);
        }));
    }

    // Escape → platform dismiss.
    if let Some(dismiss) = on_dismiss {
        let key = gtk4::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                (dismiss)();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        view.add_controller(key);
    }

    if trap_focus {
        // Best-effort: focusable + grab on map. Full modal confinement is
        // the documented gap (needs a popup window host).
        view.set_focusable(true);
        let v = view.clone();
        view.connect_map(move |_| {
            v.grab_focus();
        });
    }

    // Attach into the window overlay. Deferred to idle so it runs after
    // the mount finishes and `finish` has set the framework root as the
    // window's child (a portal in the INITIAL tree is created before
    // `finish`; a reactively-opened one after it — idle covers both).
    let view = view.clone();
    glib::idle_add_local_once(move || {
        attach_to_overlay(&host_window, &view);
    });
}

/// Ensure the window's child is a `gtk::Overlay` and add `view` as an
/// overlay child. Wraps the existing child on first use; reuses the
/// overlay thereafter.
fn attach_to_overlay(window: &gtk4::Window, view: &IdealystView) {
    // Already attached (idempotent against a double-idle).
    if view.parent().is_some() {
        return;
    }
    let overlay = ensure_overlay(window);
    let Some(overlay) = overlay else {
        return;
    };
    overlay.add_overlay(view);
}

/// Return the window's `gtk::Overlay`, wrapping the current child in one
/// if needed. Returns `None` only if the window has no child yet (no root
/// to overlay — shouldn't happen post-`finish`).
fn ensure_overlay(window: &gtk4::Window) -> Option<gtk4::Overlay> {
    let child = window.child()?;
    if let Ok(overlay) = child.clone().downcast::<gtk4::Overlay>() {
        return Some(overlay);
    }
    // Wrap: pull the root out, make it the overlay's main child, reinstall
    // the overlay as the window child. `set_child` re-parents the root.
    let overlay = gtk4::Overlay::new();
    window.set_child(Some(&overlay));
    overlay.set_child(Some(&child));
    Some(overlay)
}

/// Detach a portal container from its overlay (called on
/// `release_portal`). Best-effort: no-op if it isn't overlay-parented.
pub(crate) fn release(view: &IdealystView) {
    if let Some(parent) = view.parent() {
        if let Ok(overlay) = parent.downcast::<gtk4::Overlay>() {
            overlay.remove_overlay(view);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests — placement mapping needs no GTK context.
    use super::*;

    #[test]
    fn viewport_center_maps_to_center_center() {
        let s = placement_style(&PortalTarget::Viewport(ViewportPlacement::Center));
        assert_eq!(s.justify_content, Some(JustifyContent::Center));
        assert_eq!(s.align_items, Some(AlignItems::Center));
        // Full-viewport container.
        assert!(matches!(
            s.width.as_ref().map(|t| t.resolve()),
            Some(Length::Percent(p)) if p == 100.0
        ));
    }

    #[test]
    fn viewport_top_bottom_pin_main_axis() {
        let top = placement_style(&PortalTarget::Viewport(ViewportPlacement::Top));
        assert_eq!(top.justify_content, Some(JustifyContent::FlexStart));
        let bottom = placement_style(&PortalTarget::Viewport(ViewportPlacement::Bottom));
        assert_eq!(bottom.justify_content, Some(JustifyContent::FlexEnd));
    }

    #[test]
    fn fullscreen_stretches_cross_axis() {
        let s = placement_style(&PortalTarget::Viewport(ViewportPlacement::FullScreen));
        assert_eq!(s.align_items, Some(AlignItems::Stretch));
    }

    #[test]
    fn anchor_and_named_use_neutral_top_left() {
        // The anchored CONTAINER stays neutral on purpose: it exists only
        // to supply the viewport coordinate space, and the content child's
        // frame is overridden from the shared placement resolver (see the
        // module doc + `LinuxBackend::anchor_override`). A centering flex
        // here would fight that override. `Named` shares the match arm, so
        // it exercises the mapping without `AnchorableHandle` scaffolding.
        let s = placement_style(&PortalTarget::Named("slot"));
        assert_eq!(s.justify_content, Some(JustifyContent::FlexStart));
        assert_eq!(s.align_items, Some(AlignItems::FlexStart));
    }
}
