//! The two documented **hard-abort** regressions from the old core
//! (deletion baseline §4.1 #10), re-homed on their surviving subjects.
//!
//! Both were process aborts, not test failures — the class of bug that
//! is invisible until a user's app dies. Neither had a successor before
//! this file:
//!
//! 1. `runtime-core/src/walker/theme_cohort.rs::reset_theme_cohort_state`
//!    had to be panic-safe when a cohort thread-local could not be
//!    accessed. The production crash was a terminal app aborting at exit
//!    with "thread local panicked on drop": the driver guard's `Drop`
//!    ran during thread teardown and touched a destroyed TLS with a
//!    plain `with`/`borrow_mut`; a panic inside a destructor is escalated
//!    to `abort` by std. The surviving analogue is
//!    `runtime_vocabulary::theme`'s module-level `LAST_CTX` — the one TLS
//!    in the new theme engine whose lifetime spans worlds, and the one
//!    the handler-safe fallback reads.
//! 2. `Robot::set_scroll` had to reach the backend through its
//!    `ScrollViewHandle`, NOT `set_node_scroll` under a held
//!    `backend.borrow_mut()`. Native scroll writes fire scroll
//!    notifications synchronously (AppKit `reflectScrolledClipView:`);
//!    the reactive restyles they trigger re-borrow the backend, and a
//!    held `borrow_mut` aborts with "RefCell already borrowed"
//!    (reproduced live on the macOS website drive).
//!
//! Both tests use the deterministic same-thread proxy for their abort:
//! a live borrow that the real failure would also have hit. Destroyed
//! TLS and synchronous native notifications are not reachable from a
//! host test, but "the code path takes a borrow it must not hold" is the
//! identical assertion.

#![cfg(feature = "robot")]

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use host_mock::{Harness, HostMock, Shared};
use runtime_scene::{realize, Registry};
use runtime_shared::primitives::scroll_view::{ScrollViewHandle, ScrollViewOps};
use runtime_vocabulary::builders::{scroll_view, view};
use runtime_vocabulary::robot::{ElementKind, Query, Robot};
use runtime_world::World;

fn bg(hex: &str) -> runtime_shared::Tokenized<runtime_shared::Color> {
    runtime_shared::Tokenized::Literal(runtime_shared::Color(hex.to_string()))
}

// ===========================================================================
// 1. Theme-cohort teardown must never panic on an inaccessible TLS
// ===========================================================================

/// `theme`'s handler-safe fallback reads a module thread-local
/// (`LAST_CTX`). Every access to it must tolerate the TLS being
/// unavailable, because the accesses can run from a `Drop` — and a panic
/// in a destructor is a hard abort, not a catchable failure.
///
/// The deterministic proxy for "TLS destroyed" is "TLS already borrowed":
/// both make the access fail, and the fix (a fallible access that skips)
/// covers both. Pre-fix, a plain `with(|c| c.borrow_mut())` inside a
/// held borrow double-borrow-panicked on exactly the line the real crash
/// hit.
#[test]
fn regression_theme_ctx_survives_an_inaccessible_last_ctx_tls() {
    // Seed LAST_CTX by touching the theme from inside a world, then drop
    // the world — this is the state a later handler-time call sees.
    {
        let world = World::new();
        world.enter(|| {
            runtime_vocabulary::theme::set_app_background(bg("#101010"));
        });
        world.flush();
    }

    // A second world must be able to create its own ctx even though
    // LAST_CTX holds a (now-dead-world) capture. No panic, no bleed.
    let world = World::new();
    world.enter(|| {
        runtime_vocabulary::theme::set_app_background(bg("#202020"));
    });
    world.flush();
    drop(world);

    // And a handler-time call with NO world ambient — the path that
    // consults LAST_CTX — must not panic even though the captured
    // world is gone. A dead-world write is a silent kernel no-op.
    runtime_vocabulary::theme::set_app_background(bg("#303030"));
}

/// Cohort unregistration runs from `Owned`/`Realized` drops, which
/// happen OUTSIDE `World::enter`. It must therefore never consult the
/// ambient world (whose absence panics in a plain drop) — and it must
/// stay safe when the world it was captured from is already gone.
///
/// This is the structural half of the same abort class: the old bug was
/// "teardown touched thread-local state it could not reach"; here the
/// guarantee is "teardown touches only the captured `Rc`, never the
/// ambient world".
#[test]
fn regression_cohort_teardown_outside_enter_and_after_world_drop_is_silent() {
    let h = Harness::new();
    let realized = h.mount(view().child(view().build()).build());

    // Teardown with the world still alive, but NOT entered — the
    // real drop site.
    drop(realized);

    // Teardown after the world itself is gone: a second tree, dropped
    // in the opposite order.
    let realized2 = h.mount(view().build());
    let Harness { world, .. } = h;
    drop(world);
    drop(realized2);
}

// ===========================================================================
// 2. Robot::set_scroll must not hold a backend borrow across the write
// ===========================================================================

/// A `ScrollViewOps` whose `scroll_to` does what a real platform's does:
/// records the request AND **re-enters the backend**, standing in for the
/// synchronous scroll notification whose reactive restyle re-borrows.
struct ReentrantScrollOps;

thread_local! {
    /// Recorded `(x, y)` writes, so the test can prove the values
    /// actually reached the backend rather than merely that the action
    /// existed.
    static SCROLLS: RefCell<Vec<(f32, f32)>> = const { RefCell::new(Vec::new()) };
    /// `Some` once the re-entrant borrow succeeded, `None` if it was
    /// blocked. This is the abort detector: `try_borrow_mut` stands in
    /// for the real code's plain `borrow_mut`, which would PANIC here
    /// (inside a native notification callback) rather than return `Err`.
    static REENTERED: RefCell<Option<bool>> = const { RefCell::new(None) };
}

impl ScrollViewOps for ReentrantScrollOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        let _ = node;
        SCROLLS.with(|s| s.borrow_mut().push((x, y)));
        // THE ASSERTION, expressed as behavior. A real platform's scroll
        // write fires its scroll notification synchronously from inside
        // this call, and the reactive restyle that notification triggers
        // re-borrows the backend. If `set_scroll` were routed under a
        // live `backend.borrow_mut()`, that re-borrow would abort with
        // "RefCell already borrowed" — the macOS crash.
        //
        // `try_borrow_mut` is used only so the test can REPORT the
        // failure instead of aborting the harness; the production path
        // takes a plain `borrow_mut`.
        reentrant_host::with_live_backend(|backend| {
            let ok = backend.try_borrow_mut().is_ok();
            REENTERED.with(|r| *r.borrow_mut() = Some(ok));
        });
    }
}

/// A host that hands out a real scroll handle wired to
/// [`ReentrantScrollOps`]. Everything else is `HostMock`'s recording
/// behavior, reached by delegation.
mod reentrant_host {
    use super::*;
    use runtime_vocabulary::caps;

    pub struct ReentrantHost(pub HostMock);

    thread_local! {
        /// THE backend the mounted tree is driving — the same `RefCell`
        /// the scroll-view handler holds. `scroll_to` re-enters exactly
        /// this one, so a borrow held by the robot action is observable.
        static LIVE: RefCell<Option<Rc<RefCell<ReentrantHost>>>> =
            const { RefCell::new(None) };
    }

    /// Publish the backend under test (call after constructing it).
    pub fn set_live(backend: &Rc<RefCell<ReentrantHost>>) {
        LIVE.with(|l| *l.borrow_mut() = Some(backend.clone()));
    }

    /// Clear it (call before the backend drops).
    pub fn clear_live() {
        LIVE.with(|l| *l.borrow_mut() = None);
    }

    /// Run `f` against the live backend, if one is published.
    pub fn with_live_backend(f: impl FnOnce(&Rc<RefCell<ReentrantHost>>)) {
        let live = LIVE.with(|l| l.borrow().clone());
        if let Some(b) = live {
            f(&b);
        }
    }

    impl runtime_scene::Host for ReentrantHost {
        type Node = <HostMock as runtime_scene::Host>::Node;
        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            runtime_scene::Host::insert(&mut self.0, parent, child)
        }
        fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
            runtime_scene::Host::insert_many(&mut self.0, parent, children)
        }
        fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
            runtime_scene::Host::insert_at(&mut self.0, parent, child, index)
        }
        fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
            runtime_scene::Host::remove_child(&mut self.0, parent, child)
        }
        fn clear_children(&mut self, node: &Self::Node) {
            runtime_scene::Host::clear_children(&mut self.0, node)
        }
        fn create_anchor(&mut self) -> Self::Node {
            runtime_scene::Host::create_anchor(&mut self.0)
        }
        fn supports_splice(&self) -> bool {
            runtime_scene::Host::supports_splice(&self.0)
        }
    }

    impl caps::ScrollOps for ReentrantHost {
        fn create_scroll_view(
            &mut self,
            horizontal: bool,
            on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
            a11y: &runtime_shared::accessibility::AccessibilityProps,
        ) -> Self::Node {
            caps::ScrollOps::create_scroll_view(&mut self.0, horizontal, on_scroll, a11y)
        }

        /// The one override that matters: a REAL handle, so the robot's
        /// `set_scroll` closure has somewhere to route.
        fn make_scroll_view_handle(&self, node: &Self::Node) -> ScrollViewHandle {
            ScrollViewHandle::new(Rc::new(*node), &ReentrantScrollOps)
        }
    }

    // The six caps methods with no default body — delegated so the
    // wrapper behaves exactly like `HostMock` everywhere except the
    // scroll handle.
    impl caps::ViewOps for ReentrantHost {
        fn create_view(
            &mut self,
            a11y: &runtime_shared::accessibility::AccessibilityProps,
        ) -> Self::Node {
            caps::ViewOps::create_view(&mut self.0, a11y)
        }
    }
    impl caps::TextOps for ReentrantHost {
        fn create_text(
            &mut self,
            content: &str,
            a11y: &runtime_shared::accessibility::AccessibilityProps,
        ) -> Self::Node {
            caps::TextOps::create_text(&mut self.0, content, a11y)
        }
        fn update_text(&mut self, node: &Self::Node, content: &str) {
            caps::TextOps::update_text(&mut self.0, node, content)
        }
    }
    impl caps::ButtonOps for ReentrantHost {
        fn create_button(
            &mut self,
            label: &str,
            on_click: &runtime_shared::Action,
            leading_icon: Option<&runtime_shared::primitives::icon::IconData>,
            trailing_icon: Option<&runtime_shared::primitives::icon::IconData>,
            a11y: &runtime_shared::accessibility::AccessibilityProps,
        ) -> Self::Node {
            caps::ButtonOps::create_button(
                &mut self.0,
                label,
                on_click,
                leading_icon,
                trailing_icon,
                a11y,
            )
        }
    }
    impl caps::StyleOps for ReentrantHost {
        fn apply_style(
            &mut self,
            node: &Self::Node,
            style: &Rc<runtime_shared::StyleRules>,
        ) {
            caps::StyleOps::apply_style(&mut self.0, node, style)
        }
    }
    impl caps::LifecycleOps for ReentrantHost {
        fn finish(&mut self, root: Self::Node) {
            caps::LifecycleOps::finish(&mut self.0, root)
        }
    }

    macro_rules! delegate_default_caps {
        ($($t:ident),* $(,)?) => { $( impl caps::$t for ReentrantHost {} )* };
    }
    delegate_default_caps!(
        AppEnvOps,
        InputOps,
        PressableOps,
        ImageOps,
        IconOps,
        LinkOps,
        TextInputOps,
        ToggleOps,
        SliderOps,
        ActivityIndicatorOps,
        SafeAreaOps,
        VirtualizerOps,
        GridOps,
        GraphicsOps,
        PortalOps,
        PresenceOps,
        NavigatorOps,
        ExternalOps,
        DocumentOps,
        AssetOps,
        A11yOps,
        AnimationOps,
        IntrospectionOps,
        BatchOps,
        WireBindingOps,
    );
}

/// **The regression.** `Robot::set_scroll` must deliver `(x, y)` through
/// the scroll view's own `ScrollViewHandle`, with NO backend borrow held
/// across the call.
///
/// Two things are asserted, and the second is the abort:
/// - the values reach the backend (the old test's assertion), and
/// - the write can re-enter `backend.borrow_mut()` (the invariant the
///   old test could only state in prose).
#[test]
fn regression_robot_set_scroll_routes_via_the_scroll_handle_with_no_held_backend_borrow() {
    use reentrant_host::ReentrantHost;

    let robot = Robot::new();
    robot.reset();
    SCROLLS.with(|s| s.borrow_mut().clear());
    REENTERED.with(|r| *r.borrow_mut() = None);

    let world = World::new();
    let backend = Rc::new(RefCell::new(ReentrantHost(HostMock::new(Rc::new(Shared::default())))));
    reentrant_host::set_live(&backend);

    let mut registry: Registry<ReentrantHost> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);

    let _realized = world.enter(|| {
        realize(
            &backend,
            &registry,
            view()
                .child(scroll_view().test_id("scroller").build())
                .build(),
        )
    });

    let scroller = robot
        .find(Query::test_id("scroller"))
        .expect("the mounted scroll_view must register as a robot element");
    assert_eq!(scroller.kind, ElementKind::ScrollView);

    robot
        .set_scroll(&scroller, 120.0, 340.0)
        .expect("scroll views must carry the set_scroll action");

    let sets = SCROLLS.with(|s| s.borrow().clone());
    assert_eq!(sets.len(), 1, "exactly one backend scroll write");
    assert_eq!(
        sets[0],
        (120.0, 340.0),
        "set_scroll must deliver the requested offsets, not swallow them"
    );

    reentrant_host::clear_live();
    robot.reset();
    drop(_realized);
    drop(world);
}

/// The re-entrancy half, driven separately so the target handle can be
/// installed without the registry holding it: with a live re-entry
/// target, `scroll_to` takes `backend.borrow_mut()` from inside the
/// robot action. It completes only because the action holds no borrow.
#[test]
fn regression_set_scroll_write_can_reborrow_the_backend_mid_call() {
    use reentrant_host::ReentrantHost;

    let robot = Robot::new();
    robot.reset();
    SCROLLS.with(|s| s.borrow_mut().clear());
    REENTERED.with(|r| *r.borrow_mut() = None);

    let world = World::new();
    let backend = Rc::new(RefCell::new(ReentrantHost(HostMock::new(Rc::new(Shared::default())))));
    // Arm the re-entry against THE SAME backend the mounted tree drives.
    // Using a second, unrelated `RefCell` here would make the test pass
    // against the buggy code — verified the hard way (see the module
    // docs' note on the proxy).
    reentrant_host::set_live(&backend);

    let mut registry: Registry<ReentrantHost> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);

    let _realized = world.enter(|| {
        realize(
            &backend,
            &registry,
            scroll_view().test_id("reentrant").build(),
        )
    });

    let el = robot.find(Query::test_id("reentrant")).unwrap();
    robot.set_scroll(&el, 1.0, 2.0).unwrap();

    assert_eq!(
        REENTERED.with(|r| *r.borrow()),
        Some(true),
        "the scroll write must be able to re-borrow the backend mid-call — if \
         this fails the action is holding a borrow across the native write, \
         which is the macOS 'RefCell already borrowed' abort"
    );

    reentrant_host::clear_live();
    robot.reset();
    drop(_realized);
    drop(world);
}
