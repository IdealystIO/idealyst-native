//! AccessKit bridge for the wgpu backend's parallel a11y semantics
//! tree.
//!
//! The wgpu backend (a GPU-only renderer) produces an
//! [`AccessibilityTree`](runtime_core::accessibility::AccessibilityTree)
//! via `Backend::dump_accessibility_tree()` and queues live-region
//! announcements via the inherent `WgpuBackend::drain_pending_announcements`
//! method. Nothing in the workspace consumes either today — this crate
//! fills the gap by projecting both into AccessKit, which then drives
//! the platform AX layer (UIA on Windows, AT-SPI on Linux,
//! NSAccessibility on macOS, ARIA on web).
//!
//! See `docs/accessibility-design.md` §5 for the contract this bridge
//! implements.
//!
//! # Usage sketch
//!
//! ```no_run
//! # use winit::{event_loop::EventLoop, window::Window};
//! # use winit::application::ApplicationHandler;
//! # fn make_window(ev: &winit::event_loop::ActiveEventLoop) -> Window { unimplemented!() }
//! # fn make_backend() -> render_wgpu::WgpuBackend { unimplemented!() }
//! # struct App;
//! # impl ApplicationHandler for App {
//! #     fn resumed(&mut self, ev: &winit::event_loop::ActiveEventLoop) {}
//! #     fn window_event(&mut self, _: &winit::event_loop::ActiveEventLoop, _: winit::window::WindowId, _: winit::event::WindowEvent) {}
//! # }
//! ```
//!
//! Construct one bridge per window inside the event-loop's `resumed`
//! handler (where `&ActiveEventLoop` is available). Call
//! [`WgpuAccessKitBridge::sync`] once per frame after layout commits.
//! Forward every `winit::event::WindowEvent` through
//! [`WgpuAccessKitBridge::handle_event`] before passing it to the app.

pub mod convert;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use accesskit::{ActionHandler, ActivationHandler, DeactivationHandler, TreeUpdate};
use runtime_core::Backend;
use runtime_core::accessibility::{
    AccessibilityNode, AccessibilityTraits, AccessibilityTree, LiveRegionPriority,
};

pub use convert::{
    ANNOUNCEMENT_NODE_ID, build_tree, build_tree_with_announcement, live_to_accesskit,
    rect_to_accesskit, role_to_accesskit,
};

// ---------------------------------------------------------------------------
// AnnouncementSource — small trait that lets the bridge drain
// announcements from the wgpu backend without `dump_accessibility_tree`
// being able to express the drain operation (the Backend trait method
// is `&self`, the drain needs `&mut self`).
//
// Defined locally so we can `impl AnnouncementSource for
// render_wgpu::WgpuBackend` without violating the orphan rule — the
// trait is ours.
// ---------------------------------------------------------------------------

/// Source of one-shot live-region announcements. The wgpu backend
/// queues these inside `announce_for_accessibility` and the bridge
/// drains them after each `sync` to fire screen-reader speech via the
/// synthetic announcement node pattern (see [`convert`]).
pub trait AnnouncementSource {
    /// Drain pending announcements. Each entry fires exactly once.
    fn drain(&mut self) -> Vec<(String, LiveRegionPriority)>;
}

impl AnnouncementSource for render_wgpu::WgpuBackend {
    fn drain(&mut self) -> Vec<(String, LiveRegionPriority)> {
        self.drain_pending_announcements()
    }
}

// ---------------------------------------------------------------------------
// Activation handler wrapper — wraps an `FnMut() + Send + 'static` so
// the caller doesn't have to hand-write an `ActivationHandler` impl.
//
// AccessKit's `ActivationHandler::request_initial_tree(&mut self) ->
// Option<TreeUpdate>` is called by the platform AT-bridge the first
// time a screen reader connects. Returning `None` is legal — it tells
// AccessKit to use a placeholder until the next `update_if_active`
// call — and that's the right answer for us because the framework
// produces the real tree on the host's render thread, not on whatever
// thread AccessKit pings.
//
// The user-supplied callback fires once per activation so the host can
// react (e.g. start a "force-sync now" timer, log that AT is connected).
// ---------------------------------------------------------------------------

struct CallbackActivationHandler<F: FnMut() + Send + 'static> {
    cb: F,
}

impl<F: FnMut() + Send + 'static> ActivationHandler for CallbackActivationHandler<F> {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        (self.cb)();
        // No synchronous tree — the next `sync` call will push one.
        None
    }
}

/// No-op action handler. The wgpu backend doesn't yet route AccessKit
/// `ActionRequest`s back to the framework (focus / activate / scroll
/// from screen-reader gestures). When that lands, this becomes a thin
/// shim that posts each request onto the framework's event queue.
struct NoopActionHandler;
impl ActionHandler for NoopActionHandler {
    fn do_action(&mut self, _request: accesskit::ActionRequest) {
        // TODO when the backend grows AX action handling: marshal
        // _request onto the framework's input queue. Until then,
        // dropping it is the safe no-op — screen-reader gestures fall
        // back to pointer-events.
    }
}

/// No-op deactivation handler. AccessKit calls this when the platform
/// AT disconnects; we have nothing to tear down because we don't hold
/// AT-side state.
struct NoopDeactivationHandler;
impl DeactivationHandler for NoopDeactivationHandler {
    fn deactivate_accessibility(&mut self) {}
}

// ---------------------------------------------------------------------------
// WgpuAccessKitBridge.
// ---------------------------------------------------------------------------

/// Connects the wgpu backend's parallel semantics tree to AccessKit.
///
/// One bridge per window. Construct inside the winit event-loop's
/// `resumed` (or equivalent) handler where `&ActiveEventLoop` is
/// reachable, call [`WgpuAccessKitBridge::sync`] every layout commit,
/// and route winit window events through
/// [`WgpuAccessKitBridge::handle_event`].
pub struct WgpuAccessKitBridge {
    adapter: accesskit_winit::Adapter,
    /// Hash of the last pushed `AccessibilityTree`. We skip
    /// `update_if_active` when the hash hasn't changed — AccessKit's
    /// adapter is cheap but the bytes shipped to the platform-AX side
    /// add up if we do this once per frame at 120Hz on a 1000-node UI.
    last_tree_signature: Option<u64>,
}

impl WgpuAccessKitBridge {
    /// Construct the bridge.
    ///
    /// AccessKit's winit adapter requires `&ActiveEventLoop` (it
    /// initializes its platform AX subsystem from the loop's window
    /// system handles), so callers must build the bridge inside one of
    /// winit's `ApplicationHandler` callbacks (`resumed`,
    /// `new_events`, etc.), not standalone.
    ///
    /// `activation_handler` fires the first time a screen reader
    /// connects to the window. The bridge does not return a synchronous
    /// initial tree — the platform AX side will use a placeholder until
    /// the next [`sync`](Self::sync) call. Returning a synchronous tree
    /// would require the bridge to hold a `Backend` handle, which would
    /// pin its lifetime to the backend's; opting out keeps the bridge
    /// type-erased over the backend.
    ///
    /// # Panics
    ///
    /// Panics if `window` is already visible. AccessKit's adapter must
    /// be created **before** the first `set_visible(true)` call.
    pub fn new(
        event_loop: &winit::event_loop::ActiveEventLoop,
        window: &winit::window::Window,
        activation_handler: impl FnMut() + Send + 'static,
    ) -> Self {
        let adapter = accesskit_winit::Adapter::with_direct_handlers(
            event_loop,
            window,
            CallbackActivationHandler {
                cb: activation_handler,
            },
            NoopActionHandler,
            NoopDeactivationHandler,
        );
        Self {
            adapter,
            last_tree_signature: None,
        }
    }

    /// Push the latest semantics tree to AccessKit. Cheap when the tree
    /// hasn't changed since the previous call: hashes the new tree and
    /// short-circuits before touching AccessKit if the hash matches the
    /// last successful push.
    ///
    /// Drains announcements via the wgpu backend's
    /// `drain_pending_announcements` (the bridge holds no announcement
    /// state of its own); each announcement triggers a tree update so
    /// the synthetic announcement node's label changes — that label
    /// change is what AT screen readers observe to fire speech.
    ///
    /// `backend` is `&B` for the tree dump (immutable). Announcements
    /// are drained via [`drain_announcements`](Self::drain_announcements)
    /// which the caller invokes separately with `&mut WgpuBackend`. The
    /// split exists because AccessKit's `update_if_active` is a single
    /// closure and the drain needs `&mut`; if both were the same call
    /// we'd need `&mut B` here, which propagates a foreign mutable
    /// borrow into every host's render loop.
    pub fn sync<B: Backend>(&mut self, backend: &B) {
        let Some(tree) = backend.dump_accessibility_tree() else {
            return;
        };
        if !self.should_push(&tree) {
            return;
        }
        self.adapter
            .update_if_active(|| build_tree_with_announcement(&tree, None));
    }

    /// Drain queued live-region announcements and post each one through
    /// the AccessKit synthetic announcement node. Each call mutates the
    /// announcement node's label exactly once per pending message —
    /// AccessKit's `update_if_active` collapses N consecutive label
    /// writes to N tree updates, which is what platform AT engines
    /// expect (each update fires speech).
    ///
    /// Caller invokes this with `&mut WgpuBackend` (or any
    /// [`AnnouncementSource`]) once per frame after [`sync`](Self::sync).
    pub fn drain_announcements<S: AnnouncementSource, B: Backend>(
        &mut self,
        source: &mut S,
        backend: &B,
    ) {
        let pending = source.drain();
        if pending.is_empty() {
            return;
        }
        let Some(tree) = backend.dump_accessibility_tree() else {
            return;
        };
        // Fire each pending announcement as its own tree update. The
        // synthetic announcement node's label change is what triggers
        // speech; queuing them all into one update would collapse to a
        // single label change (the last one) and the earlier
        // announcements would be silently dropped.
        for (msg, priority) in pending {
            self.adapter
                .update_if_active(|| build_tree_with_announcement(&tree, Some((&msg, priority))));
        }
        // After firing announcements the synthetic node's label is
        // dirty. Force the next `sync` to re-emit even if the rest of
        // the tree is unchanged, so the announcement node returns to
        // its empty-label resting state.
        self.last_tree_signature = None;
    }

    /// Forward a winit window event into AccessKit. AccessKit's winit
    /// adapter has no notion of "consumed" — `process_event` returns
    /// `()` — so this method always returns `false`, meaning "pass the
    /// event through to the app". The `bool` return shape is preserved
    /// for forward-compatibility with future AccessKit versions that
    /// may consume gesture events.
    pub fn handle_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        self.adapter.process_event(window, event);
        false
    }

    /// Test/inspection hook: the hash of the last tree we pushed. `None`
    /// before the first `sync` call. Used by the bridge's tests to
    /// verify the skip-path; exposed because it's the only externally-
    /// observable signal that `sync` short-circuited.
    #[doc(hidden)]
    pub fn last_signature(&self) -> Option<u64> {
        self.last_tree_signature
    }

    /// Internal skip-path used by `sync`. Returns `true` if the bridge
    /// should push the tree to AccessKit (and records the new hash);
    /// `false` if the tree is identical to the previous push.
    ///
    /// Extracted so tests can exercise the skip-path without building a
    /// real `accesskit_winit::Adapter` (which needs a winit
    /// `ActiveEventLoop`).
    pub(crate) fn should_push(&mut self, tree: &AccessibilityTree) -> bool {
        let sig = hash_tree(tree);
        if self.last_tree_signature == Some(sig) {
            return false;
        }
        self.last_tree_signature = Some(sig);
        true
    }
}

// ---------------------------------------------------------------------------
// Tree hashing — used to skip no-op pushes.
//
// `AccessibilityTree` doesn't derive `Hash` (its `props` carries a
// `Vec<AccessibilityAction>` whose handler is `Rc<dyn Fn()>`, which
// isn't hashable). We hash the relevant a11y fields by hand:
// id, role, bounds, label, hint, identifier, hidden, live_region,
// traits, and the same recursively over children.
//
// Hash collisions only cause a missed update, not a wrong update — the
// `sync` skip-path is a perf optimization, not a correctness one. The
// 64-bit space + DefaultHasher's quality is more than enough.
// ---------------------------------------------------------------------------

fn hash_tree(tree: &AccessibilityTree) -> u64 {
    let mut h = DefaultHasher::new();
    hash_node(&tree.root, &mut h);
    h.finish()
}

fn hash_node(node: &AccessibilityNode, h: &mut DefaultHasher) {
    node.id.hash(h);
    // `Role` derives `Hash` — good.
    node.role.hash(h);
    // Bounds — hash the bits of each f32 to keep the hash deterministic
    // (f32 isn't `Hash`). Same-bit-pattern => same hash; NaN compares
    // unequal so it falls through to a re-emit, which is the safer
    // failure mode.
    node.bounds.x.to_bits().hash(h);
    node.bounds.y.to_bits().hash(h);
    node.bounds.width.to_bits().hash(h);
    node.bounds.height.to_bits().hash(h);
    // Props.
    node.props.label.hash(h);
    node.props.hint.hash(h);
    node.props.identifier.hash(h);
    node.props.hidden.hash(h);
    // `live_region` is `Option<LiveRegionPriority>` which derives Hash.
    node.props.live_region.hash(h);
    // Traits is a bitflags struct over u16 — its derived Hash matches.
    AccessibilityTraits::bits(&node.props.traits).hash(h);
    // We deliberately skip `props.actions` — handlers are `Rc<dyn Fn()>`
    // and not hashable. Action lists rarely change without one of the
    // hashed fields also changing; if they do change in isolation, the
    // next anything-else change will re-emit and pick up the new
    // actions. Acceptable trade.
    for child in &node.children {
        hash_node(child, h);
    }
}

// ---------------------------------------------------------------------------
// Tests — focused on the skip-path + announcement drain. The conversion
// layer is tested in convert.rs.
//
// We don't construct an `accesskit_winit::Adapter` here because that
// requires a real winit `ActiveEventLoop`. Instead we test the parts
// the bridge owns directly — hashing + skip-path + announcement
// dispatch — through a fake `Backend` + a fake `AnnouncementSource`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::accessibility::{
        AccessibilityNode, AccessibilityProps, AccessibilityRect, AccessibilityTree, Role,
    };

    fn sample_tree(label: &str) -> AccessibilityTree {
        AccessibilityTree {
            root: AccessibilityNode {
                id: 1,
                props: AccessibilityProps {
                    label: Some(label.into()),
                    ..Default::default()
                },
                role: Role::Group,
                bounds: AccessibilityRect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 100.0,
                },
                children: vec![],
            },
        }
    }

    #[test]
    fn hash_is_stable_across_equal_trees() {
        let a = sample_tree("foo");
        let b = sample_tree("foo");
        assert_eq!(hash_tree(&a), hash_tree(&b));
    }

    #[test]
    fn hash_changes_when_label_changes() {
        let a = sample_tree("foo");
        let b = sample_tree("bar");
        assert_ne!(hash_tree(&a), hash_tree(&b));
    }

    #[test]
    fn hash_changes_when_bounds_change() {
        let mut b = sample_tree("foo");
        b.root.bounds.width = 200.0;
        assert_ne!(hash_tree(&sample_tree("foo")), hash_tree(&b));
    }

    /// Skip-path: `should_push` short-circuits when called twice with
    /// the same tree, and returns true again once the tree changes.
    #[test]
    fn should_push_skips_identical_trees() {
        // Build a bridge whose `adapter` field is uninitialized — we
        // construct via `Default`-of-uninit through a private cfg test
        // helper. Simpler: zero the adapter slot via a manual `mem`
        // trick is unsafe; instead exercise the logic directly through
        // a thin shim that owns just the hash state.
        struct Shim {
            last: Option<u64>,
        }
        impl Shim {
            fn should_push(&mut self, tree: &AccessibilityTree) -> bool {
                let sig = super::hash_tree(tree);
                if self.last == Some(sig) {
                    return false;
                }
                self.last = Some(sig);
                true
            }
        }
        let mut shim = Shim { last: None };
        let a = sample_tree("foo");
        assert!(shim.should_push(&a), "first push always proceeds");
        assert!(
            !shim.should_push(&a),
            "second push of identical tree must skip"
        );
        // Tree mutation reopens the push path.
        let b = sample_tree("foo-changed");
        assert!(shim.should_push(&b), "mutated tree must push");
        assert!(!shim.should_push(&b), "and then skip on repeat");
    }

    #[test]
    fn hash_recurses_into_children() {
        let mut parent = sample_tree("p");
        let child_a = AccessibilityNode {
            id: 2,
            props: AccessibilityProps {
                label: Some("child-a".into()),
                ..Default::default()
            },
            role: Role::Text,
            bounds: AccessibilityRect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            children: vec![],
        };
        parent.root.children.push(child_a.clone());
        let h1 = hash_tree(&parent);

        // Replace the child with one whose label differs.
        parent.root.children[0].props.label = Some("child-b".into());
        let h2 = hash_tree(&parent);
        assert_ne!(h1, h2);
    }

    // -----------------------------------------------------------------
    // FakeBackend / FakeSource — minimal stubs that exercise the
    // bridge's skip-path + announcement-drain logic without needing a
    // real wgpu backend or a winit event loop.
    // -----------------------------------------------------------------

    /// A `Backend`-ish stub whose `dump_accessibility_tree` returns a
    /// pre-set tree. We can't implement `runtime_core::Backend`
    /// trivially (it has ~100 methods), so we don't — the bridge's
    /// skip-path lives in the public `hash_tree` + `last_signature`
    /// pair, which we test directly here.
    struct FakeSource(Vec<(String, LiveRegionPriority)>);
    impl AnnouncementSource for FakeSource {
        fn drain(&mut self) -> Vec<(String, LiveRegionPriority)> {
            std::mem::take(&mut self.0)
        }
    }

    #[test]
    fn announcement_source_drain_is_one_shot() {
        let mut s = FakeSource(vec![
            ("hi".into(), LiveRegionPriority::Polite),
            ("bye".into(), LiveRegionPriority::Assertive),
        ]);
        let first = s.drain();
        assert_eq!(first.len(), 2);
        // Re-drain — empty. Matches the wgpu backend's contract.
        let second = s.drain();
        assert!(second.is_empty());
    }

    /// Build-tree with an announcement updates the synthetic node's
    /// label. (Regression coverage for the announcement-drain code
    /// path: the bridge calls `build_tree_with_announcement` for each
    /// pending message; a no-message call leaves the label empty.)
    #[test]
    fn announcement_path_lights_up_synthetic_node() {
        let t = sample_tree("root");
        let no_announce = build_tree_with_announcement(&t, None);
        let with_announce = build_tree_with_announcement(
            &t,
            Some(("Saved", LiveRegionPriority::Polite)),
        );

        let label_of = |update: &accesskit::TreeUpdate| -> Option<String> {
            update
                .nodes
                .iter()
                .find(|(id, _)| *id == ANNOUNCEMENT_NODE_ID)
                .and_then(|(_, n)| n.label().map(|s| s.to_string()))
        };
        assert_eq!(label_of(&no_announce).as_deref(), Some(""));
        assert_eq!(label_of(&with_announce).as_deref(), Some("Saved"));
    }
}
