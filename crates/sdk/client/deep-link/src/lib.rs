//! Cross-platform inbound-URL handling — **deep links** (custom schemes,
//! `myapp://items/42`) and **universal / App Links**
//! (`https://example.com/items/42`).
//!
//! The framework routes inbound links on its own: the link that launches
//! the app opens its screen on the first mount, and a link that arrives
//! while the app runs moves the live navigators to the same screen. This
//! SDK is how app code takes part:
//!
//! - [`on_link`] — observe every link that arrives while the app runs.
//! - [`intercept`] — claim a link before it routes (an auth gate), then
//!   [`route_link`] it later.
//! - [`initial_link`] — the URL the app was launched with.
//! - [`DeepLink`] — the parsed URL ([`DeepLink::route_path`] is the app
//!   path it routes to).
//!
//! ```ignore
//! use deep_link::{initial_link, on_link};
//!
//! if let Some(link) = initial_link() {
//!     log::info!("launched via {}", link.scheme);
//! }
//! // Every warm link while this guard is alive. Drop it to unsubscribe.
//! let _sub = on_link(|link| {
//!     for (k, v) in link.query_pairs() {
//!         log::info!("{k} = {v}");
//!     }
//! });
//! ```
//!
//! # How a link reaches you
//!
//! The OS hands the host a URL — `application(_:open:options:)` /
//! `application(_:continue:restorationHandler:)` on iOS, the
//! `kAEGetURL` Apple Event on macOS, the launch `Intent` and `onNewIntent`
//! on Android, the page address on web. The host calls the framework's
//! ingress (`runtime_shared::inbound_link`): `launch` for the cold-start
//! URL, `deliver` (then its flush) for a warm one. Every target delivers
//! the same thing; only where the host calls from differs.
//!
//! How a URL maps to a screen is
//! [`runtime_shared::inbound_link::route_path`]: the path of a web link;
//! for a custom scheme, the authority is the first segment.
//!
//! # The live address
//!
//! [`initial_link`] is fixed at launch. Where the app is *now* is a
//! separate question with its own three calls — [`current_url`] (read),
//! [`replace_url`] (rewrite without navigating) and [`origin`] (for
//! building absolute links). They answer on web, the one platform with
//! an address bar, and are `None` / no-ops on native. See
//! [`current_url`] for why that is the honest answer rather than a gap.
//!
//! # Configuration
//!
//! No runtime permission. The OS only sends a link to an app that declares
//! it, under `[package.metadata.idealyst.app.links]` in the app's
//! `Cargo.toml` — see the README.

#![deny(missing_docs)]

// Exactly one platform helper compiles per target; only `web` does real
// work today (reads `window.location.href`). The native launch URL is
// recorded host-side before attach (`runtime_shared::inbound_link::
// record_launch`), so there is no native module here — `initial_link()`
// reads that record on every target. `feed_link` does NOT seed it: it
// delivers (and routes) a link arriving while the app runs.
#[cfg(target_arch = "wasm32")]
mod web;

/// Compile-checked usage recipes (docs / MCP catalog). Present only under
/// the `catalog` feature — see [`recipes`].
#[cfg(feature = "catalog")]
pub mod recipes;

// ---------------------------------------------------------------------------
// DeepLink — the parsed inbound URL.
// ---------------------------------------------------------------------------

/// A parsed inbound URL.
///
/// Constructed from a raw URL string with [`DeepLink::parse`]. The fields
/// are the parts an app routes on:
///
/// - [`scheme`](Self::scheme) — `"myapp"` in `myapp://…`, `"https"` for a
///   universal/app link. Always lowercased (schemes are case-insensitive).
/// - [`host`](Self::host) — the authority, e.g. `Some("open")` in
///   `myapp://open/x` or `Some("example.com")` for `https://example.com/x`.
///   `None` when the URL has no authority (e.g. `myapp:/path` or `mailto:`).
///   Preserved verbatim for custom schemes (not case-folded) — lowercase it
///   yourself if you route on the authority case-insensitively.
/// - [`path`](Self::path) — the path component, e.g. `"/items/42"`. Empty
///   string when absent.
/// - [`query`](Self::query) — the raw query string without the leading `?`,
///   or `None`. Decode it into pairs with [`query_pairs`](Self::query_pairs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepLink {
    /// The URL scheme, lowercased (`"myapp"`, `"https"`, …).
    pub scheme: String,
    /// The host / authority, if the URL has one.
    pub host: Option<String>,
    /// The path component (e.g. `"/items/42"`); empty string when absent.
    pub path: String,
    /// The raw query string without the leading `?`, if present.
    pub query: Option<String>,
}

/// An error parsing a raw URL into a [`DeepLink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid deep link URL: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

impl DeepLink {
    /// Parse a raw URL string into a [`DeepLink`].
    ///
    /// Accepts custom-scheme links (`myapp://open/x?y=1`) and standard
    /// web links (`https://example.com/path`) alike. The query is *not*
    /// decoded here — call [`query_pairs`](Self::query_pairs) for that.
    ///
    /// Returns [`ParseError`] for input that isn't a URL (no scheme, etc.).
    pub fn parse(raw: &str) -> Result<DeepLink, ParseError> {
        let url = url::Url::parse(raw.trim()).map_err(|e| ParseError(e.to_string()))?;

        // `url` reports the host as `None` for non-special schemes that the
        // spec treats as opaque (`mailto:`, `myapp:foo`), and as a parsed
        // authority for `myapp://host/…`. We surface whatever it found.
        let host = url.host_str().map(|h| h.to_string()).filter(|h| !h.is_empty());

        Ok(DeepLink {
            scheme: url.scheme().to_string(),
            host,
            path: url.path().to_string(),
            query: url.query().map(|q| q.to_string()),
        })
    }

    /// The query as decoded `(key, value)` pairs, percent-decoded.
    ///
    /// Empty when there is no query. A key without `=` yields an empty
    /// value (`?flag` → `("flag", "")`). Order is preserved.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        let Some(q) = self.query.as_deref() else {
            return Vec::new();
        };
        // Reuse `url`'s WHATWG-correct application/x-www-form-urlencoded
        // decoder rather than hand-splitting on `&`/`=` (which mishandles
        // `+`-as-space and percent-encoding).
        url::form_urlencoded::parse(q.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The inbound channel. The registry itself lives in the framework's host
// seam (`runtime_shared::inbound_link`), because the HOST delivers links —
// before any app code runs, for the cold-start one — and a host must not
// depend on an SDK. This crate is the typed author face over it: it parses
// the raw URLs the seam carries into `DeepLink`s.
// ---------------------------------------------------------------------------

use runtime_shared::inbound_link;

impl DeepLink {
    /// The app path this link routes to — what the framework hands the
    /// navigators. `https://example.com/items/42?x=1` → `/items/42?x=1`;
    /// for a custom scheme the authority is the first segment:
    /// `myapp://items/42` → `/items/42`. See
    /// [`runtime_shared::inbound_link::route_path`].
    pub fn route_path(&self) -> String {
        inbound_link::route_path(&self.to_url())
    }

    /// Re-serialize the parts back into a URL string.
    fn to_url(&self) -> String {
        let mut out = self.scheme.clone();
        out.push(':');
        if let Some(host) = &self.host {
            out.push_str("//");
            out.push_str(host);
        }
        out.push_str(&self.path);
        if let Some(q) = &self.query {
            out.push('?');
            out.push_str(q);
        }
        out
    }
}

/// The URL that cold-started the app, if any.
///
/// Recorded by the host before the app mounts (the launch URL / launch
/// intent / the web page's address). It never changes — links that arrive
/// later reach [`on_link`], not here. `None` when the app was opened
/// normally, or when the launch URL does not parse.
///
/// You rarely need this for routing: the framework already opened the
/// linked screen at launch. Use it for what the path alone doesn't carry
/// (the scheme, the host, attribution parameters).
pub fn initial_link() -> Option<DeepLink> {
    inbound_link::launch_url().and_then(|raw| DeepLink::parse(&raw).ok())
}

/// Observe every link that arrives while the app runs, for as long as the
/// returned [`LinkSubscription`] is alive. Dropping it unsubscribes.
///
/// Observing does not change routing — the framework still moves the
/// navigators to the link (unless an [`intercept`]or claims it). Handlers
/// run synchronously on the UI thread, in subscription order, before
/// routing. A URL that does not parse is not delivered.
pub fn on_link(handler: impl Fn(DeepLink) + 'static) -> LinkSubscription {
    LinkSubscription {
        _registration: inbound_link::observe(move |raw| {
            if let Ok(link) = DeepLink::parse(raw) {
                handler(link);
            }
        }),
    }
}

/// Claim links before the framework routes them: return `true` to take a
/// link (the navigators don't move), `false` to let it route. Active while
/// the returned [`LinkSubscription`] is alive.
///
/// The usual reason is an auth gate — hold the link while signed out, then
/// [`route_link`] it once the user is in:
///
/// ```ignore
/// let held = signal::<Option<DeepLink>>(None);
/// let gate = deep_link::intercept(move |link| {
///     if signed_in.peek() { return false; }
///     held.set(Some(link.clone()));
///     true
/// });
/// // …after sign-in:
/// if let Some(link) = held.peek() { deep_link::route_link(&link); }
/// ```
///
/// Applies to links that arrive while the app runs. The cold-start link
/// is resolved by the navigators as they first mount, so a gate that
/// mounts its navigators only after sign-in (`if signed_in { … }`) already
/// gets it: the launch path waits in the navigators' launch slot until the
/// root navigator mounts. A URL that does not parse is never claimed.
pub fn intercept(handler: impl Fn(&DeepLink) -> bool + 'static) -> LinkSubscription {
    LinkSubscription {
        _registration: inbound_link::intercept(move |raw| {
            DeepLink::parse(raw).is_ok_and(|link| handler(&link))
        }),
    }
}

/// Move the navigators to `link`'s [`route_path`](DeepLink::route_path) —
/// what the framework does with a link nobody intercepted. Returns whether
/// the link landed (a navigator moved, or its screen is already showing).
///
/// Navigation is staged and commits on the framework's next flush, which
/// every framework-delivered callback (press handlers, effects) already
/// triggers.
pub fn route_link(link: &DeepLink) -> bool {
    inbound_link::route(&link.to_url())
}

/// An RAII guard for [`on_link`] / [`intercept`]. Drop it to unregister;
/// dropping runs no author code.
#[must_use = "dropping the subscription immediately unsubscribes; keep it alive while you want links"]
pub struct LinkSubscription {
    _registration: inbound_link::Registration,
}

/// **Host ingress.** Deliver a raw inbound URL as if the OS had handed it
/// to a running app: observers fire, interceptors are asked, then it
/// routes. Returns whether it landed.
///
/// The framework's hosts already call this for you (via
/// `runtime_shared::inbound_link::deliver`, followed by their flush); it is
/// public for custom hosts and tests. From app code, prefer
/// [`route_link`]. Outside a framework callback the navigation is staged
/// until the next flush.
pub fn feed_link(raw_url: &str) -> bool {
    inbound_link::deliver(raw_url)
}

/// Record the platform's launch URL as [`initial_link`] if the host has not
/// already. The web backend records `window.location.href` at boot, so
/// this is only needed by a custom bootstrap; a no-op off web.
pub fn seed_initial_from_platform() {
    #[cfg(target_arch = "wasm32")]
    if let Some(href) = web::current_href() {
        inbound_link::record_launch(&href);
    }
}

// ---------------------------------------------------------------------------
// The live address — where the app is right now, not how it was reached.
// ---------------------------------------------------------------------------
//
// Everything above is about URLs ARRIVING. These three are about the URL
// the app currently occupies, which is a different question: on web the
// address bar moves under the app (the navigators rewrite it, the user
// edits it, Back rewinds it) while `initial_link()` stays the launch URL
// forever. Apps need the live one to decide things before a navigator
// exists (a public share path that must skip the auth gate), to build
// absolute links for other people (a share URL is `origin + path`), and
// to keep filter state in the query string.
//
// Only a platform with an address bar has any of this. Native targets
// answer `None` / do nothing — not as a stub, but because the question
// has no answer there: an iOS app is not "at" a URL, its navigators hold
// the in-memory path, and a link built for someone else has no origin
// to be relative to.

/// The address the app is at **right now**, parsed.
///
/// On **web** this is `window.location.href`, read at the time of the
/// call — it follows every navigator write, every `replace_url`, and the
/// browser's Back/Forward, unlike [`initial_link`], which is fixed at the
/// launch URL. Use it for decisions that have to be made from the URL
/// before a navigator has mounted, and for reading query-string state.
///
/// `None` on every **native** target (there is no address bar; the
/// navigators' in-memory path is the source of truth there) and on web
/// without a `window` (a worker, a server-side prerender).
///
/// ```ignore
/// let path = deep_link::current_url().map(|u| u.path).unwrap_or_else(|| "/".into());
/// ```
pub fn current_url() -> Option<DeepLink> {
    #[cfg(target_arch = "wasm32")]
    {
        web::current_href().and_then(|href| DeepLink::parse(&href).ok())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// The origin the app is served from — `scheme://host[:port]`, no
/// trailing slash — for building absolute URLs to hand to someone else
/// (a share link, an email, a second browser tab).
///
/// `Some` only on **web** (`window.location.origin`). `None` on native,
/// where the app is not served from anywhere, and for an opaque origin
/// (`file:` pages, sandboxed frames), which no URL can be built on.
pub fn origin() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web::origin()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// Rewrite the address the app is at **without navigating**:
/// `history.replaceState` on web. `url` is usually app-absolute
/// (`"/projects?tab=2"`); anything `replaceState` accepts works, and a
/// cross-origin URL is refused by the browser, silently: there is no
/// success signal.
///
/// Nothing re-renders and no history entry is added: this corrects what
/// the address bar SAYS so a reload or a copied link lands where the
/// screen already is. Screen changes belong to the navigators, which
/// write the URL themselves.
///
/// The entry's existing `history.state` is carried over, so state a
/// navigator attached to the entry survives the rewrite. No handler
/// registered with [`on_link`] fires — this is the app's own write, not
/// an inbound link.
///
/// A no-op on native targets (no address bar) and on web without a
/// `window`.
pub fn replace_url(url: &str) {
    #[cfg(target_arch = "wasm32")]
    web::replace_url(url);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = url;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    // Each test runs on its own thread to get a fresh thread-local
    // registry — the registry is process-global per thread by design, so
    // sharing it across tests on one thread would leak the initial-link
    // slot between them.
    fn fresh<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::spawn(f).join().unwrap()
    }

    #[test]
    fn parses_custom_scheme() {
        let l = DeepLink::parse("myapp://open/items/42?x=1&y=two").unwrap();
        assert_eq!(l.scheme, "myapp");
        assert_eq!(l.host.as_deref(), Some("open"));
        assert_eq!(l.path, "/items/42");
        assert_eq!(l.query.as_deref(), Some("x=1&y=two"));
        assert_eq!(
            l.query_pairs(),
            vec![
                ("x".to_string(), "1".to_string()),
                ("y".to_string(), "two".to_string()),
            ]
        );
    }

    #[test]
    fn parses_universal_link() {
        let l = DeepLink::parse("https://example.com/path/to?a=b").unwrap();
        assert_eq!(l.scheme, "https");
        assert_eq!(l.host.as_deref(), Some("example.com"));
        assert_eq!(l.path, "/path/to");
        assert_eq!(l.query.as_deref(), Some("a=b"));
    }

    #[test]
    fn scheme_is_lowercased() {
        // Schemes are case-insensitive per RFC 3986; `url` normalizes them.
        let l = DeepLink::parse("MyApp://Host/Path").unwrap();
        assert_eq!(l.scheme, "myapp");
        // The host is preserved verbatim — `url` only case-folds hosts of
        // *special* schemes (http/https/ws…), and we keep `default-features
        // = false`, so a custom-scheme authority is passed through as-is.
        // Apps that treat the authority as a case-insensitive route segment
        // should lowercase it themselves.
        assert_eq!(l.host.as_deref(), Some("Host"));
        assert_eq!(l.path, "/Path");
    }

    #[test]
    fn no_authority_yields_none_host() {
        // A path-only custom URL has no authority.
        let l = DeepLink::parse("myapp:/just/a/path").unwrap();
        assert_eq!(l.scheme, "myapp");
        assert_eq!(l.host, None);
        assert_eq!(l.path, "/just/a/path");
    }

    #[test]
    fn query_pairs_decode_and_handle_flags() {
        let l = DeepLink::parse("myapp://x?q=hello%20world&plus=a+b&flag").unwrap();
        assert_eq!(
            l.query_pairs(),
            vec![
                ("q".to_string(), "hello world".to_string()),
                ("plus".to_string(), "a b".to_string()),
                ("flag".to_string(), "".to_string()),
            ]
        );
    }

    #[test]
    fn empty_query_is_no_pairs() {
        let l = DeepLink::parse("myapp://x/y").unwrap();
        assert_eq!(l.query, None);
        assert!(l.query_pairs().is_empty());
    }

    #[test]
    fn parse_rejects_non_url() {
        assert!(DeepLink::parse("not a url").is_err());
        assert!(DeepLink::parse("").is_err());
    }

    #[test]
    fn on_link_receives_warm_links_parsed() {
        fresh(|| {
            let got: Rc<RefCell<Option<DeepLink>>> = Rc::new(RefCell::new(None));
            let g = Rc::clone(&got);
            let _sub = on_link(move |link| *g.borrow_mut() = Some(link));

            feed_link("myapp://demo/path?x=1");

            let link = got.borrow().clone().unwrap();
            assert_eq!(link.scheme, "myapp");
            assert_eq!(link.host.as_deref(), Some("demo"));
            assert_eq!(link.path, "/path");
            assert_eq!(link.query_pairs(), vec![("x".to_string(), "1".to_string())]);
        });
    }

    #[test]
    fn initial_link_is_the_host_recorded_launch_url_not_a_warm_link() {
        fresh(|| {
            assert_eq!(initial_link(), None);
            // A warm link is not the launch link.
            feed_link("myapp://warm");
            assert_eq!(initial_link(), None);
            inbound_link::launch("myapp://first/x");
            inbound_link::launch("myapp://second");
            assert_eq!(initial_link().unwrap().host.as_deref(), Some("first"));
        });
    }

    #[test]
    fn route_path_matches_the_framework_mapping() {
        let custom = DeepLink::parse("myapp://items/42?x=1").unwrap();
        assert_eq!(custom.route_path(), "/items/42?x=1");
        let web = DeepLink::parse("https://example.com/items/42").unwrap();
        assert_eq!(web.route_path(), "/items/42");
        let bare = DeepLink::parse("myapp:/items/42").unwrap();
        assert_eq!(bare.route_path(), "/items/42");
    }

    #[test]
    fn intercept_holds_a_link_and_route_link_releases_it() {
        fresh(|| {
            let routed = Rc::new(RefCell::new(Vec::<String>::new()));
            let r = Rc::clone(&routed);
            inbound_link::install_router(Rc::new(move |p| {
                r.borrow_mut().push(p.to_string());
                true
            }));
            let held = Rc::new(RefCell::new(None::<DeepLink>));
            let h = Rc::clone(&held);
            let gate = intercept(move |link| {
                *h.borrow_mut() = Some(link.clone());
                true
            });
            assert!(!feed_link("myapp://items/7"));
            assert!(routed.borrow().is_empty());

            let link = held.borrow_mut().take().unwrap();
            assert!(route_link(&link));
            assert_eq!(*routed.borrow(), vec!["/items/7".to_string()]);
            drop(gate);
            feed_link("myapp://items/8");
            assert_eq!(routed.borrow().len(), 2);
        });
    }

    #[test]
    fn unparseable_links_are_neither_observed_nor_claimed() {
        fresh(|| {
            let seen = Rc::new(Cell::new(0u32));
            let s = Rc::clone(&seen);
            let _sub = on_link(move |_| s.set(s.get() + 1));
            let claimed = Rc::new(Cell::new(0u32));
            let c = Rc::clone(&claimed);
            let _gate = intercept(move |_| {
                c.set(c.get() + 1);
                true
            });
            feed_link("garbage");
            assert_eq!(seen.get(), 0);
            assert_eq!(claimed.get(), 0);
        });
    }

    #[test]
    fn dropping_subscription_unsubscribes() {
        fresh(|| {
            let count = Rc::new(Cell::new(0u32));
            let c = Rc::clone(&count);
            let sub = on_link(move |_| c.set(c.get() + 1));
            feed_link("myapp://a");
            assert_eq!(count.get(), 1);
            drop(sub);
            feed_link("myapp://b");
            assert_eq!(count.get(), 1); // no further fires after drop
        });
    }

    #[test]
    fn multiple_subscribers_all_fire_in_order() {
        fresh(|| {
            let order = Rc::new(RefCell::new(Vec::<u8>::new()));
            let o1 = Rc::clone(&order);
            let o2 = Rc::clone(&order);
            let _s1 = on_link(move |_| o1.borrow_mut().push(1));
            let _s2 = on_link(move |_| o2.borrow_mut().push(2));
            feed_link("myapp://x");
            assert_eq!(*order.borrow(), vec![1, 2]);
        });
    }

    /// Off-web there is no address bar: the live-address API answers
    /// `None` and `replace_url` must neither panic nor feed the link
    /// registry (it is the app's own write, not an inbound link).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_has_no_live_address_and_replace_is_inert() {
        fresh(|| {
            assert_eq!(current_url(), None);
            assert_eq!(origin(), None);
            let fired = Rc::new(Cell::new(0u32));
            let f = Rc::clone(&fired);
            let _sub = on_link(move |_| f.set(f.get() + 1));
            replace_url("/somewhere?x=1");
            assert_eq!(fired.get(), 0, "replace_url must not dispatch to on_link");
            assert_eq!(initial_link(), None, "replace_url must not claim the initial slot");
        });
    }

    #[test]
    fn handler_may_subscribe_during_dispatch_without_panicking() {
        // Reentrancy guard: a handler that calls on_link while dispatching
        // must not deadlock/panic on the registry borrow.
        fresh(|| {
            let nested = Rc::new(RefCell::new(None::<LinkSubscription>));
            let n = Rc::clone(&nested);
            let _sub = on_link(move |_| {
                if n.borrow().is_none() {
                    *n.borrow_mut() = Some(on_link(|_| {}));
                }
            });
            feed_link("myapp://x"); // must not panic
        });
    }
}
