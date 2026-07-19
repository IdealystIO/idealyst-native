//! App-side state for navigators driven over the wire.
//!
//! The runtime-core navigator substrate was refactored to an
//! SDK-handler dispatch model; the previous callback-driven stub
//! infrastructure that this module exposed has been gutted pending
//! the rewrite. The struct + enum kept here are minimal scaffolding
//! so the rest of dev-client still compiles. The pre-refactor
//! wire-driven mount/dispatch lifecycle is no-op until the new
//! navigator wire protocol lands.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::primitives::navigator::{MountResult, NavigatorControl};
use wire::NodeId;

use crate::OutboundSender;

/// Discriminator for the navigator flavor the wire-replay engine
/// tags a `NodeId` with. Only the kind-agnostic stack/plain navigator
/// remains — the legacy per-kind tab/drawer navigators (and their wire
/// commands) were removed; "tab bars" and "drawers" are now author-side
/// compositions over the generic swap/stack primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigatorKind {
    Stack,
}

/// One navigator's app-side state — kept as a struct shell so
/// existing call sites continue to type-check while the wire
/// navigator path is rewritten. Most fields are unused in this
/// transitional state.
#[allow(dead_code)]
pub struct NavigatorAppState<N: Clone + 'static> {
    pub kind: NavigatorKind,
    pub node: N,
    /// Where path-matched screens mount. For a stack (full Phase-7
    /// reconstruction still pending) it's the navigator node itself,
    /// so the active screen at least renders.
    pub outlet: N,
    /// Screen nodes currently mounted, top of stack = last. The outlet
    /// shows the top. A stack navigator pushes/pops this; a Select
    /// keeps a single entry (the selected screen). Pop re-shows the new
    /// top — the popped node still lives in `nodes`, just detached.
    pub screen_stack: Rc<RefCell<Vec<NodeId>>>,
    pub control: Rc<NavigatorControl>,
    pub pending_mount: Rc<RefCell<Option<MountResult<N>>>>,
    pub suppress_release: Rc<RefCell<bool>>,
    pub outbound: OutboundSender,
    pub navigator_id: NodeId,
    pub initial_path: String,
    pub mounted_urls: Rc<RefCell<Vec<String>>>,
    pub replay_pos: Rc<RefCell<usize>>,
    /// `true` when this navigator was reconstructed by driving the
    /// client's REAL backend `create_navigator` (the registered SDK
    /// handler builds native chrome). In that mode the initial screen
    /// is attached via `Backend::navigator_attach_initial` rather than
    /// inserted into a dev-client-managed outlet. `false` = the
    /// structural-reconstruction fallback (no handler registered).
    pub native: bool,
    /// Reactive scopes for chrome subtrees materialized via
    /// `runtime_core::build_detached` (native mode only). Retained so
    /// the External cleanup Effects + theme subscriptions created during
    /// the detached build survive past `build_node`'s return; dropping a
    /// scope disposes its subtree's reactive state. Empty in structural
    /// mode (no detached builds there).
    pub chrome_scopes: Rc<RefCell<Vec<runtime_core::DetachedScope>>>,
}

/// Box<dyn Any> placeholder for unused params slots.
pub fn dummy_params() -> Box<dyn Any> {
    Box::new(())
}
