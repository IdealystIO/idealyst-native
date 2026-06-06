//! The `Backend` trait — every renderer (web DOM, Android views, iOS
//! UIKit, etc.) implements this. Plus the `VirtualizerCallbacks`
//! bundle the framework hands to backends for virtualized lists,
//! and the no-op `Ops` implementations the trait's default methods
//! use for un-implemented primitives.
//!
//! The trait is intentionally large — one method per primitive +
//! lifecycle hook — but most methods have `unimplemented!()` or
//! no-op defaults so backends can ship incrementally. The walker
//! in [`crate`] is the only caller.

use std::any::Any;
use std::rc::Rc;

use crate::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
use crate::primitives;
use crate::style::{Color, StyleRules};
use crate::{
    ButtonHandle, ButtonOps, PressableHandle, PressableOps, StateBits, TextHandle, TextOps,
    ViewHandle, ViewOps,
};

// ---------------------------------------------------------------------------
// Screenshot
// ---------------------------------------------------------------------------

/// A captured frame of the backend's real rendered surface, returned by
/// [`Backend::capture_screenshot`]. `png` is a complete PNG file; the
/// backend owns the encode so each platform uses its native encoder
/// (AppKit `NSBitmapImageRep`, UIKit `UIImagePNGRepresentation`, Android
/// `Bitmap.compress`). `width`/`height` are the PNG's pixel dimensions
/// (post device-scale), carried alongside so the bridge can report them
/// without re-decoding.
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

// ---------------------------------------------------------------------------
// VirtualizerCallbacks
// ---------------------------------------------------------------------------

/// Callbacks handed to `Backend::create_virtualizer`. All Rc'd so
/// the backend can clone into per-event closures (scroll handler,
/// cell binder, etc.). Generic over the backend's `Node` type so
/// the mount callback returns the backend's actual native node
/// type, no type erasure.
pub struct VirtualizerCallbacks<N: Clone + 'static> {
    /// Current item count. Backend calls this on data-changed.
    pub item_count: Rc<dyn Fn() -> usize>,
    /// Stable identity for an index. Backend uses this to do
    /// keyed diffs across data updates.
    pub item_key: Rc<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
    /// Initial size for an index (Known: authoritative;
    /// Measured: estimate). For Measured mode, the backend should
    /// observe the rendered size after mount and update its
    /// internal layout when the value changes.
    pub item_size: Rc<dyn Fn(usize) -> f32>,
    /// True if `item_size` is an estimate that should be refined
    /// by measuring the mounted node. False if the size is
    /// authoritative.
    pub measure_sizes: bool,
    /// Mount an item: build its subtree inside a fresh per-item
    /// Scope. Returns the freshly-built native node plus the
    /// scope's id. The backend should hold the id alongside its
    /// pooled/mounted cell so it can call `release_item` later.
    pub mount_item: Rc<dyn Fn(usize) -> (N, u64)>,
    /// Release a previously-mounted item by scope id. Drops the
    /// scope, freeing every signal/effect/ref inside the item's
    /// subtree. Backend should NOT try to use the node after this;
    /// it should also detach the node from its parent.
    pub release_item: Rc<dyn Fn(u64)>,
    /// Backend may call this to inform the framework that an
    /// observed item's measured size has changed (Measured mode).
    /// The framework stores the new size and the backend uses it
    /// for future layout passes.
    pub set_measured_size: Rc<dyn Fn(u64, f32)>,
}

// ---------------------------------------------------------------------------
// ColorScheme
// ---------------------------------------------------------------------------

/// The platform's current appearance mode. Backends return this from
/// [`Backend::color_scheme`] so the app can pick an appropriate
/// default theme before the first render.
///
/// `Auto` means the platform has no explicit preference (e.g. iOS
/// `UIUserInterfaceStyleUnspecified`, or the browser has no
/// `prefers-color-scheme` media query match). Apps should fall back
/// to whichever theme they consider the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
    /// The platform did not report an explicit preference.
    Auto,
}

// ---------------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------------

/// Identifies the host platform a backend is rendering to. Author
/// code reads this via [`Backend::platform`] to branch on host —
/// e.g. show iOS-style chrome only on `Ios`, rely on
/// `position: fixed` only on `Web`, swap keyboard shortcuts for
/// menu-bar items on `MacOs`.
///
/// Whether the backend is a simulator/emulator rather than a real
/// device is orthogonal — query [`Backend::is_simulator`] for that.
///
/// `Custom("")` is the trait default — "backend hasn't declared an
/// identity at all" (test mocks, early stubs). Author code should
/// treat `Custom(_)` as "make no UI-affordance assumptions"
/// regardless of the inner string; the string is for diagnostics,
/// telemetry, or the rare custom embedder that wants its own opt-in
/// code path (e.g. `Custom("runtime-server")`, `Custom("linux-desktop")`,
/// `Custom("my-tv-box")`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    /// Browser / wasm-in-web-page host.
    Web,
    /// iPhone / iPad (UIKit, `target_os = "ios"`).
    Ios,
    /// Phones and tablets running Android.
    Android,
    /// macOS desktop (AppKit shell).
    MacOs,
    /// Apple TV (tvOS).
    TvOs,
    /// Android TV — distinct from `Android` because input
    /// (D-pad, no touch) and form factor differ.
    AndroidTv,
    /// Roku set-top boxes.
    Roku,
    /// Backend that doesn't fit any of the named variants. The
    /// `&'static str` is a self-reported identifier — empty string
    /// means "no identity declared" (the trait default), otherwise
    /// the backend has chosen its own name (`"runtime-server"`,
    /// `"linux-desktop"`, `"my-tv-box"`, …). Author code that
    /// branches on `Custom(_)` should treat the string as opaque
    /// metadata, not a stable UI-shape signal.
    Custom(&'static str),
}

impl Platform {
    /// `Ios | MacOs | TvOs` — UIKit/AppKit/tvOS family.
    pub fn is_apple(self) -> bool {
        matches!(self, Self::Ios | Self::MacOs | Self::TvOs)
    }

    /// `Ios | Android` — touch-first phone/tablet form factor.
    pub fn is_mobile(self) -> bool {
        matches!(self, Self::Ios | Self::Android)
    }

    /// `TvOs | AndroidTv | Roku` — TV form factor, remote-driven input.
    pub fn is_tv(self) -> bool {
        matches!(self, Self::TvOs | Self::AndroidTv | Self::Roku)
    }

    /// Web / browser hosts.
    pub fn is_web(self) -> bool {
        matches!(self, Self::Web)
    }

    /// Desktop form factor (currently just `MacOs`).
    pub fn is_desktop(self) -> bool {
        matches!(self, Self::MacOs)
    }

    /// Canonical human-facing name for this platform — the form
    /// authors would write in prose ("iOS", "macOS", "Android TV",
    /// not "ios" / "macos" / "androidtv"). Stable spelling, safe to
    /// slot into UI strings, diagnostic dumps, or log lines.
    ///
    /// `Custom(name)` passes the self-reported string through
    /// verbatim — the backend is the source of truth. Empty
    /// `Custom("")` (the trait default) produces `""`.
    pub fn canonical(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Ios => "iOS",
            Self::Android => "Android",
            Self::MacOs => "macOS",
            Self::TvOs => "tvOS",
            Self::AndroidTv => "Android TV",
            Self::Roku => "Roku",
            Self::Custom(name) => name,
        }
    }
}

// ---------------------------------------------------------------------------
// Global platform accessor
// ---------------------------------------------------------------------------
//
// `mount(...)` reads the backend's `platform()` once at startup and
// stashes the value in this thread-local so author code (component
// bodies, effects, free functions) can branch on host platform
// without needing a `Backend` reference. Sim/emulator status isn't
// a separate signal — the backend folds it into the `Platform`
// value it returns (e.g. `Custom("Sim")`), so there's just one
// thing for authors to read.

thread_local! {
    static CURRENT_PLATFORM: std::cell::Cell<Platform> =
        const { std::cell::Cell::new(Platform::Custom("")) };
}

/// The host platform currently being rendered to. Set by the
/// framework during `mount(...)` from `backend.platform()`. Returns
/// `Platform::Custom("")` before any mount has happened on this
/// thread.
pub fn platform() -> Platform {
    CURRENT_PLATFORM.with(|c| c.get())
}

/// Internal: invoked by `mount(...)` to stash the active backend's
/// identity in the thread-local accessor above. Not part of the
/// public API surface; backends should override [`Backend::platform`]
/// instead.
#[doc(hidden)]
pub fn install_current_platform(platform: Platform) {
    CURRENT_PLATFORM.with(|c| c.set(platform));
}

// ---------------------------------------------------------------------------
// Color scheme
// ---------------------------------------------------------------------------

thread_local! {
    static CURRENT_COLOR_SCHEME: std::cell::Cell<ColorScheme> =
        const { std::cell::Cell::new(ColorScheme::Auto) };
}

/// The host platform's appearance preference (light / dark / auto) as
/// reported by the backend at mount. Read it at startup to pick a
/// matching default theme so the app doesn't flash the wrong one — e.g.
/// `let dark = matches!(color_scheme(), ColorScheme::Dark);`.
///
/// This is the platform's *initial* preference, captured once during
/// `mount(...)` (the same one-shot model as [`platform`]); it is not a
/// reactive subscription to live OS theme changes. Apps that let the
/// user toggle themes own that state themselves and treat this only as
/// the default. Returns [`ColorScheme::Auto`] before any mount has
/// happened on this thread.
pub fn color_scheme() -> ColorScheme {
    CURRENT_COLOR_SCHEME.with(|c| c.get())
}

/// Internal: invoked by `mount(...)` to stash the backend's reported
/// color scheme in the thread-local accessor above. Not part of the
/// public API surface; backends should override [`Backend::color_scheme`]
/// instead.
#[doc(hidden)]
pub fn install_current_color_scheme(scheme: ColorScheme) {
    CURRENT_COLOR_SCHEME.with(|c| c.set(scheme));
}

// ---------------------------------------------------------------------------
// External URL opener
// ---------------------------------------------------------------------------
//
// `mount(...)` reads the backend's `url_opener()` once at startup and
// stashes the returned closure here so author code can fire an
// external navigation (`open_url("https://…")`) from anywhere —
// component bodies, event handlers, effects — without threading a
// `Backend` reference. The opener is a self-contained closure because
// opening a URL is a stateless platform call on every backend
// (`window.open`, `UIApplication.open`, an `ACTION_VIEW` intent,
// `NSWorkspace.open`); it captures only the platform handles the
// backend needs, never the view tree.

thread_local! {
    static URL_OPENER: std::cell::RefCell<Option<Rc<dyn Fn(&str)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Open `url` in the host platform's external handler — a new browser
/// tab on web, Safari/Chrome via `UIApplication.open` on iOS, an
/// `ACTION_VIEW` intent on Android, the default browser via
/// `NSWorkspace` on macOS.
///
/// This is for *leaving* the app: external web pages, `mailto:`,
/// `tel:`, etc. It is deliberately distinct from the [`Link`]
/// primitive, which navigates *within* the app's navigator and (on
/// web) must stay single-page. A `Link` requires a `Route` and an
/// ambient navigator; an external URL has neither, so it gets its own
/// imperative entry point rather than overloading `Link`.
///
/// Routes to the opener the active backend installed during
/// [`mount`](crate::mount). Before any mount — or on a backend that
/// reports no external-open capability ([`Backend::url_opener`]
/// returned `None`: terminal, CPU, runtime-server) — this is a no-op
/// that logs once at debug level. Fire-and-forget: there is no
/// success signal, matching the lowest common denominator across
/// `window.open` / `openURL` / `startActivity`.
///
/// [`Link`]: crate::primitives::link
pub fn open_url(url: &str) {
    // Clone the Rc out before invoking so the opener can re-enter
    // framework code (e.g. trigger a re-mount) without tripping a
    // RefCell double-borrow on this thread-local.
    let opener = URL_OPENER.with(|cell| cell.borrow().clone());
    match opener {
        Some(open) => open(url),
        None => crate::logging::log(
            crate::logging::LogLevel::Debug,
            "open_url: no URL opener installed for the active backend; \
             ignoring external navigation",
        ),
    }
}

/// Internal: invoked by `mount(...)` to stash the active backend's
/// external-URL opener (from [`Backend::url_opener`]). Not part of the
/// public API surface; backends override `url_opener` instead.
#[doc(hidden)]
pub fn install_url_opener(opener: Option<Rc<dyn Fn(&str)>>) {
    URL_OPENER.with(|cell| *cell.borrow_mut() = opener);
}

thread_local! {
    static FULLSCREEN_SETTER: std::cell::RefCell<Option<Rc<dyn Fn(bool)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Enter (`true`) or leave (`false`) full-screen / immersive mode —
/// the host's "maximize the canvas, hide system chrome" state.
///
/// The observable intent is uniform — give the app the whole display
/// and get the platform's own chrome out of the way — but the
/// mechanism is per-backend (rule 7: converge in behavior, diverge in
/// mechanism):
///
/// - **Android** — immersive-sticky (hides the status + navigation
///   bars) PLUS full-bounds `setSystemGestureExclusionRects` on the
///   decor view. Immersive is the only state in which the system lifts
///   its 200dp-per-edge cap on gesture exclusion; with the cap lifted,
///   the exclusion hands left/right edge swipes to the app as ordinary
///   touches instead of the system back gesture. Net on a full-screen
///   canvas: an edge swipe becomes a stroke — no back-arrow indicator,
///   no navigation, and no transient-bar flash (the swipe never reaches
///   the reveal-bars path). The bottom home gesture stays mandatory.
///   Pair with a navigator's `back_enabled(false)` as the commit-time
///   safety net.
/// - **iOS** — hides the status bar and the home indicator on the
///   root view controller.
/// - **macOS** — toggles native window full-screen.
/// - **Web** — best-effort Fullscreen API (`requestFullscreen` /
///   `exitFullscreen`); browsers require a user gesture, so a call
///   outside one may be ignored.
/// - **Terminal / other** — no system chrome to hide; no-op.
///
/// Routes to the setter the active backend installed during
/// [`mount`](crate::mount). Before any mount — or on a backend with no
/// full-screen concept — this is a no-op that logs once at debug
/// level. It's an explicit, navigation-independent app control: any
/// screen can enter or leave full-screen, drawing surface or not.
pub fn set_fullscreen(enabled: bool) {
    // Clone the Rc out before invoking so the setter can re-enter
    // framework code without tripping a RefCell double-borrow.
    let setter = FULLSCREEN_SETTER.with(|cell| cell.borrow().clone());
    match setter {
        Some(set) => set(enabled),
        None => crate::logging::log(
            crate::logging::LogLevel::Debug,
            "set_fullscreen: no full-screen setter installed for the active \
             backend; ignoring",
        ),
    }
}

/// Internal: invoked by `mount(...)` to stash the active backend's
/// full-screen setter (from [`Backend::fullscreen_setter`]). Not part
/// of the public API surface; backends override `fullscreen_setter`.
#[doc(hidden)]
pub fn install_fullscreen_setter(setter: Option<Rc<dyn Fn(bool)>>) {
    FULLSCREEN_SETTER.with(|cell| *cell.borrow_mut() = setter);
}

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

pub trait Backend {
    type Node: Clone;

    /// Returns the platform's current color scheme. Called before the
    /// first render so the app can select a matching default theme.
    /// Defaults to `ColorScheme::Auto` (no preference).
    fn color_scheme(&self) -> ColorScheme {
        ColorScheme::Auto
    }

    /// Identifies the host platform this backend renders to. Author
    /// code reads this when it needs to branch on host — different
    /// keyboard shortcuts on `MacOs`, no `position: fixed` analog
    /// outside `Web`, etc.
    ///
    /// Defaults to `Platform::Custom("")` (no identity declared) so
    /// test mocks and early stubs compile without scaffolding; each
    /// first-party backend overrides to return its concrete identity,
    /// and custom embedders can override with `Custom("their-name")`.
    ///
    /// Sim/emulator vs. real-device is not a separate signal — each
    /// backend folds it into the `Platform` value it returns
    /// (e.g. the iOS-simulator build of the iOS backend returns
    /// `Custom("Sim")`). Author code reads one thing.
    fn platform(&self) -> Platform {
        Platform::Custom("")
    }

    /// Hand the framework a self-contained closure that opens an
    /// external URL in the host platform's default handler. Read once
    /// by [`mount`](crate::mount) and stashed in a thread-local so
    /// author code can call [`open_url`](crate::open_url) without a
    /// `Backend` reference.
    ///
    /// The closure must not borrow the backend's view tree — opening a
    /// URL is a stateless platform call (`window.open`,
    /// `UIApplication.open`, an `ACTION_VIEW` intent,
    /// `NSWorkspace.open`). It may capture cheap platform handles the
    /// call needs (e.g. Android's `Activity` global ref).
    ///
    /// Default `None`: the backend has no external-open capability
    /// (terminal, CPU, runtime-server). `open_url` calls become a
    /// logged no-op on those backends. This is distinct from the
    /// [`Link`](crate::primitives::link) primitive's in-app
    /// navigation — see [`open_url`](crate::open_url) for why the two
    /// are separate.
    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        None
    }

    /// Hand the framework a self-contained closure that toggles the
    /// host's full-screen / immersive mode (`true` = enter, `false` =
    /// leave). Read once by [`mount`](crate::mount) and stashed in a
    /// thread-local so author code can call
    /// [`set_fullscreen`](crate::set_fullscreen) without a `Backend`
    /// reference.
    ///
    /// Like [`url_opener`](Backend::url_opener), the closure must not
    /// borrow the view tree — it makes a stateless window/system call
    /// (Android `WindowInsetsController`, iOS status-bar/home-indicator
    /// override, macOS `toggleFullScreen:`, web Fullscreen API). It may
    /// capture cheap platform handles (Android `Activity`, an `NSWindow`
    /// pointer) the call needs.
    ///
    /// Default `None`: the backend has no full-screen concept (terminal,
    /// CPU, runtime-server); [`set_fullscreen`](crate::set_fullscreen)
    /// becomes a logged no-op there.
    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        None
    }

    fn create_view(&mut self, a11y: &crate::accessibility::AccessibilityProps) -> Self::Node;

    /// Create a structural element with an explicit HTML-ish tag (e.g.
    /// `"pre"`, `"code"`, `"ul"`). Lets a third-party `Element::External`
    /// handler build real DOM structure cross-backend *through the
    /// Backend* — so it renders headlessly on SSR and participates in web
    /// hydration's DOM adoption (rather than reaching for `web_sys`
    /// directly, which bypasses both).
    ///
    /// Document-backed backends (web/SSR) create the actual tag; backends
    /// with no tag concept (iOS/Android/terminal) fall back to a plain
    /// container view — the element's children/text still render, just
    /// without the tag's semantics.
    #[allow(unused_variables)]
    fn create_element(&mut self, tag: &str) -> Self::Node {
        self.create_view(&crate::accessibility::AccessibilityProps::default())
    }

    /// Whether the backend is mid SSR-hydration adoption. Read by
    /// [`mount`](crate::mount) to drain deferred microtasks inside the
    /// adoption window. Default `false`; only the web backend adopts.
    fn is_hydrating(&self) -> bool {
        false
    }

    /// Whether `Element::Lazy` should resolve its chunk and render the
    /// loaded body, or stop at the placeholder (loading state).
    ///
    /// Default `true`: web drives the async chunk load, and native
    /// compiles the chunk inline (its loader resolves on first poll). The
    /// **SSR backend returns `false`** so a headless render emits the
    /// placeholder/loading state rather than synchronously resolving the
    /// chunk — lazy content (e.g. a GPU `Graphics`/`Simulator` canvas)
    /// can't render on the server, and shipping the resolved body makes
    /// the SSR HTML diverge from the client's placeholder (which hydration
    /// then has to tear down). The live client loads the real chunk after
    /// hydrating the matching placeholder.
    fn renders_lazy_chunks(&self) -> bool {
        true
    }

    /// Stamp a stable identifier on `node` that JS code can find via
    /// `document.getElementById` (web) or analogous mechanisms
    /// elsewhere. Used by `Element::Lazy`'s web handler: the chunk
    /// loader needs a known DOM id so the chunk's `mount_chunk`
    /// export can root its own `WebBackend` at the placeholder
    /// container.
    ///
    /// Default impl is a no-op — only the web backend has a real
    /// implementation. Other backends (iOS, Android, terminal, …)
    /// don't need DOM ids; their `Element::Lazy` dispatch path
    /// runs inline through the thread-local registry and the node
    /// itself is the mount target.
    #[allow(unused_variables)]
    fn attach_html_id(&self, node: &Self::Node, id: &str) {}

    /// Stamp a layout class name on `node`, paired with
    /// [`register_raw_css`](Backend::register_raw_css) which ships the
    /// matching rules. Used for navigator chrome: the SSR backend stamps
    /// the same `ui-nav-*` classes the live web navigator does (see
    /// `css::nav_class`) so the server's first paint matches the client.
    ///
    /// Default no-op — only document-backed backends (web, SSR) need it;
    /// native backends lay out chrome via their own primitives.
    #[allow(unused_variables)]
    fn attach_html_class(&self, node: &Self::Node, class: &str) {}
    fn create_text(
        &mut self,
        content: &str,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node;
    fn create_button(
        &mut self,
        label: &str,
        on_click: &crate::derive::Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node;
    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node);

    /// Tappable container node with a click handler attached. Used
    /// by [`Element::Pressable`]. Children are inserted into this
    /// node via the regular `insert` path.
    ///
    /// Default impl falls back to `create_view` — appropriate for
    /// backends that don't yet support pressables (clicks won't
    /// fire, but the subtree still renders). Web overrides with a
    /// `<div>` that has `cursor: pointer` and an `onclick` handler.
    #[allow(unused_variables)]
    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }

    /// Install a raw touch handler on `node`. The framework calls this
    /// once per `Element::View { on_touch: Some(_), .. }` (and any
    /// other primitive that grows a touch slot in the future) after
    /// the node is created.
    ///
    /// The backend's job is to wire `handler` to whatever native touch
    /// delivery mechanism it uses (UIView subclass + `touchesBegan:`,
    /// Android `OnTouchListener`, winit pointer events, web Pointer
    /// Events) and invoke it for every event hitting this node, with
    /// the event already translated into framework coordinates.
    ///
    /// Default impl is a no-op — appropriate for backends that don't
    /// yet support raw touch. Subscribed views still render; they just
    /// receive no events. See `docs/native-touch-plan.md` for the
    /// design and the per-platform implementation notes.
    #[allow(unused_variables)]
    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: crate::TouchHandler,
    ) {
        // default: no-op
    }

    /// Called when a handler returns
    /// [`TouchResponse { claim: true, .. }`](crate::TouchResponse).
    /// The backend decides locally how to suppress competing native
    /// consumers of this touch — parent scroll containers, system
    /// gestures, pointer-capture, etc. The framework does not enumerate
    /// or know about those mechanisms; they are implementation-private
    /// to each backend.
    ///
    /// Default impl is a no-op. Backends that don't implement the
    /// claim protocol will see scroll containers win contested touches.
    #[allow(unused_variables)]
    fn claim_touch(&mut self, node: &Self::Node, touch_id: crate::TouchId) {
        // default: no-op
    }

    /// Placeholder node for reactive `when` / `switch` branches.
    /// The walker creates one of these as a stable parent that
    /// stays put across branch swaps, with the live branch's
    /// children re-inserted on each rebuild.
    ///
    /// On web the anchor needs to be layout-transparent
    /// (`display: contents`) so the branch's children inherit the
    /// surrounding flex / sizing context — otherwise an extra
    /// `<div>` collapses widths and breaks `flex: 1` / `width:
    /// 100%` on full-width children. Native backends have no such
    /// problem; the default `create_view` is fine.
    fn create_reactive_anchor(&mut self) -> Self::Node {
        self.create_view(&crate::accessibility::AccessibilityProps::default())
    }

    /// Batched insertion of many siblings into `parent`. Default
    /// implementation falls back to N `insert` calls — backends
    /// override this to collapse N FFI crossings into one (e.g.
    /// web uses a `DocumentFragment` to push 10 000 children in
    /// a single `appendChild` call). Called by the build walker
    /// when it expands a `Element::Repeat` produced by `ui!`'s
    /// `for` lowering.
    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        for child in children {
            self.insert(parent, child);
        }
    }

    /// Backend capability flag for the local-render batched-Repeat
    /// path. When `true`, the walker collapses `Element::Repeat`
    /// expansions whose rows match the batchable shape (static
    /// View+Text+style — see [`crate::BackendBatch`]) into one
    /// [`execute_batch`](Self::execute_batch) call. When `false`,
    /// the walker uses the existing per-call path: one
    /// `create_view`/`create_text`/`apply_style`/`insert` chain per
    /// row.
    ///
    /// Web backend opts in for the rebuild benchmark's pattern.
    /// Native backends keep the per-call path — their FFI cost per
    /// call is already small and the batching benefit doesn't pay
    /// for the encoding/decoding overhead. Default `false`.
    fn supports_batched_repeat(&self) -> bool {
        false
    }

    /// Execute a queued [`BackendBatch`] in a single round-trip and
    /// return the materialized nodes, indexed by `local_id`.
    ///
    /// The walker submits this when expanding a `Element::Repeat`
    /// whose rows are all batchable (static View+Text trees with
    /// static styles). On the web backend this turns ~4N FFI calls
    /// (createElement, createTextNode, setAttribute, appendChild ×
    /// N) into a single wasm→JS call carrying the whole op stream.
    ///
    /// The returned `Vec`'s length must equal
    /// `batch.node_count as usize`. Element at index `i` is the node
    /// that corresponds to `local_id == i`.
    ///
    /// Backends that don't implement batching keep the default
    /// `unimplemented!()` — the walker only calls this when
    /// [`supports_batched_repeat`](Self::supports_batched_repeat)
    /// returned `true`.
    #[allow(unused_variables)]
    fn execute_batch(&mut self, batch: crate::BackendBatch) -> Vec<Self::Node> {
        unimplemented!(
            "execute_batch is only called when supports_batched_repeat() returns true; \
             this backend opted in without implementing it"
        )
    }

    /// Execute a [`BackendBatch`] AND parent the listed row tops to
    /// `parent` — all in one backend call.
    ///
    /// `attach_locals` is a slice of `local_id`s from the batch
    /// (typically the row-top ids), in the order they should be
    /// appended to `parent`. Equivalent to:
    ///
    /// ```ignore
    /// let nodes = backend.execute_batch(batch);
    /// let rows: Vec<_> = attach_locals.iter().map(|&id| nodes[id as usize].clone()).collect();
    /// backend.insert_many(parent, rows);
    /// nodes
    /// ```
    ///
    /// Backends that override [`execute_batch`](Self::execute_batch)
    /// SHOULD also override this when they can fold the parent-attach
    /// into the same FFI round-trip. On the web backend that saves
    /// N `appendChild` FFI hops (one per child) — at 100 k rows that
    /// was measured at ~60 ms in the rebuild bench. Backends that
    /// don't override fall through to the default impl, which is
    /// the literal sequence above.
    fn execute_batch_with_attach(
        &mut self,
        batch: crate::BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        let nodes = self.execute_batch(batch);
        if !attach_locals.is_empty() {
            let rows: Vec<Self::Node> = attach_locals
                .iter()
                .map(|&id| nodes[id as usize].clone())
                .collect();
            self.insert_many(parent, rows);
        }
        nodes
    }

    fn update_text(&mut self, node: &Self::Node, content: &str);

    /// Optional batched-text-update fast path.
    ///
    /// At hierarchy-scale fan-outs (one signal feeding 2 k+ reactive
    /// text leaves), the dominant cost in the per-leaf hot path is
    /// the wasm→host FFI marshalling of each `update_text(node, str)`
    /// call. Backends that can amortize this — e.g. the web backend's
    /// JS-side text registry + batched flush — opt in by returning
    /// `Some((node, id))` here. The walker captures `id` in the
    /// reactive text effect's closure and calls
    /// [`Backend::update_text_by_id`] on subsequent fires instead of
    /// [`Backend::update_text`]; the backend buffers the updates and
    /// flushes them in one FFI hop (typically via a scheduler-driven
    /// microtask).
    ///
    /// Default: `None` — falls through to the unbatched
    /// `create_text` + per-fire `update_text` path used by every
    /// other primitive.
    ///
    /// Backends that override this **must** also override
    /// [`Backend::update_text_by_id`] (default `unreachable!`) and
    /// should arrange for the buffered updates to flush before the
    /// next paint (microtask, rAF, end of `run_effects`, etc.) so
    /// the bench's `apply` window still observes the DOM change.
    fn create_text_with_id(
        &mut self,
        _content: &str,
        _a11y: &crate::accessibility::AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        None
    }

    /// Companion to [`Backend::create_text_with_id`]. Update the text
    /// of the node registered under `id`. Default panics: only
    /// reached if [`Backend::create_text_with_id`] returned `Some`,
    /// in which case the matching backend must implement this too.
    ///
    /// Takes `String` by value (not `&str`) so the walker can move
    /// the already-allocated result of `(compute)()` straight into
    /// the backend's pending buffer — at hierarchy scale (2 k+
    /// effects per fan-out) the alternative `&str` + internal
    /// `.to_string()` would double the allocator pressure for no
    /// reason, since the caller's String is dropped right after the
    /// call anyway.
    fn update_text_by_id(&mut self, _id: u32, _content: String) {
        unreachable!(
            "update_text_by_id called without a matching create_text_with_id override; \
             a backend that returns Some(_) from create_text_with_id must also override \
             update_text_by_id"
        )
    }

    /// Release a text id previously assigned by
    /// [`Backend::create_text_with_id`]. Called by the walker via a
    /// scope-level cleanup so the backend's JS-side registry slot
    /// gets cleared on scope teardown — without this, every
    /// switch-arm flip or component unmount would leak a registry
    /// entry. Default: no-op (matches `create_text_with_id`'s
    /// default of `None`).
    fn release_text_id(&mut self, _id: u32) {}

    /// `true` if this backend implements
    /// [`Backend::register_reactive_text_binding`] (and the
    /// matching signal-side notification plumbing). The walker
    /// reads this once for each `TextSource::JsBinding` to decide
    /// whether to hand the binding to the backend (fast path: per-
    /// fire fan-out happens in backend-native code, e.g. JS for
    /// the web backend) or fall back to building a Rust Effect
    /// against the spec's `compute_fallback`.
    ///
    /// Default `false` — backends opt in only after wiring both
    /// the registration method below AND the side-channel that
    /// delivers signal-change notifications (the web backend uses
    /// `runtime_core::register_signal_js_notifier`).
    fn supports_js_text_bindings(&self) -> bool {
        false
    }

    /// Register a pre-decomposed reactive text binding with the
    /// backend's own fan-out path. After this call, the text node
    /// at `text_id` updates entirely from backend-side code
    /// whenever any signal in `signal_ids` fires — no Rust Effect
    /// is created.
    ///
    /// Called by the walker only when
    /// [`Self::supports_js_text_bindings`] returns `true` AND the
    /// matching `create_text_with_id` returned `Some(id)`. Backends
    /// that don't override this default never have the method
    /// called.
    ///
    /// `template_parts.len() == signal_ids.len() + 1` (the static
    /// text on either side of each interpolation slot).
    /// `initial_values.len() == signal_ids.len()` (the starting
    /// value of each signal, stringified).
    ///
    /// `stringifiers` carries one `Fn() -> String` per entry of
    /// `signal_ids` (parallel arrays) — each closure reads the
    /// current value of the matching signal and Display-formats it
    /// the same way the JS dispatcher will. Backends that ship
    /// updates across an FFI bridge use these to install per-signal
    /// notifiers at bind time so subsequent `signal.set/update`
    /// calls flow through. Backends without a JS bridge ignore the
    /// slice (the per-fire fan-out happens via `compute_fallback`'s
    /// Effect path on those backends, and `register_reactive_text_binding`
    /// isn't called for them — `supports_js_text_bindings()` is the
    /// gate).
    fn register_reactive_text_binding(
        &mut self,
        _text_id: u32,
        _signal_ids: &[u64],
        _template_parts: &[&str],
        _initial_values: &[&str],
        _stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        unreachable!(
            "register_reactive_text_binding called without an override; \
             a backend that returns true from supports_js_text_bindings must \
             also override register_reactive_text_binding"
        )
    }

    /// Release a binding previously registered via
    /// [`Self::register_reactive_text_binding`]. Default no-op so
    /// backends that don't support bindings can ignore the call.
    fn release_reactive_text_binding(&mut self, _text_id: u32) {}

    /// Capability flag for backend-side class bindings (the analog of
    /// [`Self::supports_js_text_bindings`] for `StyleSource::SignalClass`).
    /// When `true`, the walker hands resolved `(signal_id, values,
    /// classes)` tables to the backend at mount and trusts the
    /// backend's own dispatcher to apply the right class on signal
    /// writes — no Rust Effect fires per node. When `false`, the
    /// walker falls back to running the spec's `compute_fallback`
    /// inside a normal style Effect.
    fn supports_js_class_bindings(&self) -> bool {
        false
    }

    /// Register a pre-resolved signal→class binding with the backend.
    /// Called by the walker only when
    /// [`Self::supports_js_class_bindings`] returns `true`.
    ///
    /// - `node`           — the styled DOM/native node
    /// - `signal_id`      — the `Signal::id()` whose writes drive the
    ///                       class change
    /// - `values`         — discrete signal values, in declared order
    /// - `classes`        — class names parallel to `values`; the
    ///                       backend has already minted these via
    ///                       [`Self::mint_style_class`]
    /// - `value_reader`   — untracked accessor returning the signal's
    ///                       current value as `u32`. The backend
    ///                       uses it to seed the initial class on
    ///                       first paint AND to install a
    ///                       signal-changed notifier that ships
    ///                       writes across the FFI boundary.
    ///
    /// Returns a `binding_id` the framework hands back to
    /// [`Self::release_reactive_class_binding`] on scope teardown.
    fn register_reactive_class_binding(
        &mut self,
        _node: &Self::Node,
        _signal_id: u64,
        _values: &[u32],
        _classes: &[&str],
        _value_reader: std::rc::Rc<dyn Fn() -> u32>,
    ) -> u32 {
        unreachable!(
            "register_reactive_class_binding called without an override; \
             a backend that returns true from supports_js_class_bindings must \
             also override register_reactive_class_binding"
        )
    }

    /// Release a binding previously registered via
    /// [`Self::register_reactive_class_binding`]. Default no-op so
    /// backends that don't support class bindings can ignore the
    /// call.
    fn release_reactive_class_binding(&mut self, _binding_id: u32) {}

    /// Optional hook the walker calls when a `Element::Text`'s
    /// source is `TextSource::Bound`. Backends with declarative wire
    /// formats override this to record `signal_ids` + the
    /// transformer `method` name so they can ship the binding to a
    /// remote renderer instead of running the closure locally on
    /// every change.
    ///
    /// Effect-driven backends leave the default no-op in place — the
    /// walker still sets up an `Effect` around the binding's closure
    /// on every backend, which is what those backends rely on. The
    /// metadata is only consumed by backends that need it.
    #[allow(unused_variables)]
    fn note_text_binding(
        &mut self,
        node: &Self::Node,
        signal_ids: &[u64],
        method: &'static str,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls (once per signal per binding)
    /// when it encounters a `TextSource::Bound`. Backends that need
    /// to ship signal state across a wire boundary use this to
    /// declare each signal's existence + initial value to the remote
    /// renderer. Backends that read live signal values directly from
    /// the framework's arena leave the default no-op in place.
    ///
    /// Backends are expected to dedupe internally — the walker will
    /// call this for the *same* signal_id across multiple bindings
    /// if more than one binding reads that signal. Only the first
    /// observation needs to ship a value declaration to the wire.
    #[allow(unused_variables)]
    fn note_signal_initial(
        &mut self,
        signal_id: u64,
        value: &crate::__serde_json::Value,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls after building both branches
    /// of a `Element::When` declaratively. Backends record the
    /// signal IDs the condition reads, the name of the boolean
    /// transformer (`#[method]`) that decides which branch is
    /// active, and the node ids of the then/otherwise subtrees so
    /// the remote runtime can toggle their visibility on signal
    /// change. Only called when `handles_when_natively()` returns
    /// true and the `When` carries a `bind_when!`-produced binding.
    #[allow(unused_variables)]
    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls after building every arm +
    /// default of a `Element::Switch` on the lazy-slot-capture
    /// path. `arms` carries each arm's `(pattern_value, node)` pair
    /// so the remote runtime can compare the discriminant's value
    /// against the pattern and play / tear down the matching arm.
    #[allow(unused_variables)]
    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(crate::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        // default: no-op
    }

    /// Optional hook the walker calls after building the row
    /// template of a `Element::Virtualizer` on the structured /
    /// generator-backend path. Backends record the count method +
    /// the template node so the remote runtime can clone the
    /// template per row (with id remapping) on every
    /// count change.
    #[allow(unused_variables)]
    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        // default: no-op
    }

    /// Hook for `Element::Virtualizer` on the structured /
    /// generator-backend path. Backends opt in to native windowed
    /// list rendering here (Roku → MarkupList). Default delegates
    /// to `note_repeat_binding` so backends that don't yet
    /// implement native virtualization still get correct (if
    /// unwindowed) row rendering.
    #[allow(unused_variables)]
    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        self.note_repeat_binding(
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
        );
    }

    /// Backend capability flag for lazy slot materialization. When
    /// `true`, the walker wraps each `bind_when!`/`bind_switch!`/
    /// `bind_repeat!` slot's subtree build in `begin_slot_capture`/
    /// `end_slot_capture` calls and skips attaching the slot's root
    /// to the anchor at build time — the backend captures the slot's
    /// commands so the remote runtime can play / tear them down on
    /// demand. Default `false` (eager mode): every slot is built
    /// and attached up-front, the way the framework has always
    /// worked. Backends like Roku with no host-side runtime opt in
    /// so inactive subtrees never materialize on the device.
    fn supports_lazy_slot_capture(&self) -> bool {
        false
    }

    /// Begin a slot-capture region. Subsequent backend mutations
    /// (create_*, insert, apply_style, etc.) should be redirected
    /// from the main command stream into a capture buffer kept by
    /// the backend. Called only when `supports_lazy_slot_capture()`
    /// is true.
    fn begin_slot_capture(&mut self) {
        // default: no-op
    }

    /// End the most-recent slot-capture region. The backend should
    /// associate the captured commands with `slot_root` so a later
    /// `note_when_binding` / `note_switch_binding` /
    /// `note_repeat_binding` call can package them into the
    /// appropriate binding's wire form.
    #[allow(unused_variables)]
    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        // default: no-op
    }

    /// Create an image node with the initial URL. The framework
    /// wraps the user's `src` source in an effect that calls
    /// `update_image_src` whenever the source changes.
    #[allow(unused_variables)]
    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_image not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        // default: no-op; backends that don't implement images just
        // leave the URL static.
    }

    /// Create an icon node from static vector path data. The initial
    /// color (if any) is provided; reactive color updates flow through
    /// `update_icon_color`.
    ///
    /// Backends render the paths natively:
    /// - **Web**: inline `<svg>` with `<path>` children.
    /// - **iOS**: `CAShapeLayer` with `UIBezierPath`.
    /// - **Android**: `VectorDrawable` or `Canvas.drawPath`.
    #[allow(unused_variables)]
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_icon not implemented for this backend")
    }

    /// Update an icon's fill color reactively. Called by the walker's
    /// Effect when the color closure re-fires.
    #[allow(unused_variables)]
    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        // default: no-op
    }

    /// Set the icon's stroke progress immediately (no animation).
    /// `progress` is 0.0 (nothing drawn) to 1.0 (fully drawn).
    /// Called by the walker's reactive Effect when the `stroke`
    /// closure re-fires.
    #[allow(unused_variables)]
    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        // default: no-op — icon stays fully drawn
    }

    /// Animate the icon's stroke from `from` to `to` over `duration_ms`
    /// with the given easing. Called once on mount for `draw_in`, or
    /// imperatively via `IconHandle::animate_stroke`.
    ///
    /// When `infinite` is true, the animation loops (from→to→from→…).
    ///
    /// Platforms implement this with their native animation system:
    /// - Web: CSS `@keyframes` animation on `stroke-dashoffset`
    /// - iOS: `CABasicAnimation` on `strokeEnd` with `repeatCount = .infinity`
    /// - Android: `ObjectAnimator` with `setRepeatCount(INFINITE)`
    #[allow(unused_variables)]
    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: crate::style::Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        // default: no-op — icon renders fully drawn
    }

    /// Update a button's visible label. Called by the walker's
    /// reactive-label Effect when the user passed a closure (or any
    /// expression containing `.get()`) for the `label` prop. Default
    /// impl falls back to `update_text` — most backends use the same
    /// underlying widget API for both ("setText" on Android,
    /// `textContent` on the web button element). Backends with a
    /// distinct button-text API can override.
    #[allow(unused_variables)]
    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        self.update_text(node, label);
    }

    /// Per-frame write of an animated scalar property to `node`.
    ///
    /// Called by [`AnimatedValue`](crate::animation::AnimatedValue)
    /// listeners that the author has wired through to a node — at
    /// whatever frame rate the animation clock is running. Backends
    /// implement this against their fast property-write paths
    /// (DOM style on web, `UIView` setters on iOS, `View` setters on
    /// Android) and dispatch on `prop` to pick the right one.
    ///
    /// **Units & ranges**:
    /// - `Opacity`: `0.0..=1.0`
    /// - `TranslateX` / `TranslateY`: device-independent pixels
    /// - `Scale` / `ScaleX` / `ScaleY`: multiplicative (1.0 = identity)
    /// - `RotateZ`: degrees, clockwise
    ///
    /// Default impl is a no-op so backends without animation
    /// support remain author-portable — the value handle ticks
    /// (and listeners fire), the backend just doesn't paint it.
    #[allow(unused_variables)]
    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: crate::animation::AnimProp,
        value: f32,
    ) {
        // default: no-op
    }

    /// Per-frame write of an animated color property to `node`.
    /// `value` is sRGB `[r, g, b, a]`, channels in `0..=1`.
    ///
    /// Same lifecycle and contract as
    /// [`Backend::set_animated_f32`].
    #[allow(unused_variables)]
    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: crate::animation::AnimProp,
        value: [f32; 4],
    ) {
        // default: no-op
    }

    /// Create a text input with the initial value, placeholder, and
    /// callbacks. `on_change` fires on every native input event;
    /// `on_key_down`, if set, fires on every keydown before the
    /// platform's default action (see
    /// [`primitives::key`](crate::primitives::key) for the
    /// cross-platform contract). The framework wraps the controlled
    /// `value` signal in an effect that calls
    /// `update_text_input_value` on signal change.
    ///
    /// `secure` masks the entered text (password entry); backends map it to
    /// their native secure-entry mode (web `type="password"`, UIKit
    /// `isSecureTextEntry`, etc.). The masked behaviour is identical across
    /// backends.
    #[allow(unused_variables)]
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        secure: bool,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_text_input not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {}

    /// Create a multi-line text editor. Same controlled pattern as
    /// `create_text_input` (the framework wraps `value` in an effect
    /// that calls `update_text_area_value` on change); the only
    /// semantic difference is that Enter inserts a newline.
    ///
    /// `wrap` soft-wraps long lines at the box edge (`true`) or keeps
    /// them unwrapped with horizontal scroll (`false`, the code-editor
    /// shape).
    ///
    /// The textarea is intrinsically sized to its content: like the
    /// text primitive, a backend reports the height its text needs to
    /// the layout engine (via an intrinsic measure function on native
    /// toolkits, or the equivalent on web), and the style's
    /// `height` / `min_height` / `max_height` constrain it. There is no
    /// "autogrow" flag — growing to fit is simply what an unconstrained
    /// height does; a pinned height (or sized parent) yields a fixed,
    /// scrolling box.
    #[allow(unused_variables)]
    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_text_area not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {}

    /// Create a toggle (switch / checkbox) with the initial value and
    /// an `on_change` callback. Same controlled-update pattern as
    /// text input.
    #[allow(unused_variables)]
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_toggle not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {}

    /// Create a scrolling container. `horizontal` selects the
    /// scrolling axis (false = vertical, the default; true = horizontal).
    ///
    /// `on_scroll`, if `Some`, fires on every scroll-offset change with
    /// `(scroll_left_px, scroll_top_px)` in CSS pixels / native points.
    /// Each backend binds this to its native scroll observer (web
    /// `scroll` event, iOS `UIScrollViewDelegate::scrollViewDidScroll`,
    /// Android `OnScrollChangeListener`, etc.). Backends with no scroll
    /// concept (terminal, CPU graphics) ignore the callback.
    #[allow(unused_variables)]
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_scroll_view not implemented for this backend")
    }

    /// Create a slider widget. `min`/`max`/`step` are static after
    /// creation; controlled value updates flow through
    /// `update_slider_value`. `on_change` fires on every drag tick.
    #[allow(unused_variables)]
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_slider not implemented for this backend")
    }
    #[allow(unused_variables)]
    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {}

    /// Create a loading spinner. Size/color are static at construction.
    #[allow(unused_variables)]
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_activity_indicator not implemented for this backend")
    }

    /// Create a virtualized list. The backend gets a bundle of
    /// callbacks (via `VirtualizerCallbacks`) it uses to query the
    /// current data set, request mounted subtrees, and release
    /// them when items leave the viewport / get recycled.
    ///
    /// The backend owns the scroll handler and the visible-window
    /// math. It calls `mount_item(idx)` when an index needs to
    /// become visible, getting back `(node, scope_id)`. When the
    /// index leaves the visible window (web: scrolled out; native:
    /// cell recycled), the backend calls `release_item(scope_id)`
    /// to free the framework's per-item Scope — which drops every
    /// signal, effect, and ref nested inside that item.
    #[allow(unused_variables)]
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_virtualizer not implemented for this backend")
    }

    /// Signal that the underlying data set has changed. The backend
    /// re-queries item_count + item_key + item_size to figure out
    /// what changed, runs its diff, and updates the mounted set
    /// accordingly. Called from an Effect that reads the data signal,
    /// so it fires on every data update automatically.
    #[allow(unused_variables)]
    fn virtualizer_data_changed(&mut self, node: &Self::Node) {}

    /// Tear down a Virtualizer's backend-side state. The framework
    /// calls this when the primitive's enclosing scope drops — a
    /// `when` branch flip, a `switch` arm rebuild, list recycling,
    /// `Owner` teardown.
    ///
    /// Backends should: detach DOM/native scroll listeners and
    /// observers, drop the wasm-bindgen (or JNI) closure handles
    /// they handed the JS/JVM side, and remove the per-node
    /// instance entry from any internal map.
    ///
    /// **Why this exists**: the user's data closures (passed into
    /// `VirtualizerCallbacks`) typically capture `Signal<T>`s
    /// scoped to the same teardown event. Without this hook, a
    /// browser-queued scroll/resize event firing after the scope
    /// dropped would invoke a Rust callback against a freed
    /// `Signal` slot, panicking with "signal used after its scope
    /// was dropped". Default impl is a no-op for backends that
    /// don't yet implement Virtualizer.
    #[allow(unused_variables)]
    fn release_virtualizer(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Create a Graphics surface. The backend stands up its native
    /// drawable widget (`<canvas>` on web, `SurfaceView` on Android,
    /// `UIView`+`CAMetalLayer` on iOS), wires up its surface
    /// lifecycle to fire `on_ready` / `on_resize` / `on_lost`, and
    /// returns the host node for the layout tree. The framework
    /// doesn't know what GPU library the author will use; backends
    /// just need to expose their drawable as a
    /// `raw_window_handle::HasWindowHandle + HasDisplayHandle`.
    #[allow(unused_variables)]
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_graphics not implemented for this backend")
    }

    /// Tear down a Graphics surface. The framework calls this when
    /// the primitive's enclosing scope drops — typically a `When`
    /// branch flipping or `Owner` teardown. Backends should drop
    /// their wgpu device, queue, surface, the user's render state,
    /// any rAF / ResizeObserver closures, and remove the per-node
    /// instance entry. Default impl is a no-op for backends that
    /// don't implement Graphics.
    #[allow(unused_variables)]
    fn release_graphics(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Remove every child from `node`. Used by reactive conditionals when
    /// the active branch flips and the old subtree needs to be unmounted.
    fn clear_children(&mut self, node: &Self::Node);

    /// Capability flag for ANCHORLESS reactive regions. When `true`, the
    /// framework can splice a reactive control-flow region's children
    /// directly into the parent — adding/removing exactly the region's
    /// own nodes via [`remove_child`](Self::remove_child) — instead of
    /// nesting them under a [`create_reactive_anchor`](Self::create_reactive_anchor)
    /// wrapper. This keeps reactive `for`/`if`/`match` children as FLAT
    /// siblings on every backend (the web anchor is already
    /// `display:contents`-transparent; native backends have no such
    /// thing, so anchorless splicing is how they avoid a wrapper view).
    ///
    /// Default `false`: backends keep the anchored region path
    /// (`create_reactive_anchor` + `clear_children` + re-insert) until
    /// they implement [`remove_child`](Self::remove_child) and opt in
    /// here. This is the gate for the runtime-decided / heuristic-free
    /// control-flow lowering — a backend reporting `false` is unaffected
    /// by it.
    fn supports_child_splice(&self) -> bool {
        false
    }

    /// Remove a *specific* `child` from `parent` (unlike
    /// [`clear_children`](Self::clear_children), which removes all).
    /// Used by anchorless reactive regions to unmount exactly the rows
    /// they previously inserted before rebuilding, leaving sibling
    /// content (and other regions' rows) in place.
    ///
    /// Default is a no-op — only meaningful for backends that return
    /// `true` from [`supports_child_splice`](Self::supports_child_splice);
    /// the framework never calls it otherwise.
    #[allow(unused_variables)]
    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        // default: no-op
    }

    /// Insert `child` into `parent` at `index` among its current
    /// children (clamped to the end if `index` exceeds the child
    /// count). Companion to [`remove_child`](Self::remove_child): an
    /// anchorless reactive region uses it to splice its rows at the
    /// region's stable position, so a region with trailing siblings
    /// rebuilds in the right place instead of appending to the end.
    ///
    /// Default falls back to [`insert`](Self::insert) (append) — only
    /// meaningful for backends that return `true` from
    /// [`supports_child_splice`](Self::supports_child_splice).
    #[allow(unused_variables)]
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        self.insert(parent, child);
    }

    /// Apply a resolved style to a node. The framework has already run
    /// the stylesheet's closure against the active theme; the backend
    /// receives concrete `StyleRules` with literal values.
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>);

    /// Mint (or look up) a backend-side class identifier for a
    /// resolved style **without** touching any DOM node. Used by the
    /// batched-Repeat path so the walker can compute class names
    /// pre-batch and feed them into a single
    /// [`execute_batch`](Self::execute_batch) call.
    ///
    /// Returns `None` for backends that don't have a named-class
    /// model (most native backends — they apply styles imperatively
    /// to each node and have nothing to mint up front). In that case
    /// the walker treats the Repeat as non-batchable and falls back
    /// to the per-call path. Web overrides this to either return a
    /// cached pre-generated class name or mint a fresh dynamic class
    /// (inserting the CSS rule into the shared sheet, with no
    /// per-node tracking — the per-node bookkeeping happens later
    /// when the batch's `ApplyStyleStatic` op fires).
    #[allow(unused_variables)]
    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        None
    }

    /// Mint a class name for a `StyleApplication` without applying it
    /// to any node. Used by the `StyleSource::SignalClass` walker
    /// path to pre-resolve the (value → class) table at mount: each
    /// app needs a class name the JS-side dispatcher can stamp into
    /// `setAttribute('class', …)` on signal writes, but the framework
    /// won't run a node-level apply for these (the JS shim does
    /// that itself once signals fire).
    ///
    /// Differs from [`Self::mint_style_class`] in two ways: it
    /// takes a `StyleApplication` (not a resolved `Rc<StyleRules>`)
    /// so backends can do the registration + resolve + content-key
    /// dance internally, AND it's allowed to mint fresh dynamic
    /// classes (insert a CSS rule for the resolved content)
    /// rather than returning `None` for unknown content. Default
    /// returns `None`; backends without a named-class model leave
    /// it that way and the walker falls back to the spec's
    /// `compute_fallback` as a normal reactive style.
    #[allow(unused_variables)]
    fn mint_class_for_app(&mut self, app: &crate::style::StyleApplication) -> Option<String> {
        None
    }

    /// Apply a base style plus per-state overlays. Called when the
    /// stylesheet declares interaction-state blocks (`state hovered`,
    /// `state pressed`, etc.) AND the backend reports native state
    /// handling via [`Backend::handles_states_natively`].
    ///
    /// Web overrides this to emit the overlays as CSS pseudo-class
    /// rules scoped to the base class — the browser then handles
    /// state tracking natively. No Rust↔JS round trip per event.
    ///
    /// Backends that rely on event-driven state activation
    /// (`attach_states` + signal-driven re-resolve) leave both the
    /// default impl AND `handles_states_natively() = false`. State
    /// overlays reach those backends through the regular
    /// `apply_style` path when the state signal flips.
    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        #[allow(unused_variables)] overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        // Default: just apply the base style. Mobile backends drive
        // state overlays via signal-flip → re-resolve → apply_style.
        self.apply_style(node, base);
    }

    /// Apply a base style plus per-state overlays **and**
    /// per-breakpoint overlays in a single call. This is the
    /// declarative entry the build walker uses for backends that report
    /// [`Backend::handles_states_natively`] — it supersets
    /// [`Backend::apply_styled_states`] with the breakpoint axis so a
    /// backend can emit *all* of a node's variant overlays (interaction
    /// states + responsive breakpoints) keyed off one base class,
    /// rather than minting two competing base classes from two calls.
    ///
    /// Web overrides this: state overlays become CSS pseudo-class rules
    /// (`.ui-x:hover { … }`) and breakpoint overlays become
    /// `@media (min-width: <threshold>px) { .ui-x { … } }` rules, both
    /// scoped to the node's base class. The browser then selects the
    /// active overlays natively — which is the whole point for SSR:
    /// the statically-rendered HTML already carries the correct
    /// responsive layout in its stylesheet, with no JS/hydration round
    /// trip needed for the first paint.
    ///
    /// `breakpoint_overlays` are each the *fully resolved* rules for
    /// that bucket (base merged with the `__bp_*` overlay), in ascending
    /// breakpoint order, so stacking them by `min-width` reproduces the
    /// mobile-first cascade. See `walker::resolve_breakpoint_overlays`.
    ///
    /// **Contract:** any backend returning `handles_states_natively() ==
    /// true` MUST handle `breakpoint_overlays` here (web does). The
    /// default impl drops them and delegates to
    /// [`Backend::apply_styled_states`]; that's correct only for
    /// backends that report `false` — they never reach this path,
    /// receiving breakpoint overlays through the walker's reactive merge
    /// (driven by [`crate::current_breakpoint`]) into the regular
    /// `apply_style` path instead.
    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        #[allow(unused_variables)] breakpoint_overlays: &[(crate::Breakpoint, Rc<StyleRules>)],
    ) {
        self.apply_styled_states(node, base, state_overlays);
    }

    /// Backend capability flag. `true` means the backend wants to
    /// receive state overlays declaratively via `apply_styled_states`
    /// and handle state tracking natively (e.g. CSS pseudo-classes
    /// on web). `false` means the backend uses the event-driven path:
    /// `attach_states` registers native event listeners that flip the
    /// framework's per-node state signal, and each state change
    /// re-fires the style effect with the appropriate overlay merged
    /// into a fresh `StyleApplication`.
    ///
    /// The framework reads this once per `attach_style` to choose
    /// between the two paths. Default is `false` — backends opt in.
    fn handles_states_natively(&self) -> bool {
        false
    }

    /// True if `update_tokens` on this backend propagates new token
    /// values to every node referencing those tokens, WITHOUT
    /// requiring the framework to re-apply each styled node's
    /// resolved rules.
    ///
    /// Web backends emit `var(--token, fallback)` references in
    /// CSS for `Tokenized<T>` values; on `update_tokens` they set
    /// the corresponding `--token` on `:root` and the browser's
    /// cascade does the rest. No per-node `setAttribute` or CSS
    /// rule re-emit is needed for theme value changes.
    ///
    /// Native backends typically resolve tokens to literal values
    /// at apply time, so a value change requires per-node
    /// re-application. They return `false` (the default).
    ///
    /// The framework reads this at the cohort-driver level: when
    /// true, the driver skips iterating the cohort on token-only
    /// updates. When false (or when the stylesheet contains
    /// `Derived<T>` that resolves to a concrete value rather than
    /// a token reference), the driver fans out to all members.
    ///
    /// Caveat: if author code uses `Derived<T>` whose closure
    /// produces a *concrete* value computed from token values
    /// (e.g. a custom `Color::lighten` against `t.primary`), the
    /// resulting CSS rule body contains the literal RGB and won't
    /// re-emit on theme change. Such stylesheets need either
    /// per-node re-apply (set this backend's capability to false)
    /// or to be rewritten using `Tokenized<T>` references that
    /// emit as `var()` in the CSS output.
    fn token_updates_propagate_via_cascade(&self) -> bool {
        false
    }

    /// Pre-generate any backend-side state for a stylesheet against the
    /// current theme. Web backends typically use this to mint CSS
    /// classes for every variant + compound combination up front, so
    /// `apply_style` is a cache hit. Other backends can leave the
    /// default no-op implementation.
    ///
    /// Called by the framework:
    /// - The first time a stylesheet is `resolve`d.
    /// - After every `set_theme(...)`, for every still-live stylesheet,
    ///   so the backend's pre-generated state is refreshed.
    ///
    /// The framework passes pre-resolved `StyleRules` (one per relevant
    /// variant combination) so the backend doesn't have to think about
    /// theme tokens — it gets concrete property bags.
    #[allow(unused_variables)]
    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Release a previously-registered stylesheet's pre-generated state.
    /// Called when the stylesheet is no longer reachable (its last
    /// `Rc<StyleSheet>` has been dropped) and after every theme change
    /// (before re-registering, so old state is cleaned up).
    #[allow(unused_variables)]
    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        // default: no-op
    }

    /// Make a static asset available for use by the renderer.
    ///
    /// Called the first time an `AssetId` is observed for this backend
    /// (the framework dedupes by id). The backend decides what
    /// registration means:
    /// - **Web**: fonts inject a `@font-face` rule into the document
    ///   stylesheet; images stash the URL in a node↔URL map.
    /// - **iOS**: fonts call `CTFontManagerRegisterFontsForURL`;
    ///   images become a `UIImage(named:)` cache entry.
    /// - **Android**: fonts go through `Typeface.createFromAsset`;
    ///   images preload into a `Bitmap` cache.
    /// - **wgpu**: bytes are uploaded into the text engine / texture
    ///   atlas.
    ///
    /// `kind` exists so a single entry point can fan out without each
    /// backend writing a giant `match` on the source's extension. The
    /// type-safe [`Asset<K>`](crate::assets::Asset) handle on the
    /// author side already enforces this at compile time; `kind`
    /// repeats it for runtime dispatch.
    ///
    /// Default no-op so backends without a renderer-side asset
    /// concept (early stubs, or fully wire-driven backends that
    /// forward registration upstream) compile without scaffolding.
    #[allow(unused_variables)]
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        // default: no-op
    }

    /// Release a previously-registered asset. Currently called only
    /// from explicit unload paths (assets are otherwise `'static` and
    /// live for the duration of the program); backends with bounded
    /// caches override to evict, others can leave the default no-op.
    #[allow(unused_variables)]
    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        // default: no-op
    }

    /// Register a font family (a [`Typeface`](crate::assets::Typeface))
    /// so subsequent style applications can resolve its faces.
    ///
    /// The framework guarantees that every `face.asset` referenced
    /// here has already been registered via [`register_asset`] in the
    /// same render flush. Backends that key fonts by family name + a
    /// weight/style table (web's `font-family` / `font-weight` /
    /// `font-style`, iOS post-registration `UIFont(name:)`) use this
    /// call to record the mapping; backends that just take raw bytes
    /// per face (wgpu / cosmic-text) can leave the default no-op and
    /// drive registration entirely off `register_asset`.
    ///
    /// [`register_asset`]: Self::register_asset
    #[allow(unused_variables)]
    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        // default: no-op
    }

    /// Release a previously-registered typeface. Mirrors
    /// [`unregister_asset`](Self::unregister_asset).
    #[allow(unused_variables)]
    fn unregister_typeface(&mut self, id: TypefaceId) {
        // default: no-op
    }

    /// Install the initial token set as runtime variables. Called by
    /// the framework once at app boot, before any stylesheet is
    /// registered.
    ///
    /// Backends with a runtime variable layer (web's CSS custom
    /// properties) implement this to write `--{name}: {value}` on the
    /// document root. Backends without a variable system (iOS,
    /// Android) leave the default no-op; they read
    /// `Tokenized::value()` at apply time and behave as if the literal
    /// were set.
    #[allow(unused_variables)]
    fn install_tokens(&mut self, tokens: &[crate::TokenEntry]) {
        // default: no-op
    }

    /// Push updated token values. Called by the framework on every
    /// `update_tokens(...)`. Backends with a runtime variable layer
    /// update the existing declarations in place — one DOM op per
    /// changed token, no rule churn. Backends without a variable
    /// system leave the default no-op; the framework re-fires every
    /// styled effect via the tokens-version signal so the new
    /// fallback values flow through `apply_style`.
    #[allow(unused_variables)]
    fn update_tokens(&mut self, tokens: &[crate::TokenEntry]) {
        // default: no-op
    }

    /// Called when a styled node is being torn down (its surrounding
    /// `Effect` scope is dropping). Lets backends free per-node state —
    /// e.g. the web backend drops the node's dynamic CSS class slot
    /// and its node-id entry. Other backends typically don't need this.
    #[allow(unused_variables)]
    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // default: no-op
    }

    // -----------------------------------------------------------------
    // Accessibility surface — see `runtime_core::accessibility` and
    // `docs/accessibility-design.md` for the full design.
    //
    // Every `create_*` will grow an `a11y: &AccessibilityProps`
    // parameter in phase 5 of the a11y rollout (a separate commit that
    // touches every backend's signature at once). The three methods
    // here are the parts that DON'T live on `create_*`:
    //
    //   - `update_accessibility`: replace the prop bag on an existing
    //     node when reactive a11y state changes.
    //   - `announce_for_accessibility`: imperative live-region post
    //     ("Form submitted", "Loading complete") with no stable focus
    //     target.
    //   - `dump_accessibility_tree`: GPU/canvas backends produce a
    //     parallel semantics tree the host shell projects into the
    //     platform AX layer. Native widget backends return None
    //     (their AX data lives on the widget already).
    //
    // All three have no-op defaults so this trait change lands without
    // touching any existing backend impl.
    // -----------------------------------------------------------------

    /// Replace the accessibility prop bag attached to `node`. Called
    /// by the walker's reactive a11y Effect when any field of the
    /// node's [`AccessibilityProps`] changes.
    ///
    /// `inferred_role` is the primitive's default role (the same value
    /// the walker computed via [`accessibility::default_role`] at
    /// `create_*` time). Backends use it as the fallback when
    /// `a11y.role.is_none()`, so the update path produces the same
    /// resolved role as the create path.
    ///
    /// Backends translate to per-attribute setter calls
    /// (`accessibilityLabel = ...`, `setAttribute('aria-label', ...)`,
    /// `setContentDescription(...)`).
    ///
    /// [`AccessibilityProps`]: crate::accessibility::AccessibilityProps
    /// [`accessibility::default_role`]: crate::accessibility::default_role
    #[allow(unused_variables)]
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &crate::accessibility::AccessibilityProps,
        inferred_role: Option<crate::accessibility::Role>,
    ) {
        // default: no-op (backend doesn't implement a11y yet, the
        // node still renders; assistive technology sees an unlabelled
        // element until the backend is updated)
    }

    /// Post a one-shot live-region announcement to the platform's AX
    /// subsystem. Independent of any node — used for transient
    /// feedback that doesn't have a focus target ("Form submitted",
    /// "Loading complete").
    ///
    /// - iOS: `UIAccessibility.post(notification: .announcement, …)`.
    /// - Android: `View.announceForAccessibility(msg)` or hidden
    ///   live-region path for `Assertive`.
    /// - Web: write `msg` into a hidden `aria-live` region.
    /// - macOS: post `NSAccessibilityAnnouncementRequestedNotification`.
    /// - Roku / no-AX backends: no-op (log once at debug level).
    #[allow(unused_variables)]
    fn announce_for_accessibility(
        &mut self,
        msg: &str,
        priority: crate::accessibility::LiveRegionPriority,
    ) {
        // default: no-op
    }

    /// GPU-backend hook: return a snapshot of the parallel semantics
    /// tree. Native widget backends return `None` (their a11y data
    /// lives on the widget; the platform AX walker traverses the
    /// widget tree directly).
    ///
    /// The wgpu / future canvas backends override to return their
    /// internal [`AccessibilityTree`]. The host shell crate (winit
    /// app delegate, the iOS shell crate, AT-SPI bridge on Linux)
    /// reads the tree after every layout commit and projects it into
    /// the host platform's AX layer.
    ///
    /// [`AccessibilityTree`]: crate::accessibility::AccessibilityTree
    fn dump_accessibility_tree(&self) -> Option<crate::accessibility::AccessibilityTree> {
        None
    }

    /// Node's rect in its **parent's** coordinate system.
    /// Returns `None` if the node isn't mounted in a layout yet (e.g.
    /// queried before the first frame) or if the backend can't report
    /// positions. Default returns `None`.
    ///
    /// Use this for "where is X relative to its parent" — e.g. measuring
    /// a sidebar item's offset within its container. For viewport
    /// positions, use [`absolute_frame`](Backend::absolute_frame).
    #[allow(unused_variables)]
    fn frame(&self, node: &Self::Node) -> Option<primitives::portal::ViewportRect> {
        None
    }

    /// Node's rect in the **window/viewport's** coordinate system.
    /// Returns `None` if the node isn't mounted in a window yet.
    /// Default returns `None`.
    ///
    /// Backends that already implement `*Ops::rect` for overlay
    /// anchoring should forward to the same conversion path here
    /// (e.g. UIKit `convertRect:toView:window`, DOM
    /// `getBoundingClientRect()`).
    #[allow(unused_variables)]
    fn absolute_frame(&self, node: &Self::Node) -> Option<primitives::portal::ViewportRect> {
        None
    }

    /// Node's rect in **physical device-screen pixels** — origin at the
    /// top-left of the display (status bar included), units = real
    /// pixels, not logical/dp/pt. Returns `None` if the node isn't laid
    /// out yet or the backend has no screen-pixel mapping. Default
    /// `None`.
    ///
    /// This is distinct from [`absolute_frame`](Backend::absolute_frame),
    /// which reports *logical* pixels in *window* coordinates for layout
    /// /anchoring. `device_frame` exists for **OS-level input injection**:
    /// an external driver (Android `adb shell input tap`, iOS XCUITest/idb,
    /// macOS `CGEvent`) needs the real on-screen pixel a synthetic touch
    /// should land on. Doing the logical→physical conversion *here*, on
    /// the device where the true display density is known, avoids the host
    /// having to guess a scale factor — and avoids the window-vs-screen
    /// status-bar offset that `absolute_frame` would carry.
    ///
    /// Android forwards to `getLocationOnScreen` (already physical px);
    /// other backends can layer their own injector mapping as those paths
    /// come online.
    #[allow(unused_variables)]
    fn device_frame(&self, node: &Self::Node) -> Option<primitives::portal::ViewportRect> {
        None
    }

    /// Whether this backend can capture its rendered surface via
    /// [`capture_screenshot`](Backend::capture_screenshot). Default
    /// `false`. The Robot bridge only registers the live `"screenshot"`
    /// verb when this returns `true`, so a backend that can't snapshot
    /// natively (and a `MockBackend`) leaves the headless wgpu-replay
    /// `"screenshot"` verb in place instead of clobbering it.
    fn supports_screenshot(&self) -> bool {
        false
    }

    /// Capture the backend's **real rendered surface** as PNG bytes — a
    /// debug utility that snapshots what the device is actually drawing
    /// (native widgets, fonts, the live view hierarchy), distinct from
    /// the headless wgpu re-render of the scene model.
    ///
    /// The result is delivered through `done` rather than returned so
    /// that asynchronous backends (a future web/DOM rasterizer) fit the
    /// same signature; the native backends (AppKit / UIKit / Android)
    /// capture synchronously on the UI thread and invoke `done` inline
    /// before returning. Callers that need the value synchronously (the
    /// Robot bridge handler) stash it from the callback.
    ///
    /// Default: report unsupported. Override alongside
    /// [`supports_screenshot`](Backend::supports_screenshot).
    #[allow(unused_variables)]
    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        done(Err("screenshot capture is not supported on this backend".into()));
    }

    /// Wires the backend's native interaction events (hover, press,
    /// focus) to the framework's per-node state machinery. The
    /// framework allocates a `Signal<StateBits>` per styled node and
    /// passes a setter closure here; backends call the setter when
    /// the corresponding native event fires.
    ///
    /// The setter takes `(state, on)` where `state` is a
    /// `StateBits` flag (`StateBits::HOVERED`, etc.) and `on` is
    /// true for entering / false for leaving the state. The framework
    /// re-resolves and re-applies the node's style when state bits
    /// change — backends don't need to do any style work themselves.
    ///
    /// Default impl is a no-op for backends that don't yet support
    /// interaction states (states declared in the stylesheet simply
    /// never activate on those platforms — a documented no-op).
    #[allow(unused_variables)]
    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // default: no-op
    }

    /// Mark the native widget as disabled or enabled. Distinct from
    /// the `DISABLED` style-state bit (which controls overlay
    /// styling) — this one is about the widget being inert: web's
    /// `disabled` attribute, `setEnabled(false)` on native. Backends
    /// that don't distinguish leave the default no-op.
    #[allow(unused_variables)]
    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        // default: no-op
    }

    // ---- handle builders ------------------------------------------------
    //
    // Each one defaults to "no-op handle backed by `Rc::new(())`" —
    // backends that don't yet support `.bind()` refs for a given
    // primitive get something type-correct without having to think
    // about ops downcasting.

    #[allow(unused_variables)]
    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        ButtonHandle::new(Rc::new(()), &NoopButtonOps)
    }

    #[allow(unused_variables)]
    fn make_pressable_handle(&self, node: &Self::Node) -> PressableHandle {
        PressableHandle::new(Rc::new(()), &NoopPressableOps)
    }

    #[allow(unused_variables)]
    fn make_view_handle(&self, node: &Self::Node) -> ViewHandle {
        ViewHandle::new(Rc::new(()), &NoopViewOps)
    }

    #[allow(unused_variables)]
    fn make_text_handle(&self, node: &Self::Node) -> TextHandle {
        TextHandle::new(Rc::new(()), &NoopTextOps)
    }

    #[allow(unused_variables)]
    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        primitives::image::ImageHandle::new(Rc::new(()), &NoopImageOps)
    }

    #[allow(unused_variables)]
    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        primitives::icon::IconHandle::new(Rc::new(()), &NoopIconOps)
    }

    #[allow(unused_variables)]
    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::text_input::TextInputHandle {
        primitives::text_input::TextInputHandle::new(Rc::new(()), &NoopTextInputOps)
    }

    #[allow(unused_variables)]
    fn make_text_area_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::text_area::TextAreaHandle {
        primitives::text_area::TextAreaHandle::new(Rc::new(()), &NoopTextAreaOps)
    }

    #[allow(unused_variables)]
    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        primitives::toggle::ToggleHandle::new(Rc::new(()), &NoopToggleOps)
    }

    #[allow(unused_variables)]
    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::ScrollViewHandle::new(Rc::new(()), &NoopScrollViewOps)
    }

    #[allow(unused_variables)]
    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        primitives::slider::SliderHandle::new(Rc::new(()), &NoopSliderOps)
    }

    #[allow(unused_variables)]
    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        primitives::activity_indicator::ActivityIndicatorHandle::new(
            Rc::new(()),
            &NoopActivityIndicatorOps,
        )
    }

    #[allow(unused_variables)]
    fn make_virtualizer_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::virtualizer::VirtualizerHandle {
        primitives::virtualizer::VirtualizerHandle::new(
            Rc::new(()),
            &NoopVirtualizerOps,
        )
    }

    #[allow(unused_variables)]
    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::graphics::GraphicsHandle {
        primitives::graphics::GraphicsHandle::new(Rc::new(()), &NoopGraphicsOps)
    }


    /// Create a third-party `Element::External` node. Backends that
    /// expose an [`ExternalRegistry`](crate::external::ExternalRegistry)
    /// consult it for a registered handler; on miss they should fall
    /// through to a platform-native "not supported" placeholder.
    /// Backends with no external support leave the default panic.
    ///
    /// `type_id` drives dispatch; `type_name` is for debug/error
    /// messages only.
    #[allow(unused_variables)]
    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!(
            "create_external not implemented for this backend (external primitive: {})",
            type_name
        )
    }

    /// Tear down an external primitive's backend-side state. Default
    /// no-op; backends that hold per-node listeners / observers /
    /// closure handles override.
    #[allow(unused_variables)]
    fn release_external(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Create a navigator extension — the unified dispatch entry point
    /// for any registered navigator kind. Backends that hold a
    /// [`NavigatorRegistry`](primitives::navigator::NavigatorRegistry)
    /// consult it for a factory keyed by `type_id`; on a miss they
    /// should fall through to a "navigator not registered" placeholder
    /// node (so the build doesn't crash but the missing wiring is
    /// visible).
    ///
    /// The `host` carries every framework-owned affordance the
    /// handler needs (mount/release screens, match paths, nav-state
    /// signals, `NavigatorControl`). `type_id` drives registry
    /// dispatch; `type_name` is for debug/error messages.
    ///
    /// This single method is the unified replacement for the three
    /// per-kind `create_stack_navigator` / `create_tab_navigator` /
    /// `create_drawer_navigator` methods. The per-kind methods exist
    /// in parallel during the migration; new code should land here.
    #[allow(unused_variables)]
    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!(
            "create_navigator not implemented for this backend \
             (navigator kind: {})",
            type_name
        )
    }

    /// Tear down a navigator extension. Default no-op; backends that
    /// hold per-node handler state override and drop their handler
    /// entry keyed by `node`.
    #[allow(unused_variables)]
    fn release_navigator(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Apply a slot-style update to a navigator extension. The walker
    /// calls this when the navigator's `.with_style(...)` chain
    /// resolves a style for an SDK-defined slot (e.g. `"header"`,
    /// `"tab_bar"`, `"drawer_scrim"`). Backends look up the handler
    /// associated with `node` and delegate to its
    /// [`NavigatorHandler::apply_slot_style`].
    ///
    /// Default no-op; SDK-specific slot styling without a registered
    /// handler is silently ignored.
    #[allow(unused_variables)]
    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        // default no-op
    }

    /// Make a `NavigatorHandle` for a navigator extension. Returned
    /// from inside the backend's `create_navigator` impl
    /// when the SDK's `bind(...)` fires a `RefFill::Navigator`.
    /// Default returns a no-op handle (matches the per-kind
    /// `make_stack_navigator_handle` posture).
    #[allow(unused_variables)]
    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::navigator::NavigatorHandle {
        primitives::navigator::NavigatorHandle::new(Rc::new(()), &NoopNavigatorOps)
    }

    /// Attach the framework-realized initial screen to a navigator
    /// extension. The backend's handler is responsible for inserting
    /// `screen` into its native container; this trait method exists so
    /// the walker can hand the result of its initial `mount_screen`
    /// call to the registered handler outside the
    /// `create_navigator` borrow window.
    ///
    /// Default panics — backends that implement `create_navigator`
    /// must also implement this, typically by looking up the handler
    /// keyed by `node` and delegating to
    /// [`NavigatorHandler::attach_initial`](primitives::navigator::NavigatorHandler).
    #[allow(unused_variables)]
    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        unimplemented!(
            "navigator_attach_initial not implemented for this backend"
        )
    }

    /// Create a portal — render `children` (mounted via subsequent
    /// `insert(node, child)` calls on the returned node) at `target`,
    /// escaping the parent's layout and clipping context.
    ///
    /// Backends stand up their platform-native render-elsewhere
    /// mechanism:
    /// - **Web**: a `<div>` appended to `document.body` (escapes
    ///   `overflow:hidden` and stacking contexts).
    /// - **iOS**: a `UIView` added to the key window
    ///   (`UIWindow.addSubview:`).
    /// - **Android**: a `WindowManager.addView` window-level view, or
    ///   a `Dialog`-hosted container.
    /// - **Roku**: a `Group` parented to the root scene.
    ///
    /// For [`PortalTarget::Anchor`], backends should subscribe to
    /// scroll / layout / orientation events from the anchor's host
    /// hierarchy and re-query `target.rect()` to reposition the
    /// portal as the anchor moves.
    ///
    /// `on_dismiss` fires when the platform requests dismissal
    /// (Escape on web, back gesture on Android, swipe-down on iOS).
    /// The framework doesn't auto-tear-down — the host's open-state
    /// signal is the source of truth; flipping it drops the
    /// surrounding scope and triggers [`Backend::release_portal`].
    ///
    /// Default: panic. Backends that don't yet implement portals
    /// shouldn't have authors mounting them.
    #[allow(unused_variables)]
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        unimplemented!("create_portal not implemented for this backend")
    }

    /// Tear down a portal's backend-side state. Same contract as
    /// [`Backend::release_overlay`] — detach the platform mount,
    /// drop event-listener handles, free observer subscriptions.
    #[allow(unused_variables)]
    fn release_portal(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Default no-op handle for portals. Backends with imperative
    /// portal APIs (future: reposition, update target, …) override.
    #[allow(unused_variables)]
    fn make_portal_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::portal::PortalHandle {
        primitives::portal::PortalHandle::new(Rc::new(()), &NoopPortalOps)
    }

    /// Apply a presence-style transform (opacity + 2D translate +
    /// uniform scale) to a node. Called by the walker's presence
    /// arm at three points:
    ///
    /// - **Pre-mount enter** — `state = enter.from`, `transition =
    ///   None`. The node is snapped to the entering state before
    ///   its first paint.
    /// - **Animate to resting** — `state = PresenceState::rest()`,
    ///   `transition = Some((duration, easing))`. The next animation
    ///   frame after mount; the backend interpolates from the
    ///   pre-mount state to identity.
    /// - **Exit** — `state = exit.to`, `transition = Some((duration,
    ///   easing))`. The walker schedules a scope-drop after the
    ///   transition completes.
    /// - **Reversal** — same as "animate to resting" when an exit
    ///   is interrupted by `present()` flipping back true.
    ///
    /// `PresenceState::rest()` means "no presence override is
    /// active." Backends that don't implement presence leave the
    /// default no-op; presence-controlled subtrees still mount and
    /// unmount, just without animation.
    #[allow(unused_variables)]
    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, crate::style::Easing)>,
    ) {
        // default: no-op
    }

    /// Default no-op handle for presence. Backends with an imperative
    /// presence API can override.
    #[allow(unused_variables)]
    fn make_presence_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::presence::PresenceHandle {
        primitives::presence::PresenceHandle::new(Rc::new(()), &NoopPresenceOps)
    }

    /// Create a navigable container — the `Link` primitive.
    ///
    /// Backends are responsible for:
    /// - Producing the platform-native interactive widget that
    ///   wraps the eventual children. On web this should be a
    ///   real `<a href=config.url>` so the browser's native link
    ///   contract works (right-click "copy link," middle-click
    ///   "open in new tab," screen-reader "link" role, etc.).
    ///   On native platforms, an accessibility-Link-roled tappable
    ///   container is the right shape.
    /// - Wiring activation: when the user taps / clicks / activates
    ///   the widget, call `config.on_activate()`. The framework
    ///   has already baked the push/replace/reset dispatch into
    ///   that closure — the backend just fires it.
    /// - For web specifically: intercept the click and
    ///   `preventDefault` to keep the SPA single-page, but only
    ///   for plain clicks. Modified clicks (cmd/ctrl/middle,
    ///   shift) should fall through to the browser's default
    ///   handler so "open in new tab/window" still works.
    ///
    /// Default falls through to `create_view`, dropping
    /// `on_activate`. Backends that don't implement Link still mount
    /// the children correctly — the link just isn't tappable. This
    /// keeps a Link in a primitive tree from panicking the screen
    /// build on an unimplemented backend, which matches the posture
    /// of every other optional handle method (return a no-op rather
    /// than refuse). Backends that want real activation override.
    #[allow(unused_variables)]
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &crate::accessibility::AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }

    /// Apply safe-area-aware padding to `node`. Called by the walker
    /// for every container that opted in via `.safe_area(...)`, and
    /// again reactively whenever
    /// [`crate::safe_area_insets()`] fires (orientation flip,
    /// dynamic-island change, sheet adaptation).
    ///
    /// Backends should:
    /// 1. Read the platform's current safe-area insets (from
    ///    `UIView.safeAreaInsets`, `WindowInsets.systemBars()`,
    ///    `env(safe-area-inset-*)`, etc.).
    /// 2. For each side flag in `sides`, add the corresponding inset
    ///    to that side's *padding* on the node — combining with any
    ///    author-set padding (don't clobber it).
    /// 3. Schedule a layout pass if the padding changed.
    ///
    /// The default impl is a no-op so backends without safe-area
    /// awareness (or that don't yet implement it) silently ignore
    /// the opt-in instead of panicking.
    #[allow(unused_variables)]
    fn apply_safe_area_padding(
        &mut self,
        node: &Self::Node,
        sides: crate::SafeAreaSides,
    ) {
        // default: no-op
    }

    /// Apply safe-area treatment to a *ScrollView*. Same shape as
    /// `apply_safe_area_padding` but with native-correct semantics
    /// for a scroll container: the scroll surface bleeds edge-to-edge
    /// while the *content origin* is inset by the safe-area amount.
    /// On iOS this is `UIScrollView.contentInset` (with
    /// `contentInsetAdjustmentBehavior = .never`). On Android,
    /// `setPadding(...)` + `setClipToPadding(false)`. On web, padding
    /// on the inner content wrapper while the scroll view itself
    /// keeps `padding: 0`.
    ///
    /// Distinguished from `apply_safe_area_padding` by the dispatch
    /// site in the walker: `Element::View` with `.safe_area(...)`
    /// uses padding; `Element::ScrollView` with `.safe_area(...)`
    /// uses content insets. The user-facing builder is the same
    /// (`.safe_area(...)`); the framework picks the right path
    /// based on which primitive it's on.
    ///
    /// Default impl falls back to `apply_safe_area_padding` so
    /// backends without a separate inset path keep working (just
    /// with the old "padding on the scroll view itself" visual).
    #[allow(unused_variables)]
    fn apply_scroll_view_safe_area_inset(
        &mut self,
        node: &Self::Node,
        sides: crate::SafeAreaSides,
    ) {
        self.apply_safe_area_padding(node, sides);
    }

    /// Default no-op handle for `Ref<LinkHandle>`. Backends that
    /// can synthesize activation events override this.
    #[allow(unused_variables)]
    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        primitives::link::LinkHandle::new(Rc::new(()), &NoopLinkOps)
    }

    /// Declare page-level metadata (document title, description, Open
    /// Graph) for the screen just built. Called by [`mount`](crate::mount)
    /// after the build, draining what author code set via
    /// [`set_page_metadata`](crate::set_page_metadata).
    ///
    /// SSR emits `<head>` tags; the web backend sets `document.title` +
    /// meta on the client; platforms with no document concept no-op
    /// (default). `title` maps to the nav-bar title and the SEO fields to
    /// `NSUserActivity` / App Indexing where that representation is added.
    #[allow(unused_variables)]
    fn set_page_metadata(&mut self, meta: &crate::PageMetadata) {
        // default: no-op
    }

    /// Register a raw CSS stylesheet to ship once, paired with
    /// [`attach_html_class`](Backend::attach_html_class). The SSR backend
    /// accumulates these and emits them in the document `<head>`; the web
    /// backend injects a `<style>` element. Backends are free to dedupe
    /// identical sheets (navigator chrome registers the same layout sheet
    /// on every navigator).
    ///
    /// Default no-op — native backends don't have a stylesheet concept.
    #[allow(unused_variables)]
    fn register_raw_css(&mut self, css: &str) {
        // default: no-op
    }

    /// Theme the **host surface** behind the framework's rendered tree —
    /// `<html>`/`<body>` on web, `UIWindow` on iOS, the Activity's decor
    /// view on Android, the wgpu clear color on wgpu/macOS, the terminal
    /// background where settable. The argument is a [`Tokenized<Color>`]
    /// so backends that have a CSS-variable surface (web/SSR) can wire
    /// the host surface to `var(--<name>)` — automatically reactive on
    /// theme swap via the `:root` setProperty path, no second call
    /// needed. Native backends apply `color.value()` directly and the
    /// theme SDK calls this again on swap so they re-resolve.
    ///
    /// Default no-op for backends with no controllable host surface or
    /// that haven't wired this up yet — the theme SDK degrades silently.
    #[allow(unused_variables)]
    fn set_app_background(&mut self, color: &crate::Tokenized<crate::Color>) {
        // default: no-op
    }

    /// Theme the platform scrollbar where the backend can. Token-based
    /// for the same reason as [`set_app_background`](Backend::set_app_background)
    /// — web stays reactive without re-call. Default no-op for the many
    /// backends with no programmable scrollbar chrome (iOS exposes only
    /// `UIScrollViewIndicatorStyle::{white, black, default}`, terminal
    /// has none, wgpu paints its own).
    #[allow(unused_variables)]
    fn set_scrollbar_theme(
        &mut self,
        thumb: &crate::Tokenized<crate::Color>,
        track: &crate::Tokenized<crate::Color>,
    ) {
        // default: no-op
    }

    /// Install (or, with `None`, remove) an APP-LEVEL keyboard handler that fires
    /// for key presses regardless of focus — unlike the per-input `on_key_down`
    /// on `create_text_input`/`create_text_area`, which only fires while that
    /// input is focused. Backends that can observe key events at the window /
    /// activity / event-loop level (web `document`, AppKit `NSEvent` monitor,
    /// UIKit `pressesBegan:`, Android root-view `OnKeyListener`, winit's event
    /// loop, the terminal input loop) install a single native source here and
    /// route each press through `handler`, returning [`KeyOutcome::PreventDefault`]
    /// to swallow the platform default. The handler should be conservative — act
    /// only on the keys it cares about and return [`KeyOutcome::Default`]
    /// otherwise — because it sees EVERY key, including typing into a focused
    /// input.
    ///
    /// Routed from [`crate::set_app_key_handler`] (single-slot, drained on the
    /// next walker flush), mirroring [`set_app_background`](Backend::set_app_background).
    /// Default no-op for backends with no app-level key source (and the many that
    /// haven't wired it up).
    #[allow(unused_variables)]
    fn set_app_key_handler(&mut self, handler: Option<crate::primitives::key::KeyDownHandler>) {
        // default: no-op
    }

    fn finish(&mut self, root: Self::Node);

    /// Drive a fresh layout pass over the backend's registered view
    /// tree. Default no-op for backends whose `finish()` already
    /// applies frames synchronously (macOS, web).
    ///
    /// Override in backends that defer layout via a platform-side
    /// scheduler that relies on a globally-installed self-reference
    /// (iOS's `IOS_BACKEND_SELF`, Android's `ANDROID_BACKEND_SELF`).
    /// In runtime-server mode the backend lives by-value inside `RuntimeServerClient`,
    /// so no `Rc<RefCell<Backend>>` exists for the global to point
    /// at — the deferred path bails out, leaving frames computed
    /// by Taffy uncommitted to the native view hierarchy. The runtime-server
    /// shell calls this after each `apply_batch` that carried
    /// inbound commands so the synchronous path takes over for
    /// the missing global.
    ///
    /// In local-mount mode the global IS installed; `finish()` and
    /// the deferred path together produce the correct result and
    /// this method is never called.
    fn run_layout(&mut self) {}

    /// Schedule a layout pass for the next main-loop turn. **The navigator
    /// abstraction calls this automatically after EVERY navigation command**
    /// (via `NavigatorControl`'s request-layout hook, registered by the
    /// navigator walker) — so a freshly-pushed/selected/swapped screen always
    /// gets its Taffy/UIKit/AppKit layout recomputed, on every backend, without
    /// each navigator×backend handler having to remember to call it. That
    /// per-handler duplication was the root of the recurring "navigated, but the
    /// new screen renders at 0×0" class of bug.
    ///
    /// No `self` (the schedulers are thread-local/global), so the generic walker
    /// can register `|| B::schedule_layout_pass()` without holding the backend.
    /// Default no-op: backends that re-layout automatically (web/CSS reflow,
    /// terminal full re-render) need nothing. Native backends override it to
    /// call their coalescing scheduler (the Taffy compute pass, UIKit
    /// `setNeedsLayout`, etc.).
    fn schedule_layout_pass() {}
}

// ---------------------------------------------------------------------------
// Noop ops — default ZST impls used by the trait's `make_*_handle`
// defaults. Backends that don't support a particular primitive's refs
// can leave the defaults in place and authors get a type-correct
// no-op handle.
// ---------------------------------------------------------------------------

struct NoopIconOps;
impl primitives::icon::IconOps for NoopIconOps {
    // Default impls in the trait handle no-op behavior.
}

struct NoopImageOps;
impl primitives::image::ImageOps for NoopImageOps {}

struct NoopTextInputOps;
impl primitives::text_input::TextInputOps for NoopTextInputOps {
    fn focus(&self, _: &dyn Any) {}
    fn blur(&self, _: &dyn Any) {}
    fn select_all(&self, _: &dyn Any) {}
    fn insert_text(&self, _: &dyn Any, _: &str) {}
}

struct NoopTextAreaOps;
impl primitives::text_area::TextAreaOps for NoopTextAreaOps {
    fn focus(&self, _: &dyn Any) {}
    fn blur(&self, _: &dyn Any) {}
    fn select_all(&self, _: &dyn Any) {}
    fn insert_text(&self, _: &dyn Any, _: &str) {}
}

struct NoopToggleOps;
impl primitives::toggle::ToggleOps for NoopToggleOps {}

struct NoopScrollViewOps;
impl primitives::scroll_view::ScrollViewOps for NoopScrollViewOps {
    fn scroll_to(&self, _: &dyn Any, _: f32, _: f32) {}
}

struct NoopSliderOps;
impl primitives::slider::SliderOps for NoopSliderOps {}

struct NoopActivityIndicatorOps;
impl primitives::activity_indicator::ActivityIndicatorOps for NoopActivityIndicatorOps {}

struct NoopVirtualizerOps;
impl primitives::virtualizer::VirtualizerOps for NoopVirtualizerOps {
    fn scroll_to_index(&self, _: &dyn Any, _: usize) {}
}

struct NoopGraphicsOps;
impl primitives::graphics::GraphicsOps for NoopGraphicsOps {}

struct NoopNavigatorOps;
impl primitives::navigator::NavigatorOps for NoopNavigatorOps {}

struct NoopLinkOps;
impl primitives::link::LinkOps for NoopLinkOps {
    fn activate(&self, _node: &dyn Any) {}
}

struct NoopPresenceOps;
impl primitives::presence::PresenceOps for NoopPresenceOps {}

struct NoopPortalOps;
impl primitives::portal::PortalOps for NoopPortalOps {}

struct NoopButtonOps;
impl ButtonOps for NoopButtonOps {
    fn click(&self, _node: &dyn Any) {}
}

struct NoopPressableOps;
impl PressableOps for NoopPressableOps {
    fn click(&self, _node: &dyn Any) {}
}

struct NoopViewOps;
impl ViewOps for NoopViewOps {}

struct NoopTextOps;
impl TextOps for NoopTextOps {}

// =============================================================================
// Trait default-impl tests
// =============================================================================

#[cfg(test)]
mod tests {
    //! Unit tests for the `Backend` trait's default method
    //! implementations. The framework relies on these defaults
    //! behaving correctly for every backend that doesn't override
    //! them — silent drift here would surface as bugs in mobile
    //! backends (which inherit most defaults).

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::batch::{BackendBatch, BatchOp};

    /// Records every Backend method we care about so default-impl
    /// tests can observe the call sequence and arguments.
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        ExecuteBatch { node_count: u32, ops: usize },
        InsertMany { parent: u32, children: Vec<u32> },
        Insert { parent: u32, child: u32 },
    }

    /// Minimal `Backend` impl for default-impl coverage. Only the
    /// methods needed to exercise `execute_batch_with_attach`'s
    /// default path are implemented; everything else falls through
    /// to the trait's own defaults (`unimplemented!()` for required
    /// methods we don't call, no-op for the optional ones).
    ///
    /// Deliberately does NOT override `execute_batch_with_attach`:
    /// that's the whole point — we want to assert that the trait
    /// default fires `execute_batch` + `insert_many` in the right
    /// order with the right args.
    struct StubBackend {
        next_id: RefCell<u32>,
        calls: Rc<RefCell<Vec<Call>>>,
    }

    impl StubBackend {
        fn new() -> Self {
            Self {
                next_id: RefCell::new(0),
                calls: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        fn mint(&self) -> u32 {
            let id = *self.next_id.borrow();
            *self.next_id.borrow_mut() = id + 1;
            id
        }
    }

    impl Backend for StubBackend {
        type Node = u32;

        fn create_view(&mut self, _a11y: &crate::accessibility::AccessibilityProps) -> Self::Node {
            self.mint()
        }
        fn create_text(
            &mut self,
            _content: &str,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> Self::Node {
            self.mint()
        }
        fn create_button(
            &mut self,
            _label: &str,
            _on_click: &crate::Action,
            _leading: Option<&primitives::icon::IconData>,
            _trailing: Option<&primitives::icon::IconData>,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> Self::Node {
            self.mint()
        }
        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            self.calls.borrow_mut().push(Call::Insert {
                parent: *parent,
                child,
            });
        }
        fn update_text(&mut self, _node: &Self::Node, _content: &str) {}
        fn clear_children(&mut self, _node: &Self::Node) {}
        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {}

        // Opt into the batched-Repeat path *only* for `execute_batch`
        // — leave `execute_batch_with_attach` to the trait default.
        fn supports_batched_repeat(&self) -> bool {
            true
        }

        fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
            let count = batch.node_count;
            self.calls.borrow_mut().push(Call::ExecuteBatch {
                node_count: count,
                ops: batch.ops.len(),
            });
            (0..count).map(|_| self.mint()).collect()
        }

        fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
            self.calls.borrow_mut().push(Call::InsertMany {
                parent: *parent,
                children,
            });
        }

        fn finish(&mut self, _root: Self::Node) {}
    }

    /// `execute_batch_with_attach`'s default impl must call
    /// `execute_batch` then `insert_many` — in that order. The
    /// `insert_many` children must be the `attach_locals` resolved
    /// through the batch's return Vec.
    #[test]
    fn default_impl_calls_execute_batch_then_insert_many() {
        let mut backend = StubBackend::new();
        let mut parent: u32 = 999; // any pre-existing id
        let mut batch = BackendBatch::with_capacity(3, 0);
        // Three nodes, two of them flagged for attach.
        let id_a = batch.next_id();
        let id_b = batch.next_id();
        let id_c = batch.next_id();
        batch.ops.push(BatchOp::CreateView { local_id: id_a });
        batch.ops.push(BatchOp::CreateView { local_id: id_b });
        batch.ops.push(BatchOp::CreateView { local_id: id_c });
        let attach_locals = [id_a, id_c];

        let returned = backend.execute_batch_with_attach(batch, &mut parent, &attach_locals);

        // Default impl returns the full Vec from execute_batch
        // — sized to node_count, ordered by local_id.
        assert_eq!(
            returned.len(),
            3,
            "default impl must return execute_batch's Vec unchanged",
        );

        let calls = backend.calls();
        // First: execute_batch with 3 node_count, 3 ops.
        // Second: insert_many with the 2 attach-flagged children.
        assert_eq!(calls.len(), 2, "exactly two recorded calls; got {:?}", calls);
        match &calls[0] {
            Call::ExecuteBatch { node_count, ops } => {
                assert_eq!(*node_count, 3);
                assert_eq!(*ops, 3);
            }
            other => panic!("expected ExecuteBatch first, got {:?}", other),
        }
        match &calls[1] {
            Call::InsertMany { parent: p, children } => {
                assert_eq!(*p, 999, "parent should be the caller's parent id");
                // children = [returned[id_a], returned[id_c]].
                // returned was minted by `execute_batch` AFTER parent
                // was minted-by-test, so children should be the
                // freshly-minted ids the stub allocated inside
                // execute_batch.
                assert_eq!(children.len(), 2);
                assert_eq!(children[0], returned[id_a as usize]);
                assert_eq!(children[1], returned[id_c as usize]);
            }
            other => panic!("expected InsertMany second, got {:?}", other),
        }
    }

    /// Empty `attach_locals` short-circuits past the `insert_many`
    /// call — important so backends that override `execute_batch`
    /// but never want auto-attach (no-op repeats, or callers using
    /// the bare `execute_batch` path) don't pay an extra `insert_many`
    /// hop.
    #[test]
    fn default_impl_skips_insert_many_when_attach_locals_is_empty() {
        let mut backend = StubBackend::new();
        let mut parent: u32 = 7;
        let mut batch = BackendBatch::with_capacity(1, 0);
        let id = batch.next_id();
        batch.ops.push(BatchOp::CreateView { local_id: id });

        let _ = backend.execute_batch_with_attach(batch, &mut parent, &[]);

        let calls = backend.calls();
        assert_eq!(
            calls.len(),
            1,
            "only execute_batch should fire when attach_locals is empty; got {:?}",
            calls,
        );
        assert!(matches!(calls[0], Call::ExecuteBatch { .. }));
    }

    /// `Platform` helper predicates must agree on every variant —
    /// they're the public API author code branches on, and a missed
    /// arm would silently put a platform in the wrong category.
    #[test]
    fn platform_helpers_classify_every_variant() {
        use Platform::*;
        let all = [Web, Ios, Android, MacOs, TvOs, AndroidTv, Roku, Custom("")];

        // is_apple: Ios, MacOs, TvOs
        for p in all {
            assert_eq!(p.is_apple(), matches!(p, Ios | MacOs | TvOs), "is_apple({:?})", p);
        }
        // is_mobile: Ios, Android
        for p in all {
            assert_eq!(p.is_mobile(), matches!(p, Ios | Android), "is_mobile({:?})", p);
        }
        // is_tv: TvOs, AndroidTv, Roku
        for p in all {
            assert_eq!(p.is_tv(), matches!(p, TvOs | AndroidTv | Roku), "is_tv({:?})", p);
        }
        // is_web: only Web
        for p in all {
            assert_eq!(p.is_web(), matches!(p, Web), "is_web({:?})", p);
        }
        // is_desktop: only MacOs (for now)
        for p in all {
            assert_eq!(p.is_desktop(), matches!(p, MacOs), "is_desktop({:?})", p);
        }
        // `Custom(_)` is in no category regardless of inner string —
        // the string is opaque metadata, not a UI-shape signal.
        for sample in [Custom(""), Custom("runtime-server"), Custom("linux-desktop")] {
            assert!(!sample.is_apple(), "is_apple({:?})", sample);
            assert!(!sample.is_mobile(), "is_mobile({:?})", sample);
            assert!(!sample.is_tv(), "is_tv({:?})", sample);
            assert!(!sample.is_web(), "is_web({:?})", sample);
            assert!(!sample.is_desktop(), "is_desktop({:?})", sample);
        }
    }

    /// Default `Backend::platform()` must match the documented
    /// default — overrides land in each backend crate, but a backend
    /// that forgets to override should get a safe "no assumptions"
    /// identity, not a wrong one.
    #[test]
    fn default_platform() {
        let backend = StubBackend::new();
        assert_eq!(backend.platform(), Platform::Custom(""));
    }

    /// `Platform::canonical` is the documented display form authors
    /// slot into prose. Downstream code (UI strings, log lines, this
    /// repo's own `welcome` example) depends on the exact spelling.
    /// Lock it in, including the `Custom`-variant passthrough.
    #[test]
    fn platform_canonical_returns_display_form() {
        assert_eq!(Platform::Web.canonical(), "web");
        assert_eq!(Platform::Ios.canonical(), "iOS");
        assert_eq!(Platform::Android.canonical(), "Android");
        assert_eq!(Platform::MacOs.canonical(), "macOS");
        assert_eq!(Platform::TvOs.canonical(), "tvOS");
        assert_eq!(Platform::AndroidTv.canonical(), "Android TV");
        assert_eq!(Platform::Roku.canonical(), "Roku");
        // `Custom` passes through verbatim — empty stays empty, so
        // an undeclared backend doesn't leak a placeholder.
        assert_eq!(Platform::Custom("").canonical(), "");
        assert_eq!(Platform::Custom("Sim").canonical(), "Sim");
        assert_eq!(Platform::Custom("runtime-server").canonical(), "runtime-server");
    }

    /// `install_current_platform` must round-trip through the global
    /// accessor. Verified inside the test (not from `mount`) so the
    /// storage layer is exercised even on host targets that don't
    /// have a real Backend.
    #[test]
    fn install_current_platform_round_trips() {
        // Reset to known state in case a prior test in this thread
        // installed a value.
        install_current_platform(Platform::Custom(""));
        assert_eq!(platform(), Platform::Custom(""));

        install_current_platform(Platform::Ios);
        assert_eq!(platform(), Platform::Ios);

        install_current_platform(Platform::Custom("Sim"));
        assert_eq!(platform(), Platform::Custom("Sim"));

        install_current_platform(Platform::Web);
        assert_eq!(platform(), Platform::Web);

        // Restore default so other tests on the same thread aren't
        // surprised by leftover state.
        install_current_platform(Platform::Custom(""));
    }

    /// `install_current_color_scheme` must round-trip through the global
    /// `color_scheme()` accessor — the storage layer author code relies
    /// on to pick a platform-default theme at startup.
    #[test]
    fn install_current_color_scheme_round_trips() {
        // Default before any install on this thread is `Auto`.
        install_current_color_scheme(ColorScheme::Auto);
        assert_eq!(color_scheme(), ColorScheme::Auto);

        install_current_color_scheme(ColorScheme::Dark);
        assert_eq!(color_scheme(), ColorScheme::Dark);

        install_current_color_scheme(ColorScheme::Light);
        assert_eq!(color_scheme(), ColorScheme::Light);

        // Restore default so other tests on the same thread aren't
        // surprised by leftover state.
        install_current_color_scheme(ColorScheme::Auto);
    }

    /// `Backend::url_opener` defaults to `None` so backends without
    /// an external-open capability (terminal, CPU, runtime-server)
    /// compile without scaffolding and `open_url` no-ops on them.
    #[test]
    fn default_url_opener_is_none() {
        let backend = StubBackend::new();
        assert!(
            backend.url_opener().is_none(),
            "default url_opener must be None so non-browser backends opt out",
        );
    }

    /// `open_url` must route through whatever opener was installed for
    /// the active backend, passing the URL through verbatim.
    #[test]
    fn open_url_dispatches_to_installed_opener() {
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let seen_for_opener = seen.clone();
        install_url_opener(Some(Rc::new(move |url: &str| {
            seen_for_opener.borrow_mut().push(url.to_string());
        })));

        open_url("https://example.com/docs");
        open_url("mailto:hi@example.com");

        assert_eq!(
            *seen.borrow(),
            vec![
                "https://example.com/docs".to_string(),
                "mailto:hi@example.com".to_string(),
            ],
            "open_url must forward each URL to the installed opener in order",
        );

        // Restore default so other tests on this thread aren't
        // surprised by a leftover opener.
        install_url_opener(None);
    }

    /// With no opener installed, `open_url` must be a silent no-op
    /// (logged at debug) rather than panicking — author code can call
    /// it before mount or on a backend that doesn't support it.
    #[test]
    fn open_url_without_opener_is_noop() {
        // Ensure a clean slate on this thread.
        install_url_opener(None);
        // Must not panic.
        open_url("https://example.com");
    }

    /// `set_fullscreen` must forward each call to the installed setter,
    /// preserving the boolean — the spine the per-backend
    /// `fullscreen_setter` rides on (Android immersive, macOS
    /// toggleFullScreen, web Fullscreen API).
    #[test]
    fn set_fullscreen_dispatches_to_installed_setter() {
        let seen = Rc::new(RefCell::new(Vec::<bool>::new()));
        let seen_for_setter = seen.clone();
        install_fullscreen_setter(Some(Rc::new(move |enabled: bool| {
            seen_for_setter.borrow_mut().push(enabled);
        })));

        set_fullscreen(true);
        set_fullscreen(false);

        assert_eq!(
            *seen.borrow(),
            vec![true, false],
            "set_fullscreen must forward each value to the installed setter in order",
        );

        // Restore default so sibling tests on this thread aren't
        // surprised by a leftover setter.
        install_fullscreen_setter(None);
    }

    /// With no setter installed (default backend, pre-mount, terminal),
    /// `set_fullscreen` must be a silent no-op, not a panic.
    #[test]
    fn set_fullscreen_without_setter_is_noop() {
        install_fullscreen_setter(None);
        set_fullscreen(true);
        set_fullscreen(false);
    }

    /// The default `Backend::fullscreen_setter` reports no capability,
    /// mirroring `url_opener` — backends opt in by overriding it.
    #[test]
    fn default_fullscreen_setter_is_none() {
        let backend = StubBackend::new();
        assert!(backend.fullscreen_setter().is_none());
    }

    /// `attach_locals` ordering must be preserved into `insert_many`
    /// — the framework's contract is that the row tops appear in the
    /// surrounding View in iteration order.
    #[test]
    fn default_impl_preserves_attach_locals_order() {
        let mut backend = StubBackend::new();
        let mut parent: u32 = 100;
        let mut batch = BackendBatch::with_capacity(4, 0);
        let id_a = batch.next_id();
        let id_b = batch.next_id();
        let id_c = batch.next_id();
        let id_d = batch.next_id();
        batch.ops.push(BatchOp::CreateView { local_id: id_a });
        batch.ops.push(BatchOp::CreateView { local_id: id_b });
        batch.ops.push(BatchOp::CreateView { local_id: id_c });
        batch.ops.push(BatchOp::CreateView { local_id: id_d });
        // Out of natural order — c then a then d, skipping b.
        let attach_locals = [id_c, id_a, id_d];

        let returned = backend.execute_batch_with_attach(batch, &mut parent, &attach_locals);
        let calls = backend.calls();
        let children = match &calls[1] {
            Call::InsertMany { children, .. } => children.clone(),
            other => panic!("expected InsertMany, got {:?}", other),
        };
        assert_eq!(
            children,
            vec![
                returned[id_c as usize],
                returned[id_a as usize],
                returned[id_d as usize],
            ],
            "attach_locals order must round-trip into insert_many",
        );
    }
}
