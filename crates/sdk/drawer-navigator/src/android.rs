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
    LeadingIntent, SlotProps, TrailingIntent,
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
    IntoElement, NavigatorHandle,
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
    /// Navigator-wide native-header default (from
    /// `DrawerPresentation::native_header`). Stashed at `init` so the
    /// `attach_initial` path — which doesn't see the presentation —
    /// resolves `header_shown` with the same precedence as `mount_screen`.
    native_header: bool,
}

impl AndroidDrawerHandler {
    pub fn new() -> Self {
        Self { container: None, control: None, native_header: true }
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
            // Navigator-wide native-header default — folded into each
            // screen's `header_shown` so `.native_header(false)` drops
            // the Toolbar on every screen (even title-bearing ones).
            let native_header = presentation.native_header;
            Rc::new(move |name, params| {
                let raw = m(name, params, None);
                let drawer_opts = raw
                    .options
                    .downcast::<DrawerScreenOptions>()
                    .ok();
                let android_opts =
                    drawer_options_to_android(drawer_opts, Some(&control), native_header);
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
        self.native_header = presentation.native_header;

        // Publish ambient drawer chrome so screen content renders its
        // own menu button (the page-level header pattern), mirroring the
        // web + iOS handlers. The Android drawer is always modal (no
        // pinned/wide layout), so `collapse_below = f32::INFINITY`: the
        // button shows on every viewport. The screen's header reads this
        // via `runtime_core::primitives::navigator::ambient_drawer()` and
        // calls `open()` — the same dispatch the auto-injected Toolbar
        // hamburger used. Harmless when `native_header` stays on (the
        // screen simply won't render a page-level button).
        {
            use runtime_core::primitives::navigator::DrawerChrome;
            let c = control.clone();
            let open: Rc<dyn Fn()> = Rc::new(move || {
                c.dispatch(NavCommand::Custom(Rc::new(HelpersDrawerCmd::Open)));
            });
            runtime_core::primitives::navigator::chrome::_set_ambient_drawer(Some(DrawerChrome {
                open,
                collapse_below: f32::INFINITY,
            }));
        }

        // Materialize the SDK's sidebar Element, deferred to a
        // microtask so the outer `backend.borrow_mut()` (held across
        // `init`) has released by the time `build_node` re-enters the
        // walker. Once built, hand the `GlobalRef` to the helpers crate's
        // `drawer_attach_sidebar`.
        // Two API surfaces during the SlotProps migration:
        //   1. `leading_slot` — new shape, builder takes `SlotProps`
        //      (preferred when set; matches web / iOS / macOS handlers).
        //   2. `sidebar` — legacy, builder takes `DrawerSlotProps`.
        //
        // The user's docs site calls `.leading_with(...)`, which only
        // populates `leading_slot`. Without this branch the Android
        // sidebar was never built — the hamburger fired all the way
        // through the dispatch chain to `openDrawerProgrammatic`, but
        // `drawerChild` was null so the call was a no-op.
        let leading_slot_owned = presentation.leading_slot.borrow_mut().take();
        let legacy_sidebar = presentation.sidebar.borrow().clone();
        if leading_slot_owned.is_some() || legacy_sidebar.is_some() {
            let container_for_microtask = node.clone();
            let nav_state = nav_state.clone();
            let is_open_cap = is_open;
            let control_cap = control.clone();
            let drawer_width = presentation.drawer_width;
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
                // Push this navigator onto the ambient stack so any
                // `Link` primitives built inside the builder capture
                // it as their target. Without this push the sidebar
                // runs OUTSIDE any navigator's `mount_screen` (it's a
                // deferred microtask) — `Link::new` calls
                // `ambient_navigator()` which returns `None`, the
                // captured `target` is `None`, and `on_activate`
                // silently returns on every tap. Guard pops on drop.
                let _ambient =
                    runtime_core::primitives::navigator::AmbientNavGuard::push(
                        control_cap.clone(),
                    );

                let sidebar_primitive: runtime_core::Element =
                    if let Some(builder) = leading_slot_owned {
                        let open_drawer: Rc<dyn Fn()> = {
                            let c = control_cap.clone();
                            Rc::new(move || {
                                c.dispatch(NavCommand::Custom(Rc::new(
                                    HelpersDrawerCmd::Open,
                                )));
                            })
                        };
                        let close_drawer: Rc<dyn Fn()> = {
                            let c = control_cap.clone();
                            Rc::new(move || {
                                c.dispatch(NavCommand::Custom(Rc::new(
                                    HelpersDrawerCmd::Close,
                                )));
                            })
                        };
                        let pop: Rc<dyn Fn()> = {
                            let c = control_cap.clone();
                            Rc::new(move || {
                                c.dispatch(NavCommand::Pop);
                            })
                        };
                        // Fields the Android drawer doesn't yet track
                        // natively (leading/trailing intent, screen
                        // title) get default-valued signals — same
                        // shape as the iOS path. Scroll context is
                        // `None` because Android's drawer body owns
                        // its own per-screen scroll, not the drawer.
                        let props = SlotProps {
                            active_route: nav_state.active_route,
                            active_path: nav_state.active_path.clone(),
                            depth: nav_state.depth,
                            can_go_back: nav_state.can_go_back,
                            is_open: is_open_cap,
                            leading_intent: runtime_core::signal!(
                                LeadingIntent::OpenDrawer
                            ),
                            trailing_intent: runtime_core::signal!(
                                TrailingIntent::None
                            ),
                            screen_title: runtime_core::signal!(String::new()),
                            on_select,
                            open_drawer,
                            close_drawer,
                            pop,
                            scroll: None,
                        };
                        builder(props)
                    } else if let Some(sidebar_builder) = legacy_sidebar {
                        let on_close: Rc<dyn Fn()> = {
                            let control = control_cap.clone();
                            Rc::new(move || {
                                control.dispatch(NavCommand::Custom(Rc::new(
                                    HelpersDrawerCmd::Close,
                                )));
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
                        sidebar_builder(props)
                    } else {
                        return;
                    };

                // The sidebar is a plain full-height view — NOT a
                // scroll view. The drawer makes no assumptions about its
                // content; the author styles it (a background set here
                // spans the whole panel) and opts into scrolling by
                // making the sidebar's own child a full-height
                // `scroll_view`. We only impose the panel geometry the
                // author can't know: `width = drawer_width`, `height =
                // 100%`. (The author's sidebar is built via `build_node`
                // standalone — its Taffy root has no parent — so without
                // this wrap its children would lay out against the full
                // viewport width.) `flex_shrink: 0` keeps wide content
                // from collapsing the fixed width; `Column` stacks the
                // sidebar's children. Matches the macOS handler's plain
                // sidebar container.
                //
                // This used to wrap in a `scroll_view`, which broke the
                // fill: an Android ScrollView sizes its content to the
                // content's own height, so a short sidebar left the
                // panel transparent below it (the background didn't reach
                // the bottom). A plain view gives the author's sidebar a
                // definite full-height box to fill.
                //
                // The DrawerLayout's panel LP (set in `attachDrawer`)
                // and this view's Taffy width are both `drawer_width`,
                // so they agree on the panel size. `is_drawer_panel_child`
                // in `backend-android-mobile` keeps the layout pass from
                // overwriting the LP.
                let sized_sidebar: runtime_core::Element =
                    runtime_core::view(vec![sidebar_primitive])
                        .with_style(std::rc::Rc::new(runtime_core::StyleSheet::r#static(
                            runtime_core::StyleRules {
                                width: Some(
                                    runtime_core::Length::Px(drawer_width).into(),
                                ),
                                height: Some(runtime_core::Length::pct(100.0).into()),
                                flex_direction: Some(runtime_core::FlexDirection::Column),
                                flex_shrink: Some(0.0f32.into()),
                                ..Default::default()
                            },
                        )))
                        .into_element();
                let sidebar = build_node(sized_sidebar);
                android_navigator_helpers::drawer_attach_sidebar(
                    &container_for_microtask,
                    sidebar,
                );
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
            self.native_header,
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
    native_header: bool,
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
        // Fold in the navigator-wide native-header default. A per-screen
        // `header_shown` still wins; otherwise `native_header = false`
        // force-hides so even title-bearing screens drop their Toolbar.
        header_shown: crate::resolve_header_shown(opts.header_shown, native_header),
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

/// Install the drawer navigator handler on an Android backend. Call once
/// at startup so `Element::Navigator`s carrying a [`DrawerPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(AndroidDrawerHandler::new()));
    // Runtime-server client path: lets `dev-client` rebuild the
    // presentation from wire config and drive this same handler, so the
    // real DrawerLayout chrome renders over the wire (not the old
    // structural fallback). The sidebar leaf adopts via the
    // `WireSidebarAdopt` sentinel materialized by the walker. No-op cost
    // under `--local`.
    crate::register_wire_drawer_factory();
    // Programmatic `drawer.open()/close()/toggle()` over the wire. See the
    // iOS handler's `register` for the rationale — `dev-client` can't name
    // the helper `DrawerCmd`, so translate the generic verb here.
    wire::register_drawer_state_translator(|verb| {
        let cmd = match verb {
            wire::DrawerStateVerb::Open => HelpersDrawerCmd::Open,
            wire::DrawerStateVerb::Close => HelpersDrawerCmd::Close,
            wire::DrawerStateVerb::Toggle => HelpersDrawerCmd::Toggle,
        };
        std::rc::Rc::new(cmd) as std::rc::Rc<dyn std::any::Any>
    });
}
