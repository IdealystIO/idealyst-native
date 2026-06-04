//! macOS-backend handler for the Stack navigator SDK.
//!
//! Per `project_macos_navigator_design`, macOS doesn't ship an
//! animated iOS-style push/pop stack. The outlet swaps its single
//! child on Push/Pop/Replace/Reset — same minimalism the terminal
//! handler ships with. Author code that wants per-screen header
//! chrome builds it inside the screen Element itself.
//!
//! No `macos-navigator-helpers` crate — this handler stays small
//! and uses only the public `Backend` trait surface plus
//! `backend_macos::with_global_backend` for microtask re-entry
//! (same pattern as the macOS drawer-navigator handler).
//!
//! Frame storage: each Push stashes the screen `MacosNode` +
//! `scope_id`; Pop pops the top, clears the outlet, re-inserts
//! the previous frame's node, and fires `release_screen` for the
//! popped scope. Replace pops + replaces; Reset drains every
//! frame and starts fresh.

use crate::StackPresentation;
use backend_macos::{with_global_backend, MacosBackend, MacosNode};
use runtime_core::primitives::navigator::{
    NavCommand, NavigatorControl, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::Backend;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

struct ScreenEntry {
    node: MacosNode,
    scope_id: u64,
}

pub struct MacosStackHandler {
    outlet: Option<MacosNode>,
    stack: Rc<RefCell<Vec<ScreenEntry>>>,
    control: Option<Rc<NavigatorControl>>,
}

impl MacosStackHandler {
    pub fn new() -> Self {
        Self {
            outlet: None,
            stack: Rc::new(RefCell::new(Vec::new())),
            control: None,
        }
    }
}

impl Default for MacosStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopStackOps;
impl NavigatorOps for NoopStackOps {}
static NOOP_STACK_OPS: NoopStackOps = NoopStackOps;

impl NavigatorHandler<MacosBackend> for MacosStackHandler {
    fn init(
        &mut self,
        backend: &mut MacosBackend,
        host: NavigatorHost<MacosNode>,
        _presentation: Rc<dyn Any>,
    ) -> MacosNode {
        let outlet = backend.create_view(&Default::default());
        self.outlet = Some(outlet.clone());
        self.control = Some(host.control.clone());

        let stack_rc = self.stack.clone();
        let outlet_for_dispatch = outlet.clone();
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let depth_changed = host.depth_changed.clone();

        host.control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, params, .. } => {
                let result = mount_screen(name, params, None);
                with_global_backend(|b| {
                    let mut outlet_node = outlet_for_dispatch.clone();
                    // Outlet only ever holds one child — the top
                    // of stack. clear_children removes from both
                    // AppKit (removeFromSuperview) and Taffy,
                    // mirroring what the terminal handler's
                    // detach_child does in one call.
                    if !stack_rc.borrow().is_empty() {
                        b.clear_children(&outlet_node);
                    }
                    b.insert(&mut outlet_node, result.node.clone());
                });
                stack_rc.borrow_mut().push(ScreenEntry {
                    node: result.node,
                    scope_id: result.scope_id,
                });
                depth_changed(stack_rc.borrow().len());
                let _ = name;
            }
            NavCommand::Pop => {
                let popped = stack_rc.borrow_mut().pop();
                let Some(popped) = popped else { return };
                with_global_backend(|b| {
                    let mut outlet_node = outlet_for_dispatch.clone();
                    b.clear_children(&outlet_node);
                    // Re-insert the previous frame's node so the
                    // visible content reverts to what it was before
                    // the Push that we're popping.
                    if let Some(prev) = stack_rc.borrow().last() {
                        b.insert(&mut outlet_node, prev.node.clone());
                    }
                });
                release_screen(popped.scope_id);
                depth_changed(stack_rc.borrow().len());
            }
            NavCommand::Replace { name, params, .. } => {
                let result = mount_screen(name, params, None);
                let popped = stack_rc.borrow_mut().pop();
                with_global_backend(|b| {
                    let mut outlet_node = outlet_for_dispatch.clone();
                    if popped.is_some() {
                        b.clear_children(&outlet_node);
                    }
                    b.insert(&mut outlet_node, result.node.clone());
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
                let drained: Vec<ScreenEntry> =
                    stack_rc.borrow_mut().drain(..).collect();
                with_global_backend(|b| {
                    let mut outlet_node = outlet_for_dispatch.clone();
                    if !drained.is_empty() {
                        b.clear_children(&outlet_node);
                    }
                    b.insert(&mut outlet_node, result.node.clone());
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
                panic!(
                    "stack Navigator received a non-stack NavCommand on macOS — \
                     check that the dispatched command's shape matches the \
                     navigator kind (stack: Push/Pop/Replace/Reset)"
                );
            }
        }));

        outlet
    }

    fn attach_initial(
        &mut self,
        backend: &mut MacosBackend,
        screen: MacosNode,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        let Some(outlet) = self.outlet.clone() else { return };
        let mut outlet_mut = outlet;
        backend.insert(&mut outlet_mut, screen.clone());
        self.stack.borrow_mut().push(ScreenEntry { node: screen, scope_id });
    }

    fn release(&mut self, _backend: &mut MacosBackend) {
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

/// Install the stack navigator handler on a macOS backend. Call once at
/// startup so `Element::Navigator`s carrying a [`StackPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut MacosBackend) {
    backend.register_navigator::<StackPresentation, _>(|| {
        Box::new(MacosStackHandler::new())
    });
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_macos::MacosNavigatorRegistrar(register)
}
