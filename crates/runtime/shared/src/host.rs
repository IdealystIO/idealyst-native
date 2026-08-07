//! Host-registry surface — the per-thread slots the active backend
//! installs at mount (platform identity, color scheme, URL opener,
//! full-screen setter, AX announcer) plus the shared host data types
//! (`Screenshot`, `VirtualizerCallbacks`). Moved verbatim from
//! runtime-core's `backend.rs`; the `Backend` trait itself stays in
//! runtime-core. Each thread-local here is THE single authority — the
//! old core re-exports, never duplicates.

use std::rc::Rc;

use crate::primitives;

// ---------------------------------------------------------------------------
// Screenshot
// ---------------------------------------------------------------------------

/// A captured frame of the backend's real rendered surface, returned by
/// `IntrospectionOps::capture_screenshot`. `png` is a complete PNG file; the
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

/// Callbacks handed to `VirtualizerOps::create_virtualizer`. All Rc'd so
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
    /// Author's scroll observer, if any (`.on_scroll(..)` on the
    /// builder). The backend calls it per scroll event with the
    /// scroller's `(x, y)` offset in CSS px / native points — the same
    /// contract and coordinate space as
    /// `ScrollOps::create_scroll_view`'s `on_scroll`, so the two
    /// primitives report offsets identically.
    ///
    /// `None` when the author didn't ask for one; backends should skip
    /// installing any observation machinery in that case rather than
    /// calling a no-op closure per frame.
    ///
    /// Unlike `mount_item` / `release_item`, this does NOT need to run
    /// inside `World::enter` — it's an ordinary author callback that
    /// only stages writes through captured handles, exactly like
    /// `create_scroll_view`'s. Backends that route it through their
    /// dispatch-site glue get the flush for free.
    pub on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
}

// ---------------------------------------------------------------------------
// ColorScheme
// ---------------------------------------------------------------------------

/// The platform's current appearance mode. Backends return this from
/// `AppEnvOps::color_scheme` so the app can pick an appropriate
/// default theme before the first render; the boot seam forwards it to
/// [`color_scheme`] via [`install_current_color_scheme`].
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

/// Identifies the host platform a backend is rendering to. Backends
/// return it from `AppEnvOps::platform`; author code reads it via the
/// free [`platform`] function to branch on host —
/// e.g. show iOS-style chrome only on `Ios`, rely on
/// `position: fixed` only on `Web`, swap keyboard shortcuts for
/// menu-bar items on `MacOs`.
///
/// Whether the backend is a simulator/emulator rather than a real
/// device is deliberately *not* a public predicate — "sim vs device"
/// has no consistent cross-backend meaning. A backend that wants to
/// surface it folds it into its `Platform` identity instead (e.g.
/// `Custom("Sim")`), so author code reads one thing.
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

/// Seed the thread-local [`platform`] accessor with the booting
/// backend's identity.
///
/// Part of the **backend-author surface** — see
/// [`crate::backend`] for the full boot-time install set and the
/// ordering contract. Backends do not normally call this directly:
/// `runtime_vocabulary::backend::install_env_services` reads
/// `AppEnvOps::platform` off the backend and installs it here, and
/// every in-house boot entry calls that one function. Reach for this
/// only when a host has an identity to declare before a backend
/// instance exists.
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

/// Seed the thread-local [`color_scheme`] accessor with the booting
/// backend's reported appearance preference.
///
/// Part of the **backend-author surface** — see [`crate::backend`].
/// Normally reached through
/// `runtime_vocabulary::backend::install_env_services`, which reads
/// `AppEnvOps::color_scheme` off the backend.
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
/// Routes to the opener the active backend installed at boot (see
/// [`install_url_opener`]). Before boot — or on a backend that reports
/// no external-open capability (`AppEnvOps::url_opener` returned
/// `None`: terminal, CPU, runtime-server) — this is a no-op
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

/// Install the closure [`open_url`] routes to, or `None` for a backend
/// with no external-open capability.
///
/// Part of the **backend-author surface** — see [`crate::backend`].
/// Normally reached through
/// `runtime_vocabulary::backend::install_env_services`, which reads
/// `AppEnvOps::url_opener` off the backend.
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
/// Routes to the setter the active backend installed at boot (see
/// [`install_fullscreen_setter`]). Before boot — or on a backend with no
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

/// Install the closure [`set_fullscreen`] routes to, or `None` for a
/// backend with no full-screen concept.
///
/// Part of the **backend-author surface** — see [`crate::backend`].
/// Normally reached through
/// `runtime_vocabulary::backend::install_env_services`, which reads
/// `AppEnvOps::fullscreen_setter` off the backend.
pub fn install_fullscreen_setter(setter: Option<Rc<dyn Fn(bool)>>) {
    FULLSCREEN_SETTER.with(|cell| *cell.borrow_mut() = setter);
}

// ---------------------------------------------------------------------------
// Accessibility announcements
// ---------------------------------------------------------------------------

thread_local! {
    static ANNOUNCER: std::cell::RefCell<
        Option<Rc<dyn Fn(&str, crate::accessibility::LiveRegionPriority)>>,
    > = const { std::cell::RefCell::new(None) };
}

/// Post a one-shot live-region accessibility announcement to the host's
/// AX subsystem — transient feedback with no focus target ("Saved",
/// "Form submitted", "Loading complete"). Author-facing entry point for
/// `A11yOps::announce_for_accessibility`; callable from any event
/// handler or effect without holding a backend reference.
///
/// Per-backend mechanism (rule 7: converge in behavior, diverge in
/// mechanism): web writes a hidden `aria-live` region, iOS posts
/// `UIAccessibility.announcement`, macOS posts
/// `NSAccessibilityAnnouncementRequested`, Android calls
/// `announceForAccessibility`, the GPU backend queues into its pending
/// announcements. Use [`LiveRegionPriority::Assertive`] sparingly — it
/// interrupts whatever the screen reader is currently speaking.
///
/// Routes to the announcer the active backend installed at boot (see
/// [`install_announcer`]). Before boot — or on a backend with no
/// AX subsystem (terminal, CPU, Roku) — this is a no-op that logs once
/// at debug level.
///
/// [`LiveRegionPriority::Assertive`]: crate::accessibility::LiveRegionPriority::Assertive
pub fn announce(msg: &str, priority: crate::accessibility::LiveRegionPriority) {
    // Clone the Rc out before invoking so the announcer can re-enter
    // framework code without tripping a RefCell double-borrow.
    let announcer = ANNOUNCER.with(|cell| cell.borrow().clone());
    match announcer {
        Some(post) => post(msg, priority),
        None => crate::logging::log(
            crate::logging::LogLevel::Debug,
            "announce: no accessibility announcer installed for the active \
             backend; ignoring",
        ),
    }
}

/// Install the closure [`announce`] routes to, or `None` for a backend
/// with no AX subsystem.
///
/// Part of the **backend-author surface** — see [`crate::backend`].
/// Normally reached through
/// `runtime_vocabulary::backend::install_env_services`.
///
/// Unlike the URL opener and full-screen setter (self-contained
/// platform-singleton closures), the announcer must capture the live
/// backend handle, because `A11yOps::announce_for_accessibility` takes
/// `&mut self`. Capture it **weakly**: this thread-local outlives the
/// app, so an `Rc` here would leak the whole backend and its view tree.
/// `install_env_services` does that for you.
pub fn install_announcer(
    announcer: Option<Rc<dyn Fn(&str, crate::accessibility::LiveRegionPriority)>>,
) {
    ANNOUNCER.with(|cell| *cell.borrow_mut() = announcer);
}

// ---------------------------------------------------------------------------
// Backend trait

#[cfg(test)]
mod tests {
    //! Host-registry unit tests — moved verbatim from runtime-core's
    //! `backend.rs` alongside the code they exercise. Trait-default
    //! tests (`default_platform`, `default_url_opener_is_none`) stayed
    //! behind with the `Backend` trait.
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

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

    /// `announce` must route each call through the installed announcer,
    /// preserving the message and priority — the spine the per-backend
    /// `announce_for_accessibility` rides on (web aria-live, iOS/macOS
    /// AX post, Android announceForAccessibility).
    #[test]
    fn announce_dispatches_to_installed_announcer() {
        use crate::accessibility::LiveRegionPriority;
        let seen = Rc::new(RefCell::new(Vec::<(String, LiveRegionPriority)>::new()));
        let seen_for_announcer = seen.clone();
        install_announcer(Some(Rc::new(
            move |msg: &str, priority: LiveRegionPriority| {
                seen_for_announcer.borrow_mut().push((msg.to_string(), priority));
            },
        )));

        announce("Saved", LiveRegionPriority::Polite);
        announce("Error", LiveRegionPriority::Assertive);

        assert_eq!(
            *seen.borrow(),
            vec![
                ("Saved".to_string(), LiveRegionPriority::Polite),
                ("Error".to_string(), LiveRegionPriority::Assertive),
            ],
            "announce must forward each (msg, priority) to the installed announcer in order",
        );

        // Restore default so sibling tests on this thread aren't
        // surprised by a leftover announcer.
        install_announcer(None);
    }

    /// With no announcer installed, `announce` must be a silent no-op
    /// (logged at debug) — author code can call it before mount or on a
    /// backend without an AX subsystem (terminal, CPU, Roku).
    #[test]
    fn announce_without_announcer_is_noop() {
        install_announcer(None);
        announce("nobody is listening", crate::accessibility::LiveRegionPriority::Polite);
    }
}
