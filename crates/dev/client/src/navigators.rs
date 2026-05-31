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

/// Discriminator for the three navigator flavors the framework
/// previously supported. Retained so the wire-replay engine can
/// still tag what kind of navigator a `NodeId` references — the
/// dispatch itself is stubbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigatorKind {
    Stack,
    Tab,
    Drawer,
}

/// One navigator's app-side state — kept as a struct shell so
/// existing call sites continue to type-check while the wire
/// navigator path is rewritten. Most fields are unused in this
/// transitional state.
#[allow(dead_code)]
pub struct NavigatorAppState<N: Clone + 'static> {
    pub kind: NavigatorKind,
    pub node: N,
    /// Where path-matched screens mount. For a drawer this is the
    /// dedicated body-outlet view beside the sidebar; for stack/tab
    /// (full Phase-7 reconstruction still pending) it's the navigator
    /// node itself, so the active screen at least renders.
    pub outlet: N,
    /// The drawer's persistent sidebar column. `None` for stack/tab.
    pub sidebar_slot: Option<N>,
    pub control: Rc<NavigatorControl>,
    pub pending_mount: Rc<RefCell<Option<MountResult<N>>>>,
    pub suppress_release: Rc<RefCell<bool>>,
    pub outbound: OutboundSender,
    pub navigator_id: NodeId,
    pub initial_path: String,
    pub mounted_urls: Rc<RefCell<Vec<String>>>,
    pub replay_pos: Rc<RefCell<usize>>,
}

/// Box<dyn Any> placeholder for unused params slots.
pub fn dummy_params() -> Box<dyn Any> {
    Box::new(())
}
