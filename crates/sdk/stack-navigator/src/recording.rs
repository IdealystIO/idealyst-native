//! Recording handler for the runtime-server sidecar's recorder backend
//! (`dev_server::WireRecordingBackend`).
//!
//! The stack analogue of `drawer_navigator::recording`. Instead of
//! driving a native push/pop stack it emits the navigator wire commands
//! the protocol carries — `CreateNavigator`, `NavigatorAttachInitial`,
//! `NavigatorPush`, `NavigatorPop`, `NavigatorReplace`, `NavigatorReset`
//! — through a [`dev_server::NavRecorder`]. The screen subtrees are
//! recorded as ordinary primitive subtrees; the client replays + mounts
//! them (dev-client reconstruction).
//!
//! Mirrors `terminal.rs`'s dispatcher semantics (Push/Pop/Replace/Reset
//! over a frame stack), but tracks only `scope_id`s — the recorder has
//! no native views to attach/detach, just commands to emit + scopes to
//! release on pop.

use crate::{StackPresentation, StackScreenOptions};
use dev_server::{NavRecorder, WireRecordingBackend};
use runtime_core::primitives::navigator::{
    NavCommand, NavigatorControl, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::NavigatorHandle;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wire::{NodeId, ScopeId, WireScreenOptions};

/// Translate the SDK's per-screen options to the wire shape. Header
/// buttons are dropped pending `HandlerId` registration (Phase 3's
/// reverse channel); title + visibility carry across, which is all the
/// headless render needs.
fn screen_opts_to_wire(options: &dyn Any) -> WireScreenOptions {
    options
        .downcast_ref::<StackScreenOptions>()
        .map(|o| WireScreenOptions {
            title: o.title.clone(),
            header_shown: o.header_shown,
            header_left: None,
            header_right: None,
        })
        .unwrap_or_default()
}

struct NoopStackOps;
impl NavigatorOps for NoopStackOps {}
static NOOP_STACK_OPS: NoopStackOps = NoopStackOps;

pub struct RecordingStackHandler {
    rec: Option<NavRecorder>,
    nav: Option<NodeId>,
    control: Option<Rc<NavigatorControl>>,
    /// Scope ids of the screens currently on the stack, top = end. The
    /// recorder needs them to release popped/reset scopes (stop their
    /// effects/timers) and to report depth.
    stack: Rc<RefCell<Vec<u64>>>,
}

impl RecordingStackHandler {
    pub fn new() -> Self {
        Self {
            rec: None,
            nav: None,
            control: None,
            stack: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl Default for RecordingStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<WireRecordingBackend> for RecordingStackHandler {
    fn init(
        &mut self,
        backend: &mut WireRecordingBackend,
        host: NavigatorHost<NodeId>,
        _presentation: Rc<dyn Any>,
    ) -> NodeId {
        let rec = backend.nav_recorder();
        let nav = rec.create_stack_navigator(
            host.initial_route,
            host.initial_path,
            &Default::default(),
        );

        self.nav = Some(nav);
        self.rec = Some(rec.clone());
        self.control = Some(host.control.clone());

        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let depth_changed = host.depth_changed.clone();
        let stack = self.stack.clone();
        let nav_disp = nav;

        host.control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, url, params, state } => {
                let result = mount_screen(name, params, state);
                rec.push(
                    nav_disp,
                    result.node,
                    ScopeId(result.scope_id),
                    screen_opts_to_wire(&*result.options),
                    url,
                );
                stack.borrow_mut().push(result.scope_id);
                depth_changed(stack.borrow().len());
            }
            NavCommand::Pop => {
                let popped = stack.borrow_mut().pop();
                let Some(scope) = popped else { return };
                rec.pop(nav_disp, 1);
                release_screen(scope);
                depth_changed(stack.borrow().len());
            }
            NavCommand::Replace { name, url, params, state } => {
                let result = mount_screen(name, params, state);
                let prev = stack.borrow_mut().pop();
                rec.replace(
                    nav_disp,
                    result.node,
                    ScopeId(result.scope_id),
                    screen_opts_to_wire(&*result.options),
                    url,
                );
                if let Some(prev) = prev {
                    release_screen(prev);
                }
                stack.borrow_mut().push(result.scope_id);
                // Depth unchanged (one screen swapped for another).
            }
            NavCommand::Reset { name, url, params, state } => {
                let result = mount_screen(name, params, state);
                let drained: Vec<u64> = stack.borrow_mut().drain(..).collect();
                rec.reset(
                    nav_disp,
                    result.node,
                    ScopeId(result.scope_id),
                    screen_opts_to_wire(&*result.options),
                    url,
                );
                for scope in drained {
                    release_screen(scope);
                }
                stack.borrow_mut().push(result.scope_id);
                depth_changed(stack.borrow().len());
            }
            NavCommand::Select { .. } | NavCommand::Custom(_) => {
                // The stack vocabulary is Push/Pop/Replace/Reset. Unlike
                // the live backends (which panic on this author error),
                // the recorder logs + ignores — a stray command must not
                // kill a dev session mid-edit.
                eprintln!(
                    "[stack-recording] ignoring non-stack NavCommand — \
                     stack kind accepts Push / Pop / Replace / Reset"
                );
            }
        }));

        nav
    }

    fn attach_initial(
        &mut self,
        _backend: &mut WireRecordingBackend,
        screen: NodeId,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        if let (Some(rec), Some(nav)) = (&self.rec, self.nav) {
            rec.attach_initial(nav, screen, ScopeId(scope_id), screen_opts_to_wire(&*options));
        }
        self.stack.borrow_mut().push(scope_id);
    }

    fn make_handle(&self) -> NavigatorHandle {
        match &self.control {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &NOOP_STACK_OPS, c.clone()),
            None => NavigatorHandle::new(Rc::new(()), &NOOP_STACK_OPS),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut WireRecordingBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        if let (Some(rec), Some(nav)) = (&self.rec, self.nav) {
            rec.apply_slot_style(nav, slot, style);
        }
    }

    fn release(&mut self, _backend: &mut WireRecordingBackend) {
        self.stack.borrow_mut().clear();
        self.control = None;
        self.rec = None;
    }
}

/// Register the recording stack handler on the runtime-server recorder.
pub fn register(backend: &mut WireRecordingBackend) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(RecordingStackHandler::new()));
}
