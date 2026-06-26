//! iOS-backend handler for the Drawer navigator SDK.
//!
//! The UIKit machinery (outer container, scrim, embedded
//! `UINavigationController` for header bar, sidebar slide-in animation,
//! drawer open/close dispatcher) lives in the `ios-navigator-helpers`
//! crate, shared with stack + tab. This module's `IosDrawerHandler`
//! synthesizes an `IosDrawerCallbacks` from the framework-supplied
//! `NavigatorHost` + the SDK's `DrawerPresentation`, then calls
//! `ios_navigator_helpers::create_drawer`.

use crate::{
    BarButton, DrawerCmd, DrawerHandle, DrawerPresentation, DrawerScreenOptions, DrawerSide,
    DrawerSlotProps, DrawerType, LeadingIntent, MountPolicy, SlotProps, TrailingIntent,
    DRAWER_OPS,
};
use backend_ios::{with_backend, IosBackend, IosNode};
use runtime_core::IntoElement;
use ios_navigator_helpers::{
    self as helpers, BarButton as HelpersBarButton, DrawerCmd as HelpersDrawerCmd,
    DrawerSide as HelpersDrawerSide, DrawerType as HelpersDrawerType, IosDrawerCallbacks,
    IosNavCallbacks, IosScreenOptions, MountPolicy as HelpersMountPolicy,
};
use runtime_core::primitives::navigator::{
    MountResult, NavCommand, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;

pub struct IosDrawerHandler {
    container: Option<IosNode>,
    /// Active-route signal, kept on the handler so `attach_initial`
    /// can default the nav-bar title to the route name when the
    /// author hasn't set one (same fallback the `mount_screen`
    /// closure applies for subsequent pushes).
    active_route: Option<runtime_core::Signal<&'static str>>,
    /// Navigator-wide native-header default (from
    /// `DrawerPresentation::native_header`). Stashed at `init` so
    /// `attach_initial` resolves `header_shown` with the same precedence
    /// as `mount_screen` — without it the INITIAL screen's nav bar
    /// stayed visible (only navigated-to screens, which go through
    /// `mount_2arg`, were hidden).
    native_header: bool,
}

impl IosDrawerHandler {
    pub fn new() -> Self {
        Self { container: None, active_route: None, native_header: true }
    }
}
impl Default for IosDrawerHandler {
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

fn translate_bar_button(btn: &BarButton) -> HelpersBarButton {
    HelpersBarButton {
        icon: btn.icon.clone(),
        on_press: btn.on_press.clone(),
        tint: btn.tint.clone(),
    }
}

fn translate_options(opts: &DrawerScreenOptions) -> IosScreenOptions {
    IosScreenOptions {
        title: opts.title.clone(),
        header_shown: opts.header_shown,
        header_left: opts.header_left.as_ref().map(translate_bar_button),
        header_right: opts.header_right.as_ref().map(translate_bar_button),
        header_background: opts.header_background.clone(),
        header_tint: opts.header_tint.clone(),
        title_color: opts.title_color.clone(),
        mount_policy: opts.mount_policy.map(mount_policy_to_helpers),
        // The drawer has no native swipe-back affordance to lock, so
        // back-lock is a stack-only knob; leave it unset here.
        back_enabled: None,
        // Per-screen full-screen is a stack-navigator concern; the drawer
        // doesn't drive it per screen.
        fullscreen: None,
    }
}

impl NavigatorHandler<IosBackend> for IosDrawerHandler {
    fn init(
        &mut self,
        backend: &mut IosBackend,
        host: NavigatorHost<IosNode>,
        presentation: Rc<dyn Any>,
    ) -> IosNode {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("IosDrawerHandler: presentation must be DrawerPresentation");

        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path: _,
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node: _,
            build_node_scoped,
            build_node_into: _,
            build_in_screen: _,
            // `resolve_entry` + `base`: framework/web deep-link plumbing; the
            // iOS drawer handler doesn't read them.
            ..
        } = host;

        // Navigator-wide native-header default — folded into each
        // screen's `header_shown` below via `resolve_header_shown`, so
        // `.native_header(false)` hides the nav bar on every screen
        // (even title-bearing ones) unless a screen overrides.
        let native_header = presentation.native_header;
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>> = {
            let m = mount_screen;
            Rc::new(move |name, params| {
                let result = m(name, params, None);
                let mut new_options: IosScreenOptions = if let Some(opts) =
                    result.options.downcast_ref::<DrawerScreenOptions>()
                {
                    translate_options(opts)
                } else if let Some(opts) =
                    result.options.downcast_ref::<IosScreenOptions>()
                {
                    opts.clone()
                } else {
                    IosScreenOptions::default()
                };
                // Fold in the navigator-wide native-header default. A
                // per-screen `header_shown` still wins; otherwise
                // `native_header = false` force-hides the nav bar.
                new_options.header_shown =
                    crate::resolve_header_shown(new_options.header_shown, native_header);
                // No title fallback. Showing `route.name()` (the
                // short kebab identifier) as the nav-bar title is a
                // misleading crutch — author intent is "no title".
                // Match Android, which renders an empty Toolbar
                // title when `options.title` is None.
                MountResult {
                    node: result.node,
                    scope_id: result.scope_id,
                    options: Box::new(new_options),
                }
            })
        };

        let navigator = IosNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            depth_changed,
            nav_state: nav_state.clone(),
            defer_initial_mount,
        };

        let is_open = presentation.is_open;
        let open_changed: Rc<dyn Fn(bool)> = {
            let signal = is_open;
            Rc::new(move |o| signal.set(o))
        };
        let active_changed_helpers: Rc<dyn Fn(&'static str)> = {
            let ac = active_changed;
            Rc::new(move |name| ac(name, String::new()))
        };

        let drawer_callbacks = IosDrawerCallbacks {
            navigator,
            side: side_to_helpers(presentation.side),
            drawer_type: type_to_helpers(presentation.drawer_type),
            drawer_width: presentation.drawer_width,
            swipe_to_open: presentation.swipe_to_open,
            mount_policy: mount_policy_to_helpers(presentation.mount_policy),
            is_open,
            // The helper crate's `build_content` slot is the
            // closure-shaped sidebar builder. The SDK's typed
            // `SidebarBuilder` returns a `Element`; we wrap it in a
            // microtask-deferred closure that calls `host.build_node`
            // to materialize and returns the resulting `IosNode`. The
            // helpers crate's drawer engine doesn't currently invoke
            // `build_content` directly — the SDK still attaches the
            // sidebar via the deferred microtask below — so this slot
            // is left `None`.
            build_content: None,
            active_changed: active_changed_helpers,
            open_changed,
            // Default the drawer chrome (nav-bar background + body)
            // to the active theme's `color-background` token. Resolving
            // inside this closure subscribes the helpers' bg Effect to
            // the token's signal, so swapping themes re-paints the
            // chrome without any author wiring. Authors can still
            // override via `.header(...)` on the builder, which routes
            // through `apply_drawer_header_style` and replaces the
            // default once it sets a `background`.
            background_color: Some(Rc::new(|| {
                runtime_core::Tokenized::<runtime_core::Color>::token(
                    "color-background",
                    runtime_core::Color("#ffffff".into()),
                )
                .resolve()
            })),
        };

        let node = helpers::create_drawer(backend.mtm(), drawer_callbacks, control.clone());
        self.container = Some(node.clone());
        self.active_route = Some(nav_state.active_route);
        self.native_header = native_header;

        // Publish ambient drawer chrome so screen content renders its
        // own menu button (the page-level header pattern), mirroring the
        // web handler. On iOS the drawer is always modal — there is no
        // pinned/wide layout — so `collapse_below = f32::INFINITY`: the
        // button shows on every viewport. The screen's header reads this
        // via `runtime_core::primitives::navigator::ambient_drawer()` and
        // calls `open()` to slide the drawer in (same path the formerly
        // native hamburger used). Published unconditionally; harmless when
        // `native_header` is left on (the screen simply won't render a
        // page-level button).
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

        // Sidebar build + attach — deferred so the outer
        // `backend.borrow_mut()` window (held across this `init` call)
        // is released before the walker re-enters via `build_node`.
        //
        // Two API surfaces during the SlotProps migration:
        //   1. `leading_slot` — new shape, builder takes `SlotProps`
        //      (preferred when set, matches web/Android handlers).
        //   2. `sidebar` — legacy, builder takes `DrawerSlotProps`.
        //
        // The website calls `.leading_with(...)`, which only
        // populates `leading_slot`. Without this branch the iOS
        // sidebar was never built — the menu button opened the
        // scrim but no panel slid in.
        let leading_slot_owned = presentation.leading_slot.borrow_mut().take();
        let legacy_sidebar = presentation.sidebar.borrow().clone();
        if leading_slot_owned.is_some() || legacy_sidebar.is_some() {
            let active_route = nav_state.active_route;
            let active_path = nav_state.active_path.clone();
            let depth = nav_state.depth;
            let can_go_back = nav_state.can_go_back;
            let is_open_sig = presentation.is_open;
            let control_for_select = control.clone();
            let control_for_open = control.clone();
            let control_for_close = control.clone();
            let control_for_pop = control.clone();
            let control_for_legacy_close = control.clone();
            let control_for_ambient = control.clone();
            let _ = control;
            let node_for_attach = node.clone();
            let drawer_width = presentation.drawer_width;
            runtime_core::schedule_microtask(move || {
                // Construct the sidebar Element INSIDE the navigator's retained
                // chrome scope via `build_node_scoped`, NOT `build_node`. The
                // sidebar contains `#[component]` bodies (idea-ui's animated
                // `Switch`) whose standalone `effect!` runs at construction;
                // building the Element here and only then calling `build_node`
                // would create those effects with no active scope → freed when
                // the body returns → run once, never re-fire (frozen Switch
                // thumb). See `NavigatorHost::build_node_scoped`.
                let builder: Box<dyn FnOnce() -> runtime_core::Element> = Box::new(move || {
                let on_select: Rc<dyn Fn(&'static str)> = {
                    let c = control_for_select;
                    Rc::new(move |name| {
                        c.dispatch(NavCommand::Select {
                            name,
                            url: String::new(),
                            params: Box::new(()),
                            state: None,
                        });
                    })
                };
                // Push this navigator onto the ambient stack so any
                // `Link` primitives built inside the builder capture
                // it as their target. The sidebar microtask runs
                // OUTSIDE any navigator's `mount_screen`, so without
                // this `Link::new` captures `target=None` and
                // `on_activate` silently no-ops on tap. Guard pops at
                // end of scope.
                let _ambient =
                    runtime_core::primitives::navigator::AmbientNavGuard::push(
                        control_for_ambient.clone(),
                    );

                let sidebar_primitive: runtime_core::Element =
                    if let Some(builder) = leading_slot_owned {
                        let open_drawer: Rc<dyn Fn()> = {
                            let c = control_for_open;
                            Rc::new(move || {
                                c.dispatch(NavCommand::Custom(Rc::new(
                                    HelpersDrawerCmd::Open,
                                )));
                            })
                        };
                        let close_drawer: Rc<dyn Fn()> = {
                            let c = control_for_close;
                            Rc::new(move || {
                                c.dispatch(NavCommand::Custom(Rc::new(
                                    HelpersDrawerCmd::Close,
                                )));
                            })
                        };
                        let pop: Rc<dyn Fn()> = {
                            let c = control_for_pop;
                            Rc::new(move || {
                                c.dispatch(NavCommand::Pop);
                            })
                        };
                        // SlotProps fields the iOS drawer doesn't yet
                        // track natively (leading/trailing intent,
                        // screen title) get default-valued signals;
                        // the website's sidebar only reads
                        // `active_route` so these defaults are
                        // observably equivalent. Scroll context is
                        // None — UINavigationController owns the
                        // body's scroll, not the drawer.
                        let props = SlotProps {
                            active_route,
                            active_path,
                            depth,
                            can_go_back,
                            is_open: is_open_sig,
                            leading_intent: runtime_core::signal!(LeadingIntent::OpenDrawer),
                            trailing_intent: runtime_core::signal!(TrailingIntent::None),
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
                            let c = control_for_legacy_close;
                            Rc::new(move || {
                                c.dispatch(NavCommand::Custom(Rc::new(
                                    HelpersDrawerCmd::Close,
                                )));
                            })
                        };
                        let props = DrawerSlotProps {
                            active_route,
                            active_path,
                            depth,
                            can_go_back,
                            is_open: is_open_sig,
                            on_select,
                            on_close,
                        };
                        sidebar_builder(props)
                    } else {
                        // Unreachable: guarded by the `is_some()` check above.
                        unreachable!("sidebar build with neither slot set")
                    };

                // Match the web drawer's architecture:
                //
                //   web (drawer-navigator/web.rs + web-navigator-helpers):
                //     <div class="ui-nav-drawer-sidebar"        ← scroll container
                //          style="flex:0 0 auto; height:100%; overflow-y:auto;
                //                 width: drawer_width">
                //       <SidebarBody …>                         ← author's view
                //         …children…
                //       </SidebarBody>
                //     </div>
                //
                // The `.ui-nav-drawer-sidebar` div carries the
                // scroll + viewport-sized chrome; the author's
                // `SidebarBody` sits inside it as ordinary flow
                // content. The SidebarBody's own padding and
                // background stay where the author put them and
                // operate inside the scroll container, so the
                // scrollbar tracks the right edge of the scroll
                // container (= the panel edge) — the UX the web
                // drawer ships.
                //
                // iOS mirror: a `scroll_view` with `width =
                // drawer_width` and `height = 100%` plays the role
                // of `.ui-nav-drawer-sidebar`. The author's sidebar
                // primitive renders inside it unchanged.
                //
                // Safe-area handling lives on the author's `View`
                // (mutated in place below), not the scroll_view —
                // safe-area is a no-op on web (browsers handle
                // chrome themselves), so an author with a
                // cross-target sidebar doesn't have to opt in
                // anywhere. On iOS the framework turns it into
                // top/bottom padding INSIDE the SidebarBody, which
                // then scrolls with the rest of the content.
                // Safe-area handling: let UIScrollView's
                // `contentInsetAdjustmentBehavior:.automatic` inset
                // the content for the device's status bar / dynamic
                // island / home indicator. The author's SidebarBody
                // padding (the framework's `padding_*` rules)
                // remains the visual padding inside the safe-area
                // zone. Adding `.safe_area(TOP | BOTTOM)` here would
                // SUM the author padding with the device inset
                // (Taffy writes `author + safe_area_extra` into the
                // node's `padding` Rect), pushing the brand row
                // ~75 pt below the top edge on a notched device —
                // far more than the 16 pt of breathing room the
                // author asked for.
                let sidebar_primitive = sidebar_primitive;
                // The sidebar is a plain full-height view — NOT a scroll
                // view. The drawer makes no assumptions about its content
                // and imposes no background: the author styles the sidebar
                // (a background set on it spans the whole panel), and opts
                // into scrolling by making the sidebar's own child a
                // full-height `scroll_view`. We impose only the panel
                // geometry the author can't know: `width = drawer_width`,
                // `height = 100%`. (The author's sidebar is built via
                // `build_node` standalone — its Taffy root has no parent —
                // so without this wrap its children would lay out against
                // the full viewport width.) `flex_shrink: 0` keeps wide
                // content from collapsing the fixed width. Matches the
                // macOS + Android handlers' plain sidebar container.
                //
                // Previously this wrapped in a `scroll_view` and painted
                // `color-surface` on it to cover scroll-overflow children.
                // With a plain view there's no overflow region — the
                // author's sidebar fills the panel and owns its surface —
                // so neither the scroll wrap nor the imposed background is
                // needed.
                let sized_sidebar: runtime_core::Element =
                    runtime_core::view(vec![sidebar_primitive])
                        .with_style(std::rc::Rc::new(
                            runtime_core::StyleSheet::r#static(
                                runtime_core::StyleRules {
                                    width: Some(
                                        runtime_core::Length::Px(drawer_width).into(),
                                    ),
                                    height: Some(
                                        runtime_core::Length::pct(100.0).into(),
                                    ),
                                    flex_direction: Some(runtime_core::FlexDirection::Column),
                                    flex_shrink: Some(0.0f32.into()),
                                    ..Default::default()
                                },
                            ),
                        ))
                        .into_element();
                sized_sidebar
                });
                let sidebar_node = build_node_scoped(builder);
                let _ = with_backend(|b| {
                    helpers::drawer_attach_sidebar(b.mtm(), &node_for_attach, sidebar_node);
                });
            });
        }

        // Quiet a lint: `_` keeps the import in scope on iOS builds
        // even when no path below directly names the SDK-side
        // `DrawerCmd` (we translate to `HelpersDrawerCmd` via the
        // `Custom` payload). The compile-time cast keeps the
        // SDK's enum exposed for downstream typed-handle work.
        let _: Option<DrawerCmd> = None;

        node
    }

    fn attach_initial(
        &mut self,
        backend: &mut IosBackend,
        screen: IosNode,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(container) = self.container.clone() else { return };
        let mut ios_opts = options
            .downcast_ref::<DrawerScreenOptions>()
            .map(translate_options)
            .unwrap_or_default();
        // Fold in the navigator-wide native-header default so the INITIAL
        // screen hides its nav bar too (mount_2arg does this for
        // navigated-to screens). Without it `.native_header(false)` left
        // the first screen's native UINavigationController bar visible.
        ios_opts.header_shown =
            crate::resolve_header_shown(ios_opts.header_shown, self.native_header);
        helpers::drawer_attach_initial(backend.mtm(), &container, screen, scope_id, &ios_opts);
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "IosDrawerHandler::on_command — helpers::create_drawer owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut IosBackend) {
        if let Some(container) = self.container.take() {
            helpers::release_tab_drawer(&container);
        }
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        // The SDK's `DrawerHandle` carries the `is_open` signal too,
        // but the framework-level `NavigatorHandle` returned here is
        // wrapped by the SDK's `RefFill::Navigator` callback in
        // `lib.rs::DrawerBuilder::bind`, which threads the is_open
        // signal in at the wrap site. Returning a plain control-wired
        // `NavigatorHandle` is enough.
        let Some(container) = self.container.as_ref() else {
            return runtime_core::NavigatorHandle::new(Rc::new(()), &DRAWER_OPS);
        };
        let _ = DrawerHandle::from_inner; // keep typed-handle ctor in scope
        helpers::make_drawer_handle(container)
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut IosBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        match slot {
            "sidebar" => helpers::apply_drawer_sidebar_style(&container, style),
            // The drawer wraps its body in a self-owned
            // `UINavigationController`, so header/title/button slots
            // route to the same chrome helpers the stack navigator uses.
            // Without these, themed `HeaderStyle.background/title/tint`
            // is silently dropped on iOS even though Android applies it.
            "header" => helpers::apply_drawer_header_style(&container, style),
            "title" => helpers::apply_drawer_title_style(&container, style),
            "button" => helpers::apply_drawer_button_style(&container, style),
            // `body` paints the screen-outlet's background — same role as
            // Android's `apply_body_style`. Without this, the
            // `HeaderStyle.body_background` slot fed by themed drawer
            // builders is silently dropped on iOS.
            "body" => helpers::apply_drawer_body_style(&container, style),
            _ => {}
        }
    }
}

/// Install the drawer navigator handler on an iOS backend. Call once
/// at startup so `Element::Navigator`s carrying a [`DrawerPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut IosBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(IosDrawerHandler::new()));
    // Runtime-server client path: lets `dev-client` rebuild the
    // presentation from wire config and drive this same handler, so the
    // real UIKit drawer chrome renders over the wire (not the old
    // structural fallback). The sidebar leaf adopts via the
    // `WireSidebarAdopt` sentinel materialized by the walker. No-op cost
    // under `--local`.
    crate::register_wire_drawer_factory();
    // Programmatic `drawer.open()/close()/toggle()` on the dev side ride
    // `Command::OpenDrawer`/`CloseDrawer`/`ToggleDrawer` over the wire.
    // `dev-client` can't name the helper `DrawerCmd`, so translate the
    // generic verb into the `Custom` payload this handler's dispatcher
    // downcasts (see `tab_drawer`'s `Custom` arm). Navigation auto-close
    // doesn't come through here — it rides the `Select` dispatch.
    wire::register_drawer_state_translator(|verb| {
        let cmd = match verb {
            wire::DrawerStateVerb::Open => HelpersDrawerCmd::Open,
            wire::DrawerStateVerb::Close => HelpersDrawerCmd::Close,
            wire::DrawerStateVerb::Toggle => HelpersDrawerCmd::Toggle,
        };
        std::rc::Rc::new(cmd) as std::rc::Rc<dyn std::any::Any>
    });
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_ios::IosNavigatorRegistrar(register)
}
