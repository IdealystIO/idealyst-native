//! Terminal-backend handler for the Stack navigator SDK.
//!
//! Minimalist by design (see `[[feedback_terminal_minimalism]]`): no
//! per-screen header chrome, no push/pop animation, no slot styling.
//! The handler owns a single outlet `View` node; Push detaches the
//! current screen and inserts the new one, Pop reverses, Replace /
//! Reset behave the same way modulo what they do to the saved
//! history.
//!
//! Why no auto-rendered header bar: the terminal renders to a flat
//! character grid and any auto-injected header would steal rows from
//! the page content — pages can build their own top bar inside their
//! screen Element if they want one.

use crate::StackPresentation;
use backend_terminal::{TermNode, TerminalBackend};
use runtime_core::primitives::navigator::{
    NavCommand, NavigatorControl, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::Backend;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

/// One frame on the navigator's internal stack. We retain the
/// screen's `TermNode` (so Pop can re-attach it without remounting)
/// plus the `scope_id` we got from `mount_screen` (so Pop can release
/// the popped scope and Reset can release every frame).
struct ScreenEntry {
    node: TermNode,
    scope_id: u64,
}

pub struct TerminalStackHandler {
    /// Outlet `View` returned by `init`. Subsequent screens are
    /// attached / detached as children of this node.
    outlet: Option<TermNode>,
    /// Frame stack. Last entry is the currently-attached screen.
    stack: Rc<RefCell<Vec<ScreenEntry>>>,
    /// Shared control plane. Captured from `NavigatorHost` in `init` so
    /// `make_handle` can hand author code a handle whose `dispatch`
    /// actually reaches the SDK dispatcher. Without this, `nav.push()`
    /// is a silent no-op because `NavigatorHandle::new` builds a
    /// control-less handle.
    control: Option<Rc<NavigatorControl>>,
}

impl TerminalStackHandler {
    pub fn new() -> Self {
        Self {
            outlet: None,
            stack: Rc::new(RefCell::new(Vec::new())),
            control: None,
        }
    }
}

impl Default for TerminalStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopStackOps;
impl NavigatorOps for NoopStackOps {}

impl NavigatorHandler<TerminalBackend> for TerminalStackHandler {
    fn init(
        &mut self,
        backend: &mut TerminalBackend,
        host: NavigatorHost<TermNode>,
        _presentation: Rc<dyn Any>,
    ) -> TermNode {
        let outlet = backend.create_view(&Default::default());
        self.outlet = Some(outlet);
        self.control = Some(host.control.clone());

        // Install the command dispatcher. Mount/detach work needs the
        // backend handle, which closures don't carry directly — the
        // backend's `install_global_self` weak handle is the canonical
        // way for closures to call back into it (same pattern the
        // toggle-press and spinner-advance paths use).
        let stack_rc = self.stack.clone();
        let outlet_id = outlet;
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let depth_changed = host.depth_changed.clone();

        host.control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, params, .. } => {
                let result = mount_screen(name, params, None);
                with_backend(|b| {
                    if let Some(top) = stack_rc.borrow().last() {
                        b.detach_child(&outlet_id, &top.node);
                    }
                    let mut outlet = outlet_id;
                    b.insert(&mut outlet, result.node);
                });
                stack_rc.borrow_mut().push(ScreenEntry {
                    node: result.node,
                    scope_id: result.scope_id,
                });
                depth_changed(stack_rc.borrow().len());
                let _ = name; // route name is tracked by substrate
            }
            NavCommand::Pop => {
                let popped = stack_rc.borrow_mut().pop();
                let Some(popped) = popped else { return };
                with_backend(|b| {
                    b.detach_child(&outlet_id, &popped.node);
                    if let Some(prev) = stack_rc.borrow().last() {
                        let mut outlet = outlet_id;
                        b.insert(&mut outlet, prev.node);
                    }
                });
                release_screen(popped.scope_id);
                depth_changed(stack_rc.borrow().len());
            }
            NavCommand::Replace { name, params, .. } => {
                let result = mount_screen(name, params, None);
                let popped = stack_rc.borrow_mut().pop();
                with_backend(|b| {
                    if let Some(ref prev) = popped {
                        b.detach_child(&outlet_id, &prev.node);
                    }
                    let mut outlet = outlet_id;
                    b.insert(&mut outlet, result.node);
                });
                if let Some(prev) = popped {
                    release_screen(prev.scope_id);
                }
                stack_rc.borrow_mut().push(ScreenEntry {
                    node: result.node,
                    scope_id: result.scope_id,
                });
                let _ = name;
            }
            NavCommand::Reset { name, params, .. } => {
                let result = mount_screen(name, params, None);
                // Drain prior frames, detach + release each.
                let drained: Vec<ScreenEntry> =
                    stack_rc.borrow_mut().drain(..).collect();
                with_backend(|b| {
                    for entry in &drained {
                        b.detach_child(&outlet_id, &entry.node);
                    }
                    let mut outlet = outlet_id;
                    b.insert(&mut outlet, result.node);
                });
                for entry in drained {
                    release_screen(entry.scope_id);
                }
                stack_rc.borrow_mut().push(ScreenEntry {
                    node: result.node,
                    scope_id: result.scope_id,
                });
                depth_changed(stack_rc.borrow().len());
                let _ = name;
            }
            NavCommand::Select { .. } | NavCommand::Custom(_) => {
                // The stack navigator's command vocabulary is
                // Push / Pop / Replace / Reset only. Same posture as
                // the web/iOS/Android handlers.
                panic!(
                    "stack Navigator received a non-stack NavCommand — \
                     check that the dispatched command's shape matches \
                     the navigator kind (stack: Push/Pop/Replace/Reset)"
                );
            }
        }));

        outlet
    }

    fn attach_initial(
        &mut self,
        backend: &mut TerminalBackend,
        screen: TermNode,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        let Some(mut outlet) = self.outlet else { return };
        backend.insert(&mut outlet, screen);
        self.stack.borrow_mut().push(ScreenEntry {
            node: screen,
            scope_id,
        });
    }

    fn release(&mut self, _backend: &mut TerminalBackend) {
        // Framework drops the navigator's enclosing scope, which
        // releases every screen scope under it. We just clear our
        // local bookkeeping; the outlet `TermNode` itself is owned by
        // the layout tree and will be dropped when its parent is.
        self.stack.borrow_mut().clear();
        self.outlet = None;
        self.control = None;
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        match self.control.as_ref() {
            Some(c) => runtime_core::NavigatorHandle::with_control(
                Rc::new(()),
                &NOOP_STACK_OPS,
                c.clone(),
            ),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_STACK_OPS),
        }
    }
}

static NOOP_STACK_OPS: NoopStackOps = NoopStackOps;

/// Reach into the backend via the global self-handle the host
/// installed at startup. Closures created in `init` don't carry
/// `&mut TerminalBackend`, but the dispatcher needs to manipulate
/// the layout tree on each NavCommand — `install_global_self` is
/// the canonical bridge (same pattern used by the toggle-press and
/// spinner paths inside `backend-terminal`).
fn with_backend<F: FnOnce(&mut TerminalBackend)>(f: F) {
    backend_terminal::with_global_backend(|b| f(b));
}

pub fn register(backend: &mut TerminalBackend) {
    backend.register_navigator::<StackPresentation, _>(|| {
        Box::new(TerminalStackHandler::new())
    });
}
