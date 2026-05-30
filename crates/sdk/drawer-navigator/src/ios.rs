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
}

impl IosDrawerHandler {
    pub fn new() -> Self {
        Self { container: None }
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
            build_node,
            build_node_into: _,
            build_in_screen: _,
        } = host;

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>> = {
            let m = mount_screen;
            Rc::new(move |name, params| {
                let result = m(name, params, None);
                let new_options: Box<dyn Any> = if let Some(opts) =
                    result.options.downcast_ref::<DrawerScreenOptions>()
                {
                    Box::new(translate_options(opts))
                } else if result.options.downcast_ref::<IosScreenOptions>().is_some() {
                    result.options
                } else {
                    Box::new(IosScreenOptions::default())
                };
                MountResult {
                    node: result.node,
                    scope_id: result.scope_id,
                    options: new_options,
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
            background_color: None,
        };

        let node = helpers::create_drawer(backend.mtm(), drawer_callbacks, control.clone());
        self.container = Some(node.clone());

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
                        return;
                    };

                // If the user's sidebar root is a View, fold the
                // top + bottom safe-area sides into IT directly.
                // The framework converts safe-area sides into extra
                // padding INSIDE the marked view — so the view's
                // own background (typically the SidebarBody's white
                // surface color) keeps filling edge-to-edge, while
                // the brand row / nav links sit below the dynamic
                // island and above the home indicator.
                //
                // Putting the safe-area on a wrapper would leave a
                // transparent gap at the top: the wrapper has no
                // background, so the inset showed the page through.
                let mut sidebar_primitive = sidebar_primitive;
                if let runtime_core::Element::View {
                    safe_area_sides, ..
                } = &mut sidebar_primitive
                {
                    *safe_area_sides |= runtime_core::SafeAreaSides::TOP
                        | runtime_core::SafeAreaSides::BOTTOM;
                }

                // Wrap so the sidebar's outermost Taffy node has an
                // explicit width matching the configured drawer
                // width. The user-supplied sidebar root typically has
                // `width: auto`; without this wrap Taffy lays it out
                // at viewport width while the iOS UIView is
                // simultaneously pinned to `drawer_width` via Auto
                // Layout, so the inner nav links overflow past the
                // visible clip. With the wrap, Taffy and Auto Layout
                // agree on the width.
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
                                    ..Default::default()
                                },
                            ),
                        ))
                        .into_element();
                let sidebar_node = build_node(sized_sidebar);
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
        let ios_opts = options
            .downcast_ref::<DrawerScreenOptions>()
            .map(translate_options)
            .unwrap_or_default();
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

pub fn register(backend: &mut IosBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(IosDrawerHandler::new()));
}
