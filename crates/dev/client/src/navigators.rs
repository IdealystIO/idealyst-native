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

use runtime_shared::primitives::navigator::{MountResult, NavigatorControl};
use wire::NodeId;

use crate::OutboundSender;

/// Discriminator for the navigator flavor the wire-replay engine
/// tags a `NodeId` with. Only the kind-agnostic stack/plain navigator
/// exists; "tab bars" and "drawers" are author-side compositions over
/// the generic swap/stack primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigatorKind {
    Stack,
}

/// One navigator's app-side state. Several fields are unread on the
/// structural replay path, hence the `dead_code` allow.
#[allow(dead_code)]
pub struct NavigatorAppState<N: Clone + 'static> {
    pub kind: NavigatorKind,
    pub node: N,
    /// Where path-matched screens mount. For a stack it's the navigator
    /// node itself (no stack reconstruction), so the active screen
    /// renders.
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
    /// `true` = a native handler owns the navigator's body outlet: the
    /// initial screen is attached via `NavigatorOps::navigator_attach_initial`
    /// rather than inserted into a dev-client-managed outlet. `false` =
    /// structural reconstruction. The replayer currently only builds the
    /// `false` form (see `apply_create_navigator`'s state).
    pub native: bool,
}

/// Box<dyn Any> placeholder for unused params slots.
pub fn dummy_params() -> Box<dyn Any> {
    Box::new(())
}
