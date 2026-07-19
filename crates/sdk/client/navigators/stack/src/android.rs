//! Android native push surface for the outlet-model stack.
//!
//! Same composition as iOS (`src/ios.rs`): the author `.layout(|nav| …)`
//! renders on every backend, and INSIDE `{nav.outlet}` lives the shared
//! `android-navigator-helpers` stack engine — the Kotlin `RustNavigator`
//! (FragmentManager back stack, native push/pop transitions, system-back
//! integration, `OnBackPressedCallback` back-lock, immersive
//! full-screen). The native Toolbar is never shown
//! (`header_shown: Some(false)`); the header is the author's
//! `idea_ui_nav::StackHeader`, driven by [`crate::StackContext::screen_chrome`].
//!
//! Unlike iOS (whose engine got a `top_changed` hook), Android's
//! revealed-top tracking lives here: the engine reports pops only
//! through `release_screen` (Kotlin `onDestroyView` →
//! `nativeReleaseScreen`), so this handler mirrors the scope stack in
//! its mount/release adapters and republishes `screen_chrome` — and
//! `depth` — from them. That also fixes system-back leaving the `depth`
//! signal stale (the legacy handler only updated depth on dispatcher
//! commands, so `can_go_back` drifted after a hardware back).
//!
//! Retention: covered fragments stay alive in the FragmentManager
//! (native semantics — [`crate::StackRetention::Retain`]).

use crate::{StackPresentation, StackScreenOptions};
use android_navigator_helpers::{self as helpers, AndroidNavCallbacks, AndroidScreenOptions};
use backend_android::AndroidBackend;
use jni::objects::GlobalRef;
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

/// Native Toolbar hidden; only the flags the engine enforces cross the
/// boundary (back-lock, immersive full-screen). Title/buttons render in
/// the author `StackHeader` from `screen_chrome`.
fn to_android_options(opts: &StackScreenOptions) -> AndroidScreenOptions {
    AndroidScreenOptions {
        header_shown: Some(false),
        back_enabled: Some(opts.back_enabled),
        fullscreen: Some(opts.fullscreen),
        ..Default::default()
    }
}

/// Handler-owned mirror of the engine's screen stack + captured
/// per-screen options, shared by the mount/release adapters. Owns its
/// publish targets (set once at init) so every mutation site can
/// republish uniformly.
struct Mirror {
    /// Scope ids bottom→top. Pushed by the mount adapter / initial
    /// attach, pruned by the release adapter (which is how Kotlin-side
    /// pops report back).
    stack: Vec<u64>,
    options: HashMap<u64, StackScreenOptions>,
    screen_chrome: Option<runtime_core::Signal<Option<Rc<dyn Any>>>>,
    depth_changed: Option<Rc<dyn Fn(usize)>>,
}

impl Mirror {
    /// Publish the current top's author-header state + depth. `native =
    /// false`: the Toolbar is hidden, the author `StackHeader` shows.
    /// (Signals are generation-guarded, so publishes after navigator
    /// teardown are safe no-ops.)
    fn publish(&self) {
        if let (Some(chrome), Some(top)) = (&self.screen_chrome, self.stack.last()) {
            let state = self
                .options
                .get(top)
                .cloned()
                .unwrap_or_default()
                .to_state(false);
            chrome.set(Some(Rc::new(state) as Rc<dyn Any>));
        }
        if let Some(depth) = &self.depth_changed {
            depth(self.stack.len());
        }
    }
}

/// The Android stack handler: author layout + outlet, with the shared
/// Kotlin `RustNavigator` engine (Toolbar hidden) seated inside the
/// outlet for native push/pop transitions and system-back handling.
pub struct AndroidStackV2Handler {
    engine: Option<GlobalRef>,
    mirror: Rc<RefCell<Mirror>>,
    control: Option<Rc<runtime_core::primitives::navigator::NavigatorControl>>,
}

impl AndroidStackV2Handler {
    /// A fresh, uninitialized handler; `init` wires the rest.
    pub fn new() -> Self {
        Self {
            engine: None,
            mirror: Rc::new(RefCell::new(Mirror {
                stack: Vec::new(),
                options: HashMap::new(),
                screen_chrome: None,
                depth_changed: None,
            })),
            control: None,
        }
    }
}

impl Default for AndroidStackV2Handler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<AndroidBackend> for AndroidStackV2Handler {
    fn init(
        &mut self,
        backend: &mut AndroidBackend,
        host: NavigatorHost<GlobalRef>,
        presentation: Rc<dyn Any>,
    ) -> GlobalRef {
        let a11y = AccessibilityProps::default();
        let root = backend.create_view(&a11y);
        backend.apply_style(&root, &navigator_fill_rules());

        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path,
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

        // Seat the publish targets on the mirror.
        {
            let mut mir = self.mirror.borrow_mut();
            mir.screen_chrome = Some(screen_chrome);
            mir.depth_changed = Some(depth_changed.clone());
        }

        // Mount adapter: capture v2 options, mirror the push, publish
        // the new top, hand the engine the translated flags.
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>> = {
            let m = mount_screen;
            let mirror = self.mirror.clone();
            Rc::new(move |name, params| {
                let result = m(name, params, None);
                let v2 = result
                    .options
                    .downcast_ref::<StackScreenOptions>()
                    .cloned()
                    .unwrap_or_default();
                let android: Box<dyn Any> = Box::new(to_android_options(&v2));
                {
                    let mut mir = mirror.borrow_mut();
                    mir.options.insert(result.scope_id, v2);
                    mir.stack.push(result.scope_id);
                    mir.publish();
                }
                MountResult { node: result.node, scope_id: result.scope_id, options: android }
            })
        };

        // Release adapter: Kotlin-side pops (dispatcher pop, system
        // back, replace/reset teardown) all funnel through here — prune
        // the mirror and republish the revealed top + depth.
        let release_2: Rc<dyn Fn(u64)> = {
            let r = release_screen;
            let mirror = self.mirror.clone();
            Rc::new(move |scope_id| {
                {
                    let mut mir = mirror.borrow_mut();
                    mir.options.remove(&scope_id);
                    mir.stack.retain(|s| *s != scope_id);
                    mir.publish();
                }
                r(scope_id);
            })
        };

        // The Kotlin engine can be created inside the init borrow (pure
        // JNI object construction — it never mounts screens); only the
        // author layout build defers.
        let callbacks = AndroidNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen: release_2,
            match_path,
            depth_changed,
            nav_state: nav_state.clone(),
            defer_initial_mount,
        };
        let engine = helpers::create_stack(backend, callbacks, control.clone());
        self.engine = Some(engine.clone());

        // Deferred author-layout build: splice the chrome into the root
        // and seat the engine container inside the outlet.
        {
            let root = root.clone();
            let control_for_task = control.clone();
            let active_route = nav_state.active_route;
            let active_path = nav_state.active_path;
            let depth = nav_state.depth;
            let can_go_back = nav_state.can_go_back;
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
                    screen_chrome,
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
                    "stack-navigator (Android): the author `.layout(...)` must splat `{{nav.outlet}}`"
                );
                insert_node(root, layout_root);
                if let Some(outlet) = outlet {
                    insert_node(outlet, engine);
                }
            });
        }

        self.control = Some(control);
        root
    }

    fn attach_initial(
        &mut self,
        _backend: &mut AndroidBackend,
        screen: GlobalRef,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(engine) = self.engine.clone() else { return };
        let v2 = options
            .downcast_ref::<StackScreenOptions>()
            .cloned()
            .unwrap_or_default();
        // The walker mounted the initial through the RAW host
        // `mount_screen`, so mirror + options enter here; then seed the
        // author header for the seated screen.
        {
            let mut mir = self.mirror.borrow_mut();
            mir.options.insert(scope_id, v2.clone());
            mir.stack.push(scope_id);
        }
        helpers::attach_initial(&engine, screen, scope_id, &to_android_options(&v2));
        self.mirror.borrow().publish();
    }

    fn release(&mut self, _backend: &mut AndroidBackend) {
        if let Some(engine) = self.engine.take() {
            helpers::release(&engine);
        }
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
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
        _backend: &mut AndroidBackend,
        _slot: &'static str,
        _style: &Rc<runtime_core::StyleRules>,
    ) {
        // Toolbar hidden — header/title/button styles belong to the
        // author StackHeader. The legacy "body" slot only ever painted
        // the tab/drawer Toolbar engine's container (a registry-miss
        // no-op for stack nodes), so there is nothing native to style
        // here; screens paint their own backgrounds.
    }
}

/// Register the Android native-surface stack handler.
pub fn register(backend: &mut AndroidBackend) {
    backend
        .register_navigator::<StackPresentation, _>(|| Box::new(AndroidStackV2Handler::new()));
}

inventory::submit! { backend_android::AndroidNavigatorRegistrar(register) }
