//! Recording handler for the runtime-server sidecar's recorder backend
//! (`dev_server::WireRecordingBackend`).
//!
//! The tab analogue of `drawer_navigator::recording`. Emits
//! `CreateTabNavigator` (carrying the tab registrations as data — no
//! closures cross the wire), `NavigatorAttachInitial`, and
//! `NavigatorSelect` (tabs switch via `Select`, like the drawer). The
//! active tab's screen is recorded as a primitive subtree the client
//! reconstructs.
//!
//! Mount policy note: this records each `Select` as a fresh
//! mount+`NavigatorSelect` and releases the previous tab's scope (the
//! same single-visible-screen model the thin client renders). True
//! keep-all-tabs-mounted persistence is a per-platform native concern;
//! the recorder/headless path shows one tab at a time, which is correct
//! for a dev screenshot.

use crate::{MountPolicy, TabPlacement, TabPresentation};
use dev_server::{NavRecorder, WireRecordingBackend};
use runtime_core::primitives::navigator::{
    NavCommand, NavigatorControl, NavigatorHandler, NavigatorHost, NavigatorOps,
};
use runtime_core::NavigatorHandle;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wire::{NodeId, ScopeId, WireMountPolicy, WireScreenOptions, WireTabPlacement, WireTabRegistration};

fn placement_to_wire(p: TabPlacement) -> WireTabPlacement {
    match p {
        TabPlacement::Top => WireTabPlacement::Top,
        // The wire only models Top/Bottom; Auto + Sidebar fall back to
        // Bottom (the conventional default + what the thin client lays
        // the tab bar out as).
        TabPlacement::Auto | TabPlacement::Bottom | TabPlacement::Sidebar => {
            WireTabPlacement::Bottom
        }
    }
}

fn policy_to_wire(m: MountPolicy) -> WireMountPolicy {
    match m {
        MountPolicy::EagerPersistent => WireMountPolicy::EagerPersistent,
        MountPolicy::LazyPersistent => WireMountPolicy::LazyPersistent,
        MountPolicy::LazyDisposing => WireMountPolicy::LazyDisposing,
    }
}

struct NoopTabsOps;
impl NavigatorOps for NoopTabsOps {}
static NOOP_TABS_OPS: NoopTabsOps = NoopTabsOps;

pub struct RecordingTabHandler {
    rec: Option<NavRecorder>,
    nav: Option<NodeId>,
    control: Option<Rc<NavigatorControl>>,
    initial_route: &'static str,
    /// (scope_id, route_name) of the visible tab — for the same-route
    /// guard + releasing the previous tab on switch.
    current: Rc<RefCell<Option<(u64, &'static str)>>>,
}

impl RecordingTabHandler {
    pub fn new() -> Self {
        Self {
            rec: None,
            nav: None,
            control: None,
            initial_route: "",
            current: Rc::new(RefCell::new(None)),
        }
    }
}

impl Default for RecordingTabHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<WireRecordingBackend> for RecordingTabHandler {
    fn init(
        &mut self,
        backend: &mut WireRecordingBackend,
        host: NavigatorHost<NodeId>,
        presentation: Rc<dyn Any>,
    ) -> NodeId {
        let pres = presentation
            .downcast::<TabPresentation>()
            .expect("RecordingTabHandler: presentation must be TabPresentation");

        let tabs: Vec<WireTabRegistration> = pres
            .tab_order
            .iter()
            .map(|(route, spec)| WireTabRegistration {
                route: route.to_string(),
                label: spec.label.clone(),
                icon: spec.icon.clone(),
            })
            .collect();

        let rec = backend.nav_recorder();
        let nav = rec.create_tab_navigator(
            host.initial_route,
            host.initial_path,
            tabs,
            placement_to_wire(pres.placement),
            policy_to_wire(pres.mount_policy),
            &Default::default(),
        );

        self.nav = Some(nav);
        self.rec = Some(rec.clone());
        self.control = Some(host.control.clone());
        self.initial_route = host.initial_route;

        // Link activations select tabs (not push).
        let select_activator: Rc<dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand> =
            Rc::new(|name, url, params| NavCommand::Select { name, url, params, state: None });
        host.control.install_link_activator(select_activator);

        let rec_disp = rec;
        let nav_disp = nav;
        let mount_screen = host.mount_screen.clone();
        let release_screen = host.release_screen.clone();
        let active_changed = host.active_changed.clone();
        let current = self.current.clone();
        let dispatching = Rc::new(RefCell::new(false));

        host.control.install(Box::new(move |cmd| match cmd {
            NavCommand::Select { name, url, params, state } => {
                if *dispatching.borrow() {
                    drop(params);
                    return;
                }
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
                let result = mount_screen(name, params, state);
                let prev = current.borrow_mut().take();
                rec_disp.select(
                    nav_disp,
                    result.node,
                    ScopeId(result.scope_id),
                    WireScreenOptions::default(),
                    url.clone(),
                );
                *current.borrow_mut() = Some((result.scope_id, name));
                active_changed(name, url);
                if let Some((prev_scope, _)) = prev {
                    release_screen(prev_scope);
                }
                *dispatching.borrow_mut() = false;
            }
            NavCommand::Push { .. }
            | NavCommand::Pop
            | NavCommand::Replace { .. }
            | NavCommand::Reset { .. }
            | NavCommand::Custom(_) => {
                // Tabs switch via Select only. The recorder logs +
                // ignores a stray command rather than killing the
                // session (the live backends panic on this author error).
                eprintln!("[tab-recording] ignoring non-tab NavCommand — tab kind accepts Select");
            }
        }));

        nav
    }

    fn attach_initial(
        &mut self,
        _backend: &mut WireRecordingBackend,
        screen: NodeId,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        if let (Some(rec), Some(nav)) = (&self.rec, self.nav) {
            rec.attach_initial(nav, screen, ScopeId(scope_id), WireScreenOptions::default());
        }
        *self.current.borrow_mut() = Some((scope_id, self.initial_route));
    }

    fn make_handle(&self) -> NavigatorHandle {
        match &self.control {
            Some(c) => NavigatorHandle::with_control(Rc::new(()), &NOOP_TABS_OPS, c.clone()),
            None => NavigatorHandle::new(Rc::new(()), &NOOP_TABS_OPS),
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
        *self.current.borrow_mut() = None;
        self.control = None;
        self.rec = None;
    }
}

/// Register the recording tab handler on the runtime-server recorder.
pub fn register(backend: &mut WireRecordingBackend) {
    backend.register_navigator::<TabPresentation, _>(|| Box::new(RecordingTabHandler::new()));
}
