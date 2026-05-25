// The entire crate is wasm-only; on non-wasm targets it compiles to
// an empty rlib so `cargo check --workspace` succeeds without
// dragging `web-sys` / `backend-web` (both target-gated deps) into
// scope. Per-SDK crates already cfg-gate their `mod web` references
// to wasm32, so nothing host-side touches this module.
#![cfg(target_arch = "wasm32")]

//! `Primitive::Navigator` — a `<div>` driven by the browser's
//! `history` API.
//!
//! # Model
//!
//! The web navigator is an SPA router. Each push calls
//! `history.pushState` so the URL bar updates and the browser's back
//! button is wired into our pop logic. A global `popstate` handler
//! reconciles the current `window.location.pathname` against our
//! per-instance URL stack:
//!
//! - If the new URL matches a URL deeper in our stack, pop screens
//!   off until we're back at the matching URL.
//! - If the new URL doesn't appear in our stack but matches a known
//!   route, treat it as a forward navigation: push that screen.
//! - If the URL doesn't match any route, leave the stack alone — the
//!   user can hit back to recover.
//!
//! # Deep linking
//!
//! On mount, the backend reads `window.location.pathname` and tries
//! to match it against the registered routes via the framework's
//! `match_path` callback. If the URL is the initial route's path (or
//! `/` with the initial route at `/`), the initial route mounts as
//! the only screen. If it's a different known route, that route
//! mounts as the root *under* the navigator's declared initial — so
//! tapping back returns the user to the app's home screen, mirroring
//! a normal SPA's UX.
//!
//! # SSR
//!
//! The path-matching machinery lives in runtime-core
//! (`match_pattern` + `RouteParams::from_segments`); this file only
//! adds the *DOM* and *history-API* glue. A future SSR backend can
//! call the same `match_path` callback against an HTTP request path,
//! mount the matched screen, render the resulting tree to a string,
//! and never touch `window` / `history`.

use backend_web::WebBackend;
use runtime_core::primitives::navigator::{
    DrawerHandle, DrawerNavigatorCallbacks, MountResult, NavCommand, NavigatorCallbacks,
    NavigatorControl, NavigatorHandle, NavigatorOps, TabNavigatorCallbacks, TabsHandle,
};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

/// Per-screen entry stored by the web navigator. `node` is the DOM
/// element produced for the screen; `scope_id` is the framework's
/// per-screen scope identifier, which we hand back to
/// `release_screen` on pop / replace / reset. `url` is the URL the
/// screen represents, used by the popstate reconciliation logic.
pub struct ScreenEntry {
    node: Node,
    scope_id: u64,
    url: String,
}

/// Per-navigator instance state. Lives in
/// `WebBackend::navigator_instances`, keyed by the container's
/// `data-navigator-id`.
pub struct NavigatorInstance {
    /// The outer `<div>` whose `data-navigator-id` attribute keys
    /// this instance. When no layout is set, screens append
    /// directly here; when a layout is set, screens append into
    /// `outlet` instead.
    pub container: Node,
    /// When a layout is registered, the framework supplies a
    /// dedicated `<div>` (built by `LayoutPlan.outlet_ref`) and we
    /// mount screens into that instead of the container. `None`
    /// means no layout — screens go into `container`.
    pub outlet: Option<Node>,
    /// Active stack — top is the visible screen. Always non-empty
    /// while the navigator exists; `pop` of the only entry is a no-op.
    ///
    /// In **no-layout mode** every push appends to `stack` and the
    /// previous entry stays in the DOM (hidden via CSS class) so
    /// scroll/form state survives navigation. In **layout mode**
    /// the outlet holds exactly one child — `stack` has exactly
    /// one entry at all times, and the URL history for "where to
    /// go back to" is tracked separately in `url_history`.
    pub stack: Vec<ScreenEntry>,
    /// Layout-mode-only: URLs of previously-visited screens, in
    /// push order (top = most recently navigated away from). On
    /// pop, the URL is matched back against the route table and
    /// the resulting screen is rebuilt. In no-layout mode this
    /// stays empty.
    pub url_history: Vec<String>,
    /// `build_layout` closure, retained across the navigator's
    /// lifetime. The framework's `build_layout` populates an
    /// internal scope slot during the microtask call; the closure
    /// itself owns the only `Rc` clone of that slot, so dropping
    /// the closure also drops the scope (and all layout effects
    /// die with it). Holding it here keeps the layout's reactive
    /// effects alive past the microtask. `None` means no layout
    /// was registered.
    #[allow(dead_code)]
    pub build_layout_retainer:
        Option<Rc<dyn Fn() -> runtime_core::LayoutPlan<Node>>>,
    mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<Node>>,
    release_screen: Rc<dyn Fn(u64)>,
    match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,
    /// Framework-owned reactive nav-state. Updated by
    /// `mount_internal` on every actual mount so the layout's
    /// reactive chrome (route-name title, can-go-back button)
    /// reflects whatever's *currently visible* — not whatever
    /// command was most recently dispatched. This matters most
    /// for pop, where `NavigatorControl::dispatch` can't know
    /// the new active route's name (it knows only that we're
    /// popping).
    nav_state: runtime_core::NavState,
    depth_changed: Rc<dyn Fn(usize)>,
    /// `true` while the instance is applying a programmatic push /
    /// replace / reset. The popstate handler checks this so it
    /// doesn't try to "reconcile" a URL change we just made
    /// ourselves.
    suppress_popstate: RefCell<bool>,
    /// When `true`, the create-time microtask skips its URL-based
    /// auto-mount — initial mounting comes through
    /// [`attach_initial_with_node`] with a screen node the caller
    /// already has. The runtime-server dev-client sets this so the wire's
    /// `NavigatorAttachInitial` is what actually mounts the home
    /// screen (it carries the canonical server-built subtree).
    defer_initial_mount: bool,
}

impl NavigatorInstance {
    /// The DOM node screens append to: the layout's outlet `<div>`
    /// when a layout is registered, otherwise the navigator's own
    /// container `<div>`.
    fn mount_point(&self) -> &Node {
        self.outlet.as_ref().unwrap_or(&self.container)
    }

    /// True when a user-supplied layout is in effect. In layout
    /// mode the outlet holds exactly one child at a time
    /// (React-Router-style); without a layout, screens stack with
    /// hide/show classes so their DOM survives push/pop.
    fn has_layout(&self) -> bool {
        self.outlet.is_some()
    }

    /// Toggle hide/show on stacked screens. Only meaningful in
    /// no-layout mode; in layout mode the outlet has a single child
    /// and visibility is moot.
    fn refocus(&self) {
        if self.has_layout() {
            return;
        }
        for (i, entry) in self.stack.iter().enumerate() {
            let Ok(elem) = entry.node.clone().dyn_into::<web_sys::Element>() else {
                continue;
            };
            let is_top = i == self.stack.len() - 1;
            set_class_present(&elem, "ui-nav-hidden", !is_top);
            set_class_present(&elem, "ui-nav-active", is_top);
        }
    }

    /// Drop the current top screen's DOM + scope. Called by
    /// layout-mode push/pop/replace/reset to clear the outlet
    /// before mounting the next screen. The popped entry's URL is
    /// preserved in the caller's local — we only clean up DOM +
    /// scope here.
    fn detach_top(&mut self) -> Option<ScreenEntry> {
        let top = self.stack.pop()?;
        let _ = self.mount_point().remove_child(&top.node);
        (self.release_screen)(top.scope_id);
        Some(top)
    }

    /// Mount an externally-built screen node. Used by the runtime-server path
    /// where `mount_screen` shouldn't be invoked (the screen node
    /// and scope id are already known — server-built, shipped via
    /// the wire).
    ///
    /// Mirrors the tail of [`Self::mount_internal`] from after the
    /// `mount_screen` call: stamps the class, appends to the
    /// mount point, records the stack entry, and fires
    /// `depth_changed`. The reactive `nav_state` signals aren't
    /// updated here — runtime-server mode renders layout chrome on the
    /// server, so those signals only matter for local-mode
    /// rendering.
    ///
    /// Skipped in local-render mode (`defer_initial_mount = false`)
    /// — the create-time microtask handles the initial mount via
    /// `mount_internal`. The walker calls
    /// `Backend::*_navigator_attach_initial` unconditionally, so
    /// without this guard the screen would land in the DOM twice
    /// (once from here, once from the microtask).
    fn attach_initial_with_node(&mut self, screen: Node, scope_id: u64) {
        if !self.defer_initial_mount {
            return;
        }
        if !self.has_layout() {
            Self::stamp_screen_class(&screen);
        }
        self.mount_point()
            .append_child(&screen)
            .expect("append_child screen failed (attach_initial)");
        self.stack.push(ScreenEntry {
            node: screen,
            scope_id,
            url: String::new(),
        });
        (self.depth_changed)(self.stack.len());
    }

    /// Mount + append a screen with a known URL. Internal helper —
    /// callers that should also `pushState` do so themselves.
    fn mount_internal(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        let result = (self.mount_screen)(name, params);
        let node = result.node;
        let scope_id = result.scope_id;
        // The `ui-nav-screen` class adds `position:absolute; inset:0`
        // so stacked screens overlap inside the `.ui-nav-root`
        // container — the right behavior for no-layout mode, where
        // the navigator's own div is the positioning context.
        //
        // In layout mode the screen is a normal child of the
        // outlet (which lives somewhere inside the user's layout
        // tree). Absolute-positioning to `.ui-nav-root` would
        // teleport the screen out of the layout's bounds — so we
        // skip the class entirely and let the screen flow as a
        // regular block.
        if !self.has_layout() {
            Self::stamp_screen_class(&node);
        }
        self.mount_point()
            .append_child(&node)
            .expect("append_child screen failed");
        // Update the reactive nav-state to match the newly visible
        // screen. `NavigatorControl::dispatch` sets these on
        // Push/Replace/Reset commands, but it can't on Pop — and
        // popstate-driven reconciliation never goes through dispatch
        // at all. Updating from `mount_internal` covers every
        // path uniformly: every actual visible-screen change runs
        // through here.
        self.nav_state.active_route.set(name);
        self.nav_state.active_path.set(url.clone());
        self.stack.push(ScreenEntry { node, scope_id, url });
        self.refocus();
        (self.depth_changed)(self.logical_depth());
    }

    /// Logical stack depth, including URL-only history entries
    /// (layout mode). The framework's `depth` signal mirrors this
    /// so `can_go_back` works regardless of whether previous
    /// screens have DOM nodes still attached or are pending
    /// rebuild from URL history.
    fn logical_depth(&self) -> usize {
        self.stack.len() + self.url_history.len()
    }

    fn push(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        push_state(&url);
        *self.suppress_popstate.borrow_mut() = true;
        if self.has_layout() {
            // Layout mode: outlet holds one child at a time.
            // Remember the URLs we've visited (for pop to rebuild
            // from) by leaving them in `url_history`. The DOM +
            // scope of the previous screen are dropped now.
            if let Some(prev) = self.detach_top() {
                self.url_history.push(prev.url);
            }
        }
        self.mount_internal(name, params, url);
        *self.suppress_popstate.borrow_mut() = false;
    }

    fn pop(&mut self) {
        if !self.can_pop() {
            return;
        }
        // Defer to the browser back; the popstate handler does the
        // actual stack mutation. One code path for both programmatic
        // pop and user-hit-back.
        history_back();
    }

    /// True if there's somewhere to go back to.
    fn can_pop(&self) -> bool {
        if self.has_layout() {
            !self.url_history.is_empty()
        } else {
            self.stack.len() > 1
        }
    }

    /// Pop the top screen without going through history.back. Used
    /// by the popstate handler when reconciling to a deeper URL.
    fn pop_in_place(&mut self) {
        if !self.can_pop() {
            return;
        }
        if self.has_layout() {
            // Layout mode: outlet currently shows the active
            // screen. Pop the previous URL off the history, drop
            // the active screen, re-match the URL against the
            // route table, and mount the result.
            self.detach_top();
            let prev_url = match self.url_history.pop() {
                Some(u) => u,
                None => return,
            };
            if let Some((name, params)) = (self.match_path)(&prev_url) {
                self.mount_internal(name, params, prev_url);
            }
        } else {
            self.detach_top();
            self.refocus();
            (self.depth_changed)(self.logical_depth());
        }
    }

    fn replace(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        replace_state(&url);
        *self.suppress_popstate.borrow_mut() = true;
        // Replace = drop the active screen, mount a new one in its
        // place. `url_history` is unchanged because depth is
        // unchanged.
        self.detach_top();
        self.mount_internal(name, params, url);
        *self.suppress_popstate.borrow_mut() = false;
    }

    fn reset(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        replace_state(&url);
        *self.suppress_popstate.borrow_mut() = true;
        // Drop everything and mount the new screen as the only one.
        while self.detach_top().is_some() {}
        self.url_history.clear();
        self.mount_internal(name, params, url);
        *self.suppress_popstate.borrow_mut() = false;
    }

    /// Called from the global `popstate` handler. Reconciles the
    /// current URL with our internal navigation state.
    fn on_popstate(&mut self) {
        if *self.suppress_popstate.borrow() {
            return;
        }
        let current = current_pathname();

        if self.has_layout() {
            // Layout mode: outlet has one active screen; the URL
            // we navigated *from* sits at the top of url_history.
            // If `current` matches a URL in url_history, the user
            // hit back N times — pop down to that URL. Otherwise
            // they navigated forward to a new URL, push it.
            let pos = self
                .url_history
                .iter()
                .rposition(|u| paths_equal(u, &current));
            if let Some(idx) = pos {
                // Drop the active screen, drop history entries
                // above `idx`, mount the entry at `idx`.
                self.detach_top();
                let prev_url = self.url_history.remove(idx);
                // Anything between idx and the end is now skipped
                // — discard those entries too (the user could only
                // skip forward to them via a `go(n)`-like call,
                // which the browser collapses into a single
                // popstate fire).
                self.url_history.truncate(idx);
                if let Some((name, params)) = (self.match_path)(&prev_url) {
                    self.mount_internal(name, params, prev_url);
                }
                return;
            }
            // Forward navigation we haven't seen. Detach the
            // current top, mount the new URL. The current URL
            // goes into history.
            if let Some((name, params)) = (self.match_path)(&current) {
                if let Some(prev) = self.detach_top() {
                    self.url_history.push(prev.url);
                }
                self.mount_internal(name, params, current);
            }
            return;
        }

        // No-layout mode: walk the visible stack and pop down to
        // the matching URL if found.
        let target_index = self
            .stack
            .iter()
            .rposition(|entry| paths_equal(&entry.url, &current));
        if let Some(idx) = target_index {
            while self.stack.len() > idx + 1 {
                self.pop_in_place();
            }
            return;
        }
        // Unknown URL — forward navigation. Mount it as a fresh
        // push.
        if let Some((name, params)) = (self.match_path)(&current) {
            self.mount_internal(name, params, current);
        }
    }

    fn stamp_screen_class(node: &Node) {
        if let Ok(elem) = node.clone().dyn_into::<web_sys::Element>() {
            set_class_present(&elem, "ui-nav-screen", true);
        }
    }
}

/// Add or remove `class` on `elem` by splicing the existing
/// space-separated class string. Drop-in stand-in for `class_list`
/// without the corresponding web-sys feature gate.
fn set_class_present(elem: &web_sys::Element, class: &str, present: bool) {
    let current = elem.get_attribute("class").unwrap_or_default();
    let mut tokens: Vec<&str> = current
        .split_whitespace()
        .filter(|t| *t != class)
        .collect();
    if present {
        tokens.push(class);
    }
    let next = tokens.join(" ");
    let _ = elem.set_attribute("class", &next);
}

/// Create the navigator container, mount the initial / deep-linked
/// stack, install the dispatcher on the control plane, and wire up
/// the global popstate handler.
///
/// On web, all navigator kinds (stack, tabs, drawer) reduce to
/// "screen-swap with author-supplied layout chrome": the layout slot
/// owns the visual differences (tab bar, drawer side panel), and
/// the underlying screen-swap machinery is identical. So this
/// function builds the per-instance state + microtask + popstate
/// registration unconditionally; the *dispatcher* installed on
/// `control` is what varies by kind, and the caller supplies it via
/// `install_dispatcher`.
fn create_inner<F>(
    b: &mut WebBackend,
    callbacks: NavigatorCallbacks<Node>,
    control: Rc<NavigatorControl>,
    install_dispatcher: F,
) -> Node
where
    F: FnOnce(Rc<RefCell<NavigatorInstance>>),
{
    ensure_navigator_css(b);

    let doc = web_sys::window()
        .expect("window")
        .document()
        .expect("document");
    let container = doc
        .create_element("div")
        .expect("create_element nav container failed");
    // No `.ui-default` — see view.rs. The `.ui-nav-root` rule
    // sets `position: relative` on the container; layout chrome
    // (when present) stacks via normal block flow inside.
    set_class_present(&container, "ui-nav-root", true);

    let nav_id = NEXT_NAVIGATOR_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    let _ = container.set_attribute("data-navigator-id", &nav_id.to_string());

    let container_node: Node = container.unchecked_into();

    let instance = Rc::new(RefCell::new(NavigatorInstance {
        container: container_node.clone(),
        outlet: None,
        stack: Vec::new(),
        url_history: Vec::new(),
        mount_screen: callbacks.mount_screen.clone(),
        release_screen: callbacks.release_screen.clone(),
        match_path: callbacks.match_path.clone(),
        depth_changed: callbacks.depth_changed.clone(),
        nav_state: callbacks.nav_state.clone(),
        suppress_popstate: RefCell::new(false),
        // Keep the layout-build closure alive for the navigator's
        // lifetime. The closure owns the only `Rc` reference to
        // the layout's reactive `Scope` — if it dropped after the
        // microtask, every reactive Effect in the layout chrome
        // (route-name Text, can-go-back Button, …) would free
        // immediately and stop firing on navigation.
        build_layout_retainer: callbacks.build_layout.clone(),
        defer_initial_mount: callbacks.defer_initial_mount,
    }));

    // Mount the initial / deep-linked stack.
    //
    // Deferred to a microtask so the build walker's outer
    // `backend.borrow_mut()` (held across the `create_stack_navigator`
    // call) is released before `mount_screen` calls back into
    // `build(&backend, ...)`. Calling synchronously here would trip
    // a "RefCell already borrowed" panic. Same defer-trick used by
    // the Virtualizer's initial refresh on the JS side.
    //
    // Layout (if any) is already attached by the walker before the
    // microtask runs — `Backend::attach_navigator_layout` populates
    // `instance.outlet`. The microtask just covers initial-screen
    // auto-mount.
    let initial_path = callbacks.initial_path;
    let initial_route = callbacks.initial_route;
    let match_path = callbacks.match_path.clone();
    {
        let instance = instance.clone();
        runtime_core::schedule_microtask(move || {
            // Layout (if any) was already wired by the walker via
            // `attach_navigator_layout` — see
            // `walker::invoke_layout_and_attach`. The microtask
            // exists for everything *after* layout: URL-driven
            // auto-mount of the initial screen.

            let mut inst = instance.borrow_mut();

            // runtime-server / deferred-mount mode: skip URL-driven auto-mount.
            // The caller mounts via `stack_navigator_attach_initial` with
            // an externally-built screen node — the framework's wire
            // delivers it shortly after this microtask runs.
            if inst.defer_initial_mount {
                return;
            }

            let current = current_pathname();

            if paths_equal(&current, initial_path) {
                // Plain root mount. Replace state so we own the entry
                // (clears any prior hash/state from page load).
                replace_state(initial_path);
                inst.mount_internal(initial_route, Box::new(()), initial_path.to_string());
            } else if let Some((name, params)) = match_path(&current) {
                // Deep link to a non-root screen. We want back to
                // return to the home route, so the browser's
                // history gets two entries — but the rendered DOM
                // only shows the deep-linked screen.
                //
                // - No-layout mode: mount the home screen as the
                //   bottom of the visible stack (hidden), then
                //   push the deep-link on top.
                // - Layout mode: skip mounting home entirely;
                //   instead push the home URL into url_history so
                //   pop can rebuild it later from match_path.
                replace_state(initial_path);
                if inst.has_layout() {
                    inst.url_history.push(initial_path.to_string());
                    push_state(&current);
                    inst.mount_internal(name, params, current);
                } else {
                    inst.mount_internal(initial_route, Box::new(()), initial_path.to_string());
                    push_state(&current);
                    inst.mount_internal(name, params, current);
                }
            } else {
                // Unrecognized URL. Fall back to the initial route.
                replace_state(initial_path);
                inst.mount_internal(initial_route, Box::new(()), initial_path.to_string());
            }
        });
    }

    // Install the per-instance dispatcher. The exact command set
    // each kind accepts differs (stack uses Push/Pop/Replace/Reset;
    // tabs uses Select; drawer uses Select + Open/Close/Toggle), so
    // the caller picks the dispatcher.
    install_dispatcher(instance.clone());

    // Wire up the global popstate handler if this is the first
    // navigator on the page, and register this instance with it.
    register_popstate_target(instance.clone());

    NAVIGATOR_INSTANCES.with(|m| {
        m.borrow_mut().insert(nav_id, NavigatorEntry { instance, control });
    });
    container_node
}

/// Stack navigator entry point. Installs a dispatcher that accepts
/// Push / Pop / Replace / Reset and panics on tab/drawer commands.
pub fn create(
    b: &mut WebBackend,
    callbacks: NavigatorCallbacks<Node>,
    control: Rc<NavigatorControl>,
) -> Node {
    create_inner(b, callbacks, control.clone(), move |instance| {
        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, url, params, .. } => {
                instance.borrow_mut().push(name, params, url)
            }
            NavCommand::Pop => instance.borrow_mut().pop(),
            NavCommand::Replace { name, url, params, .. } => {
                instance.borrow_mut().replace(name, params, url)
            }
            NavCommand::Reset { name, url, params, .. } => {
                instance.borrow_mut().reset(name, params, url)
            }
            NavCommand::Select { .. }
            | NavCommand::OpenDrawer
            | NavCommand::CloseDrawer
            | NavCommand::ToggleDrawer => {
                panic!(
                    "stack Navigator received a non-stack NavCommand — \
                     check that the dispatched command's shape matches \
                     the navigator kind (stack: Push/Pop/Replace/Reset)"
                );
            }
        }));
    })
}

/// Tab navigator entry point on web.
///
/// Web treats all navigator kinds as "screen-swap with author
/// chrome" — the underlying machinery (mount/release scopes, URL
/// history, popstate reconciliation) is identical to the stack
/// navigator. The `TabSpec` metadata (labels, icons, badges) is
/// ignored at this layer; authors render their own tab bar through
/// `.layout(...)` and call `handle.select(...)` from the bar
/// buttons. This mirrors how `idea-ui` is expected to ship themed
/// tab bars: the bar is *just a styled View*, not a navigator
/// concern.
///
/// `Select` maps to `Replace`: the new screen takes the outlet's
/// active slot, no URL stack growth.
pub fn create_tab(
    b: &mut WebBackend,
    callbacks: TabNavigatorCallbacks<Node>,
    control: Rc<NavigatorControl>,
) -> Node {
    let TabNavigatorCallbacks { navigator, .. } = callbacks;
    create_inner(b, navigator, control.clone(), move |instance| {
        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Select { name, url, params, .. } => {
                // Selecting the already-active tab is a no-op.
                {
                    let inst = instance.borrow();
                    if inst.stack.last().map(|e| paths_equal(&e.url, &url)).unwrap_or(false) {
                        return;
                    }
                }
                instance.borrow_mut().replace(name, params, url);
            }
            // `Reset` is accepted as a "go back to initial tab"
            // hatch — useful for analytics flows that programmatically
            // re-home the user.
            NavCommand::Reset { name, url, params, .. } => {
                instance.borrow_mut().reset(name, params, url)
            }
            NavCommand::Push { .. }
            | NavCommand::Pop
            | NavCommand::Replace { .. }
            | NavCommand::OpenDrawer
            | NavCommand::CloseDrawer
            | NavCommand::ToggleDrawer => {
                panic!(
                    "TabNavigator received an unsupported NavCommand — \
                     tabs accept Select (and Reset for go-home); pushing / \
                     popping a tab navigator is a programmer error"
                );
            }
        }));
    })
}

/// Drawer navigator entry point on web.
///
/// Same machinery as tabs: screen-swap with author chrome. The
/// drawer's visual side panel is rendered by the author's
/// `.layout(...)` closure; the drawer commands flip the
/// `callbacks.is_open` signal that the layout subscribes to.
pub fn create_drawer(
    b: &mut WebBackend,
    callbacks: DrawerNavigatorCallbacks<Node>,
    control: Rc<NavigatorControl>,
) -> Node {
    let DrawerNavigatorCallbacks {
        navigator,
        is_open,
        open_changed,
        ..
    } = callbacks;
    create_inner(b, navigator, control.clone(), move |instance| {
        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Select { name, url, params, .. } => {
                {
                    let inst = instance.borrow();
                    if inst.stack.last().map(|e| paths_equal(&e.url, &url)).unwrap_or(false) {
                        // Selecting the already-active screen still
                        // closes the drawer (matches the typical
                        // mobile UX: tap an item, drawer slides
                        // shut). The is_open signal flip is the
                        // only side effect.
                        is_open.set(false);
                        open_changed(false);
                        return;
                    }
                }
                instance.borrow_mut().replace(name, params, url);
                // Auto-close the drawer on selection. The is_open
                // signal is what the author's layout subscribes
                // to; we update it directly here AND call
                // `open_changed` so any analytics/listeners fire.
                is_open.set(false);
                open_changed(false);
            }
            NavCommand::Reset { name, url, params, .. } => {
                instance.borrow_mut().reset(name, params, url);
                is_open.set(false);
                open_changed(false);
            }
            NavCommand::OpenDrawer => {
                is_open.set(true);
                open_changed(true);
            }
            NavCommand::CloseDrawer => {
                is_open.set(false);
                open_changed(false);
            }
            NavCommand::ToggleDrawer => {
                let now = !is_open.get();
                is_open.set(now);
                open_changed(now);
            }
            NavCommand::Push { .. }
            | NavCommand::Pop
            | NavCommand::Replace { .. } => {
                panic!(
                    "DrawerNavigator received an unsupported NavCommand — \
                     drawer accepts Select + Open/Close/ToggleDrawer (and \
                     Reset for go-home)"
                );
            }
        }));
    })
}

/// runtime-server / deferred-mount entry point. Called by the SDK
/// handler's `attach_initial` when the navigator was created with
/// `defer_initial_mount = true`. Mounts the externally-built `screen`
/// node into the navigator's outlet without going through `mount_screen`.
///
/// Post-create helpers (`attach_initial`, `attach_layout`, `release`,
/// the `make_*_handle` family) read from the thread-local
/// `NAVIGATOR_INSTANCES` registry and don't need a backend handle —
/// the SDK handler invokes them with just the navigator `Node`.
pub fn attach_initial(navigator: &Node, screen: Node, scope_id: u64) {
    let Some(nav_id) = navigator_id_of(navigator) else {
        return;
    };
    let instance = NAVIGATOR_INSTANCES.with(|m| {
        m.borrow().get(&nav_id).map(|e| e.instance.clone())
    });
    let Some(instance) = instance else { return };
    instance.borrow_mut().attach_initial_with_node(screen, scope_id);
}

/// runtime-server layout attach. The dev-side recording backend ran the user's
/// `.layout(...)` closure, the wire shipped every node it built
/// (sidebar, chrome, outlet placeholder), and now we have to (1)
/// drop the layout root into the navigator container and (2) record
/// the outlet node so subsequent `attach_initial`s mount screens
/// inside the layout's outlet — not the bare container, which would
/// dump the screen on top of the sidebar.
pub fn attach_layout(navigator: &Node, root: Node, outlet: Node) {
    let Some(nav_id) = navigator_id_of(navigator) else {
        return;
    };
    let instance = NAVIGATOR_INSTANCES.with(|m| {
        m.borrow().get(&nav_id).map(|e| e.instance.clone())
    });
    let Some(instance) = instance else { return };
    let mut inst = instance.borrow_mut();
    // Container is freshly created with no children in runtime-server mode
    // (defer_initial_mount = true, so the create-time microtask
    // bails before mounting anything). Safe to just append the
    // layout root.
    inst.container
        .append_child(&root)
        .expect("attach_navigator_layout: append root to container failed");
    inst.outlet = Some(outlet);
}

/// Tear down a navigator: release every still-mounted screen scope
/// and drop the instance entry (which drops the dispatcher closures).
pub fn release(node: &Node) {
    let Some(nav_id) = navigator_id_of(node) else {
        return;
    };
    let Some(entry) = NAVIGATOR_INSTANCES.with(|m| m.borrow_mut().remove(&nav_id)) else {
        return;
    };
    let mut inst = entry.instance.borrow_mut();
    let mp = inst.mount_point().clone();
    while let Some(screen) = inst.stack.pop() {
        let _ = mp.remove_child(&screen.node);
        (inst.release_screen)(screen.scope_id);
    }
    drop(inst);
    // Drop the strong ref from POPSTATE_TARGETS so the instance
    // (and its closures) actually free.
    unregister_popstate_target(&entry.instance);
    let _ = entry.control;
}

pub fn make_handle(node: &Node) -> NavigatorHandle {
    let Some(nav_id) = navigator_id_of(node) else {
        return NavigatorHandle::new(Rc::new(()), &WebNavigatorOps);
    };
    let control = NAVIGATOR_INSTANCES.with(|m| {
        m.borrow().get(&nav_id).map(|e| e.control.clone())
    });
    match control {
        Some(c) => NavigatorHandle::with_control(Rc::new(()), &WebNavigatorOps, c),
        None => NavigatorHandle::new(Rc::new(()), &WebNavigatorOps),
    }
}

/// Make a `TabsHandle`. Same wiring as `make_handle` but wraps the
/// underlying `NavigatorHandle` so the type-system enforces "tabs
/// only `.select(...)`, no `.push`".
pub fn make_tab_handle(node: &Node) -> TabsHandle {
    TabsHandle::from_inner(make_handle(node))
}

/// Make a `DrawerHandle`. The drawer's `is_open` probe lives behind
/// an `Rc<Cell<bool>>` shared with the dispatcher; we hand the same
/// Cell to every handle clone so they observe each other's writes.
pub fn make_drawer_handle(node: &Node) -> DrawerHandle {
    let inner = make_handle(node);
    // The probe `Cell` lives on the entry below. For now we use a
    // fresh `Cell` per handle — the authoritative state is the
    // signal carried in `DrawerNavigatorCallbacks::is_open`, which
    // every reactive read should go through. The non-reactive
    // `is_open()` probe is just a convenience for one-shot reads.
    DrawerHandle::from_inner(inner, Rc::new(std::cell::Cell::new(false)))
}

/// Per-instance bundle stored on the backend so `make_handle` /
/// `release` can find the right navigator at lookup time.
pub struct NavigatorEntry {
    pub instance: Rc<RefCell<NavigatorInstance>>,
    pub control: Rc<NavigatorControl>,
}

struct WebNavigatorOps;
impl NavigatorOps for WebNavigatorOps {}

pub type NavigatorInstances = HashMap<u32, NavigatorEntry>;

// ---------------------------------------------------------------------------
// URL + history helpers
// ---------------------------------------------------------------------------

fn navigator_id_of(node: &Node) -> Option<u32> {
    let elem: web_sys::Element = node.clone().dyn_into().ok()?;
    elem.get_attribute("data-navigator-id")?.parse().ok()
}

/// `window.location.pathname` with `/` normalization. Always returns
/// a leading-`/` string; empty becomes `/`.
fn current_pathname() -> String {
    let Some(win) = web_sys::window() else {
        return "/".to_string();
    };
    let Ok(path) = win.location().pathname() else {
        return "/".to_string();
    };
    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

/// Path equivalence with trailing-slash tolerance, matching the
/// framework's `match_pattern` semantics.
fn paths_equal(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> String {
        s.split('/')
            .filter(|seg| !seg.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    };
    norm(a) == norm(b)
}

fn push_state(url: &str) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let Ok(hist) = win.history() else {
        return;
    };
    let _ = hist.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(url));
}

fn replace_state(url: &str) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let Ok(hist) = win.history() else {
        return;
    };
    let _ = hist.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(url));
}

fn history_back() {
    let Some(win) = web_sys::window() else {
        return;
    };
    let Ok(hist) = win.history() else {
        return;
    };
    let _ = hist.back();
}

// ---------------------------------------------------------------------------
// Global popstate handler
// ---------------------------------------------------------------------------

thread_local! {
    /// Live popstate listener. We hold one global listener registered
    /// on `window`; it dispatches into every nav instance on each
    /// fire. Stored as a `Closure` so wasm-bindgen keeps the JS
    /// function alive for the page's lifetime.
    static POPSTATE_LISTENER: RefCell<Option<Closure<dyn FnMut(web_sys::PopStateEvent)>>> =
        const { RefCell::new(None) };
    /// Strong-Rc registry of every live navigator on the page. The
    /// `NAVIGATOR_INSTANCES` map below holds another strong Rc;
    /// `unregister_popstate_target` drops this list's entry so release
    /// lifecycles complete normally.
    static POPSTATE_TARGETS: RefCell<Vec<Rc<RefCell<NavigatorInstance>>>> =
        const { RefCell::new(Vec::new()) };
    /// Per-instance bundle indexed by `data-navigator-id` attribute
    /// on the navigator container. Mirrors what used to live on
    /// `WebBackend.navigator_instances`; moved here so the SDK owns
    /// it (backend-web has no nav-specific state anymore).
    pub static NAVIGATOR_INSTANCES: RefCell<NavigatorInstances> =
        RefCell::new(HashMap::new());
    /// Monotonic counter for `data-navigator-id`. Mirrors
    /// `WebBackend.next_navigator_id` from before the port. Never
    /// reused — release frees the entry but the id is gone.
    pub static NEXT_NAVIGATOR_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// `true` once the navigator's CSS (`.ui-nav-root`,
    /// `.ui-nav-screen`, `.ui-nav-hidden`) has been injected into
    /// `<head>` once for the page's lifetime.
    static NAVIGATOR_CSS_INJECTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn register_popstate_target(target: Rc<RefCell<NavigatorInstance>>) {
    POPSTATE_TARGETS.with(|t| t.borrow_mut().push(target));
    POPSTATE_LISTENER.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let cb = Closure::wrap(Box::new(move |_ev: web_sys::PopStateEvent| {
            // Snapshot targets so a target's on_popstate can't
            // mutate the list mid-iteration.
            let targets = POPSTATE_TARGETS.with(|t| t.borrow().clone());
            for target in targets {
                target.borrow_mut().on_popstate();
            }
        }) as Box<dyn FnMut(web_sys::PopStateEvent)>);
        if let Some(win) = web_sys::window() {
            let _ = win.add_event_listener_with_callback(
                "popstate",
                cb.as_ref().unchecked_ref(),
            );
        }
        *slot.borrow_mut() = Some(cb);
    });
}

fn unregister_popstate_target(target: &Rc<RefCell<NavigatorInstance>>) {
    POPSTATE_TARGETS.with(|t| {
        t.borrow_mut().retain(|other| !Rc::ptr_eq(other, target));
    });
}

// ---------------------------------------------------------------------------
// One-shot CSS injection
// ---------------------------------------------------------------------------

fn ensure_navigator_css(_b: &mut WebBackend) {
    NAVIGATOR_CSS_INJECTED.with(|injected| {
        if injected.get() {
            return;
        }
        let css = ".ui-nav-root{position:relative;width:100%;height:100%;}\
                   .ui-nav-screen{position:absolute;inset:0;width:100%;height:100%;}\
                   .ui-nav-hidden{display:none;}";
        let Some(win) = web_sys::window() else { return };
        let Some(doc) = win.document() else { return };
        if let Some(head) = doc.head() {
            if let Ok(style) = doc.create_element("style") {
                style.set_text_content(Some(css));
                let _ = head.append_child(&style);
            }
        }
        injected.set(true);
    });
}
