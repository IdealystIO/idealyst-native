//! Cross-platform inbound-URL handling — **deep links** and **universal /
//! app links**.
//!
//! This SDK delivers the URL that launched or resumed the app, parsed into
//! a small [`DeepLink`], and notifies you of every subsequent inbound link.
//! It is deliberately a *raw channel*: it hands you the parsed URL and
//! stops there. Turning a URL into a navigator route is the app's (or a
//! router SDK's) job — see [the scope note](#scope).
//!
//! ```ignore
//! use deep_link::{initial_link, on_link};
//!
//! // The URL that cold-started the app, if any.
//! if let Some(link) = initial_link() {
//!     println!("launched via {}{}", link.scheme, link.path);
//! }
//!
//! // Every inbound link while this guard is alive. Drop it to unsubscribe.
//! let _sub = on_link(|link| {
//!     for (k, v) in link.query_pairs() {
//!         println!("{k} = {v}");
//!     }
//! });
//! ```
//!
//! # How a link reaches you
//!
//! The OS hands the host (AppKit / UIKit / the Android Activity / the web
//! bootstrap) a URL; the host forwards it to the single ingress door
//! [`feed_link`]. The SDK parses it, records the very first one as the
//! [`initial_link`], and dispatches it to every live [`on_link`] handler.
//! [`feed_link`] is the platform-agnostic seam — the parse + registry +
//! dispatch below it is pure Rust and identical on every target, so once a
//! host calls [`feed_link`] everything works the same everywhere.
//!
//! Per-platform, the host calls [`feed_link`] from:
//!
//! - **apple** — `application(_:open:options:)` (custom scheme) and
//!   `application(_:continue:restorationHandler:)` (universal links). The
//!   cold-start launch URL seeds [`initial_link`]. *Compile-checked only.*
//! - **android** — the launch `Intent.getData()` in `onCreate` (→
//!   [`initial_link`]) and `onNewIntent` (→ [`feed_link`]); `<intent-filter>`
//!   entries in the manifest declare the scheme/host. *Compile-checked only.*
//! - **web** — on bootstrap, `window.location.href` seeds [`initial_link`];
//!   custom-scheme links don't apply in a browser, but app-internal
//!   navigations / `popstate` can be fed via [`feed_link`].
//!
//! # Permissions
//!
//! None at runtime. Inbound links instead require **build-time manifest
//! configuration** — a custom URL scheme / Associated Domains on Apple, an
//! `<intent-filter>` with `android:scheme` / `android:host` on Android. See
//! the README's "Permissions" section.

#![deny(missing_docs)]

use std::cell::RefCell;
use std::rc::Rc;

// Exactly one platform helper compiles per target; only `web` does real
// work today (reads `window.location.href`). The native launch-URL reads
// are wired host-side by the orchestrator (see crate docs), so there is no
// native module here to mislead — `initial_link()` is seeded by whoever
// calls `feed_link` first, on every target.
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
// The dispatch registry. A process-global, single-threaded registry: the
// app UI (and thus every host that delivers links) runs on one thread on
// every backend, so a thread-local avoids any Send/Sync requirement on the
// handlers (web closures hold non-Send JS values). This mirrors how the
// navigator SDK keeps its per-window state thread-local.
// ---------------------------------------------------------------------------

type Handler = Rc<dyn Fn(DeepLink)>;

struct Registry {
    /// The first link ever fed — the cold-start URL. Set once, never
    /// overwritten, so `initial_link()` is stable for the app's lifetime.
    initial: Option<DeepLink>,
    /// Whether `initial` has been claimed. Distinct from `initial.is_some()`
    /// so that even an *unparseable* first feed marks the slot as decided
    /// (we never retroactively promote a later link to "initial").
    initial_claimed: bool,
    /// Live subscriptions, keyed by a monotonic id so an RAII guard can
    /// remove exactly its own entry on drop.
    handlers: Vec<(u64, Handler)>,
    next_id: u64,
}

impl Registry {
    const fn new() -> Self {
        Registry {
            initial: None,
            initial_claimed: false,
            handlers: Vec::new(),
            next_id: 0,
        }
    }
}

thread_local! {
    static REGISTRY: RefCell<Registry> = const { RefCell::new(Registry::new()) };
}

/// The cold-start URL that launched the app, if any.
///
/// This is the **first** link [`feed_link`] ever received (typically the
/// launch URL / launch intent the host forwards at startup). It is set once
/// and never changes — later links arrive via [`on_link`], not here, so a
/// handler registered after launch can still recover the launch URL.
pub fn initial_link() -> Option<DeepLink> {
    REGISTRY.with(|r| r.borrow().initial.clone())
}

/// Subscribe to every inbound link delivered while the returned
/// [`LinkSubscription`] is alive.
///
/// The handler fires for each [`feed_link`] **including** the cold-start
/// link if it arrives after you subscribe. Dropping the returned guard
/// unsubscribes; there is no other teardown to remember.
///
/// Handlers run synchronously on the thread that calls [`feed_link`] (the
/// app/UI thread on every backend), in subscription order.
pub fn on_link(handler: impl Fn(DeepLink) + 'static) -> LinkSubscription {
    REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        let id = reg.next_id;
        reg.next_id += 1;
        reg.handlers.push((id, Rc::new(handler)));
        LinkSubscription { id }
    })
}

/// An RAII subscription guard. Drop it to unsubscribe the [`on_link`]
/// handler it represents. Holds no closure itself — the registry owns that
/// — so dropping is cheap and cannot run author code.
#[must_use = "dropping the subscription immediately unsubscribes; keep it alive while you want links"]
pub struct LinkSubscription {
    id: u64,
}

impl Drop for LinkSubscription {
    fn drop(&mut self) {
        // The thread-local may already be torn down at process exit; ignore.
        let _ = REGISTRY.try_with(|r| {
            r.borrow_mut().handlers.retain(|(id, _)| *id != self.id);
        });
    }
}

/// **Host ingress.** Feed a raw inbound URL into the SDK.
///
/// This is the door the platform host calls when the OS hands it a URL
/// (`application(_:open:options:)` on Apple, `onNewIntent` on Android, the
/// web bootstrap / a `popstate` handler on web). Wiring the host to call
/// this is the orchestrator's job; the SDK side — parse, initial-link
/// dedupe, dispatch — is all here.
///
/// Behavior:
/// - The **first** call ever (per thread/process) seeds [`initial_link`]
///   with the parsed URL. Subsequent calls never overwrite it.
/// - Every call dispatches the parsed link to all live [`on_link`]
///   handlers, in subscription order, on the calling thread.
/// - An unparseable URL is dropped (no handler fires) but still *claims*
///   the initial-link slot, so a malformed launch URL won't let a later
///   link masquerade as the cold-start link.
pub fn feed_link(raw_url: &str) {
    let parsed = DeepLink::parse(raw_url).ok();

    // Snapshot the handlers and seed `initial` under the borrow, then
    // release it *before* invoking handlers — a handler may call `on_link`
    // / drop a `LinkSubscription`, which re-borrows the registry. Holding
    // the borrow across dispatch would panic on that reentrancy.
    let handlers: Vec<Handler> = REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if !reg.initial_claimed {
            reg.initial_claimed = true;
            reg.initial = parsed.clone();
        }
        reg.handlers.iter().map(|(_, h)| Rc::clone(h)).collect()
    });

    if let Some(link) = parsed {
        for h in handlers {
            h(link.clone());
        }
    }
}

/// Seed [`initial_link`] from the platform's launch URL at startup.
///
/// A thin convenience the host bootstrap can call before any [`on_link`]
/// subscriber exists. On **web** it reads `window.location.href`; on every
/// other target it is a no-op (the native host reads the launch URL /
/// intent itself and calls [`feed_link`]). Calling [`feed_link`] directly
/// is equivalent — this just spares the web host from reaching into
/// `web_sys`.
pub fn seed_initial_from_platform() {
    #[cfg(target_arch = "wasm32")]
    if let Some(href) = web::current_href() {
        feed_link(&href);
    }
    // Non-web: nothing to read here; the host seeds via `feed_link`.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

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
    fn feed_link_fires_subscribed_handler() {
        fresh(|| {
            let count = Rc::new(Cell::new(0u32));
            let got: Rc<RefCell<Option<DeepLink>>> = Rc::new(RefCell::new(None));

            let c = Rc::clone(&count);
            let g = Rc::clone(&got);
            let _sub = on_link(move |link| {
                c.set(c.get() + 1);
                *g.borrow_mut() = Some(link);
            });

            feed_link("myapp://demo/path?x=1");

            assert_eq!(count.get(), 1);
            let link = got.borrow().clone().unwrap();
            assert_eq!(link.scheme, "myapp");
            assert_eq!(link.host.as_deref(), Some("demo"));
            assert_eq!(link.path, "/path");
            assert_eq!(link.query_pairs(), vec![("x".to_string(), "1".to_string())]);
        });
    }

    #[test]
    fn initial_link_is_first_feed_and_stable() {
        fresh(|| {
            assert_eq!(initial_link(), None);
            feed_link("myapp://first");
            feed_link("myapp://second");
            // initial stays the very first link, regardless of later feeds.
            assert_eq!(initial_link().unwrap().host.as_deref(), Some("first"));
        });
    }

    #[test]
    fn unparseable_first_feed_still_claims_initial_slot() {
        fresh(|| {
            feed_link("garbage"); // unparseable: dropped, but claims the slot
            assert_eq!(initial_link(), None);
            feed_link("myapp://real");
            // The later valid link must NOT be promoted to initial.
            assert_eq!(initial_link(), None);
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
