//! Stack navigator on the **outlet model** — the push/pop sibling of
//! `swap-navigator`.
//!
//! A stack has depth: `push` mounts a screen on top of a back-stack, `pop`
//! removes the top and reveals the one below (whose scope stayed alive, so its
//! state is intact). The visible screen is the top of the stack, swapped into
//! the navigator's single outlet.
//!
//! Chrome is **author layout**: `.layout(|nav| …)` wraps `{nav.outlet}` and can
//! read `nav.active_route` / `nav.can_go_back` / `nav.depth` and call `nav.pop`
//! — e.g. an `idea_ui_nav::StackHeader`. The author derives the title from the
//! active route. This mirrors `swap-navigator`; the only difference is the
//! command vocabulary (Push/Pop/Replace/Reset vs Select) and that lower screens
//! stay mounted beneath the top.
//!
//! ```ignore
//! StackNavigator::new(&HOME)
//!     .screen(HOME, |_| Screen::new(/* … */))
//!     .screen(DETAIL, |p: DetailParams| Screen::new(/* … */))
//!     .layout(|nav| ui! {
//!         view {
//!             StackHeader(
//!                 title = rx!(title_for(nav.active_route.get())),
//!                 show_back = nav.can_go_back,
//!                 on_back = nav.pop.clone(),
//!             )
//!             { nav.outlet }
//!         }
//!     })
//!     .bind(nav);
//! ```
//!
//! # One handler, every host
//!
//! The authored surface lowers to the vocabulary's
//! [`stack_navigator`](runtime_vocabulary::builders::stack_navigator)
//! builder; the runtime side is the vocabulary's own stack handler,
//! installed on every host by `register_builtins`. There is no
//! per-backend handler to register — [`register`] survives only as a
//! documented no-op for historical bootstrap code — and URL sync needs
//! no opt-in (every navigator registers with the host-installed
//! `handlers::nav_url_sync::UrlSyncService`).
//!
//! # The native push surface — lost in the runtime-v2 migration, and how to restore it
//!
//! Before runtime v2 this crate shipped `src/ios.rs` and `src/android.rs`:
//! `NavigatorHandler<IosBackend>` / `NavigatorHandler<AndroidBackend>`
//! implementations that seated a real `UINavigationController` (via
//! `ios-navigator-helpers`) or a Kotlin `RustNavigator` **inside** the
//! outlet, so a stack got native push/pop transitions and — the part that
//! is not cosmetic — the **interactive swipe-back gesture** and the
//! system Back button. Chrome stayed author layout in both (the native
//! bar was force-hidden, `header_shown: Some(false)`), so output was
//! uniform per CLAUDE.md §7 while the *transition mechanics* were
//! platform-idiomatic.
//!
//! Those files were deleted in the migration because they were written
//! against the old per-backend navigator registry
//! (`runtime_core::primitives::navigator::NavigatorHandler` +
//! `Backend::create_navigator`), a seam the surviving core does not have:
//! navigation is now `runtime_vocabulary::handlers::navigator`'s built-in,
//! generic over `NavCaps`. **This is a real capability regression** — iOS
//! and Android currently have no swipe-back and no system-Back
//! integration on a stack — and it should be restored, not accepted. It
//! cannot be restored from this crate alone: the presenter has to be
//! consulted by the vocabulary handler, which is where every stack
//! mutation lands.
//!
//! ## The seam the vocabulary must grow
//!
//! Mirror `handlers::navigator::url_sync` exactly — same shape, same
//! lifecycle, same type-erasure trick. That module is already the worked
//! precedent for "a host installs a service at boot; the navigator
//! handler consults it if present, and behaves exactly as today if not".
//!
//! ```ignore
//! // crates/runtime/vocabulary/src/handlers/navigator.rs
//! pub mod native_push {
//!     /// The per-host presenter. Node types are erased (`Rc<dyn Any>`
//!     /// around the backend's `Node`) for the same reason
//!     /// `NavSyncRegistration::outlet` erases them: the trait must be
//!     /// object-safe and installable from a host that the generic
//!     /// handler knows nothing about. A presenter downcasts to its own
//!     /// node type and MUST decline (return `None` from `attach`) on a
//!     /// foreign one.
//!     pub trait StackPresenter {
//!         /// One-time setup inside the navigator's outlet, before the
//!         /// initial screen is seated. Return the node the handler
//!         /// should treat as the content host (the nav controller's
//!         /// container), or `None` to decline — declining must leave
//!         /// today's behavior byte-identical.
//!         fn attach(&self, outlet: Rc<dyn Any>) -> Option<NativePushHandle>;
//!     }
//!
//!     /// What a presenter hands back: the content host plus the four
//!     /// directional transitions the handler drives, plus the reverse
//!     /// channel for a USER-initiated back.
//!     pub struct NativePushHandle {
//!         pub host: Rc<dyn Any>,
//!         /// Seat the bottom screen, unanimated.
//!         pub seat:    Rc<dyn Fn(Rc<dyn Any>)>,
//!         /// Push on top, animated.
//!         pub push:    Rc<dyn Fn(Rc<dyn Any>)>,
//!         /// Pop the top, animated, revealing the given screen.
//!         pub pop:     Rc<dyn Fn(Rc<dyn Any>)>,
//!         /// Swap the top in place, unanimated (Replace).
//!         pub replace: Rc<dyn Fn(Rc<dyn Any>)>,
//!         /// Collapse to a single screen, unanimated (Reset).
//!         pub reset:   Rc<dyn Fn(Rc<dyn Any>)>,
//!         /// Install the handler's LOGICAL-ONLY pop. The presenter
//!         /// calls this when the user completed a swipe-back or hit
//!         /// system Back, i.e. the native stack ALREADY popped. The
//!         /// handler must then pop its `Vec<StackEntry>`, republish
//!         /// depth/chrome/active_* and notify url-sync — and must NOT
//!         /// call `pop` back into the presenter (that would double-pop).
//!         pub set_user_pop: Rc<dyn Fn(Rc<dyn Fn()>)>,
//!     }
//!
//!     thread_local! { static PRESENTER: RefCell<Option<Rc<dyn StackPresenter>>> = … }
//!     pub fn install_stack_presenter(p: Rc<dyn StackPresenter>);
//!     pub fn clear_stack_presenter();
//!     pub(super) fn presenter() -> Option<Rc<dyn StackPresenter>>;
//! }
//! ```
//!
//! ## Where it plugs in (exact call sites, as of this writing)
//!
//! All of them are in `StackShared<H>` in
//! `crates/runtime/vocabulary/src/handlers/navigator.rs`:
//!
//! - `mount_stack_navigator` — after the outlet node exists, call
//!   `native_push::presenter()` and `attach(outlet)`. Store the
//!   `Option<NativePushHandle>` on `StackShared` next to `outlet`.
//! - `show_top()` is the ONE place that reveals a screen today
//!   (`clear_children(outlet)` + `insert`). It is deliberately
//!   direction-blind, which is precisely why it cannot be the hook:
//!   `UINavigationController` needs to know push from pop. **Replace the
//!   five `self.show_top()` call sites with direction-tagged reveals** —
//!   `seat_initial` ⇒ `seat`, `push` ⇒ `push`, `pop` ⇒ `pop`,
//!   `replace` ⇒ `replace`, `reset` ⇒ `reset` — each of which falls
//!   through to today's `show_top()` body verbatim when no presenter is
//!   attached. That keeps the no-presenter op stream byte-identical, so
//!   the existing scene-parity `nav_stack_push_pop` golden still pins it.
//! - `pop()` — split into `pop()` (app-initiated: drive the presenter)
//!   and `pop_logical()` (presenter-initiated: everything except the
//!   presenter call). Hand `pop_logical` to `set_user_pop` at attach.
//!   Note the existing ordering invariant must survive: the popped
//!   screen's `Realized` is dropped (author cleanups run) BEFORE the
//!   revealed screen is shown. On a user-initiated pop the native
//!   transition has already run, so the drop happens after the animation
//!   — that is a behavior difference worth a comment, not a bug.
//! - `seat_initial` cold-deep-link path — the synthesized parent below
//!   the deep-linked screen must be seated into the presenter too
//!   (`seat` then `push`), or the native back-stack has one entry while
//!   the logical stack has two and swipe-back dead-ends. The old
//!   `ios.rs` called this "cold-start deep-link back-stack
//!   reconstruction" and it was the subtlest part of the original.
//! - `StackRetention` — a native presenter retains covered screens by
//!   construction, so `Rebuild` (`dispose_covered_top`) is incompatible
//!   with it. The old iOS handler documented exactly this and forced
//!   `Retain`. The handler should do the same when a presenter is
//!   attached, and say so.
//!
//! ## Then, in this crate and the backends
//!
//! Nothing in this crate's *authored* surface changes — that is the point
//! of the outlet model. The presenters are backend-side
//! (`backend_ios::newcore` installs the `UINavigationController` one at
//! boot; `backend_android::newcore` the `RustNavigator` one), and both
//! can reuse `ios-navigator-helpers` / `android-navigator-helpers`
//! unchanged: those crates are engine code, not old-core code. The
//! recoverable originals are `git show HEAD:crates/sdk/client/navigators/stack/src/ios.rs`
//! and `…/android.rs`.
//!
//! # The header-options carrier
//!
//! Per-screen header options ride the vocabulary's [`Screen`] value
//! (`Element` + opaque `Rc<dyn Any>` options) that a route's render
//! closure returns (`Into<Screen>` keeps bare-`Element` screens
//! source-compatible). The stack handler stores the options on each
//! back-stack entry and republishes the ACTIVE screen's options into
//! `StackNav::screen_chrome` as a rev-stamped [`ScreenChrome`] on every
//! navigation; [`header_state`] downcasts to [`StackScreenOptions`] and
//! derives the [`StackHeaderState`] an author `StackHeader` renders. So
//! `Screen::new(x).title("…").header_right(btn)` is all an author needs.
//!
//! # Sizing
//!
//! The navigator's root **fills its container by default** (width/height
//! 100% + `flex-grow: 1` — `navigator_fill_rules`). Override by styling
//! the navigator element itself:
//! `StackNavigator::new(&home)…​.with_style(my_style)`.
//!
//! # Screen retention — what happens below the top
//!
//! Covered screens follow [`StackRetention`], resolved per platform by
//! default: on **web**, a push **disposes** the covered screen and pop
//! re-mounts it from its URL (browser semantics — nothing below the visible
//! page stays resident, and a cold deep link never mounts the parent it
//! synthesizes for Back until you actually pop to it); everywhere else,
//! covered screens stay alive (native-stack semantics — pop reveals them
//! with state intact). Force either with `.retention(...)`.

#![deny(missing_docs)]

use std::rc::Rc;

use runtime_vocabulary::builders::{self, navigator_outlet};
use runtime_vocabulary::glue::{inject, ChildList, Element, IntoElement, Ref, Signal};
use runtime_vocabulary::prims::{NavHandle, ScreenChrome, StackNav};

pub use runtime_vocabulary::glue::primitives::navigator::{
    HeaderButton, Route, RouteParams, StackHeaderState,
};
pub use runtime_vocabulary::prims::{Screen, StackRetention};

/// Presentation-label marker. There is no typed navigator payload any
/// more; the label string below is what introspection serves as
/// `NavSnapshot::type_name`, and this ZST keeps the historical name
/// resolvable for app code that spelled it.
pub struct StackPresentation;

/// The presentation label reported by navigator introspection (and by
/// the wire, for a recorded session) — frozen at the historical
/// payload-struct path so snapshots stay comparable.
const STACK_LABEL: &str = "stack_navigator::StackPresentation";

// =============================================================================
// StackContext — handed to the author `.layout(|nav| …)` closure
// =============================================================================

/// The value a stack navigator hands its `.layout(|nav| …)` closure.
/// Not `Clone` (the outlet is one-shot).
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
/// yields the default (empty-title) state (downcast-or-default);
/// `native` is `false` — this is the backend-neutral outlet handler, so
/// there is no platform bar to defer to.
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

/// Per-screen header options for a stack navigator: title,
/// leading/trailing [`HeaderButton`] slots, `hide_header`, plus the
/// native-mobile `back_enabled` / `fullscreen` requests (carried, and
/// consumed once a native push surface lands). Set them via
/// [`StackScreenExt`] on the [`Screen`] a `.screen(...)` render closure
/// returns.
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
/// .header_right(btn)`.
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
/// Equal exactly when the two handles drive the same navigator —
/// DERIVED for the same reason as `SwapHandle`: `NavHandle` owns the
/// identity rule (pointer equality on its dispatch closure) and the
/// wrapper adds no state of its own.
#[derive(Clone, PartialEq, Eq)]
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
        install_native_presenter();
        Self {
            b: builders::stack_navigator(initial).nav_label(STACK_LABEL),
        }
    }
}

/// Install this platform's native-transition presenter, once per thread.
///
/// # Why here and not at backend boot
///
/// The presenter implementations live in the per-platform helper crates
/// (`ios-navigator-helpers`, `android-navigator-helpers`), and those
/// depend on their backend crate — so the backend cannot install one
/// without a dependency cycle. This crate depends on both sides, which
/// makes it the only place the wiring can live.
///
/// # Why on `new` and not on `bind`
///
/// The seam must be populated before any stack MOUNTS, and `new` is the
/// one call every stack makes first. It is idempotent and costs a
/// thread-local bool after the first call, so paying it per navigator is
/// cheaper than the alternatives are complicated.
///
/// On a platform with no presenter this compiles to nothing and the
/// handler keeps its outlet-swap path.
fn install_native_presenter() {
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        use std::cell::Cell;
        thread_local! {
            static INSTALLED: Cell<bool> = const { Cell::new(false) };
        }
        INSTALLED.with(|done| {
            if !done.get() {
                ios_navigator_helpers::install_stack_presenter();
                done.set(true);
            }
        });
    }
}

/// Fluent builder methods. A trait (not inherent methods) so the builder
/// surface stays swappable underneath.
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

// NOTE: this SDK intentionally exposes NO registration seam. The stack
// handler is a vocabulary built-in installed by `register_builtins` on
// every host, so there is nothing for an app to register. The 1.0-era
// no-op `register` / `register_generic` shims were removed in 1.1.0:
// an unconstrained `fn register<B>(_: &mut B) {}` accepts a
// `&mut Registry<H>` happily, so a caller who assumed the usual seam
// convention got a call that compiled, did nothing, and sent them
// debugging a registration they had already "fixed".

/// Convenience re-exports, including the shared data surface (`Route`,
/// `Screen`, `StackContext`), so an app imports everything from here.
pub mod prelude {
    pub use super::{
        header_state, HeaderButton, Route, Screen, StackBuilder, StackContext,
        StackHandle, StackHeaderState, StackNavigator, StackPresentation, StackRetention,
        StackScreenExt, StackScreenOptions,
    };
}
