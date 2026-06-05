//! Web-backend handler for the Drawer navigator SDK.
//!
//! Synthesizes a `WebDrawerCallbacks` from the framework-supplied
//! `NavigatorHost` + the SDK's `DrawerPresentation`, then calls
//! `web_navigator_helpers::create_drawer`. Kind-specific callback
//! types live in `web-navigator-helpers` after the navigator-substrate
//! refactor — the SDK's local `DrawerSide` / `DrawerType` /
//! `MountPolicy` enums translate to the helpers crate's
//! identically-shaped variants via the per-enum shims below.
//!
//! Sidebar materialization: the SDK's `DrawerPresentation.sidebar`
//! slot holds a `SidebarBuilder` (closure that takes
//! `DrawerSlotProps` and returns a `Element`). The web handler
//! wraps it in a `Fn() -> Node` closure that defers to a microtask,
//! invokes `host.build_node` against the synthesized props, and
//! returns the materialized Node. The closure is handed to the
//! helpers crate via `WebDrawerCallbacks.build_content` for the
//! helper engine to mount alongside the screen outlet.

use crate::{
    DrawerCmd, DrawerPresentation, DrawerSide, DrawerSlotProps, DrawerType, LeadingIntent,
    MountPolicy, SlotProps, TopSlot, TrailingIntent,
};
use runtime_core::Signal;
use wasm_bindgen::JsCast;
use backend_web::WebBackend;
use runtime_core::primitives::navigator::{
    MountResult, NavCommand, NavigatorHandler, NavigatorHost,
};
use std::any::Any;
use std::rc::Rc;
use web_navigator_helpers::{
    DrawerCmd as HelpersDrawerCmd, DrawerSide as HelpersDrawerSide,
    DrawerType as HelpersDrawerType, MountPolicy as HelpersMountPolicy,
    WebDrawerCallbacks, WebNavCallbacks,
};
use web_sys::Node;

pub struct WebDrawerHandler {
    /// Container `Node` returned by `helpers::create_drawer`. Same
    /// posture as the stack/tab handlers — retained for post-init
    /// dispatch.
    container: Option<Node>,
}

impl WebDrawerHandler {
    pub fn new() -> Self {
        Self { container: None }
    }
}
impl Default for WebDrawerHandler {
    fn default() -> Self {
        Self::new()
    }
}

struct NoopDrawerOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopDrawerOps {}

fn side_to_helpers(s: DrawerSide) -> HelpersDrawerSide {
    match s {
        DrawerSide::Start => HelpersDrawerSide::Left,
        DrawerSide::End => HelpersDrawerSide::Right,
    }
}

fn type_to_helpers(t: DrawerType) -> HelpersDrawerType {
    match t {
        // SDK's `Front` (slides over content with backdrop) maps to
        // the helpers crate's `Overlay`; SDK's `Slide` (pushes
        // content sideways) maps to `Slide`. The third helpers
        // variant `Permanent` is exposed only via SDK `drawer_type`
        // = a future "always visible" variant, which the SDK doesn't
        // currently expose.
        DrawerType::Front => HelpersDrawerType::Overlay,
        DrawerType::Slide => HelpersDrawerType::Slide,
    }
}

fn mount_policy_to_helpers(m: MountPolicy) -> HelpersMountPolicy {
    match m {
        MountPolicy::EagerPersistent => HelpersMountPolicy::Eager,
        MountPolicy::LazyPersistent | MountPolicy::LazyDisposing => HelpersMountPolicy::Lazy,
    }
}

impl NavigatorHandler<WebBackend> for WebDrawerHandler {
    fn init(
        &mut self,
        backend: &mut WebBackend,
        host: NavigatorHost<Node>,
        presentation: Rc<dyn Any>,
    ) -> Node {
        let presentation = presentation
            .downcast::<DrawerPresentation>()
            .expect("WebDrawerHandler: presentation must be DrawerPresentation");

        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path,
            resolve_entry,
            base,
            nav_state,
            depth_changed,
            active_changed,
            control,
            build_node,
            build_node_scoped: _,
            build_node_into: _,
            build_in_screen: _,
        } = host;

        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<Node>> = {
            let m = mount_screen;
            Rc::new(move |name, params| m(name, params, None))
        };

        let navigator = WebNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            match_path,
            base,
            resolve_entry,
            depth_changed,
            // Pass the substrate's reactive `nav_state` straight through;
            // the helpers engine updates active_route / active_path as
            // screens mount, and the sidebar builder's
            // `DrawerSlotProps` mirrors them.
            nav_state: nav_state.clone(),
            build_layout: None,
            defer_initial_mount,
        };

        // Capture the shared open-state signal from the presentation —
        // it's the SAME `Signal<bool>` the `DrawerHandle` exposes via
        // `is_open_signal()` and that the SDK's dispatcher flips on
        // `DrawerCmd::Open/Close/Toggle`. Stash a copy for the
        // change-observer closure below.
        let is_open = presentation.is_open;
        let open_changed: Rc<dyn Fn(bool)> = {
            let signal = is_open;
            Rc::new(move |o| signal.set(o))
        };

        // ---- Shared SlotProps + dispatchers for every new-API slot ----
        //
        // Built once at navigator init. Each slot closure clones the
        // `SlotProps` to invoke its builder with. Dispatcher closures
        // hold the navigator's `NavigatorControl` so calling
        // `slot.open_drawer()` dispatches the helper's `DrawerCmd`
        // verbatim (the helper's `Custom` downcast accepts the
        // helpers-crate enum; the SDK's own `DrawerCmd` mismatches —
        // see memory note `project_drawer_helpers_cmd_enum`).
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
        let open_drawer_fn: Rc<dyn Fn()> = {
            let control = control.clone();
            Rc::new(move || {
                control.dispatch(NavCommand::Custom(Rc::new(HelpersDrawerCmd::Open)));
            })
        };
        let close_drawer_fn: Rc<dyn Fn()> = {
            let control = control.clone();
            Rc::new(move || {
                control.dispatch(NavCommand::Custom(Rc::new(HelpersDrawerCmd::Close)));
            })
        };
        let pop_fn: Rc<dyn Fn()> = {
            let control = control.clone();
            Rc::new(move || {
                control.dispatch(NavCommand::Pop);
            })
        };

        // For drawer-only screens these intents never change (no
        // inner stack pushes to switch leading from `OpenDrawer` to
        // `PopStack`). When the framework grows composable
        // navigators these signals will be driven by the SDK in
        // response to depth changes. `screen_title` is a placeholder
        // for now — Phase-2 doesn't yet hook
        // `DrawerScreenOptions::title` through to it.
        let leading_intent_sig = Signal::new(LeadingIntent::OpenDrawer);
        let trailing_intent_sig = Signal::new(TrailingIntent::None);
        let screen_title_sig = Signal::new(String::new());

        // The drawer never owns scroll — the navigator's body is a plain
        // flex container. Screens (and chrome slots) own their own scroll
        // via the `scroll_view` primitive and read its handle directly, so
        // there's no navigator `ScrollContext` to build or publish.
        let slot_props = SlotProps {
            active_route: nav_state.active_route,
            active_path: nav_state.active_path.clone(),
            depth: nav_state.depth,
            can_go_back: nav_state.can_go_back,
            is_open,
            leading_intent: leading_intent_sig,
            trailing_intent: trailing_intent_sig,
            screen_title: screen_title_sig,
            on_select: on_select.clone(),
            open_drawer: open_drawer_fn.clone(),
            close_drawer: close_drawer_fn.clone(),
            pop: pop_fn.clone(),
            scroll: None,
        };

        // Publish ambient drawer chrome so screen content (not just slot
        // closures) can render its own menu button — the page-level
        // header pattern. The consumer compares `viewport_size()` against
        // `collapse_below` inside a `ui!` region, so the menu button shows
        // only when the sidebar is collapsed to a modal drawer (narrow
        // viewports). On wide viewports the sidebar is pinned and the
        // button stays hidden.
        {
            use runtime_core::primitives::navigator::DrawerChrome;
            let open = open_drawer_fn.clone();
            // Same breakpoint the responsive sidebar `@media` query uses,
            // so the menu button toggles in lockstep with the pin/modal
            // switch.
            let collapse_below = crate::navigator_pin_width();
            runtime_core::primitives::navigator::chrome::_set_ambient_drawer(Some(
                DrawerChrome { open, collapse_below },
            ));
        }

        // ---- Slot builder factory ----
        //
        // Each slot's `Fn(SlotProps) -> Element` closure is
        // curried into the helper's expected `Fn() -> Node` shape:
        // capture the props + build_node + control, push the
        // navigator onto the ambient stack so Links inside the
        // slot's primitive tree resolve to this navigator, then
        // invoke the user's builder and materialize the result.
        let mk_slot_cb = |
            builder: Box<dyn Fn(SlotProps) -> runtime_core::Element>,
        | -> Rc<dyn Fn() -> Node> {
            let build_node = build_node.clone();
            let control = control.clone();
            let props = slot_props.clone();
            Rc::new(move || {
                let _ambient =
                    runtime_core::primitives::navigator::AmbientNavGuard::push(
                        control.clone(),
                    );
                let prim = builder(props.clone());
                build_node(prim)
            })
        };

        // ---- Leading (sidebar) slot ----
        //
        // Prefer the new `leading_slot` (SlotProps-based) over the
        // legacy `sidebar` builder. Both populate the helpers'
        // `build_content` field — same DOM position, two API
        // surfaces during the migration window.
        let leading_slot_owned = presentation.leading_slot.borrow_mut().take();
        let build_content: Option<Rc<dyn Fn() -> Node>> = if let Some(builder) =
            leading_slot_owned
        {
            Some(mk_slot_cb(builder))
        } else {
            // Legacy fallback: `sidebar_with(DrawerSlotProps)`.
            let sidebar_slot = presentation.sidebar.borrow().clone();
            sidebar_slot.map(|sidebar_builder| {
                let build_node = build_node.clone();
                let nav_state = nav_state.clone();
                let is_open = is_open;
                let control = control.clone();
                let on_select_for_legacy = on_select.clone();
                let on_close_for_legacy = close_drawer_fn.clone();
                let cb: Rc<dyn Fn() -> Node> = Rc::new(move || {
                    let props = DrawerSlotProps {
                        active_route: nav_state.active_route,
                        active_path: nav_state.active_path.clone(),
                        depth: nav_state.depth,
                        can_go_back: nav_state.can_go_back,
                        is_open,
                        on_select: on_select_for_legacy.clone(),
                        on_close: on_close_for_legacy.clone(),
                    };
                    let _ambient =
                        runtime_core::primitives::navigator::AmbientNavGuard::push(
                            control.clone(),
                        );
                    let prim = sidebar_builder(props);
                    build_node(prim)
                });
                cb
            })
        };

        // ---- Top slot ----
        //
        // Currently only `TopSlot::Custom` is materialized on web.
        // `TopSlot::Filled` is reserved for Phase-3 — its
        // platform-conventional layout (leading buttons + title +
        // trailing buttons) maps directly to UIBarButtonItem /
        // Toolbar on iOS/Android, but the web rendering path needs
        // a default toolbar stylesheet that hasn't been designed
        // yet. Filled-mode top slots no-op with a console warning.
        let top_slot_owned = presentation.top_slot.borrow_mut().take();
        let build_top: Option<Rc<dyn Fn() -> Node>> = match top_slot_owned {
            Some(TopSlot::Custom(builder)) => Some(mk_slot_cb(builder)),
            Some(TopSlot::Filled { .. }) => {
                web_sys::console::warn_1(
                    &"drawer-navigator: TopSlot::Filled is not yet \
                      implemented on the web backend; use TopSlot::Custom \
                      for now"
                        .into(),
                );
                None
            }
            None => None,
        };

        let bottom_slot_owned = presentation.bottom_slot.borrow_mut().take();
        let build_bottom: Option<Rc<dyn Fn() -> Node>> =
            bottom_slot_owned.map(mk_slot_cb);

        let trailing_slot_owned = presentation.trailing_slot.borrow_mut().take();
        let build_trailing: Option<Rc<dyn Fn() -> Node>> =
            trailing_slot_owned.map(mk_slot_cb);

        let drawer_callbacks = WebDrawerCallbacks {
            navigator,
            side: side_to_helpers(presentation.side),
            drawer_type: type_to_helpers(presentation.drawer_type),
            drawer_width: presentation.drawer_width,
            mount_policy: mount_policy_to_helpers(presentation.mount_policy),
            is_open,
            build_content,
            build_top,
            build_bottom,
            build_trailing,
            active_changed,
            open_changed,
            background_color: None,
        };

        let node = web_navigator_helpers::create_drawer(backend, drawer_callbacks, control);

        // Re-assert the navigator's framework classes. The helper sets
        // `ui-nav-root ui-nav-drawer-root` on this container at creation,
        // but the walker applies the navigator's `style` AFTER `init`
        // returns via `apply_style`, which on web SWAPS the element's
        // className — clobbering those classes. They drive the responsive
        // pin/modal layout AND the `.drawer-open` toggle that slides the
        // off-canvas drawer in, so losing them silently breaks opening the
        // drawer on narrow viewports. This microtask runs after the
        // (synchronous) attach_style and adds them back alongside whatever
        // style class was minted.
        {
            let node = node.clone();
            runtime_core::schedule_microtask(move || {
                if let Some(el) = node.dyn_ref::<web_sys::Element>() {
                    let cur = el.get_attribute("class").unwrap_or_default();
                    let mut next = cur.clone();
                    for c in ["ui-nav-root", "ui-nav-drawer-root"] {
                        if !next.split_whitespace().any(|x| x == c) {
                            if !next.is_empty() {
                                next.push(' ');
                            }
                            next.push_str(c);
                        }
                    }
                    if next != cur {
                        let _ = el.set_attribute("class", &next);
                    }
                }
            });
        }

        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut WebBackend,
        screen: Node,
        scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        if let Some(container) = self.container.as_ref() {
            web_navigator_helpers::attach_initial(container, screen, scope_id);
        }
    }

    fn on_command(&mut self, _cmd: NavCommand) {
        unreachable!(
            "WebDrawerHandler::on_command — helpers::create_drawer owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut WebBackend) {
        if let Some(container) = self.container.take() {
            web_navigator_helpers::release(&container);
        }
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        match self.container.as_ref() {
            Some(c) => web_navigator_helpers::make_handle(c),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NoopDrawerOps),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut WebBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        match slot {
            // `body` paints the screen-outlet div's background — same
            // role as Android's `apply_body_style` and iOS's
            // `apply_drawer_body_style`. Without this the themed
            // `HeaderStyle.body_background` is silently dropped on web.
            "body" => web_navigator_helpers::apply_body_style(&container, style),
            _ => {}
        }
    }
}

/// Install the drawer navigator handler on a web backend. Call once
/// at startup so `Element::Navigator`s carrying a [`DrawerPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut WebBackend) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(WebDrawerHandler::new()));
    // Runtime-server client path: lets `dev-client` rebuild the
    // presentation from wire config and drive this same handler, so the
    // real `ui-nav-drawer-*` chrome renders over the wire (not the old
    // structural fallback). No-op cost under `--local`.
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

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_web::WebNavigatorRegistrar(register)
}
