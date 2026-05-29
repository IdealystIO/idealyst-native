//! Backend-neutral "primitive chrome" handler for the Drawer navigator.
//!
//! Builds the drawer layout from framework primitives using only the
//! generic [`Backend`] trait + `NavigatorHost`, so it works on any
//! primitive-rendering backend (the SSR backend registers it today).
//! Nothing here is SSR-specific — no `backend-ssr` dependency, no cfg.
//!
//! **Layout is not invented here.** The handler stamps the exact same
//! `ui-nav-drawer-*` classes the live web navigator stamps (see
//! [`css::nav_class`]) and ships [`css::NAVIGATOR_LAYOUT_CSS`] — the
//! single source of truth for navigator layout. The server's first
//! paint is therefore styled identically to the live web layout (no
//! style-flash on hydration), and there are no guessed inline styles.
//!
//! Structure (mirrors `web-navigator-helpers`):
//! `column[ top?, row[ sidebar?, body-outlet, trailing? ], bottom? ]`.
//! Wrappers are created only when their slot is set (no empty gutters).
//! The body outlet carries `ui-nav-drawer-body` plus
//! `ui-nav-drawer-body-scrolls` in the default `bottom_in_scroll` mode,
//! where the footer is the body's last child (after the screen) and
//! scrolls with it; in `bottom_pinned` mode the footer is a sibling of
//! the middle row and stays pinned.
//!
//! Each chrome slot is an author closure producing an `Element`, so it's
//! materialized via `host.build_node_into` — deferred to a microtask
//! because `build_node` can't run inside the `create_navigator` borrow
//! (the SSR backend installs a queuing scheduler that `render_path`
//! drains after the borrow releases). The active screen mounts into the
//! outlet via `attach_initial`.
//!
//! Both the next-gen slot system (`leading_with`/`top_with`/`bottom_with`
//! /`trailing_with`, via `SlotProps`) and the legacy `.sidebar` form
//! (via `DrawerSlotProps`) are supported; `leading_slot` is preferred
//! over the legacy `sidebar` when both are set, matching the web handler.
//!
//! Open/close animation + gestures are the live runtime's job on
//! hydration; the server needs the structural first paint with the
//! sidebar/footer nav links present (so crawlers see site navigation).

use crate::{DrawerPresentation, DrawerSlotProps, LeadingIntent, SidebarBuilder, SlotProps, TopSlot, TrailingIntent};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{
    AmbientNavGuard, NavState, NavigatorControl, NavigatorHandler, NavigatorHost, RegisterNavigator,
};
use runtime_core::{schedule_microtask, Backend, Element, Signal};
use std::any::Any;
use std::rc::Rc;

/// Renders a drawer navigator's slot chrome + body outlet on `B`.
pub struct DrawerChromeHandler<B: Backend> {
    /// Where the path-matched screen mounts (the body outlet).
    outlet: Option<B::Node>,
    /// In `bottom_in_scroll` mode the footer is already the body's last
    /// child, so the screen must mount BEFORE it (index 0).
    screen_at_front: bool,
}

impl<B: Backend> DrawerChromeHandler<B> {
    pub fn new() -> Self {
        Self { outlet: None, screen_at_front: false }
    }
}

impl<B: Backend> Default for DrawerChromeHandler<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// A fresh `SlotProps` for the headless render. Nav-state mirrors come
/// from the host (so a sidebar can highlight the active route — already
/// the path-matched route via the SSR initial-path seam); the rest are
/// inert (no interaction before hydration). Signals are created in the
/// navigator's scope so they stay valid through the post-mount drain.
fn make_slot_props(nav: &NavState) -> SlotProps {
    SlotProps {
        active_route: nav.active_route,
        active_path: nav.active_path,
        depth: nav.depth,
        can_go_back: nav.can_go_back,
        is_open: Signal::new(true),
        leading_intent: Signal::new(LeadingIntent::OpenDrawer),
        trailing_intent: Signal::new(TrailingIntent::None),
        screen_title: Signal::new(String::new()),
        on_select: Rc::new(|_| {}),
        open_drawer: Rc::new(|| {}),
        close_drawer: Rc::new(|| {}),
        pop: Rc::new(|| {}),
        scroll: None,
    }
}

/// Defer-build a next-gen slot (`SlotProps`) into `node`.
fn defer_slot<B: Backend + 'static>(
    bni: &Rc<dyn Fn(B::Node, Element)>,
    control: &Rc<NavigatorControl>,
    node: B::Node,
    props: SlotProps,
    builder: Box<dyn Fn(SlotProps) -> Element>,
) {
    let bni = bni.clone();
    let control = control.clone();
    schedule_microtask(move || {
        let _ambient = AmbientNavGuard::push(control);
        bni(node, builder(props));
    });
}

/// Defer-build the legacy `.sidebar` form (`DrawerSlotProps`) into `node`.
fn defer_legacy_sidebar<B: Backend + 'static>(
    bni: &Rc<dyn Fn(B::Node, Element)>,
    control: &Rc<NavigatorControl>,
    node: B::Node,
    nav: &NavState,
    builder: SidebarBuilder,
) {
    let bni = bni.clone();
    let control = control.clone();
    let props = DrawerSlotProps {
        active_route: nav.active_route,
        active_path: nav.active_path,
        depth: nav.depth,
        can_go_back: nav.can_go_back,
        is_open: Signal::new(true),
        on_select: Rc::new(|_| {}),
        on_close: Rc::new(|| {}),
    };
    schedule_microtask(move || {
        let _ambient = AmbientNavGuard::push(control);
        bni(node, builder(props));
    });
}

impl<B: Backend + 'static> NavigatorHandler<B> for DrawerChromeHandler<B> {
    fn init(
        &mut self,
        backend: &mut B,
        host: NavigatorHost<B::Node>,
        presentation: Rc<dyn Any>,
    ) -> B::Node {
        use css::nav_class as cls;

        // Ship the canonical navigator layout sheet (deduped by the
        // backend). This — not any inline style here — defines the
        // layout, identical to the live web navigator.
        backend.register_raw_css(css::NAVIGATOR_LAYOUT_CSS);

        let a11y = AccessibilityProps::default();

        // root = column[ top?, middle, bottom? ]; stamp the same pair the
        // web container carries (`ui-nav-root ui-nav-drawer-root`).
        let mut root = backend.create_view(&a11y);
        backend.attach_html_class(&root, cls::ROOT);
        backend.attach_html_class(&root, cls::DRAWER_ROOT);

        // middle = row[ sidebar?, body, trailing? ].
        let mut middle = backend.create_view(&a11y);
        backend.attach_html_class(&middle, cls::DRAWER_MIDDLE);

        // body outlet — always present; the screen mounts here.
        let outlet = backend.create_view(&a11y);
        backend.attach_html_class(&outlet, cls::DRAWER_BODY);

        let pres = presentation.downcast_ref::<DrawerPresentation>();
        let bottom_in_scroll = pres.map(|p| p.bottom_in_scroll).unwrap_or(true);
        if bottom_in_scroll {
            backend.attach_html_class(&outlet, cls::DRAWER_BODY_SCROLLS);
        }

        if let Some(pres) = pres {
            let bni = &host.build_node_into;
            let control = &host.control;
            let nav = &host.nav_state;

            // --- Leading (sidebar): prefer next-gen slot, else legacy. ---
            let leading_slot = pres.leading_slot.borrow_mut().take();
            let legacy = if leading_slot.is_none() {
                pres.sidebar.borrow_mut().take()
            } else {
                None
            };
            if leading_slot.is_some() || legacy.is_some() {
                let sidebar = backend.create_view(&a11y);
                backend.attach_html_class(&sidebar, cls::DRAWER_SIDEBAR);
                backend.insert(&mut middle, sidebar.clone());
                if let Some(builder) = leading_slot {
                    defer_slot::<B>(bni, control, sidebar, make_slot_props(nav), builder);
                } else if let Some(legacy) = legacy {
                    defer_legacy_sidebar::<B>(bni, control, sidebar, nav, legacy);
                }
            }

            // Body outlet goes after the sidebar.
            backend.insert(&mut middle, outlet.clone());

            // --- Trailing (optional) ---
            if let Some(builder) = pres.trailing_slot.borrow_mut().take() {
                let trailing = backend.create_view(&a11y);
                backend.attach_html_class(&trailing, cls::DRAWER_TRAILING);
                backend.insert(&mut middle, trailing.clone());
                defer_slot::<B>(bni, control, trailing, make_slot_props(nav), builder);
            }

            // --- Top (optional) — Custom renders an author bar; Filled is
            // the native-chrome path (no SSR/web-primitive equivalent). ---
            if let Some(TopSlot::Custom(f)) = pres.top_slot.borrow_mut().take() {
                let top = backend.create_view(&a11y);
                backend.attach_html_class(&top, cls::DRAWER_TOP);
                backend.insert(&mut root, top.clone());
                defer_slot::<B>(bni, control, top, make_slot_props(nav), f);
            }

            // middle is always present (holds the body outlet).
            backend.insert(&mut root, middle);

            // --- Bottom (optional) ---
            if let Some(builder) = pres.bottom_slot.borrow_mut().take() {
                let bottom = backend.create_view(&a11y);
                backend.attach_html_class(&bottom, cls::DRAWER_BOTTOM);
                if bottom_in_scroll {
                    // Footer is the body's last child and scrolls with the
                    // content; the screen mounts before it (see
                    // `attach_initial`), matching the web layout.
                    backend.insert(&mut outlet.clone(), bottom.clone());
                    self.screen_at_front = true;
                } else {
                    // Pinned footer: sibling of the middle row.
                    backend.insert(&mut root, bottom.clone());
                }
                defer_slot::<B>(bni, control, bottom, make_slot_props(nav), builder);
            }
        } else {
            // No presentation downcast: still produce a valid shell.
            backend.insert(&mut middle, outlet.clone());
            backend.insert(&mut root, middle);
        }

        self.outlet = Some(outlet);
        root
    }

    fn attach_initial(
        &mut self,
        backend: &mut B,
        screen: B::Node,
        _scope_id: u64,
        _options: Box<dyn Any>,
    ) {
        if let Some(mut outlet) = self.outlet.clone() {
            if self.screen_at_front {
                // Footer is already the body's last child; the screen
                // mounts before it so order is [screen, footer].
                backend.insert_at(&mut outlet, screen, 0);
            } else {
                backend.insert(&mut outlet, screen);
            }
        }
    }
}

/// Register the Drawer navigator's primitive-chrome handler on any
/// primitive-rendering backend (the SSR backend today).
pub fn register<B: RegisterNavigator>(backend: &mut B) {
    backend.register_navigator::<DrawerPresentation, _>(|| Box::new(DrawerChromeHandler::<B>::new()));
}
