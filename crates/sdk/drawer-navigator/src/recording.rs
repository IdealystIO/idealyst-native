//! Recording handler for the runtime-server sidecar's recorder backend
//! (`dev_server::WireRecordingBackend`).
//!
//! In `idealyst dev`'s default runtime-server mode the app runs in a
//! sidecar that *records* its reactive tree into a wire command stream;
//! thin clients (web / iOS / Android dev-client, and the headless
//! screenshotter) replay that stream. Without a navigator handler the
//! recorder hits the `Backend::create_navigator` trait default
//! (`unimplemented!()`) the moment a `DrawerNavigator` mounts, killing
//! the session — so navigator apps (the website, idea-ui-docs) only
//! worked in `--local` mode.
//!
//! This handler closes that gap. Instead of building a native drawer it
//! emits the navigator wire commands the protocol already carries —
//! `CreateDrawerNavigator`, `DrawerAttachSidebar`,
//! `NavigatorAttachInitial`, `NavigatorSelect`, and
//! `OpenDrawer`/`CloseDrawer`/`ToggleDrawer` — through a
//! [`dev_server::NavRecorder`]. The `SceneModel` already interprets all
//! of these (both live and in the reconnect snapshot), so feeding it is
//! all that's needed on the recorder side; the *client* reconstructs the
//! navigator via its own registered handler (Phase 4).
//!
//! ## Why the sidebar build is deferred via a stored `after_ms(0)`
//!
//! `host.build_node` re-enters `backend.borrow_mut()` (the walker holds
//! that borrow across `create_navigator`). On web/iOS the SDK defers the
//! sidebar build to a `schedule_microtask`, which is genuinely async
//! there. The sidecar scheduler runs microtasks **synchronously**
//! (`SidecarScheduler::schedule_microtask` calls `f()` immediately), so
//! deferring the build that way would run `build_node` *inside* the
//! still-held walker borrow → a double-borrow panic. `after_ms(0)`
//! queues to the deadline list instead, which `drive_pending()` drains
//! only after the walk completes and the borrow has released. We store
//! the handle on the handler (not `mem::forget`) so it isn't cancelled
//! before it fires yet still drops with the navigator on `release`.

use crate::{
    DrawerCmd, DrawerPresentation, DrawerScreenOptions, DrawerSide, DrawerSlotProps, DrawerType,
    LeadingIntent, MountPolicy, SidebarBuilder, SlotBuilder, SlotProps, TrailingIntent,
};
use dev_server::{NavRecorder, WireRecordingBackend};
use runtime_core::primitives::navigator::{
    AmbientNavGuard, NavCommand, NavState, NavigatorControl, NavigatorHandler, NavigatorHost,
    NavigatorOps,
};
use runtime_core::{NavigatorHandle, ScheduledTask, Signal};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wire::{NodeId, ScopeId, WireDrawerSide, WireDrawerType, WireMountPolicy, WireScreenOptions};

// --- SDK enum → wire enum shims (mirror web.rs's `*_to_helpers`) ---

fn side_to_wire(s: DrawerSide) -> WireDrawerSide {
    match s {
        DrawerSide::Start => WireDrawerSide::Left,
        DrawerSide::End => WireDrawerSide::Right,
    }
}

fn type_to_wire(t: DrawerType) -> WireDrawerType {
    match t {
        DrawerType::Front => WireDrawerType::Front,
        DrawerType::Slide => WireDrawerType::Slide,
    }
}

fn policy_to_wire(m: MountPolicy) -> WireMountPolicy {
    match m {
        MountPolicy::EagerPersistent => WireMountPolicy::EagerPersistent,
        MountPolicy::LazyPersistent => WireMountPolicy::LazyPersistent,
        MountPolicy::LazyDisposing => WireMountPolicy::LazyDisposing,
    }
}

/// Translate the SDK's per-screen `DrawerScreenOptions` to the wire
/// shape. `header_left` / `header_right` are deliberately dropped for
/// now: a `WireHeaderButton` needs a registered `HandlerId` so the
/// reverse channel can route its `on_press`, which is the
/// reverse-channel work in Phase 3. Title + visibility carry across
/// today; the headless screenshot (the immediate goal) doesn't depend
/// on native header buttons.
fn screen_opts_to_wire(options: &dyn Any) -> WireScreenOptions {
    options
        .downcast_ref::<DrawerScreenOptions>()
        .map(|o| WireScreenOptions {
            title: o.title.clone(),
            header_shown: o.header_shown,
            header_left: None,
            header_right: None,
        })
        .unwrap_or_default()
}

struct NoopDrawerOps;
impl NavigatorOps for NoopDrawerOps {}
static NOOP_DRAWER_OPS: NoopDrawerOps = NoopDrawerOps;

/// `(scope_id, route_name)` of the navigator's single visible screen.
/// Shared between the Select dispatcher (which swaps it) and
/// `attach_initial` (which seeds it) so the same-route guard works.
type Current = Rc<RefCell<Option<(u64, &'static str)>>>;

pub struct RecordingDrawerHandler {
    rec: Option<NavRecorder>,
    nav: Option<NodeId>,
    control: Option<Rc<NavigatorControl>>,
    initial_route: &'static str,
    current: Current,
    /// Keeps the deferred sidebar build alive until it fires. Dropping a
    /// `ScheduledTask` cancels it; storing it here means it survives the
    /// `after_ms(0)` window but still drops with the handler on
    /// `release_navigator`.
    _sidebar_task: Option<ScheduledTask>,
}

impl RecordingDrawerHandler {
    pub fn new() -> Self {
        Self {
            rec: None,
            nav: None,
            control: None,
            initial_route: "",
            current: Rc::new(RefCell::new(None)),
            _sidebar_task: None,
        }
    }
}

impl Default for RecordingDrawerHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the new-API `SlotProps` for a slot/sidebar builder, with live
/// dispatchers wired to `control` so nav-links / hamburger inside the
/// sidebar actually dispatch through the navigator.
fn slot_props(nav: &NavState, control: &Rc<NavigatorControl>, is_open: Signal<bool>) -> SlotProps {
    let on_select: Rc<dyn Fn(&'static str)> = {
        let control = control.clone();
        Rc::new(move |name| {
            control.dispatch(NavCommand::Select {
                name,
                url: String::new(),
                params: Box::new(()),
                state: None,
            });
        })
    };
    let open_drawer: Rc<dyn Fn()> = {
        let control = control.clone();
        Rc::new(move || control.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Open))))
    };
    let close_drawer: Rc<dyn Fn()> = {
        let control = control.clone();
        Rc::new(move || control.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close))))
    };
    let pop: Rc<dyn Fn()> = {
        let control = control.clone();
        Rc::new(move || control.dispatch(NavCommand::Pop))
    };
    SlotProps {
        active_route: nav.active_route,
        active_path: nav.active_path,
        depth: nav.depth,
        can_go_back: nav.can_go_back,
        is_open,
        leading_intent: Signal::new(LeadingIntent::OpenDrawer),
        trailing_intent: Signal::new(TrailingIntent::None),
        screen_title: Signal::new(String::new()),
        on_select,
        open_drawer,
        close_drawer,
        pop,
        scroll: None,
    }
}

/// Build the legacy `DrawerSlotProps` for the `.sidebar`/`.sidebar_with`
/// form.
fn legacy_props(
    nav: &NavState,
    control: &Rc<NavigatorControl>,
    is_open: Signal<bool>,
) -> DrawerSlotProps {
    let on_select: Rc<dyn Fn(&'static str)> = {
        let control = control.clone();
        Rc::new(move |name| {
            control.dispatch(NavCommand::Select {
                name,
                url: String::new(),
                params: Box::new(()),
                state: None,
            });
        })
    };
    let on_close: Rc<dyn Fn()> = {
        let control = control.clone();
        Rc::new(move || control.dispatch(NavCommand::Custom(Rc::new(DrawerCmd::Close))))
    };
    DrawerSlotProps {
        active_route: nav.active_route,
        active_path: nav.active_path,
        depth: nav.depth,
        can_go_back: nav.can_go_back,
        is_open,
        on_select,
        on_close,
    }
}

impl NavigatorHandler<WireRecordingBackend> for RecordingDrawerHandler {
    fn init(
        &mut self,
        backend: &mut WireRecordingBackend,
        host: NavigatorHost<NodeId>,
        presentation: Rc<dyn Any>,
    ) -> NodeId {
        let pres = presentation
            .downcast::<DrawerPresentation>()
            .expect("RecordingDrawerHandler: presentation must be DrawerPresentation");

        let rec = backend.nav_recorder();

        // Emit the navigator node. Runs under the navigator's ambient
        // identity (the walker set it), so the node id is stable across
        // sidecar respawns — incremental hot reload.
        let nav = rec.create_drawer_navigator(
            host.initial_route,
            host.initial_path,
            side_to_wire(pres.side),
            type_to_wire(pres.drawer_type),
            pres.drawer_width,
            pres.swipe_to_open,
            policy_to_wire(pres.mount_policy),
            // The handler trait doesn't forward the navigator's
            // `AccessibilityProps` (no backend's drawer handler gets it
            // — web/terminal/iOS are all in the same boat), so default.
            &Default::default(),
        );

        self.nav = Some(nav);
        self.rec = Some(rec.clone());
        self.control = Some(host.control.clone());
        self.initial_route = host.initial_route;

        // Register the open-state signal so the reverse channel
        // (`handle_drawer_state_changed`) can sync it when the client
        // opens/closes the drawer via a platform gesture — the recorder
        // analogue of web/iOS's `open_changed` callback.
        backend.register_drawer_open_signal(nav, pres.is_open);

        // Map `Link(route=...)` activations to `Select` (drawer shape),
        // not the substrate default `Push` (stack shape). Same fix as
        // every other drawer handler — without it sidebar links would
        // dispatch a stack `Push`, which a drawer doesn't accept.
        let select_activator: Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand> =
            Rc::new(|name, url, params| NavCommand::Select { name, url, params, state: None });
        host.control.install_link_activator(select_activator);

        // --- Sidebar: build to primitives + attach, deferred past the
        //     walker borrow (see module docs for why `after_ms(0)`). ---
        let leading: Option<SlotBuilder> = pres.leading_slot.borrow_mut().take();
        let legacy: Option<SidebarBuilder> = if leading.is_none() {
            pres.sidebar.borrow_mut().take()
        } else {
            None
        };
        if leading.is_some() || legacy.is_some() {
            let build_node = host.build_node.clone();
            let control = host.control.clone();
            let nav_state = host.nav_state.clone();
            let is_open = pres.is_open;
            let rec_sidebar = rec.clone();
            self._sidebar_task = Some(runtime_core::after_ms(0, move || {
                // Push the navigator onto the ambient stack so `Link`s
                // built inside the sidebar capture it as their dispatch
                // target (see [[project_drawer_sidebar_ambient_nav]]).
                let _guard = AmbientNavGuard::push(control.clone());
                let element = if let Some(builder) = leading {
                    builder(slot_props(&nav_state, &control, is_open))
                } else {
                    let builder = legacy.expect("legacy sidebar present");
                    builder(legacy_props(&nav_state, &control, is_open))
                };
                // Build the sidebar subtree (records its primitive
                // commands) and reference its root in DrawerAttachSidebar.
                let sidebar_root = build_node(element);
                rec_sidebar.attach_sidebar(nav, sidebar_root);
            }));
        }

        // --- Control dispatcher (runs later, on user events) ---
        let rec_disp = rec;
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let active_changed = host.active_changed.clone();
        let current = self.current.clone();
        let is_open = pres.is_open;
        // Re-entrancy guard: a Select can re-enter via an effect on the
        // active-route chain. The inner call would observe half-swapped
        // state (prev taken, not yet released). Mirrors terminal.rs.
        let dispatching = Rc::new(RefCell::new(false));

        host.control.install(Box::new(move |cmd| match cmd {
            NavCommand::Select { name, url, params, state } => {
                if *dispatching.borrow() {
                    drop(params);
                    return;
                }
                // Reselecting the active route is a no-op (matches the
                // web/terminal handlers' same-route guard).
                let already_active = current
                    .borrow()
                    .as_ref()
                    .map(|(_, n)| *n == name)
                    .unwrap_or(false);
                if already_active {
                    drop(params);
                    return;
                }
                *dispatching.borrow_mut() = true;

                // Build the new screen subtree (records its primitives),
                // then emit NavigatorSelect referencing its root. Order:
                // commit bookkeeping, then release the previous scope
                // last — any cleanup that re-enters the dispatcher hits
                // the guard above.
                let result = mount_screen(name, params, state);
                let prev = current.borrow_mut().take();
                rec_disp.select(
                    nav,
                    result.node,
                    ScopeId(result.scope_id),
                    screen_opts_to_wire(&*result.options),
                    url.clone(),
                );
                *current.borrow_mut() = Some((result.scope_id, name));
                active_changed(name, url);
                // Auto-close the drawer on selection — navigating shuts
                // the drawer (matches the web handler at
                // web-navigator-helpers `Select`). Flip the dev-side signal
                // only so server-built reactive sidebars stay coherent; the
                // CLIENT closes its own drawer when it replays this Select
                // through its native handler's dispatcher (the handler's
                // Select arm auto-closes), so emitting a wire CloseDrawer
                // here would be a redundant second animation. Guarded on
                // actually-open to avoid churn on a programmatic select
                // while already closed.
                if is_open.get() {
                    is_open.set(false);
                }
                if let Some((prev_scope, _)) = prev {
                    release_screen(prev_scope);
                }
                *dispatching.borrow_mut() = false;
            }
            NavCommand::Custom(payload) => {
                if let Some(cmd) = payload.downcast_ref::<DrawerCmd>() {
                    // Flip the shared signal (so reactive sidebars stay
                    // coherent) AND emit the wire command (so the client
                    // animates the real drawer).
                    match cmd {
                        DrawerCmd::Open => {
                            is_open.set(true);
                            rec_disp.open_drawer(nav);
                        }
                        DrawerCmd::Close => {
                            is_open.set(false);
                            rec_disp.close_drawer(nav);
                        }
                        DrawerCmd::Toggle => {
                            is_open.set(!is_open.get());
                            rec_disp.toggle_drawer(nav);
                        }
                    }
                }
            }
            NavCommand::Push { .. }
            | NavCommand::Pop
            | NavCommand::Replace { .. }
            | NavCommand::Reset { .. } => {
                // A drawer doesn't carry a stack. Unlike the live
                // backends (which `panic!` on this author error), the
                // recorder swallows it with a diagnostic: a stray stack
                // command must not kill a dev session mid-edit.
                eprintln!(
                    "[drawer-recording] ignoring stack-shaped NavCommand — \
                     drawer kind only accepts Select / Custom(DrawerCmd)"
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
            rec.attach_initial(
                nav,
                screen,
                ScopeId(scope_id),
                screen_opts_to_wire(&*options),
            );
        }
        *self.current.borrow_mut() = Some((scope_id, self.initial_route));
    }

    fn make_handle(&self) -> NavigatorHandle {
        match &self.control {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &NOOP_DRAWER_OPS, c.clone()),
            None => NavigatorHandle::new(Rc::new(()), &NOOP_DRAWER_OPS),
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
        // Framework drops the navigator's enclosing scope (releasing
        // every screen scope under it). Just clear local bookkeeping;
        // dropping `_sidebar_task` cancels any not-yet-fired build.
        *self.current.borrow_mut() = None;
        self._sidebar_task = None;
        self.control = None;
        self.rec = None;
    }
}

/// Register the recording drawer handler on the runtime-server recorder.
/// Called from the sidecar's `register_extensions(&mut recorder)` path
/// (Phase 5) — the recorder analogue of `drawer_navigator::register`.
pub fn register(backend: &mut WireRecordingBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(RecordingDrawerHandler::new()));
}
