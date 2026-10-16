//! Portals on the painted Windows scene: overlays, popovers, menus,
//! toasts.
//!
//! A portal is a view node that is a layout ROOT of its own: `insert`
//! leaves it detached from its parent (as a private-layer root is), the
//! layout pass lays it out against the host's client area, and the scene
//! paints every portal after the main tree, in creation order, and hit
//! tests them before it, topmost first. So an overlay draws over the page
//! and takes the clicks it covers, wherever in the tree it was written.
//!
//! - **Viewport** portals carry the shared container rules
//!   (`viewport_container_rules`): a full-client flex column whose
//!   justify / align put the content where web puts it.
//! - **Anchored** portals span the client area the same way
//!   (`anchored_container_rules`) and their content is moved next to the
//!   anchor by the shared [`AnchoredPlacer`] (flip to fit, align, clamp)
//!   — the macOS model. The anchor's rect is read through its handle,
//!   which borrows the backend, so placement runs on a fresh turn after
//!   each layout or scroll ([`place_anchored`]), and the content stays
//!   unpainted until it has been placed once.
//!
//! The portal's own style (an overlay's `pointer-events: none`) is merged
//! over its container rules rather than replacing them, so a style apply
//! can't undo the placement.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use runtime_shared::primitives::portal::{
    anchored_container_rules, viewport_container_rules, AnchorTarget, AnchoredPlacer, PortalTarget,
    ViewportRect, ANCHOR_EDGE_GAP,
};
use runtime_shared::StyleRules;

use crate::WindowsBackend;

pub(crate) struct Entry {
    /// The container rules the portal's own style is layered over.
    pub(crate) base: StyleRules,
    pub(crate) on_dismiss: Option<Rc<dyn Fn()>>,
    pub(crate) anchor: Option<Anchor>,
}

pub(crate) struct Anchor {
    target: AnchorTarget,
    placer: AnchoredPlacer,
    /// Where the content was last placed (container-relative = client
    /// coordinates). `None` until the anchor has been measured.
    pub(crate) at: Option<(f32, f32)>,
}

/// The portals of one backend, in creation (paint) order.
#[derive(Default)]
pub(crate) struct Portals {
    order: Vec<u64>,
    entries: HashMap<u64, Entry>,
    /// A placement turn is queued (see [`place_anchored`]).
    pub(crate) placement_queued: bool,
    /// Placement is being applied: its own repositioning must not queue
    /// another turn.
    pub(crate) placing: bool,
}

impl Portals {
    pub(crate) fn contains(&self, id: u64) -> bool {
        self.entries.contains_key(&id)
    }

    /// Roots in paint order (bottom first).
    pub(crate) fn roots(&self) -> impl DoubleEndedIterator<Item = u64> + '_ {
        self.order.iter().copied()
    }

    pub(crate) fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.get(&id)
    }

    pub(crate) fn add(&mut self, id: u64, target: &PortalTarget, on_dismiss: Option<Rc<dyn Fn()>>) -> StyleRules {
        let (base, anchor) = match target {
            PortalTarget::Viewport(p) => (viewport_container_rules(*p), None),
            PortalTarget::Anchor { target, side, align, offset } => (
                anchored_container_rules(),
                Some(Anchor {
                    target: target.clone(),
                    placer: AnchoredPlacer::new(*side, *align, *offset, ANCHOR_EDGE_GAP),
                    at: None,
                }),
            ),
            PortalTarget::Named(_) => (StyleRules::default(), None),
        };
        self.order.push(id);
        self.entries.insert(id, Entry { base: base.clone(), on_dismiss, anchor });
        base
    }

    pub(crate) fn remove(&mut self, id: u64) {
        self.entries.remove(&id);
        self.order.retain(|&p| p != id);
    }

    /// Whether any portal is anchored (placement has work to do).
    pub(crate) fn any_anchored(&self) -> bool {
        self.entries.values().any(|e| e.anchor.is_some())
    }

    /// Whether `id`'s content may paint and take input: always for a
    /// viewport portal, once placed for an anchored one.
    pub(crate) fn placed(&self, id: u64) -> bool {
        self.entries.get(&id).is_none_or(|e| e.anchor.as_ref().is_none_or(|a| a.at.is_some()))
    }

    /// The `on_dismiss` of the topmost visible portal that has one — what
    /// Escape closes.
    pub(crate) fn topmost_dismiss(&self, visible: impl Fn(u64) -> bool) -> Option<Rc<dyn Fn()>> {
        self.order
            .iter()
            .rev()
            .filter(|&&id| visible(id))
            .find_map(|id| self.entries.get(id)?.on_dismiss.clone())
    }
}

/// Queue a placement turn for the anchored portals, unless one is queued
/// or this IS the placement turn.
pub(crate) fn queue_placement(b: &mut WindowsBackend) {
    if b.portals.placement_queued || b.portals.placing || !b.portals.any_anchored() {
        return;
    }
    b.portals.placement_queued = true;
    let weak = b.self_ref.clone();
    runtime_shared::scheduling::after_ms_detached(0, move || place_anchored(&weak));
}

/// Read every anchored portal's anchor rect (with the backend NOT
/// borrowed — the handle borrows it), then place each portal's content.
fn place_anchored(weak: &Weak<RefCell<WindowsBackend>>) {
    let Some(rc) = weak.upgrade() else { return };
    let targets: Vec<(u64, AnchorTarget)> = {
        let Ok(mut b) = rc.try_borrow_mut() else { return };
        b.portals.placement_queued = false;
        b.portals
            .entries
            .iter()
            .filter_map(|(id, e)| Some((*id, e.anchor.as_ref()?.target.clone())))
            .collect()
    };
    let rects: Vec<(u64, Option<ViewportRect>)> =
        targets.into_iter().map(|(id, t)| (id, t.rect().filter(|r| r.width > 0.0 || r.height > 0.0))).collect();
    let Ok(mut b) = rc.try_borrow_mut() else { return };
    let viewport = b.logical_client_size();
    let mut moved = false;
    for (id, rect) in rects {
        let Some(rect) = rect else { continue };
        let content = b.children.get(&id).and_then(|k| k.first().copied());
        let Some(size) = content.and_then(|c| b.nodes.get(&c)).map(|m| (m.frame.2, m.frame.3)) else { continue };
        if size.0 <= 0.0 || size.1 <= 0.0 {
            continue;
        }
        let Some(anchor) = b.portals.entries.get_mut(&id).and_then(|e| e.anchor.as_mut()) else { continue };
        let p = anchor.placer.place(rect, size, viewport);
        if anchor.at != Some((p.x, p.y)) {
            anchor.at = Some((p.x, p.y));
            moved = true;
        }
    }
    if moved {
        b.portals.placing = true;
        b.apply_anchor_placements();
        b.position_native_children();
        b.portals.placing = false;
        b.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::primitives::portal::ViewportPlacement;

    #[test]
    fn portals_keep_creation_order_and_escape_closes_the_topmost_visible() {
        let mut p = Portals::default();
        let hits = Rc::new(RefCell::new(Vec::new()));
        for id in [3u64, 5, 9] {
            let h = hits.clone();
            p.add(id, &PortalTarget::Viewport(ViewportPlacement::Center), Some(Rc::new(move || h.borrow_mut().push(id))));
        }
        assert_eq!(p.roots().collect::<Vec<_>>(), [3, 5, 9]);
        (p.topmost_dismiss(|_| true).unwrap())();
        (p.topmost_dismiss(|id| id != 9).unwrap())();
        assert_eq!(*hits.borrow(), [9, 5], "the topmost visible one");
        p.remove(5);
        assert_eq!(p.roots().collect::<Vec<_>>(), [3, 9]);
    }

    #[test]
    fn a_viewport_portal_takes_the_shared_container_rules() {
        let mut p = Portals::default();
        let rules = p.add(1, &PortalTarget::Viewport(ViewportPlacement::Bottom), None);
        assert_eq!(rules, viewport_container_rules(ViewportPlacement::Bottom));
        assert!(p.placed(1), "a viewport portal paints at once");
        assert!(!p.any_anchored());
    }
}
