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
    MountResult, NavCommand, NavigatorControl, NavigatorHandler, NavigatorHost, NavigatorOps,
    NavState,
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

/// Stack navigator handler for the wire-recording backend. Mirrors the
/// native handlers but records `NavCommand`s into a [`NavRecorder`] so the
/// sidecar can replay push/pop/reset over the wire instead of rendering
/// native chrome.
pub struct RecordingStackHandler {
    rec: Option<NavRecorder>,
    nav: Option<NodeId>,
    control: Option<Rc<NavigatorControl>>,
    /// Scope ids of the screens currently on the stack, top = end. The
    /// recorder needs them to release popped/reset scopes (stop their
    /// effects/timers) and to report depth.
    stack: Rc<RefCell<Vec<u64>>>,
    /// Configured initial route + screen builder + nav-state mirror, kept
    /// so `attach_initial` can reconstruct the back stack on a cold-start
    /// deep link (see `attach_initial`).
    initial_route: &'static str,
    mount_screen: Option<
        Rc<dyn Fn(&'static str, Box<dyn Any>, Option<Rc<dyn Any>>) -> MountResult<NodeId>>,
    >,
    depth_changed: Option<Rc<dyn Fn(usize)>>,
    nav_state: Option<NavState>,
    /// Keeps the deferred deep-link reconstruction alive until it fires.
    /// Dropping a `ScheduledTask` cancels it; storing it here means it
    /// survives the `after_ms(0)` window but still drops with the handler.
    _reconstruct_task: Option<runtime_core::ScheduledTask>,
}

impl RecordingStackHandler {
    /// Create an unattached handler; recorder, nav node, and control are
    /// wired in when the navigator initializes.
    pub fn new() -> Self {
        Self {
            rec: None,
            nav: None,
            control: None,
            stack: Rc::new(RefCell::new(Vec::new())),
            initial_route: "",
            mount_screen: None,
            depth_changed: None,
            nav_state: None,
            _reconstruct_task: None,
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
        self.initial_route = host.initial_route;
        self.mount_screen = Some(host.mount_screen.clone());
        self.depth_changed = Some(host.depth_changed.clone());
        self.nav_state = Some(host.nav_state.clone());

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
        let Some(rec) = self.rec.clone() else { return };
        let Some(nav) = self.nav else { return };

        // Cold-start deep-link back-stack reconstruction. The walker resolves
        // the launch URL and mounts the RESOLVED screen as the initial — so the
        // deep-linked detail is what `attach_initial` carries. If that resolved
        // route differs from the navigator's configured `initial`, the back stack
        // would be just [detail] and Back would have nowhere to go. Reconstruct
        // [index, detail]: attach the configured `initial` as the BOTTOM, then
        // push the already-mounted resolved `screen` on top so Back returns to
        // the index. Only the stack knows it's a stack, which is why this lives
        // here and not in the kind-blind walker.
        let active_route = self
            .nav_state
            .as_ref()
            .map(|s| s.active_route.get())
            .unwrap_or(self.initial_route);
        let is_deep_link = active_route != self.initial_route;

        if is_deep_link {
            if let (Some(mount_screen), Some(nav_state)) =
                (self.mount_screen.clone(), self.nav_state.clone())
            {
                // Defer past the walker's `attach_initial` borrow: `mount_screen`
                // re-enters the backend (`borrow_mut`), which the walker is still
                // holding at this call site. The recorder drains microtasks
                // before snapshotting, so the [index, detail] stack is in place
                // by the time anything reads the stream. (Same defer-past-the-
                // borrow rule the drawer sidebar build follows.)
                let initial_route = self.initial_route;
                let stack = self.stack.clone();
                let depth_changed = self.depth_changed.clone();
                let detail_opts = screen_opts_to_wire(&*options);
                let detail_url = nav_state.active_path.get();
                // `after_ms(0)` (NOT `schedule_microtask`): the sidecar
                // scheduler runs microtasks synchronously-at-queue-time, which
                // would fire this inside the walker's still-held borrow. The
                // deadline list is drained by `tick_animations`, safely after.
                let task = runtime_core::after_ms(0, move || {
                    // Index → stack base.
                    let base = mount_screen(initial_route, Box::new(()), None);
                    rec.attach_initial(
                        nav,
                        base.node,
                        ScopeId(base.scope_id),
                        screen_opts_to_wire(&*base.options),
                    );
                    stack.borrow_mut().push(base.scope_id);
                    // Resolved detail (already mounted by the walker) → on top.
                    rec.push(nav, screen, ScopeId(scope_id), detail_opts, detail_url);
                    stack.borrow_mut().push(scope_id);
                    if let Some(dc) = &depth_changed {
                        dc(stack.borrow().len());
                    }
                });
                self._reconstruct_task = Some(task);
                return;
            }
        }

        // No deep link (or no builder): the resolved screen IS the index.
        rec.attach_initial(nav, screen, ScopeId(scope_id), screen_opts_to_wire(&*options));
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
