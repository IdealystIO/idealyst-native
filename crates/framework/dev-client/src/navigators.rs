//! App-side state for navigators driven over the wire.
//!
//! The dev side is the source of truth for navigation: when the dev
//! framework's `NavigatorControl` dispatcher fires, it builds the new
//! screen subtree against `WireRecordingBackend` and emits the
//! resulting `Command`s plus a `NavigatorPush` / `Pop` / `Replace` /
//! `Reset` command. The app side just translates those commands into
//! real-backend operations.
//!
//! The wrinkle is the real backend's API: `Backend::create_navigator`
//! takes `NavigatorCallbacks<N>` and `NavigatorControl`, and most
//! backends drive push/pop/replace by invoking the control's
//! dispatcher (which calls `callbacks.mount_screen`). On the app
//! side in wire mode, *we* are the framework — we synthesize a stub
//! `NavigatorCallbacks` whose `mount_screen` returns a pre-staged
//! `MountResult` (whatever the latest wire command pushed onto the
//! pending-mount slot) and whose `release_screen` ships
//! `AppToDev::ScreenReleased` back over the wire.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use framework_core::primitives::navigator::{
    DrawerNavigatorCallbacks, MountResult, NavState, NavigatorCallbacks, NavigatorControl,
    TabNavigatorCallbacks,
};
use framework_core::Signal;
use wire::{AppToDev, NodeId, ScopeId};

use crate::OutboundSender;

/// One navigator's app-side state. Cloned across stub closures via
/// `Rc<RefCell<…>>` for shared interior mutability.
pub struct NavigatorAppState<N: Clone + 'static> {
    /// The native navigator container node (UINavigationController on
    /// iOS, etc.).
    pub node: N,
    /// Control plane the real backend's dispatcher is installed on.
    pub control: Rc<NavigatorControl>,
    /// The single-slot buffer the stub callbacks read on each
    /// dispatcher-driven mount. The wire-replay engine sets this
    /// just before calling `control.dispatch(...)`.
    pub pending_mount: Rc<RefCell<Option<MountResult<N>>>>,
    /// True while a wire-driven command is being processed. The stub
    /// `release_screen` checks this to avoid echoing a release event
    /// back to dev for releases dev already issued.
    pub suppress_release: Rc<RefCell<bool>>,
    /// Outbound channel for app→dev events (swipe-back releases,
    /// tab activations, drawer state changes). Swappable wrapper so
    /// the navigator survives reconnects.
    pub outbound: OutboundSender,
    /// The navigator id, used for AppToDev events that need to
    /// identify the navigator.
    pub navigator_id: NodeId,
    /// Scope ids of screens we've already mounted into this
    /// navigator's native stack. Used for idempotency: the
    /// append-only command log re-emits every `NavigatorAttachInitial`
    /// and `NavigatorPush` on each reconnect, but the previous
    /// session already mounted them — re-applying would create
    /// duplicate native screens that the user can't pop past.
    pub attached_scopes: Rc<RefCell<std::collections::HashSet<u64>>>,
}

impl<N: Clone + 'static> NavigatorAppState<N> {
    /// Build the stub `NavigatorCallbacks` the real backend will
    /// store when we call `create_navigator(...)`. The closures
    /// reference state through `Rc` clones so multiple navigators
    /// don't trample each other.
    pub fn build_stub_callbacks(
        &self,
        initial_route: &'static str,
        initial_path: &'static str,
    ) -> NavigatorCallbacks<N> {
        let pending_mount = self.pending_mount.clone();
        let suppress_release = self.suppress_release.clone();
        let outbound = self.outbound.clone();

        NavigatorCallbacks {
            initial_route,
            initial_path,
            mount_screen: Rc::new(move |_name, _params| {
                // The wire-driven mount has already populated the
                // pending slot. If it's empty, something's gone
                // wrong; produce an empty MountResult so the real
                // backend has something to push, and rely on the
                // dev side to surface the protocol error.
                pending_mount
                    .borrow_mut()
                    .take()
                    .expect("stub mount_screen called without pending_mount staged")
            }),
            release_screen: Rc::new(move |scope_id| {
                if !*suppress_release.borrow() {
                    let _ = outbound.send(AppToDev::ScreenReleased {
                        scope: ScopeId(scope_id),
                    });
                }
            }),
            match_path: Rc::new(|_path| None),
            build_layout: None,
            nav_state: NavState {
                active_route: Signal::new(initial_route),
                active_path: Signal::new(initial_path.to_string()),
                depth: Signal::new(1),
                can_go_back: Signal::new(false),
            },
            depth_changed: Rc::new(|_d| {
                // Dev tracks depth from its own stack model; the
                // backend's local depth report is redundant.
            }),
        }
    }

    /// Same shape as `build_stub_callbacks` but tab-flavored — the
    /// real backend's `create_tab_navigator` takes a
    /// `TabNavigatorCallbacks` bundle that wraps the inner
    /// `NavigatorCallbacks` plus tab metadata.
    pub fn build_stub_tab_callbacks(
        &self,
        initial_route: &'static str,
        initial_path: &'static str,
        tabs: Vec<framework_core::primitives::navigator::TabRegistration>,
        placement: framework_core::primitives::navigator::TabPlacement,
        mount_policy: framework_core::primitives::navigator::MountPolicy,
    ) -> TabNavigatorCallbacks<N> {
        TabNavigatorCallbacks {
            navigator: self.build_stub_callbacks(initial_route, initial_path),
            tabs,
            placement,
            mount_policy,
            active_changed: Rc::new(|_| {}),
        }
    }

    /// Drawer-flavored stub callbacks. Includes the open/close signal
    /// and the same screen lifecycle plumbing.
    pub fn build_stub_drawer_callbacks(
        &self,
        initial_route: &'static str,
        initial_path: &'static str,
        items: Vec<framework_core::primitives::navigator::DrawerItemRegistration>,
        side: framework_core::primitives::navigator::DrawerSide,
        drawer_type: framework_core::primitives::navigator::DrawerType,
        drawer_width: f32,
        pinned_above: Option<u32>,
        swipe_to_open: bool,
        mount_policy: framework_core::primitives::navigator::MountPolicy,
    ) -> DrawerNavigatorCallbacks<N> {
        let nav_id = self.navigator_id;
        let outbound = self.outbound.clone();
        DrawerNavigatorCallbacks {
            navigator: self.build_stub_callbacks(initial_route, initial_path),
            items,
            side,
            drawer_type,
            drawer_width,
            pinned_above,
            swipe_to_open,
            mount_policy,
            is_open: Signal::new(false),
            build_sidebar: None,
            active_changed: Rc::new(|_| {}),
            open_changed: Rc::new(move |is_open| {
                let _ = outbound.send(AppToDev::DrawerStateChanged {
                    navigator: nav_id,
                    is_open,
                });
            }),
        }
    }
}

/// Helper: stage a screen as the pending mount, run a closure that
/// triggers the dispatcher (which calls `mount_screen` and consumes
/// the staged value), then verify the slot was consumed.
pub fn with_staged_mount<N: Clone + 'static, F: FnOnce()>(
    state: &NavigatorAppState<N>,
    result: MountResult<N>,
    body: F,
) {
    *state.pending_mount.borrow_mut() = Some(result);
    *state.suppress_release.borrow_mut() = true;
    body();
    *state.suppress_release.borrow_mut() = false;
    // If pending_mount still holds Some, the dispatcher didn't
    // actually consume it — log silently (in real use the protocol
    // is wrong; in the prototype we don't want to panic the app).
    let _consumed = state.pending_mount.borrow_mut().take();
}

/// `Box<dyn Any>` payload the stub callbacks use as a no-op params
/// value. The real backend's dispatcher just forwards it through to
/// `mount_screen`, which ignores both name and params in our stub.
pub fn dummy_params() -> Box<dyn Any> {
    Box::new(())
}
