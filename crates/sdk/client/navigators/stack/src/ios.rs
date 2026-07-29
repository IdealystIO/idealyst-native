//! iOS native push surface for the outlet-model stack.
//!
//! The outlet model keeps chrome as author layout, but a stack on iOS
//! should still FEEL native: real `UINavigationController` push/pop
//! transitions and the interactive swipe-back gesture. This handler
//! composes both:
//!
//! - the author `.layout(|nav| …)` builds exactly as on every other
//!   backend, and `{nav.outlet}` marks where screens go;
//! - INSIDE the outlet lives a `UINavigationController` (the shared
//!   `ios-navigator-helpers` stack engine — the same battle-tested
//!   machinery the legacy stack used: interactive-pop delegate,
//!   per-top `back_enabled` re-sync, full-screen sync, cold-start
//!   deep-link back-stack reconstruction);
//! - the NATIVE BAR is always hidden (`header_shown: Some(false)`).
//!   The header is the author's `idea_ui_nav::StackHeader`, driven by
//!   [`StackContext::screen_chrome`] — published from the engine's
//!   `top_changed` hook so swipe-back updates it too. Uniform output
//!   across backends (CLAUDE.md §7): same chrome everywhere, native
//!   transition mechanics underneath.
//!
//! Retention: covered screens stay alive inside the controller
//! (native semantics — [`StackRetention::Retain`]); the `Rebuild`
//! browser semantics don't apply to a native push surface.

use crate::{StackPresentation, StackScreenOptions};
use backend_ios::{IosBackend, IosNode};
use ios_navigator_helpers::{self as helpers, IosNavCallbacks, IosScreenOptions};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    navigator_fill_rules, navigator_outlet, MountResult, NavCommand, NavigatorHandler,
    NavigatorHost,
};
use runtime_core::Backend;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Force the native bar hidden and carry the per-screen flags the
/// engine honors (`back_enabled` lock, `fullscreen`). Title/buttons are
/// NOT translated — the author `StackHeader` renders them from
/// `screen_chrome`, so native bar-button targets would be dead weight.
fn to_ios_options(opts: &StackScreenOptions) -> IosScreenOptions {
    IosScreenOptions {
        header_shown: Some(false),
        back_enabled: Some(opts.back_enabled),
        fullscreen: Some(opts.fullscreen),
        ..Default::default()
    }
}

/// Screen options captured at mount, keyed by scope id — the source the
/// engine's `top_changed` hook publishes `screen_chrome` from (the
/// engine itself only retains the flags it enforces).
type OptionsMap = Rc<RefCell<HashMap<u64, StackScreenOptions>>>;

struct PendingInitial {
    screen: IosNode,
    scope_id: u64,
    options: StackScreenOptions,
}

/// The iOS stack handler: author layout + outlet, with the shared
/// `UINavigationController` engine (bar hidden) seated inside the
/// outlet for native push/pop transitions and swipe-back.
pub struct IosStackV2Handler {
    /// The helpers-engine nav-controller node (lives inside the outlet).
    engine: Rc<RefCell<Option<IosNode>>>,
    /// Framework-attached initial screen, held until the deferred
    /// layout build creates the outlet + engine.
    pending_initial: Rc<RefCell<Option<PendingInitial>>>,
    control: Option<Rc<runtime_core::primitives::navigator::NavigatorControl>>,
}

impl IosStackV2Handler {
    /// A fresh, uninitialized handler; `init` wires the rest.
    pub fn new() -> Self {
        Self {
            engine: Rc::new(RefCell::new(None)),
            pending_initial: Rc::new(RefCell::new(None)),
            control: None,
        }
    }
}

impl Default for IosStackV2Handler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<IosBackend> for IosStackV2Handler {
    fn init(
        &mut self,
        backend: &mut IosBackend,
        host: NavigatorHost<IosNode>,
        presentation: Rc<dyn Any>,
    ) -> IosNode {
        let mtm = backend.mtm();
        let a11y = AccessibilityProps::default();
        let root = backend.create_view(&a11y);
        backend.apply_style(&root, &navigator_fill_rules());

        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            insert_node,
            control,
            build_layout_with_outlet,
            nav_state,
            screen_chrome,
            depth_changed,
            ..
        } = host;

        let layout = presentation
            .downcast_ref::<StackPresentation>()
            .and_then(|p| p.layout.clone());

        let options_map: OptionsMap = Rc::new(RefCell::new(HashMap::new()));

        // Mount adapter for the engine: run the framework mount, capture
        // the screen's v2 options for `screen_chrome`, and hand the
        // engine the translated flags (bar hidden, back-lock, fullscreen).
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>> = {
            let m = mount_screen;
            let map = options_map.clone();
            Rc::new(move |name, params| {
                let result = m(name, params, None);
                let v2 = result
                    .options
                    .downcast_ref::<StackScreenOptions>()
                    .cloned()
                    .unwrap_or_default();
                let ios: Box<dyn Any> = Box::new(to_ios_options(&v2));
                map.borrow_mut().insert(result.scope_id, v2);
                MountResult { node: result.node, scope_id: result.scope_id, options: ios }
            })
        };

        // Release adapter: drop the captured options with the screen.
        let release_2: Rc<dyn Fn(u64)> = {
            let r = release_screen;
            let map = options_map.clone();
            Rc::new(move |scope_id| {
                map.borrow_mut().remove(&scope_id);
                r(scope_id);
            })
        };

        // `top_changed` → publish the revealed screen's author-header
        // state. `native = false`: the native bar is hidden, so the
        // author `StackHeader` must SHOW (it auto-hides when native).
        // Fires on push/pop/replace/reset AND swipe-back.
        let top_changed: Rc<dyn Fn(u64)> = {
            let map = options_map.clone();
            Rc::new(move |scope_id| {
                let state = map
                    .borrow()
                    .get(&scope_id)
                    .cloned()
                    .unwrap_or_default()
                    .to_state(false);
                // `set_always`: `Rc<dyn Any>` has no `PartialEq` (guarded `set`
                // requires it), and chrome must republish on every stack
                // change anyway — same contract as the shared handler's
                // `sync_chrome`. (Pre-existing guarded-set miss on this
                // compile-only target, caught by the P6 cross-check.)
                screen_chrome.set_always(Some(Rc::new(state) as Rc<dyn Any>));
            })
        };

        // Deferred: build the author layout, seat the native engine
        // inside the outlet, then seat the framework-attached initial.
        {
            let engine_slot = self.engine.clone();
            let pending = self.pending_initial.clone();
            let map_for_task = options_map.clone();
            let root = root.clone();
            let control_for_task = control.clone();
            let nav_state_for_task = nav_state.clone();
            let active_route = nav_state.active_route;
            let active_path = nav_state.active_path;
            let depth = nav_state.depth;
            let can_go_back = nav_state.can_go_back;
            let screen_chrome_sig = screen_chrome;
            runtime_core::schedule_microtask(move || {
                let pop: Rc<dyn Fn()> = {
                    let control = control_for_task.clone();
                    Rc::new(move || control.dispatch(NavCommand::Pop))
                };
                let ctx = crate::StackContext {
                    outlet: navigator_outlet(),
                    active_route,
                    active_path,
                    depth,
                    can_go_back,
                    pop,
                    screen_chrome: screen_chrome_sig,
                };
                // Producer closure runs inside the framework's retained
                // nav-chrome scope, so an `effect!` in author chrome is
                // owned by the navigator.
                let (layout_root, outlet) =
                    (build_layout_with_outlet)(Box::new(move || match &layout {
                        Some(f) => f(ctx),
                        None => ctx.outlet,
                    }));
                debug_assert!(
                    outlet.is_some(),
                    "stack-navigator (iOS): the author `.layout(...)` must splat `{{nav.outlet}}`"
                );
                insert_node(root, layout_root);

                // Native engine (UINavigationController) inside the outlet.
                let callbacks = IosNavCallbacks {
                    initial_route,
                    initial_path,
                    mount_screen: mount_2arg,
                    release_screen: release_2,
                    depth_changed,
                    nav_state: nav_state_for_task,
                    defer_initial_mount,
                    top_changed: Some(top_changed),
                };
                let nav_node = helpers::create_stack(mtm, callbacks, control_for_task);
                if let Some(outlet) = outlet {
                    insert_node(outlet, nav_node.clone());
                }

                // Seat the framework-attached initial screen (deep-link
                // aware — the engine reconstructs the back stack itself).
                if let Some(p) = pending.borrow_mut().take() {
                    // The walker mounted the initial through the RAW host
                    // `mount_screen` (not the engine adapter), so its
                    // options enter the chrome map here.
                    map_for_task.borrow_mut().insert(p.scope_id, p.options.clone());
                    helpers::stack_attach_initial(
                        mtm,
                        &nav_node,
                        p.screen,
                        p.scope_id,
                        &to_ios_options(&p.options),
                    );
                }
                *engine_slot.borrow_mut() = Some(nav_node);
            });
        }

        self.control = Some(control);
        root
    }

    fn attach_initial(
        &mut self,
        _backend: &mut IosBackend,
        screen: IosNode,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let v2 = options
            .downcast_ref::<StackScreenOptions>()
            .cloned()
            .unwrap_or_default();
        // Normal (non-deferred) flow: the walker attaches synchronously,
        // BEFORE the layout microtask builds the engine — stash. Deferred
        // hosts can attach after the engine exists; seat directly then.
        if let Some(engine) = self.engine.borrow().clone() {
            let mtm = _backend.mtm();
            helpers::stack_attach_initial(mtm, &engine, screen, scope_id, &to_ios_options(&v2));
            return;
        }
        *self.pending_initial.borrow_mut() = Some(PendingInitial { screen, scope_id, options: v2 });
    }

    fn release(&mut self, _backend: &mut IosBackend) {
        if let Some(engine) = self.engine.borrow_mut().take() {
            helpers::release_stack(&engine);
        }
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        // Prefer the control we retained at init (always present after
        // init); the engine-node lookup is equivalent but only valid
        // after the deferred build.
        match &self.control {
            Some(c) => runtime_core::NavigatorHandle::with_control(
                Rc::new(()),
                &crate::STACK_OPS,
                c.clone(),
            ),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &crate::STACK_OPS),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut IosBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(engine) = self.engine.borrow().clone() else { return };
        match slot {
            // The native bar is hidden; header/title/button styling
            // belongs to the author `StackHeader`. Only the body (the
            // controller's root view — the screen outlet) is native.
            "body" => helpers::apply_stack_body_style(&engine, style),
            _ => {}
        }
    }
}

/// Register the iOS native-surface stack handler.
pub fn register(backend: &mut IosBackend) {
    backend
        .register_navigator::<StackPresentation, _>(|| Box::new(IosStackV2Handler::new()));
}

inventory::submit! { backend_ios::IosNavigatorRegistrar(register) }
