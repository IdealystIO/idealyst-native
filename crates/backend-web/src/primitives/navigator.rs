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
//! The path-matching machinery lives in framework-core
//! (`match_pattern` + `RouteParams::from_segments`); this file only
//! adds the *DOM* and *history-API* glue. A future SSR backend can
//! call the same `match_path` callback against an HTTP request path,
//! mount the matched screen, render the resulting tree to a string,
//! and never touch `window` / `history`.

use crate::WebBackend;
use framework_core::primitives::navigator::{
    NavCommand, NavigatorCallbacks, NavigatorControl, NavigatorHandle, NavigatorOps,
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
pub(crate) struct ScreenEntry {
    node: Node,
    scope_id: u64,
    url: String,
}

/// Per-navigator instance state. Lives in
/// `WebBackend::navigator_instances`, keyed by the container's
/// `data-navigator-id`.
pub(crate) struct NavigatorInstance {
    /// The outer `<div>` whose `data-navigator-id` attribute keys
    /// this instance. When no layout is set, screens append
    /// directly here; when a layout is set, screens append into
    /// `outlet` instead.
    pub(crate) container: Node,
    /// When a layout is registered, the framework supplies a
    /// dedicated `<div>` (built by `LayoutPlan.outlet_ref`) and we
    /// mount screens into that instead of the container. `None`
    /// means no layout — screens go into `container`.
    pub(crate) outlet: Option<Node>,
    /// Active stack — top is the visible screen. Always non-empty
    /// while the navigator exists; `pop` of the only entry is a no-op.
    ///
    /// In **no-layout mode** every push appends to `stack` and the
    /// previous entry stays in the DOM (hidden via CSS class) so
    /// scroll/form state survives navigation. In **layout mode**
    /// the outlet holds exactly one child — `stack` has exactly
    /// one entry at all times, and the URL history for "where to
    /// go back to" is tracked separately in `url_history`.
    pub(crate) stack: Vec<ScreenEntry>,
    /// Layout-mode-only: URLs of previously-visited screens, in
    /// push order (top = most recently navigated away from). On
    /// pop, the URL is matched back against the route table and
    /// the resulting screen is rebuilt. In no-layout mode this
    /// stays empty.
    pub(crate) url_history: Vec<String>,
    /// `build_layout` closure, retained across the navigator's
    /// lifetime. The framework's `build_layout` populates an
    /// internal scope slot during the microtask call; the closure
    /// itself owns the only `Rc` clone of that slot, so dropping
    /// the closure also drops the scope (and all layout effects
    /// die with it). Holding it here keeps the layout's reactive
    /// effects alive past the microtask. `None` means no layout
    /// was registered.
    #[allow(dead_code)]
    pub(crate) build_layout_retainer:
        Option<Rc<dyn Fn() -> framework_core::LayoutPlan<Node>>>,
    mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> (Node, u64)>,
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
    nav_state: framework_core::NavState,
    depth_changed: Rc<dyn Fn(usize)>,
    /// `true` while the instance is applying a programmatic push /
    /// replace / reset. The popstate handler checks this so it
    /// doesn't try to "reconcile" a URL change we just made
    /// ourselves.
    suppress_popstate: RefCell<bool>,
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

    /// Mount + append a screen with a known URL. Internal helper —
    /// callers that should also `pushState` do so themselves.
    fn mount_internal(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        let (node, scope_id) = (self.mount_screen)(name, params);
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
pub(crate) fn create(
    b: &mut WebBackend,
    callbacks: NavigatorCallbacks<Node>,
    control: Rc<NavigatorControl>,
) -> Node {
    ensure_navigator_css(b);

    let container = b
        .doc
        .create_element("div")
        .expect("create_element nav container failed");
    b.apply_default_class(&container);
    set_class_present(&container, "ui-nav-root", true);

    let nav_id = b.next_navigator_id;
    b.next_navigator_id += 1;
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
    }));

    // Mount the initial / deep-linked stack.
    //
    // Deferred to a microtask so the build walker's outer
    // `backend.borrow_mut()` (held across the `create_navigator`
    // call) is released before `mount_screen` calls back into
    // `build(&backend, ...)`. Calling synchronously here would trip
    // a "RefCell already borrowed" panic. Same defer-trick used by
    // the Virtualizer's initial refresh on the JS side.
    //
    // The same microtask also builds the user-supplied layout (if
    // any) and stashes the outlet DOM node on the instance, so
    // screens append into the outlet rather than the container.
    let initial_path = callbacks.initial_path;
    let initial_route = callbacks.initial_route;
    let match_path = callbacks.match_path.clone();
    let build_layout = callbacks.build_layout.clone();
    {
        let instance = instance.clone();
        let container_for_layout = container_node.clone();
        framework_core::schedule_microtask(move || {
            // If the author registered a layout, build its subtree
            // and stash the outlet node before mounting any screens.
            // `build_layout` invokes the framework's build walker
            // internally, so this microtask is also the only safe
            // time to call it (outside the `create_navigator`
            // borrow window).
            if let Some(build_layout) = build_layout.as_ref() {
                let plan = build_layout();
                // Insert the layout root into the navigator
                // container. The container is the framework-
                // visible navigator node; the layout root goes
                // inside it, and the outlet (somewhere inside the
                // layout root) is where screens append.
                container_for_layout
                    .append_child(&plan.root)
                    .expect("append_child layout root failed");
                // Resolve the outlet ref to its native DOM node.
                // The framework's `bind(outlet_ref)` on the
                // freshly-created outlet `View` populated this
                // handle during build. The handle wraps an
                // `Rc<dyn Any>` over the backend's `Node`, which
                // for web is `web_sys::Node`.
                if let Some(handle) = plan.outlet_ref.get() {
                    let any_node = handle.as_any();
                    if let Some(node) = any_node.downcast_ref::<Node>() {
                        instance.borrow_mut().outlet = Some(node.clone());
                    }
                }
            }

            let mut inst = instance.borrow_mut();
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

    // Install the per-instance dispatcher.
    {
        let instance = instance.clone();
        control.install(Box::new(move |cmd| match cmd {
            NavCommand::Push { name, url, params } => {
                instance.borrow_mut().push(name, params, url)
            }
            NavCommand::Pop => instance.borrow_mut().pop(),
            NavCommand::Replace { name, url, params } => {
                instance.borrow_mut().replace(name, params, url)
            }
            NavCommand::Reset { name, url, params } => {
                instance.borrow_mut().reset(name, params, url)
            }
        }));
    }

    // Wire up the global popstate handler if this is the first
    // navigator on the page, and register this instance with it.
    register_popstate_target(instance.clone());

    b.navigator_instances.insert(nav_id, NavigatorEntry { instance, control });
    container_node
}

/// Tear down a navigator: release every still-mounted screen scope
/// and drop the instance entry (which drops the dispatcher closures).
pub(crate) fn release(b: &mut WebBackend, node: &Node) {
    let Some(nav_id) = navigator_id_of(node) else {
        return;
    };
    let Some(entry) = b.navigator_instances.remove(&nav_id) else {
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

pub(crate) fn make_handle(b: &WebBackend, node: &Node) -> NavigatorHandle {
    let Some(nav_id) = navigator_id_of(node) else {
        return NavigatorHandle::new(Rc::new(()), &WebNavigatorOps);
    };
    let Some(entry) = b.navigator_instances.get(&nav_id) else {
        return NavigatorHandle::new(Rc::new(()), &WebNavigatorOps);
    };
    NavigatorHandle::with_control(Rc::new(()), &WebNavigatorOps, entry.control.clone())
}

/// Per-instance bundle stored on the backend so `make_handle` /
/// `release` can find the right navigator at lookup time.
pub(crate) struct NavigatorEntry {
    pub(crate) instance: Rc<RefCell<NavigatorInstance>>,
    pub(crate) control: Rc<NavigatorControl>,
}

struct WebNavigatorOps;
impl NavigatorOps for WebNavigatorOps {}

pub(crate) type NavigatorInstances = HashMap<u32, NavigatorEntry>;

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
    /// on `window`; it dispatches into every backend's nav instances
    /// on each fire. Stored as a `Closure` so wasm-bindgen keeps the
    /// JS function alive for the page's lifetime.
    static POPSTATE_LISTENER: RefCell<Option<Closure<dyn FnMut(web_sys::PopStateEvent)>>> =
        const { RefCell::new(None) };
    /// Strong-Rc registry of every live navigator on the page. The
    /// WebBackend's `navigator_instances` map holds another strong
    /// Rc; `unregister_popstate_target` drops this list's entry so
    /// release lifecycles complete normally.
    static POPSTATE_TARGETS: RefCell<Vec<Rc<RefCell<NavigatorInstance>>>> =
        const { RefCell::new(Vec::new()) };
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

fn ensure_navigator_css(b: &mut WebBackend) {
    if b.navigator_css_injected {
        return;
    }
    let css = ".ui-nav-root{position:relative;width:100%;height:100%;}\
               .ui-nav-screen{position:absolute;inset:0;width:100%;height:100%;}\
               .ui-nav-hidden{display:none;}";
    if let Some(head) = b.doc.head() {
        if let Ok(style) = b.doc.create_element("style") {
            style.set_text_content(Some(css));
            let _ = head.append_child(&style);
        }
    }
    b.navigator_css_injected = true;
}
