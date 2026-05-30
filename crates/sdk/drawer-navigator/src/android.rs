//! Android-backend handler for the Drawer navigator SDK.
//!
//! Synthesizes an `AndroidDrawerCallbacks` from the framework-supplied
//! `NavigatorHost` + the SDK's `DrawerPresentation`, then calls
//! `android_navigator_helpers::create_drawer`. The SDK's typed enums
//! (`DrawerSide` / `DrawerType` / `MountPolicy`) translate to the
//! helpers crate's identically-shaped variants via per-enum shims.
//!
//! Sidebar materialization: the SDK's `DrawerPresentation.sidebar` slot
//! holds a `SidebarBuilder` (closure taking `DrawerSlotProps`,
//! returning a `Element`). The Android handler defers the build to a
//! microtask, invokes `host.build_node` to materialize the Element
//! into a `GlobalRef`, then calls
//! `android_navigator_helpers::drawer_attach_sidebar`.

use crate::{
    DrawerCmd, DrawerPresentation, DrawerScreenOptions, DrawerSide, DrawerSlotProps, DrawerType,
    MountPolicy,
};
use android_navigator_helpers::{
    AndroidDrawerCallbacks, AndroidNavCallbacks, AndroidScreenOptions, BarButton,
    DrawerCmd as HelpersDrawerCmd, DrawerSide as HelpersDrawerSide,
    DrawerType as HelpersDrawerType, MountPolicy as HelpersMountPolicy,
};
use backend_android::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    primitives::navigator::{MountResult, NavCommand, NavigatorHandler, NavigatorHost, NavigatorOps},
    NavigatorHandle,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidDrawerHandler {
    container: Option<GlobalRef>,
    /// Stashed from `init` so per-screen toolbar callbacks can dispatch
    /// `Custom(DrawerCmd::Open)` from the auto-injected hamburger
    /// header_left. The SDK injects this default on every screen that
    /// doesn't already set a `header_left` — iOS gets the hamburger
    /// "for free" because `UINavigationController` persists across
    /// screen swaps; Android rebuilds the Toolbar per-screen so we
    /// inject the button at options-translation time.
    control: Option<Rc<runtime_core::NavigatorControl>>,
}

impl AndroidDrawerHandler {
    pub fn new() -> Self {
        Self { container: None, control: None }
    }
}

impl Default for AndroidDrawerHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn side_to_helpers(s: DrawerSide) -> HelpersDrawerSide {
    match s {
        DrawerSide::Start => HelpersDrawerSide::Start,
        DrawerSide::End => HelpersDrawerSide::End,
    }
}

fn type_to_helpers(t: DrawerType) -> HelpersDrawerType {
    match t {
        DrawerType::Front => HelpersDrawerType::Front,
        DrawerType::Slide => HelpersDrawerType::Slide,
    }
}

fn mount_policy_to_helpers(m: MountPolicy) -> HelpersMountPolicy {
    match m {
        MountPolicy::EagerPersistent => HelpersMountPolicy::EagerPersistent,
        MountPolicy::LazyPersistent => HelpersMountPolicy::LazyPersistent,
        MountPolicy::LazyDisposing => HelpersMountPolicy::LazyDisposing,
    }
}

impl NavigatorHandler<AndroidBackend> for AndroidDrawerHandler {
    fn init(
        &mut self,
        backend: &mut AndroidBackend,
        host: NavigatorHost<GlobalRef>,
        presentation: Rc<dyn Any>,
    ) -> GlobalRef {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("AndroidDrawerHandler: presentation must be DrawerPresentation");

        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path,
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node,
            build_node_into: _,
            build_in_screen: _,
        } = host;

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>> = {
            let m = mount_screen;
            // Per-screen swap_body in the helpers crate reads
            // `result.options` and rebuilds the Toolbar against it.
            // Translate the SDK's typed DrawerScreenOptions to the
            // helpers crate's AndroidScreenOptions and inject the
            // auto-hamburger so every screen gets a tap-to-open
            // navigation icon — iOS gets this for free from the
            // persistent UINavigationController, Android needs it
            // baked into every per-screen Toolbar.
            let control = control.clone();
            Rc::new(move |name, params| {
                let raw = m(name, params, None);
                let drawer_opts = raw
                    .options
                    .downcast::<DrawerScreenOptions>()
                    .ok();
                let android_opts = drawer_options_to_android(drawer_opts, Some(&control));
                MountResult {
                    node: raw.node,
                    scope_id: raw.scope_id,
                    options: Box::new(android_opts),
                }
            })
        };

        let navigator = AndroidNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            match_path,
            depth_changed,
            nav_state: nav_state.clone(),
            defer_initial_mount,
        };

        // Shared open-state signal — same `Signal<bool>` the
        // `DrawerHandle` exposes via `is_open_signal()` and the SDK's
        // dispatcher flips on `DrawerCmd::Open/Close/Toggle`.
        let is_open = presentation.is_open;
        let open_changed: Rc<dyn Fn(bool)> = {
            Rc::new(move |o| is_open.set(o))
        };

        let drawer_callbacks = AndroidDrawerCallbacks {
            navigator,
            side: side_to_helpers(presentation.side),
            drawer_type: type_to_helpers(presentation.drawer_type),
            drawer_width: presentation.drawer_width,
            swipe_to_open: presentation.swipe_to_open,
            mount_policy: mount_policy_to_helpers(presentation.mount_policy),
            is_open,
            active_changed,
            open_changed,
        };

        let node = android_navigator_helpers::create_drawer(backend, drawer_callbacks, control.clone());
        self.container = Some(node.clone());
        self.control = Some(control.clone());

        // Materialize the SDK's sidebar Element, deferred to a
        // microtask so the outer `backend.borrow_mut()` (held across
        // `init`) has released by the time `build_node` re-enters the
        // walker. Once built, hand the `GlobalRef` to the helpers crate's
        // `drawer_attach_sidebar`.
        let sidebar_slot = presentation.sidebar.borrow().clone();
        if let Some(sidebar_builder) = sidebar_slot {
            let container_for_microtask = node.clone();
            let nav_state = nav_state.clone();
            let is_open_cap = is_open;
            let control_cap = control.clone();
            runtime_core::schedule_microtask(move || {
                let on_select: Rc<dyn Fn(&'static str)> = {
                    let control = control_cap.clone();
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
                    let control = control_cap.clone();
                    Rc::new(move || {
                        control.dispatch(NavCommand::Custom(Rc::new(HelpersDrawerCmd::Close)));
                    })
                };
                let props = DrawerSlotProps {
                    active_route: nav_state.active_route,
                    active_path: nav_state.active_path.clone(),
                    depth: nav_state.depth,
                    can_go_back: nav_state.can_go_back,
                    is_open: is_open_cap,
                    on_select,
                    on_close,
                };
                // Push this navigator onto the ambient stack so any
                // `Link` primitives built inside `sidebar_builder`
                // capture it as their target. Without this push the
                // sidebar runs OUTSIDE any navigator's `mount_screen`
                // (it's a deferred microtask) — `Link::new` calls
                // `ambient_navigator()` which returns `None`, the
                // captured `target` is `None`, and `on_activate`
                // silently returns on every tap. Guard pops on drop.
                let _ambient =
                    runtime_core::primitives::navigator::AmbientNavGuard::push(
                        control_cap.clone(),
                    );
                let prim = sidebar_builder(props);
                let sidebar = build_node(prim);
                android_navigator_helpers::drawer_attach_sidebar(&container_for_microtask, sidebar);
            });
        }

        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut AndroidBackend,
        screen: GlobalRef,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(container) = self.container.clone() else { return };
        let android_options = drawer_options_to_android(
            options.downcast::<DrawerScreenOptions>().ok(),
            self.control.as_ref(),
        );
        android_navigator_helpers::attach_initial(&container, screen, scope_id, &android_options);
    }

    fn on_command(&mut self, _cmd: NavCommand) {
        unreachable!(
            "AndroidDrawerHandler::on_command — helpers::create_drawer owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut AndroidBackend) {
        if let Some(container) = self.container.take() {
            android_navigator_helpers::release(&container);
        }
    }

    fn make_handle(&self) -> NavigatorHandle {
        match self.container.as_ref() {
            Some(c) => android_navigator_helpers::make_handle(c),
            None => NavigatorHandle::new(Rc::new(()), &NoopDrawerOps),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut AndroidBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        match slot {
            "header" => android_navigator_helpers::apply_header_style(&container, style),
            "title" => android_navigator_helpers::apply_title_style(&container, style),
            "button" => android_navigator_helpers::apply_button_style(&container, style),
            "body" => android_navigator_helpers::apply_body_style(&container, style),
            _ => {}
        }
    }
}

struct NoopDrawerOps;
impl NavigatorOps for NoopDrawerOps {}

fn drawer_options_to_android(
    opts: Option<Box<DrawerScreenOptions>>,
    control: Option<&Rc<runtime_core::NavigatorControl>>,
) -> AndroidScreenOptions {
    let opts = opts.map(|b| *b).unwrap_or_default();
    // Auto-inject a hamburger BarButton when the screen doesn't
    // provide its own `header_left`. iOS sets this up on the
    // persistent UINavigationController in `create_drawer`; Android
    // rebuilds the Toolbar per-screen, so we ride along on the
    // options translation. The Kotlin side's `RustActionBarHelper`
    // already renders a `HamburgerDrawable` whenever any
    // header_left callback is supplied — we just need to wire one.
    let header_left = opts.header_left.as_ref().map(|btn| BarButton {
        icon: btn.icon.clone(),
        on_press: btn.on_press.clone(),
    }).or_else(|| control.map(|c| {
        let c = c.clone();
        BarButton {
            icon: "menu".to_string(),
            // Dispatch the HELPERS DrawerCmd, not the SDK's — the
            // dispatcher installed by `create_drawer` downcasts the
            // Custom payload to the helpers crate's enum.
            on_press: Rc::new(move || {
                c.dispatch(NavCommand::Custom(Rc::new(HelpersDrawerCmd::Open)));
            }),
        }
    }));
    AndroidScreenOptions {
        title: opts.title.clone(),
        header_shown: opts.header_shown,
        header_left,
        header_right: opts.header_right.as_ref().map(|btn| BarButton {
            icon: btn.icon.clone(),
            on_press: btn.on_press.clone(),
        }),
        header_background: opts.header_background.clone(),
        header_tint: opts.header_tint.clone(),
        title_color: opts.title_color.clone(),
        mount_policy: opts.mount_policy.map(mount_policy_to_helpers),
    }
}

pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(AndroidDrawerHandler::new()));
}
