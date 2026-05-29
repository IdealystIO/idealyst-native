// The entire crate is wasm-only; on non-wasm targets it compiles to
// an empty rlib so `cargo check --workspace` succeeds without
// dragging `web-sys` / `backend-web` (both target-gated deps) into
// scope. Per-SDK crates already cfg-gate their `mod web` references
// to wasm32, so nothing host-side touches this module.
#![cfg(target_arch = "wasm32")]

//! Shared web-side machinery for the three first-party navigator SDKs
//! (stack / tab / drawer).
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
//! On mount, the helper reads `window.location.pathname` and tries
//! to match it against the registered routes via the SDK-supplied
//! `match_path` callback. If the URL is the initial route's path (or
//! `/` with the initial route at `/`), the initial route mounts as
//! the only screen. If it's a different known route, that route
//! mounts as the root *under* the navigator's declared initial — so
//! tapping back returns the user to the app's home screen, mirroring
//! a normal SPA's UX.
//!
//! # Substrate boundary
//!
//! The framework's navigator substrate (runtime-core) owns the
//! kind-agnostic command vocabulary, the per-screen scope mechanics,
//! and the reactive `NavState`. Everything kind-specific — chrome,
//! typed handles, the dispatcher mapping from `NavCommand` to native
//! action — lives in the SDK crates. This helper crate is the SDK-side
//! shared engine that all three first-party web SDKs (stack-navigator,
//! tab-navigator, drawer-navigator) call into for DOM and history-API
//! glue.

use backend_web::WebBackend;
use runtime_core::primitives::navigator::{
    MountResult, NavCommand, NavState, NavigatorControl, NavigatorHandle, NavigatorOps,
};
use runtime_core::Signal;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::Node;

// ---------------------------------------------------------------------------
// Local callback bundle types
// ---------------------------------------------------------------------------
//
// Mirrors the shape of the OLD `NavigatorCallbacks<N>` that lived in
// runtime-core before the substrate refactor. Each SDK crate fills one
// of these in and passes it to `create` / `create_tab` / `create_drawer`.

/// Optional layout-build closure: builds the navigator's chrome and
/// returns `(root_node, optional_outlet_node)`. When `outlet` is
/// `Some(node)`, screens mount inside that node instead of the
/// navigator's container — that's how author-supplied chrome (sidebar,
/// top bar, tab bar) wraps screens.
pub type WebLayoutBuilder<N> = Rc<dyn Fn() -> (N, Option<N>)>;

/// Kind-agnostic web navigator callbacks. Every SDK passes one of
/// these; the tab and drawer variants embed it.
pub struct WebNavCallbacks<N: Clone + 'static> {
    pub initial_route: &'static str,
    pub initial_path: &'static str,
    pub mount_screen:
        Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<N>>,
    pub release_screen: Rc<dyn Fn(u64)>,
    pub match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>>,
    pub depth_changed: Rc<dyn Fn(usize)>,
    pub nav_state: NavState,
    pub build_layout: Option<WebLayoutBuilder<N>>,
    pub defer_initial_mount: bool,
}

/// Tab-navigator-specific callbacks. The web engine treats tabs as
/// "screen-swap with author chrome" — the registrations are kept for
/// SDK-side decisions (active highlighting, route mapping) but the
/// helper itself only needs `placement` + `mount_policy` to wire the
/// outlet, and `active_changed` to notify the SDK when the active tab
/// changes.
pub struct WebTabCallbacks<N: Clone + 'static> {
    pub navigator: WebNavCallbacks<N>,
    pub tabs: Vec<TabRegistration>,
    pub placement: TabPlacement,
    pub mount_policy: MountPolicy,
    pub active_changed: Rc<dyn Fn(&'static str, String)>,
}

/// Drawer-navigator-specific callbacks. Same screen-swap engine as
/// tabs, plus the `is_open` signal the author's layout subscribes to
/// and the open/close notification callback.
///
/// **Persistent chrome slots.** `build_content` (legacy sidebar)
/// plus the four named slots (`build_top` / `build_bottom` /
/// `build_trailing` — and `build_content` doubles as `leading`)
/// each materialize ONCE at navigator init and survive every
/// screen swap. The drawer's create-time code assembles them
/// around the screen outlet as:
///
/// ```text
/// drawer-root  (column)
/// ├ top slot          (if build_top is Some)
/// ├ middle row
/// │   ├ sidebar       (if build_content is Some — leading)
/// │   ├ outlet/body   (the screen container; always)
/// │   └ trailing      (if build_trailing is Some)
/// └ bottom slot       (if build_bottom is Some)
/// ```
///
/// All four optional slots default to `None` — drawers that only
/// set `build_content` get the historical row layout with no
/// rebuild penalty.
pub struct WebDrawerCallbacks<N: Clone + 'static> {
    pub navigator: WebNavCallbacks<N>,
    pub side: DrawerSide,
    pub drawer_type: DrawerType,
    pub drawer_width: f32,
    pub mount_policy: MountPolicy,
    pub is_open: Signal<bool>,
    /// Sidebar / leading slot — historical name; the SDK's
    /// `sidebar_with` and `leading_with` builders both populate
    /// this. Materializes once at init.
    pub build_content: Option<Rc<dyn Fn() -> N>>,
    /// Top bar slot — header chrome that pins above the outlet
    /// and survives navigations.
    pub build_top: Option<Rc<dyn Fn() -> N>>,
    /// Bottom bar slot — footer / toolbar chrome that pins below
    /// the outlet.
    pub build_bottom: Option<Rc<dyn Fn() -> N>>,
    /// Trailing slot — utility panel / inspector on the right.
    pub build_trailing: Option<Rc<dyn Fn() -> N>>,
    pub active_changed: Rc<dyn Fn(&'static str, String)>,
    pub open_changed: Rc<dyn Fn(bool)>,
    pub background_color: Option<String>,
    /// When `true` (the default), the drawer's `body` div becomes
    /// the scroll context and the bottom slot mounts INSIDE the
    /// body, as a flow sibling AFTER the screen — so the footer
    /// scrolls with content. Screens drop their own
    /// `ScrollView` wrappers and render directly as flow content
    /// of the body. When `false`, the body has `overflow: hidden`
    /// and the bottom slot mounts as a viewport-pinned sibling
    /// of the middle row (historical behavior). Authors set this
    /// via [`DrawerBuilder::bottom_pinned`] in the SDK.
    pub bottom_in_scroll: bool,
}

// ---------------------------------------------------------------------------
// Local kind-specific enums and structs
// ---------------------------------------------------------------------------
//
// These used to live in runtime-core but are SDK-side concepts after
// the substrate refactor. They live here so each web SDK doesn't have
// to redeclare them separately — the three first-party SDKs share this
// helper crate and these definitions.

/// Identifier + display metadata for a single tab. Opaque on web — the
/// helper itself doesn't render tabs (authors build their own tab bar
/// via the layout closure), but the SDK passes the registrations
/// through so they're available for any helper-side lookups in the
/// future.
pub struct TabRegistration {
    pub route: &'static str,
    pub path: &'static str,
    pub label: Option<String>,
}

/// Where the tab bar lives relative to the screen content. Currently
/// informational from the helper's standpoint; the author's `.layout()`
/// closure owns actual positioning.
#[derive(Clone, Copy, Debug)]
pub enum TabPlacement {
    Top,
    Bottom,
}

/// When to materialize a screen's subtree relative to navigation.
///
/// - `Lazy`: only on first activation (default for tabs / drawer items).
/// - `Eager`: at navigator creation time.
#[derive(Clone, Copy, Debug)]
pub enum MountPolicy {
    Lazy,
    Eager,
}

/// Which side of the screen the drawer slides in from.
#[derive(Clone, Copy, Debug)]
pub enum DrawerSide {
    Left,
    Right,
}

/// Visual presentation style for the drawer chrome.
#[derive(Clone, Copy, Debug)]
pub enum DrawerType {
    /// Slides over the content; backdrop dims the content.
    Overlay,
    /// Pushes the content sideways; no backdrop.
    Slide,
    /// Always visible alongside the content (typical for wide screens).
    Permanent,
}

/// Drawer-specific commands ridden across the substrate's
/// `NavCommand::Custom` channel. The drawer SDK builds one of these
/// inside an `Rc<dyn Any>`, dispatches it, and the helper's dispatcher
/// downcasts to flip `is_open`.
#[derive(Clone, Copy, Debug)]
pub enum DrawerCmd {
    Open,
    Close,
    Toggle,
}

// ---------------------------------------------------------------------------
// Per-screen + per-instance state
// ---------------------------------------------------------------------------

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

/// One entry in [`NavigatorInstance::url_history`] — a previously-
/// visited screen the navigator can rebuild on browser back. Beyond
/// the URL itself we record the scroll position at the moment the
/// user navigated AWAY from it, so popstate-driven returns land
/// the user where they last looked (standard browser convention).
struct HistoryEntry {
    url: String,
    scroll_y: f32,
    scroll_x: f32,
}

/// Per-navigator instance state. Lives in the thread-local
/// `NAVIGATOR_INSTANCES` registry, keyed by the container's
/// `data-navigator-id`.
pub struct NavigatorInstance {
    /// The outer `<div>` whose `data-navigator-id` attribute keys
    /// this instance. When no layout is set, screens append
    /// directly here; when a layout is set, screens append into
    /// `outlet` instead.
    pub container: Node,
    /// When a layout is registered, the SDK supplies a dedicated
    /// `<div>` (built by the layout closure) and we mount screens
    /// into that instead of the container. `None` means no layout
    /// — screens go into `container`.
    pub outlet: Option<Node>,
    /// When set, screen mounts call `insertBefore(screen, anchor)`
    /// instead of `appendChild(screen)`. The anchor stays in the
    /// outlet across navigations — a typical use is a persistent
    /// footer node parked after the screen-mount position so each
    /// new screen lands above it. Set by the SDK at create time
    /// (e.g., drawer's `bottom_in_scroll` mode).
    pub screen_anchor: Option<Node>,
    /// Active stack — top is the visible screen. Always non-empty
    /// while the navigator exists; `pop` of the only entry is a no-op.
    ///
    /// Always exactly one entry — the currently-visible screen.
    /// On push, the previous entry is detached (DOM removed, scope
    /// released) before the new screen is mounted. On pop, the
    /// previous URL is pulled from `url_history` and the resulting
    /// screen is rebuilt fresh via `match_path`. This matches the
    /// usual SPA convention (React Router, etc.) — back navigation
    /// rebuilds rather than preserving scroll/form state of an
    /// off-screen DOM tree. Native stack navigators (UIKit /
    /// Android) get the opposite via their own platform machinery;
    /// the web SDK doesn't try to mimic that.
    pub stack: Vec<ScreenEntry>,
    /// Previously-visited screens, in push order (top = most
    /// recently navigated away from). On pop, the top entry is
    /// matched back against the route table and the resulting
    /// screen is rebuilt and mounted with its recorded scroll
    /// position restored.
    url_history: Vec<HistoryEntry>,
    /// `build_layout` closure, retained across the navigator's
    /// lifetime. The SDK's `build_layout` populates an internal
    /// scope slot during the microtask call; the closure itself
    /// owns the only `Rc` clone of that slot, so dropping the
    /// closure also drops the scope (and all layout effects die
    /// with it). Holding it here keeps the layout's reactive
    /// effects alive past the microtask. `None` means no layout
    /// was registered.
    #[allow(dead_code)]
    pub build_layout_retainer: Option<WebLayoutBuilder<Node>>,
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
    nav_state: NavState,
    depth_changed: Rc<dyn Fn(usize)>,
    /// `true` while the instance is applying a programmatic push /
    /// replace / reset. The popstate handler checks this so it
    /// doesn't try to "reconcile" a URL change we just made
    /// ourselves.
    suppress_popstate: RefCell<bool>,
    /// When `true`, the create-time microtask skips its URL-based
    /// auto-mount — initial mounting comes through
    /// [`attach_initial`] with a screen node the caller already
    /// has. The runtime-server dev-client sets this so the wire's
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

    /// True when a user-supplied layout is in effect. The layout's
    /// chrome (sidebar, top bar, etc.) wraps the navigator's
    /// outlet; without a layout, the screen mounts directly into
    /// the navigator's container with `position: absolute; inset: 0`.
    /// Mount/unmount lifecycle is the same in both modes — pushes
    /// detach the previous screen, pops rebuild from `url_history`.
    fn has_layout(&self) -> bool {
        self.outlet.is_some()
    }

    /// Mount a screen node into `mount_point()`. When `screen_anchor`
    /// is set (e.g., drawer's `bottom_in_scroll` mode parks a
    /// footer node at the end of the outlet), the screen is
    /// inserted *before* the anchor so persistent chrome stays
    /// after the screen content. When no anchor is set, this is
    /// equivalent to `appendChild`.
    fn insert_screen_node(&self, node: &Node) -> Result<Node, wasm_bindgen::JsValue> {
        self.mount_point().insert_before(node, self.screen_anchor.as_ref())
    }

    /// Current scroll position of the outlet (or 0/0 if the outlet
    /// isn't a scrollable element). Used to snapshot the screen
    /// the user is navigating AWAY from, so a future browser-back
    /// restores it.
    fn current_outlet_scroll(&self) -> (f32, f32) {
        self.mount_point()
            .dyn_ref::<web_sys::Element>()
            .map(|el| (el.scroll_left() as f32, el.scroll_top() as f32))
            .unwrap_or((0.0, 0.0))
    }

    /// Apply a scroll position to the outlet. No-op on outlets
    /// that aren't `Element`s or that don't scroll
    /// (`overflow: hidden`). Always sets — even setting to
    /// `(0, 0)` is meaningful, since `mount_internal` relies on
    /// this for "fresh screen starts at top."
    fn set_outlet_scroll(&self, x: f32, y: f32) {
        if let Some(el) = self.mount_point().dyn_ref::<web_sys::Element>() {
            el.set_scroll_left(x as i32);
            el.set_scroll_top(y as i32);
        }
    }

    /// Mark the currently-visible screen as active. Vestigial now
    /// that only one screen is mounted at a time — the `ui-nav-active`
    /// class is left in place as a hook for app-level CSS that
    /// wants to target the visible screen, but it's no longer
    /// load-bearing for visibility.
    fn refocus(&self) {
        if let Some(entry) = self.stack.last() {
            if let Ok(elem) = entry.node.clone().dyn_into::<web_sys::Element>() {
                set_class_present(&elem, "ui-nav-active", true);
            }
        }
    }

    /// Drop the current top screen's DOM + scope. Called by every
    /// transition (push/pop/replace/reset) before mounting the next
    /// screen. The popped entry's URL is preserved in the caller's
    /// local — we only clean up DOM + scope here.
    fn detach_top(&mut self) -> Option<ScreenEntry> {
        let top = self.stack.pop()?;
        let _ = self.mount_point().remove_child(&top.node);
        (self.release_screen)(top.scope_id);
        Some(top)
    }

    /// Mount an externally-built screen node. Used by the runtime-server
    /// path where `mount_screen` shouldn't be invoked (the screen
    /// node and scope id are already known — server-built, shipped
    /// via the wire).
    ///
    /// Mirrors the tail of [`Self::mount_internal`] from after the
    /// `mount_screen` call: stamps the class, appends to the
    /// mount point, records the stack entry, and fires
    /// `depth_changed`. The reactive `nav_state` signals aren't
    /// updated here — runtime-server mode renders layout chrome on
    /// the server, so those signals only matter for local-mode
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
        self.insert_screen_node(&screen)
            .expect("insert screen failed (attach_initial)");
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
        // HYDRATION: enter the outlet so the screen build adopts the
        // server's screen DOM rather than rebuilding it. No-op off
        // hydration (later push/pop navigations build fresh).
        backend_web::hydrate_enter(self.mount_point());
        let result = (self.mount_screen)(name, params);
        let node = result.node;
        let scope_id = result.scope_id;
        // The `ui-nav-screen` class adds `position:absolute; inset:0`
        // so the screen fills the `.ui-nav-root` container regardless
        // of its intrinsic size. Only applied in no-layout mode —
        // in layout mode the screen is a normal flow child of the
        // outlet (which lives somewhere inside the user's layout
        // tree); absolute-positioning to `.ui-nav-root` would
        // teleport it out of the layout's bounds.
        if !self.has_layout() {
            Self::stamp_screen_class(&node);
        }
        self.insert_screen_node(&node)
            .expect("insert screen failed");
        // Reset the outlet's scroll position to the top — standard
        // web/iOS UX is "a fresh screen starts at the top," but
        // when the navigator owns a persistent scroll container
        // (drawer's `bottom_in_scroll` mode, where the body
        // outlives every push) the previous screen's scroll
        // position would otherwise carry over and the user would
        // land mid-page.
        //
        // Browser back/forward restores scroll by overwriting this
        // AFTER `mount_internal` returns — see `pop_in_place` /
        // `on_popstate`. Mount-time reset is the default; restore
        // is the explicit caller opt-in.
        //
        // Harmless when the outlet isn't a scroll surface
        // (`overflow: hidden`) — setting scrollTop on a clipped
        // container is a no-op.
        self.set_outlet_scroll(0.0, 0.0);
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

    /// Logical stack depth: currently-mounted screen (always 1 once
    /// a screen is up) + URL history entries waiting to be rebuilt
    /// on pop. Mirrored into the framework's `depth` signal so
    /// `can_go_back` works.
    fn logical_depth(&self) -> usize {
        self.stack.len() + self.url_history.len()
    }

    fn push(&mut self, name: &'static str, params: Box<dyn Any>, url: String) {
        push_state(&url);
        *self.suppress_popstate.borrow_mut() = true;
        // Snapshot the current screen's scroll position BEFORE we
        // detach it so a future browser-back can restore where the
        // user was looking. Read happens against the live outlet,
        // which is still showing the about-to-be-popped screen.
        let (scroll_x, scroll_y) = self.current_outlet_scroll();
        // Drop the previous screen's DOM + scope, preserve its URL
        // + scroll position in `url_history` so pop can rebuild it
        // from `match_path` AND restore the scroll.
        if let Some(prev) = self.detach_top() {
            self.url_history.push(HistoryEntry {
                url: prev.url,
                scroll_y,
                scroll_x,
            });
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
        !self.url_history.is_empty()
    }

    /// Pop the top screen without going through history.back. Used
    /// by the popstate handler when reconciling to a deeper URL.
    fn pop_in_place(&mut self) {
        if !self.can_pop() {
            return;
        }
        // Drop the active screen, pop the previous entry off
        // history, re-match it against the route table, mount, and
        // restore the recorded scroll position. Subtree itself is
        // rebuilt fresh; only the scroll offset survives.
        self.detach_top();
        let entry = match self.url_history.pop() {
            Some(e) => e,
            None => return,
        };
        if let Some((name, params)) = (self.match_path)(&entry.url) {
            self.mount_internal(name, params, entry.url.clone());
            // `mount_internal` resets to (0,0); overwrite with the
            // recorded position to restore the user's last spot.
            self.set_outlet_scroll(entry.scroll_x, entry.scroll_y);
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
    ///
    /// Only one screen is mounted at a time; `url_history` tracks
    /// where we've been so back navigation can rebuild. If
    /// `current` matches a URL in `url_history`, the user hit back
    /// — pop down to that URL. Otherwise they navigated forward,
    /// push the new URL.
    fn on_popstate(&mut self) {
        if *self.suppress_popstate.borrow() {
            return;
        }
        let current = current_pathname();

        let pos = self
            .url_history
            .iter()
            .rposition(|e| paths_equal(&e.url, &current));
        if let Some(idx) = pos {
            // Drop the active screen, drop history entries above
            // `idx`, mount the entry at `idx`. Anything between
            // idx and the end is skipped — the user could only
            // reach those via a `go(n)`-like call, which the
            // browser collapses into a single popstate fire.
            self.detach_top();
            let entry = self.url_history.remove(idx);
            self.url_history.truncate(idx);
            if let Some((name, params)) = (self.match_path)(&entry.url) {
                self.mount_internal(name, params, entry.url.clone());
                // Restore the scroll the user was at when they
                // navigated away from this URL.
                self.set_outlet_scroll(entry.scroll_x, entry.scroll_y);
            }
            return;
        }
        // Forward navigation we haven't seen. Snapshot the current
        // screen's scroll (so a future back to this same URL can
        // restore it), detach the current top, mount the new URL.
        let (scroll_x, scroll_y) = self.current_outlet_scroll();
        if let Some((name, params)) = (self.match_path)(&current) {
            if let Some(prev) = self.detach_top() {
                self.url_history.push(HistoryEntry {
                    url: prev.url,
                    scroll_y,
                    scroll_x,
                });
            }
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

/// Build one frame `<div>` under `parent`, adopting the server node by
/// `match_class` during hydration (else create fresh + set `set_class` +
/// append). Match-by-class is parent-relative — order-independent, unlike
/// the linear cursor, which can't follow the frame's skeleton-then-fill
/// build order.
fn frame_div(
    b: &WebBackend,
    doc: &web_sys::Document,
    parent: &Node,
    match_class: &str,
    set_class: &str,
) -> web_sys::Element {
    // Use the BACKEND METHOD (not the global-handle free fn): the frame is
    // built inside `create_navigator`'s `borrow_mut`, so a global-handle
    // `try_borrow` would fail and silently skip adoption.
    if let Some(adopted) = b.hydrate_adopt_child_of(parent, match_class) {
        return adopted.unchecked_into();
    }
    let d = doc
        .create_element("div")
        .expect("create_element drawer frame div failed");
    let _ = d.set_attribute("class", set_class);
    let _ = parent.append_child(&d);
    d
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
    callbacks: WebNavCallbacks<Node>,
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
    // HYDRATION: adopt the server `.ui-nav-root` so the navigator's `root`
    // IS `#app`'s existing child — `finish` then sees `root_in_mount` and
    // keeps the server DOM instead of clear+append. Frame + content adopt
    // below via the match/enter helpers.
    let container: web_sys::Element = match b.hydrate_adopt_container("ui-nav-root") {
        Some(adopted) => adopted.unchecked_into(),
        None => {
            let c = doc
                .create_element("div")
                .expect("create_element nav container failed");
            // No `.ui-default` — see view.rs. The `.ui-nav-root` rule
            // sets `position: relative` on the container; layout chrome
            // (when present) stacks via normal block flow inside.
            set_class_present(&c, "ui-nav-root", true);
            c
        }
    };

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
        screen_anchor: None,
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

    // Web's `.layout(...)` escape hatch — author-supplied chrome
    // (sidebar, top bar, etc.) wrapping the navigator's screen
    // outlet. The SDK's layout closure returns `(root, optional
    // outlet)`; invoking it materializes the chrome subtree into
    // backend nodes (each sub-primitive re-enters the walker,
    // which calls `backend.borrow_mut()`).
    //
    // Defer to a microtask: `create_inner` is called from inside
    // the SDK handler's `init`, which itself may be called inside
    // an outer `backend.borrow_mut()`. Invoking `build_layout`
    // synchronously here can re-enter that borrow and panic
    // ("RefCell already borrowed"). The same microtask trick the
    // initial-screen mount uses applies — by the time the
    // microtask fires, the outer borrow has dropped.
    if let Some(build_layout) = callbacks.build_layout.clone() {
        let instance_for_layout = instance.clone();
        runtime_core::schedule_microtask(move || {
            let (root, outlet_node) = build_layout();
            let inst = instance_for_layout.borrow();
            inst.container
                .append_child(&root)
                .expect("attach navigator layout root to container");
            drop(inst);
            if let Some(outlet) = outlet_node {
                instance_for_layout.borrow_mut().outlet = Some(outlet);
            }
        });
    }

    // Mount the initial / deep-linked stack.
    //
    // Deferred to a microtask so any outer `backend.borrow_mut()`
    // (held across the SDK handler's `init` call) is released
    // before `mount_screen` calls back into the framework. Calling
    // synchronously here would trip a "RefCell already borrowed"
    // panic. Same defer-trick used by the Virtualizer's initial
    // refresh on the JS side.
    //
    // Layout (if any) is wired by the microtask above, which is
    // queued before this one. The microtask here just covers the
    // initial-screen auto-mount.
    let initial_path = callbacks.initial_path;
    let initial_route = callbacks.initial_route;
    let match_path = callbacks.match_path.clone();
    {
        let instance = instance.clone();
        runtime_core::schedule_microtask(move || {
            let mut inst = instance.borrow_mut();

            // runtime-server / deferred-mount mode: skip URL-driven
            // auto-mount. The caller mounts via `attach_initial` with
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
                    // Deep-link cold start has no prior screen
                    // visible, so there's no real scroll to
                    // record — initial entry uses (0, 0).
                    inst.url_history.push(HistoryEntry {
                        url: initial_path.to_string(),
                        scroll_y: 0.0,
                        scroll_x: 0.0,
                    });
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
    // tabs uses Select; drawer uses Select + Custom(DrawerCmd)), so
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
    callbacks: WebNavCallbacks<Node>,
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
            NavCommand::Select { .. } | NavCommand::Custom(_) => {
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
/// navigator. The `TabRegistration` metadata (labels, icons, badges)
/// is ignored at this layer; authors render their own tab bar through
/// `.layout(...)` and call `handle.select(...)` from the bar
/// buttons. This mirrors how `idea-ui` is expected to ship themed
/// tab bars: the bar is *just a styled View*, not a navigator
/// concern.
///
/// `Select` maps to `Replace`: the new screen takes the outlet's
/// active slot, no URL stack growth.
pub fn create_tab(
    b: &mut WebBackend,
    callbacks: WebTabCallbacks<Node>,
    control: Rc<NavigatorControl>,
) -> Node {
    let WebTabCallbacks {
        navigator,
        active_changed,
        ..
    } = callbacks;
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
                instance.borrow_mut().replace(name, params, url.clone());
                active_changed(name, url);
            }
            // `Reset` is accepted as a "go back to initial tab"
            // hatch — useful for analytics flows that programmatically
            // re-home the user.
            NavCommand::Reset { name, url, params, .. } => {
                instance.borrow_mut().reset(name, params, url.clone());
                active_changed(name, url);
            }
            NavCommand::Push { .. }
            | NavCommand::Pop
            | NavCommand::Replace { .. }
            | NavCommand::Custom(_) => {
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
/// `.layout(...)` closure; drawer commands ride the substrate's
/// `NavCommand::Custom` channel carrying a `DrawerCmd` payload —
/// the dispatcher downcasts and flips the `is_open` signal that the
/// layout subscribes to.
pub fn create_drawer(
    b: &mut WebBackend,
    callbacks: WebDrawerCallbacks<Node>,
    control: Rc<NavigatorControl>,
) -> Node {
    let WebDrawerCallbacks {
        navigator,
        is_open,
        open_changed,
        active_changed,
        build_content,
        build_top,
        build_bottom,
        build_trailing,
        bottom_in_scroll,
        ..
    } = callbacks;
    // Rewrite default `Link` activation to `Select` — without this
    // `Link::new(...)` falls back to `NavCommand::Push` which the
    // dispatcher below panics on. Same fix as iOS/Android helpers.
    let select_activator: Rc<
        dyn Fn(&'static str, String, Box<dyn Any>) -> NavCommand,
    > = Rc::new(|name, url, params| NavCommand::Select {
        name,
        url,
        params,
        state: None,
    });
    control.install_link_activator(select_activator);

    let container = create_inner(b, navigator, control.clone(), move |instance| {
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
                // `push` (not `replace`) so the browser back button
                // walks back through visited screens. The drawer's
                // structural model is "tabs-style" (one active
                // screen + persistent sidebar), but on web the URL
                // bar is real and users expect the back button to
                // work. Without a real history entry the back
                // button skips the entire site and goes to the
                // previous origin.
                instance.borrow_mut().push(name, params, url.clone());
                active_changed(name, url);
                // Auto-close the drawer on selection. The is_open
                // signal is what the author's layout subscribes
                // to; we update it directly here AND call
                // `open_changed` so any analytics/listeners fire.
                is_open.set(false);
                open_changed(false);
            }
            NavCommand::Reset { name, url, params, .. } => {
                instance.borrow_mut().reset(name, params, url.clone());
                active_changed(name, url);
                is_open.set(false);
                open_changed(false);
            }
            NavCommand::Custom(payload) => {
                // Drawer-specific verbs ride here. Downcast the
                // payload to `DrawerCmd`; ignore foreign types so
                // future SDK additions don't accidentally panic.
                if let Ok(cmd) = payload.downcast::<DrawerCmd>() {
                    match *cmd {
                        DrawerCmd::Open => {
                            is_open.set(true);
                            open_changed(true);
                        }
                        DrawerCmd::Close => {
                            is_open.set(false);
                            open_changed(false);
                        }
                        DrawerCmd::Toggle => {
                            let now = !is_open.get();
                            is_open.set(now);
                            open_changed(now);
                        }
                    }
                }
            }
            // Explicit author overrides via `Link.kind(NavKind::X)`.
            // The drawer's default Link activation is `Select` (above)
            // which already does the equivalent of `Push`; these arms
            // exist so authors who want non-default semantics on a
            // per-link basis aren't blocked by an SDK that refuses
            // to honor their choice.
            NavCommand::Push { name, url, params, .. } => {
                instance.borrow_mut().push(name, params, url.clone());
                active_changed(name, url);
                is_open.set(false);
                open_changed(false);
            }
            NavCommand::Replace { name, url, params, .. } => {
                instance.borrow_mut().replace(name, params, url.clone());
                active_changed(name, url);
                is_open.set(false);
                open_changed(false);
            }
            NavCommand::Pop => {
                // "Back" gesture from author code (`nav.pop()` /
                // `Link.kind(Pop)` if anyone ever wires that). The
                // drawer rebuilds the previous URL from
                // `url_history`, same flow as the browser back
                // button hitting popstate.
                instance.borrow_mut().pop_in_place();
            }
        }));
    });

    // Drawer-on-web layout: column flex with optional top + bottom
    // wrappers and a middle row containing optional sidebar + the
    // mandatory body outlet + optional trailing column. See the
    // diagram in `ensure_navigator_css` for the structure.
    //
    // Each slot's content is invoked in a microtask AFTER the
    // create-time `backend.borrow_mut()` window closes (the slot
    // builder closures synchronously call `build_node`, which
    // wants the backend borrow-free).
    if let Some(container_elem) = container.dyn_ref::<web_sys::Element>() {
        let _ = container_elem.set_attribute(
            "class",
            "ui-nav-root ui-nav-drawer-root",
        );
    }
    let doc = web_sys::window()
        .expect("window")
        .document()
        .expect("document");

    // Frame divs adopt the server node (by `ui-nav-drawer-*` class) during
    // hydration, else create fresh + append. See `frame_div`.

    // --- TOP slot (optional) ---
    let top_div = if build_top.is_some() {
        Some(frame_div(b, &doc, &container, "ui-nav-drawer-top", "ui-nav-drawer-top"))
    } else {
        None
    };

    // --- MIDDLE row (always — contains body outlet) ---
    let middle_div = frame_div(b, &doc, &container, "ui-nav-drawer-middle", "ui-nav-drawer-middle");
    let middle_node: Node = middle_div.clone().unchecked_into();

    // Sidebar (leading) — created only if there's a builder; gives
    // the middle row a clean two-or-three-cell layout when no
    // sidebar is configured.
    let sidebar_div = if build_content.is_some() {
        Some(frame_div(b, &doc, &middle_node, "ui-nav-drawer-sidebar", "ui-nav-drawer-sidebar"))
    } else {
        None
    };

    // Body outlet — always present. Adoption matches the base
    // `ui-nav-drawer-body` token (server node may also carry `-scrolls`).
    let body_class = if bottom_in_scroll {
        "ui-nav-drawer-body ui-nav-drawer-body-scrolls"
    } else {
        "ui-nav-drawer-body"
    };
    let body_div = frame_div(b, &doc, &middle_node, "ui-nav-drawer-body", body_class);
    let body_node: Node = body_div.unchecked_into();

    // Trailing slot (optional).
    let trailing_div = if build_trailing.is_some() {
        Some(frame_div(b, &doc, &middle_node, "ui-nav-drawer-trailing", "ui-nav-drawer-trailing"))
    } else {
        None
    };

    // --- BOTTOM slot (optional) ---
    //
    // In `bottom_in_scroll` mode, the footer mounts INSIDE the
    // body (as the last child) and screens insert_before it via
    // `NavigatorInstance::screen_anchor`. The body's overflow:auto
    // (set on `.ui-nav-drawer-body-scrolls`) makes both the
    // screen and footer scroll together — the footer slides up
    // from below as the user scrolls down through the content,
    // matching the iOS / Safari / docs-site convention.
    //
    // In `bottom_pinned` mode (legacy), the footer is a sibling
    // of `.ui-nav-drawer-middle` and stays pinned at the viewport
    // bottom regardless of scroll.
    let bottom_div = if build_bottom.is_some() {
        let parent: &Node = if bottom_in_scroll { &body_node } else { &container };
        Some(frame_div(b, &doc, parent, "ui-nav-drawer-bottom", "ui-nav-drawer-bottom"))
    } else {
        None
    };

    let nav_id = navigator_id_of(&container).expect("nav id stamped by create_inner");
    let instance_rc = NAVIGATOR_INSTANCES
        .with(|m| m.borrow().get(&nav_id).map(|e| e.instance.clone()))
        .expect("instance registered by create_inner");
    {
        let mut inst = instance_rc.borrow_mut();
        inst.outlet = Some(body_node.clone());
        if bottom_in_scroll {
            // Park the footer node as the insertion anchor so
            // every subsequent screen mount lands BEFORE it. New
            // screens flow naturally above the footer; the footer
            // node itself is never detached.
            inst.screen_anchor = bottom_div.as_ref().map(|d| {
                let n: Node = d.clone().unchecked_into();
                n
            });
        }
    }

    // Mount every slot's content in a single microtask so the
    // SDK's `build_node` calls happen outside the create-time
    // backend borrow. Each builder is a no-arg `Fn() -> Node`
    // already curried by the SDK over the typed `SlotProps`.
    {
        let build_content = build_content;
        let build_top = build_top;
        let build_bottom = build_bottom;
        let build_trailing = build_trailing;
        runtime_core::schedule_microtask(move || {
            // HYDRATION: enter each slot's region before building its
            // content so the builder adopts the server content instead of
            // appending a fresh duplicate. Order-independent (each slot
            // re-enters its own region). No-op off hydration.
            if let (Some(parent), Some(builder)) =
                (sidebar_div.map(|d| -> Node { d.unchecked_into() }), build_content)
            {
                backend_web::hydrate_enter(&parent);
                let _ = parent.append_child(&builder());
            }
            if let (Some(parent), Some(builder)) =
                (top_div.map(|d| -> Node { d.unchecked_into() }), build_top)
            {
                backend_web::hydrate_enter(&parent);
                let _ = parent.append_child(&builder());
            }
            if let (Some(parent), Some(builder)) =
                (bottom_div.map(|d| -> Node { d.unchecked_into() }), build_bottom)
            {
                backend_web::hydrate_enter(&parent);
                let _ = parent.append_child(&builder());
            }
            if let (Some(parent), Some(builder)) =
                (trailing_div.map(|d| -> Node { d.unchecked_into() }), build_trailing)
            {
                backend_web::hydrate_enter(&parent);
                let _ = parent.append_child(&builder());
            }
        });
    }

    // HYDRATION: frame adopted; deferred slot/screen builds re-enter their
    // regions when drained. Suspend the cursor (via the backend METHOD —
    // we're inside `create_navigator`'s borrow) so the walker's throwaway
    // initial-screen build (discarded in local mode) builds fresh without
    // corrupting adoption.
    b.hydrate_suspend_cursor();

    container
}

/// runtime-server / deferred-mount entry point. Called by the SDK
/// handler's `attach_initial` when the navigator was created with
/// `defer_initial_mount = true`. Mounts the externally-built `screen`
/// node into the navigator's outlet without going through `mount_screen`.
///
/// Post-create helpers (`attach_initial`, `attach_layout`, `release`,
/// `make_handle`) read from the thread-local `NAVIGATOR_INSTANCES`
/// registry and don't need a backend handle — the SDK handler invokes
/// them with just the navigator `Node`.
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

/// runtime-server layout attach. The dev-side recording backend ran
/// the user's `.layout(...)` closure, the wire shipped every node it
/// built (sidebar, chrome, outlet placeholder), and now we have to
/// (1) drop the layout root into the navigator container and (2)
/// record the outlet node so subsequent `attach_initial`s mount
/// screens inside the layout's outlet — not the bare container,
/// which would dump the screen on top of the sidebar.
pub fn attach_layout(navigator: &Node, root: Node, outlet: Node) {
    let Some(nav_id) = navigator_id_of(navigator) else {
        return;
    };
    let instance = NAVIGATOR_INSTANCES.with(|m| {
        m.borrow().get(&nav_id).map(|e| e.instance.clone())
    });
    let Some(instance) = instance else { return };
    let mut inst = instance.borrow_mut();
    // Container is freshly created with no children in runtime-server
    // mode (defer_initial_mount = true, so the create-time microtask
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

/// Paint the navigator's screen-outlet background. SDK handlers
/// call this from their `apply_slot_style("body", ...)` branch so
/// that themed builders' `body_background` slot reaches the DOM —
/// mirrors Android's `apply_body_style` and iOS's
/// `apply_drawer_body_style`. Honors `rules.background` only; other
/// fields are ignored for now (the slot's contract on the other
/// backends is background-only).
pub fn apply_body_style(navigator: &Node, rules: &Rc<runtime_core::StyleRules>) {
    let Some(nav_id) = navigator_id_of(navigator) else {
        return;
    };
    let instance = NAVIGATOR_INSTANCES.with(|m| {
        m.borrow().get(&nav_id).map(|e| e.instance.clone())
    });
    let Some(instance) = instance else { return };
    let outlet = instance.borrow().outlet.clone();
    let Some(outlet) = outlet else { return };
    let Some(bg) = rules.background.as_ref() else { return };
    let css = bg.resolve().0;
    // `set_attribute("style", ...)` rather than the typed `.style()`
    // API mirrors the convention in backend-web/src/primitives/link.rs
    // and avoids pulling in the `CssStyleDeclaration` web-sys feature.
    // The outlet div carries no other inline styles, so overwriting
    // the whole `style` attribute is safe here.
    if let Ok(elem) = outlet.dyn_into::<web_sys::Element>() {
        let _ = elem.set_attribute("style", &format!("background-color: {};", css));
    }
}

/// Build a `NavigatorHandle` for the navigator identified by `node`.
/// SDK crates wrap this in their own typed handle (`StackHandle`,
/// `TabsHandle`, `DrawerHandle`) that exposes the kind-specific
/// methods. Returns an inert (no-control) handle when `node` isn't a
/// registered navigator.
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

/// Per-instance bundle stored in the thread-local registry so
/// `make_handle` / `release` can find the right navigator at lookup
/// time.
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
    /// `.ui-nav-screen`, `.ui-nav-drawer-*`) has been injected into
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
        // `.ui-nav-root` — plain stack navigator container. Holds
        // exactly one `.ui-nav-screen` at a time; navigation
        // unmounts the previous and mounts the new.
        //
        // `.ui-nav-drawer-root` — drawer navigator on web pins the
        // sidebar to the left and the body (the screen outlet) takes
        // the remaining width. The drawer SDK creates two child
        // divs: `.ui-nav-drawer-sidebar` and `.ui-nav-drawer-body`.
        // Screens mount into the body, which is reused as the
        // navigator's outlet.
        //
        // `!important` on `.ui-nav-screen`'s position+inset: this
        // stylesheet is injected at navigator-init time, BEFORE any
        // framework per-node rule emitted by `rules_to_css`. If a
        // screen's root view sets `position` in its own stylesheet,
        // source order would win against the navigator and the
        // screen would lose its full-bleed absolute placement.
        // `!important` is the targeted defense — `.ui-nav-screen`
        // is a navigator-controlled invariant, not a styling
        // suggestion.
        // Drawer chrome layout — column-of-rows:
        //
        // ```
        // .ui-nav-drawer-root      (column flex, full viewport)
        // ├ .ui-nav-drawer-top     (auto-height; mounted only when a top slot is set)
        // ├ .ui-nav-drawer-middle  (row flex, fills remaining vertical space)
        // │   ├ .ui-nav-drawer-sidebar    (leading; flex:0 0 auto; optional)
        // │   ├ .ui-nav-drawer-body       (outlet; flex:1; always present)
        // │   └ .ui-nav-drawer-trailing   (flex:0 0 auto; optional)
        // └ .ui-nav-drawer-bottom  (auto-height; mounted only when bottom slot set)
        // ```
        //
        // The old layout was a single row with sidebar + body as
        // direct children of root. The new layout keeps that
        // structure visible when only the sidebar slot is set
        // (the middle row degenerates to "sidebar | body"); adding
        // top/bottom/trailing slots populates the surrounding
        // wrappers. Authors that only use `sidebar_with` get the
        // same visual result with one extra wrapper div (the
        // middle row).
        // Drawer body has two modes:
        //   - `.ui-nav-drawer-body` (default `bottom_pinned` mode)
        //     keeps the historical `overflow: hidden` behavior;
        //     screens fill the body via their own `ScrollView`,
        //     and the footer pins to the viewport bottom as a
        //     sibling of `.ui-nav-drawer-middle`.
        //   - `.ui-nav-drawer-body-scrolls` (new default
        //     `bottom_in_scroll` mode) makes the body the scroll
        //     context; the footer is a flow sibling AFTER the
        //     screen inside the body. Screens drop their inner
        //     `ScrollView` since the body now provides scrolling.
        //
        // The new mode's `min-height: 100%` ensures short screens
        // still fill the viewport vertically so the footer sits at
        // the bottom of the visible area (not floating mid-screen).
        // Single source of truth — the same sheet the generic SSR chrome
        // handlers ship via `Backend::register_raw_css`, so the live web
        // layout and the server's first paint are identical.
        // `navigator_layout_css` bakes the responsive sidebar pin/modal
        // `@media` query into the sheet (off-canvas modal base, pinned
        // above the customizable `navigator_pin_width`), so the live web
        // layout matches the SSR first paint with no render-time decision.
        let sheet = css::navigator_layout_css();
        let Some(win) = web_sys::window() else { return };
        let Some(doc) = win.document() else { return };
        if let Some(head) = doc.head() {
            if let Ok(style) = doc.create_element("style") {
                style.set_text_content(Some(sheet.as_str()));
                let _ = head.append_child(&style);
            }
        }
        injected.set(true);
    });
}
