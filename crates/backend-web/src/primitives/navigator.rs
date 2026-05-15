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
    /// The `<div>` we mount screens into.
    pub(crate) container: Node,
    /// Active stack — top is the visible screen. Always non-empty
    /// while the navigator exists; `pop` of the only entry is a no-op.
    pub(crate) stack: Vec<ScreenEntry>,
    mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> (Node, u64)>,
    release_screen: Rc<dyn Fn(u64)>,
    match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,
    depth_changed: Rc<dyn Fn(usize)>,
    /// `true` while the instance is applying a programmatic push /
    /// replace / reset. The popstate handler checks this so it
    /// doesn't try to "reconcile" a URL change we just made
    /// ourselves.
    suppress_popstate: RefCell<bool>,
}

impl NavigatorInstance {
    /// Show the top screen; hide everything else. We toggle the
    /// `ui-nav-hidden` class to flip display.
    fn refocus(&self) {
        for (i, entry) in self.stack.iter().enumerate() {
            let Ok(elem) = entry.node.clone().dyn_into::<web_sys::Element>() else {
                continue;
            };
            let is_top = i == self.stack.len() - 1;
            set_class_present(&elem, "ui-nav-hidden", !is_top);
            set_class_present(&elem, "ui-nav-active", is_top);
        }
    }

    /// Mount + append a screen with a known URL. Internal helper —
    /// callers that should also `pushState` do so themselves.
    fn mount_internal(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        let (node, scope_id) = (self.mount_screen)(name, params);
        Self::stamp_screen_class(&node);
        self.container
            .append_child(&node)
            .expect("append_child screen failed");
        self.stack.push(ScreenEntry { node, scope_id, url });
        self.refocus();
        (self.depth_changed)(self.stack.len());
    }

    fn push(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        push_state(&url);
        *self.suppress_popstate.borrow_mut() = true;
        self.mount_internal(name, params, url);
        *self.suppress_popstate.borrow_mut() = false;
    }

    fn pop(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        // Defer to the browser back; the popstate handler does the
        // actual stack mutation. One code path for both programmatic
        // pop and user-hit-back.
        history_back();
    }

    /// Pop the top screen without going through history.back. Used
    /// by the popstate handler when reconciling to a deeper URL.
    fn pop_in_place(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let top = self.stack.pop().expect("stack non-empty");
        let _ = self.container.remove_child(&top.node);
        (self.release_screen)(top.scope_id);
        self.refocus();
        (self.depth_changed)(self.stack.len());
    }

    fn replace(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        replace_state(&url);
        *self.suppress_popstate.borrow_mut() = true;
        let (node, scope_id) = (self.mount_screen)(name, params);
        Self::stamp_screen_class(&node);
        self.container
            .append_child(&node)
            .expect("append_child replacement screen failed");
        if let Some(prev) = self.stack.pop() {
            let _ = self.container.remove_child(&prev.node);
            (self.release_screen)(prev.scope_id);
        }
        self.stack.push(ScreenEntry { node, scope_id, url });
        self.refocus();
        (self.depth_changed)(self.stack.len());
        *self.suppress_popstate.borrow_mut() = false;
    }

    fn reset(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        replace_state(&url);
        *self.suppress_popstate.borrow_mut() = true;
        let (node, scope_id) = (self.mount_screen)(name, params);
        Self::stamp_screen_class(&node);
        self.container
            .append_child(&node)
            .expect("append_child reset screen failed");
        while let Some(prev) = self.stack.pop() {
            let _ = self.container.remove_child(&prev.node);
            (self.release_screen)(prev.scope_id);
        }
        self.stack.push(ScreenEntry { node, scope_id, url });
        self.refocus();
        (self.depth_changed)(self.stack.len());
        *self.suppress_popstate.borrow_mut() = false;
    }

    /// Called from the global `popstate` handler. Reconciles the
    /// current URL with our stack.
    fn on_popstate(&mut self) {
        if *self.suppress_popstate.borrow() {
            return;
        }
        let current = current_pathname();
        // Try to find the URL deeper in our stack — that means the
        // user hit back one or more times and we need to pop down.
        let mut target_index: Option<usize> = None;
        for (i, entry) in self.stack.iter().enumerate() {
            if paths_equal(&entry.url, &current) {
                target_index = Some(i);
            }
        }
        if let Some(idx) = target_index {
            // Pop everything above `idx`. The browser already moved
            // its pointer; we just unmount the visual stack to match.
            while self.stack.len() > idx + 1 {
                self.pop_in_place();
            }
            return;
        }
        // The URL isn't in our stack — forward navigation or side-
        // channel URL change. Match it as a fresh push (without
        // calling pushState, since the browser already advanced).
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
        stack: Vec::new(),
        mount_screen: callbacks.mount_screen.clone(),
        release_screen: callbacks.release_screen.clone(),
        match_path: callbacks.match_path.clone(),
        depth_changed: callbacks.depth_changed.clone(),
        suppress_popstate: RefCell::new(false),
    }));

    // Mount the initial / deep-linked stack.
    //
    // Deferred to a microtask so the build walker's outer
    // `backend.borrow_mut()` (held across the `create_navigator`
    // call) is released before `mount_screen` calls back into
    // `build(&backend, ...)`. Calling synchronously here would trip
    // a "RefCell already borrowed" panic. Same defer-trick used by
    // the Virtualizer's initial refresh on the JS side.
    let initial_path = callbacks.initial_path;
    let initial_route = callbacks.initial_route;
    let match_path = callbacks.match_path.clone();
    {
        let instance = instance.clone();
        framework_core::schedule_microtask(move || {
            let mut inst = instance.borrow_mut();
            let current = current_pathname();

            if paths_equal(&current, initial_path) {
                // Plain root mount. Replace state so we own the entry
                // (clears any prior hash/state from page load).
                replace_state(initial_path);
                inst.mount_internal(initial_route, Box::new(()), initial_path.to_string());
            } else if let Some((name, params)) = match_path(&current) {
                // Deep link to a non-root screen. Mount initial as the
                // root (so back returns to home), then push the deep
                // link. The browser's history gets two entries: root
                // (via replaceState) + deep link (via pushState).
                replace_state(initial_path);
                inst.mount_internal(initial_route, Box::new(()), initial_path.to_string());
                push_state(&current);
                inst.mount_internal(name, params, current);
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
    while let Some(screen) = inst.stack.pop() {
        let _ = inst.container.remove_child(&screen.node);
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
