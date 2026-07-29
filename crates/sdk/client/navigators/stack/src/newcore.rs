//! The new-core (idea-lite) implementation: the SAME authored surface
//! as [`oldcore`](crate) — `StackNavigator::new(&route).screen(…)
//! .layout(|nav| …).retention(…).bind(nav)`, per-screen header options
//! via [`StackScreenExt`] on [`Screen`], `header_state(&nav.
//! screen_chrome)` for an author `StackHeader` — re-expressed over the
//! vocabulary's [`stack_navigator`](runtime_vocabulary::builders::
//! stack_navigator) builder.
//!
//! # The header-options carrier (P6 design)
//!
//! The old options rode `Element::Screen` → `MountResult::options` →
//! the SDK handler's `sync_chrome`. The new scene has no `Screen`
//! element variant; the carrier is now the vocabulary's
//! [`Screen`] value (`Element` + opaque `Rc<dyn Any>` options) that a
//! route's render closure returns (`Into<Screen>` keeps bare-`Element`
//! screens source-compatible). The vocabulary stack handler stores the
//! options on each back-stack entry and republishes the ACTIVE screen's
//! options into `StackNav::screen_chrome` as a rev-stamped
//! [`ScreenChrome`] on every navigation; [`header_state`] downcasts to
//! [`StackScreenOptions`] and derives the [`StackHeaderState`] the
//! author `StackHeader` renders — so the authored `Screen::new(x)
//! .title("…").header_right(btn)` surface is unchanged.
//!
//! No-body surfaces (correct, not stubs): [`register`] /
//! [`register_generic`] (vocabulary `register_builtins` covers), the
//! `recording` sidecar module (old-core dev surface), URL sync (every
//! navigator auto-registers with the host's `UrlSyncService`), and the
//! iOS/Android native push surfaces (the new-core native-nav seam is
//! P5/P6 backend work — the outlet-model handler runs everywhere
//! meanwhile, exactly like the old backend-neutral default).

use std::rc::Rc;

use runtime_vocabulary::builders::{self, navigator_outlet};
use runtime_vocabulary::glue::{inject, ChildList, Element, IntoElement, Ref, Signal};
use runtime_vocabulary::prims::{NavHandle, ScreenChrome, StackNav};

pub use runtime_core::primitives::navigator::{
    HeaderButton, Route, RouteParams, StackHeaderState,
};
pub use runtime_vocabulary::prims::{Screen, StackRetention};

/// Presentation-label marker (parity name for the old typed payload).
pub struct StackPresentation;

/// The wire-parity presentation label (`std::any::type_name` of the old
/// payload struct at its old path).
const STACK_LABEL: &str = "stack_navigator::StackPresentation";

// =============================================================================
// StackContext — handed to the author `.layout(|nav| …)` closure
// =============================================================================

/// The value a stack navigator hands its `.layout(|nav| …)` closure —
/// same field names and roles as the old-core `StackContext`, with
/// new-core signal handles. Not `Clone` (the outlet is one-shot).
pub struct StackContext {
    /// Splat into the layout (`{nav.outlet}`) where the top screen
    /// mounts.
    pub outlet: Element,
    /// The active (top) screen's route name.
    pub active_route: Signal<&'static str>,
    /// The active screen's full path.
    pub active_path: Signal<String>,
    /// Stack depth (1 at the root).
    pub depth: Signal<usize>,
    /// Whether a `pop` is possible (depth > 1) — gate the back
    /// affordance on it.
    pub can_go_back: Signal<bool>,
    /// Pop the top screen (no-op at the root).
    pub pop: Rc<dyn Fn()>,
    /// The active screen's chrome payload, republished on every
    /// navigation. Read it via [`header_state`] inside `rx!` and feed
    /// the result to an `idea_ui_nav::StackHeader`.
    pub screen_chrome: Signal<ScreenChrome>,
}

/// Read the current [`StackHeaderState`] out of a
/// [`StackContext::screen_chrome`] signal. Reactive: call it inside
/// `rx!` / a component read so the header re-renders when the active
/// screen (hence its slots) changes. A live top screen with no options
/// yields the default (empty-title) state — the old handler's
/// downcast-or-default contract; `native` is `false` (this is the
/// backend-neutral outlet handler).
pub fn header_state(screen_chrome: &Signal<ScreenChrome>) -> Option<StackHeaderState> {
    let chrome = screen_chrome.get();
    if chrome.rev == 0 {
        // Nothing published yet (pre-seat) — no header.
        return None;
    }
    let opts = chrome
        .options
        .as_ref()
        .and_then(|any| any.downcast_ref::<StackScreenOptions>().cloned())
        .unwrap_or_default();
    Some(opts.to_state(false))
}

// =============================================================================
// Per-screen header slots
// =============================================================================

/// Per-screen header options for a stack navigator — same fields as the
/// old-core type: title, leading/trailing [`HeaderButton`] slots,
/// `hide_header`, plus the native-mobile `back_enabled` / `fullscreen`
/// requests (carried for parity; the native push surfaces consume them
/// when that seam lands). Set them via [`StackScreenExt`] on the
/// [`Screen`] a `.screen(...)` render closure returns.
#[derive(Clone)]
pub struct StackScreenOptions {
    /// The screen title.
    pub title: Option<String>,
    /// Leading header slot.
    pub header_left: Option<HeaderButton>,
    /// Trailing header slot.
    pub header_right: Option<HeaderButton>,
    /// Hide the header entirely for this screen.
    pub hide_header: bool,
    /// Whether the platform back affordance may pop THIS screen
    /// (default `true`; native mobile only).
    pub back_enabled: bool,
    /// Request full-screen while THIS screen is on top (default
    /// `false`; native mobile only).
    pub fullscreen: bool,
}

impl Default for StackScreenOptions {
    fn default() -> Self {
        Self {
            title: None,
            header_left: None,
            header_right: None,
            hide_header: false,
            back_enabled: true,
            fullscreen: false,
        }
    }
}

impl StackScreenOptions {
    fn to_state(&self, native: bool) -> StackHeaderState {
        StackHeaderState {
            title: self.title.clone().unwrap_or_default(),
            left: self.header_left.clone(),
            right: self.header_right.clone(),
            hidden: self.hide_header,
            native,
        }
    }
}

/// Fluent per-screen header setters on the [`Screen`] a `.screen(...)`
/// render closure returns: `Screen::new(...).title("Detail")
/// .header_right(btn)` — the same authored surface as the old core.
pub trait StackScreenExt {
    /// Set the screen title.
    fn title(self, t: impl Into<String>) -> Self;
    /// Set the leading header slot.
    fn header_left(self, btn: HeaderButton) -> Self;
    /// Set the trailing header slot.
    fn header_right(self, btn: HeaderButton) -> Self;
    /// Hide the header for this screen.
    fn hide_header(self, hide: bool) -> Self;
    /// Allow/deny the platform back affordance for this screen.
    fn back_enabled(self, enabled: bool) -> Self;
    /// Request full-screen while this screen is on top (native mobile).
    fn fullscreen(self, fullscreen: bool) -> Self;
}

fn with_stack_options<F: FnOnce(&mut StackScreenOptions)>(screen: Screen, f: F) -> Screen {
    let mut opts = screen
        .options_as::<StackScreenOptions>()
        .cloned()
        .unwrap_or_default();
    f(&mut opts);
    screen.with(opts)
}

impl StackScreenExt for Screen {
    fn title(self, t: impl Into<String>) -> Self {
        with_stack_options(self, |o| o.title = Some(t.into()))
    }
    fn header_left(self, btn: HeaderButton) -> Self {
        with_stack_options(self, |o| o.header_left = Some(btn))
    }
    fn header_right(self, btn: HeaderButton) -> Self {
        with_stack_options(self, |o| o.header_right = Some(btn))
    }
    fn hide_header(self, hide: bool) -> Self {
        with_stack_options(self, |o| o.hide_header = hide)
    }
    fn back_enabled(self, enabled: bool) -> Self {
        with_stack_options(self, |o| o.back_enabled = enabled)
    }
    fn fullscreen(self, fullscreen: bool) -> Self {
        with_stack_options(self, |o| o.fullscreen = fullscreen)
    }
}

// =============================================================================
// Handle / builder
// =============================================================================

/// Typed handle to a live stack navigator, filled into the `Ref` passed
/// to [`StackBuilder::bind`]. Wraps the vocabulary's unified
/// [`NavHandle`].
#[derive(Clone)]
pub struct StackHandle {
    inner: NavHandle,
}

impl StackHandle {
    /// Wrap the vocabulary handle (called by the `.bind` glue).
    pub fn from_inner(inner: NavHandle) -> Self {
        Self { inner }
    }

    /// Push `route` onto the stack, building its URL from typed
    /// `params`.
    pub fn push<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        self.inner.push(route, params);
    }

    /// Pop the top screen (no-op at the root).
    pub fn pop(&self) {
        self.inner.pop();
    }

    /// Replace the top screen with `route`.
    pub fn replace<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        self.inner.replace(route, params);
    }

    /// Reset the whole stack to a single `route`.
    pub fn reset<P: RouteParams + Clone>(&self, route: &Route<P>, params: P) {
        self.inner.reset(route, params);
    }

    /// Borrow the underlying kind-agnostic handle.
    pub fn inner(&self) -> &NavHandle {
        &self.inner
    }
}

/// The stack-navigator builder. [`StackNavigator::new`] starts one; the
/// result drops into a `ui!` tree (it coerces to an `Element`).
pub struct StackNavigator {
    b: builders::StackNavigatorBuilder,
}

impl StackNavigator {
    /// Start a stack whose root screen is `initial`.
    pub fn new(initial: &Route<()>) -> Self {
        Self {
            b: builders::stack_navigator(initial).nav_label(STACK_LABEL),
        }
    }
}

/// Fluent builder methods — same trait shape as the old core.
pub trait StackBuilder: Sized {
    /// Register a screen: its route and the closure building it from
    /// params.
    fn screen<P, R, F>(self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static;
    /// Set the author layout — wraps `{nav.outlet}` with chrome.
    fn layout<F>(self, f: F) -> Self
    where
        F: Fn(StackContext) -> Element + 'static;
    /// Set the covered-screen lifecycle — see [`StackRetention`].
    fn retention(self, r: StackRetention) -> Self;
    /// Bind a `Ref<StackHandle>` for imperative push/pop.
    fn bind(self, r: Ref<StackHandle>) -> Self;
}

impl StackBuilder for StackNavigator {
    fn screen<P, R, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        R: Into<Screen> + 'static,
        F: Fn(P) -> R + 'static,
    {
        self.b = self.b.screen(route, render);
        self
    }

    fn layout<F>(mut self, f: F) -> Self
    where
        F: Fn(StackContext) -> Element + 'static,
    {
        self.b = self.b.layout(move || {
            let nav = inject::<StackNav>()
                .expect("stack-navigator: StackNav provided by the navigator mount");
            f(StackContext {
                outlet: navigator_outlet().build(),
                active_route: nav.active_route,
                active_path: nav.active_path,
                depth: nav.depth,
                can_go_back: nav.can_go_back,
                pop: nav.pop,
                screen_chrome: nav.screen_chrome,
            })
        });
        self
    }

    fn retention(mut self, r: StackRetention) -> Self {
        self.b = self.b.retention(r);
        self
    }

    fn bind(mut self, r: Ref<StackHandle>) -> Self {
        self.b = self
            .b
            .on_handle(move |handle| r.fill(StackHandle::from_inner(handle)));
        self
    }
}

impl IntoElement for StackNavigator {
    fn into_element(self) -> Element {
        self.b.build()
    }
}

impl ChildList for StackNavigator {
    fn append_to(self, out: &mut Vec<Element>) {
        out.push(self.into_element());
    }
}

/// No-op on the new core: the vocabulary's `register_builtins` installs
/// the stack handler on every host. Kept so app bootstrap compiles
/// unchanged.
pub fn register<B>(_backend: &mut B) {}

/// No-op on the new core — see [`register`].
pub fn register_generic<B>(_backend: &mut B) {}

/// Convenience re-exports. Superset of the old prelude: also exports
/// the shared data surface (`Route`, `Screen`, `StackContext`) so a
/// same-source app imports everything from here.
pub mod prelude {
    pub use super::{
        header_state, register, HeaderButton, Route, Screen, StackBuilder, StackContext,
        StackHandle, StackHeaderState, StackNavigator, StackPresentation, StackRetention,
        StackScreenExt, StackScreenOptions,
    };
}
