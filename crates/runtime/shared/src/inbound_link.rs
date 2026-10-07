//! Inbound links — the ingress a platform host calls when the OS hands the
//! app a URL (a custom-scheme deep link like `myapp://items/42`, or a
//! universal / App Link like `https://example.com/items/42`).
//!
//! Two moments, two doors:
//!
//! - **Cold start** — [`launch`]. The host calls it with the URL that
//!   launched the app, BEFORE the app mounts. It records the URL (read back
//!   through [`launch_url`]) and seeds the navigators' launch-path slot
//!   ([`set_initial_path`]) with [`route_path`] of it, so the first mount
//!   opens the linked screen and reconstructs its back stack.
//! - **Warm** — [`deliver`]. The host calls it when a link arrives while
//!   the app is running. Every observer sees the URL; then, unless an
//!   interceptor claims it, the installed router moves the live navigators
//!   to [`route_path`] of it. The vocabulary's navigator handlers install
//!   that router; a host with no navigators has none and the link is only
//!   observed.
//!
//! The two doors meet at one rule: **the same URL opens the same screen
//! whether it launched the app or arrived while it ran.** That is why the
//! warm path routes by default instead of leaving every app to write the
//! glue the cold path never needed.
//!
//! [`deliver`] only STAGES navigation (navigator commands commit on the
//! world's next flush), so a host calling it from a raw platform callback —
//! outside every framework-wrapped event handler — must flush afterwards,
//! the same contract as web's `popstate` listener. Each native backend
//! wraps that pair in its own entry point; hosts call the wrapper.
//!
//! Observers, interceptors and the router run on the UI thread, in
//! registration order. The author-facing API is the `deep-link` SDK
//! (`on_link`, `intercept`, `route_link`), which parses the raw URL; this
//! module stays string-typed so it needs no URL parser and the host seam
//! carries nothing an SDK has to agree on.
//!
//! [`set_initial_path`]: crate::primitives::navigator::set_initial_path

use std::cell::RefCell;
use std::rc::Rc;

use crate::primitives::navigator::set_initial_path;

type Observer = Rc<dyn Fn(&str)>;
type Interceptor = Rc<dyn Fn(&str) -> bool>;
type Router = Rc<dyn Fn(&str) -> bool>;

#[derive(Default)]
struct Ingress {
    /// The URL that cold-started the app. First [`launch`] /
    /// [`record_launch`] wins and it never changes afterwards.
    launch: Option<String>,
    observers: Vec<(u64, Observer)>,
    interceptors: Vec<(u64, Interceptor)>,
    router: Option<Router>,
    next_id: u64,
}

thread_local! {
    static INGRESS: RefCell<Ingress> = RefCell::new(Ingress::default());
}

/// Which list a [`Registration`] removes itself from on drop.
#[derive(Copy, Clone)]
enum Kind {
    Observer,
    Interceptor,
}

/// RAII guard for an [`observe`] / [`intercept`] registration. Dropping it
/// unregisters; it holds no closure, so dropping cannot run author code.
#[must_use = "dropping the registration immediately unregisters it"]
pub struct Registration {
    id: u64,
    kind: Kind,
}

impl Drop for Registration {
    fn drop(&mut self) {
        // The thread-local may already be gone at thread exit; ignore.
        let _ = INGRESS.try_with(|cell| {
            let mut ingress = cell.borrow_mut();
            match self.kind {
                Kind::Observer => ingress.observers.retain(|(id, _)| *id != self.id),
                Kind::Interceptor => ingress.interceptors.retain(|(id, _)| *id != self.id),
            }
        });
    }
}

/// **Host ingress, cold start.** Record `raw_url` as the URL that launched
/// the app and seed the navigators' launch-path slot with its
/// [`route_path`].
///
/// Call once, on the UI thread, BEFORE the app mounts — the root navigator
/// reads the slot during its synchronous initial mount and clears it once
/// its subtree is up. A second call does not replace the recorded launch
/// URL (the first wins).
pub fn launch(raw_url: &str) {
    record_launch(raw_url);
    set_initial_path(Some(route_path(raw_url)));
}

/// Record `raw_url` as the launch URL WITHOUT seeding the launch-path
/// slot — for a host that seeds the slot itself (web reads
/// `location.pathname` on boot). First call wins.
pub fn record_launch(raw_url: &str) {
    INGRESS.with(|cell| {
        let mut ingress = cell.borrow_mut();
        if ingress.launch.is_none() {
            ingress.launch = Some(raw_url.to_string());
        }
    });
}

/// The URL that cold-started the app, as the host delivered it. `None` when
/// the app was launched normally (no link).
pub fn launch_url() -> Option<String> {
    INGRESS.with(|cell| cell.borrow().launch.clone())
}

/// **Host ingress, warm.** A link arrived while the app is running.
///
/// Observers fire first (every one, every link). Then each interceptor is
/// asked in registration order; the first that returns `true` claims the
/// link and nothing is routed. Otherwise the link is [`route`]d.
///
/// Returns whether a navigator took the link. Navigation is staged, not
/// committed — see the module docs for the flush the caller owes.
pub fn deliver(raw_url: &str) -> bool {
    // Snapshot under the borrow and release it before running author
    // code: a callback may register or drop a registration, which
    // re-borrows the ingress.
    let (observers, interceptors) = INGRESS.with(|cell| {
        let ingress = cell.borrow();
        (
            ingress.observers.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
            ingress.interceptors.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
        )
    });
    for observe in observers {
        observe(raw_url);
    }
    if interceptors.iter().any(|claims| claims(raw_url)) {
        return false;
    }
    route(raw_url)
}

/// Move the live navigators to `raw_url`'s [`route_path`] — what
/// [`deliver`] does for a link nobody intercepted. Public so an
/// interceptor that held a link (an auth gate) can route it later.
///
/// Returns whether a navigator took the link; `false` when no router is
/// installed (no navigator ever mounted) or no navigator's routes match.
/// Staged, like [`deliver`].
pub fn route(raw_url: &str) -> bool {
    let router = INGRESS.with(|cell| cell.borrow().router.clone());
    match router {
        Some(router) => router(&route_path(raw_url)),
        None => false,
    }
}

/// Watch every warm link. See [`deliver`] for the order.
pub fn observe(f: impl Fn(&str) + 'static) -> Registration {
    INGRESS.with(|cell| {
        let mut ingress = cell.borrow_mut();
        let id = ingress.next_id;
        ingress.next_id += 1;
        ingress.observers.push((id, Rc::new(f)));
        Registration { id, kind: Kind::Observer }
    })
}

/// Offer to claim every warm link before it is routed: return `true` to
/// take it (the navigators don't move), `false` to let it route.
pub fn intercept(f: impl Fn(&str) -> bool + 'static) -> Registration {
    INGRESS.with(|cell| {
        let mut ingress = cell.borrow_mut();
        let id = ingress.next_id;
        ingress.next_id += 1;
        ingress.interceptors.push((id, Rc::new(f)));
        Registration { id, kind: Kind::Interceptor }
    })
}

/// Install the router [`route`] hands app paths to (idempotent replace).
/// The vocabulary's navigator registration installs it; the closure
/// returns whether a navigator took the path.
pub fn install_router(router: Rc<dyn Fn(&str) -> bool>) {
    INGRESS.with(|cell| cell.borrow_mut().router = Some(router));
}

/// The app path (`/path?query`, fragment dropped) a URL routes to.
///
/// - **Web links** (`http`/`https`): the URL's path —
///   `https://example.com/items/42?tab=2` → `/items/42?tab=2`. The host is
///   the domain the link was verified for, not part of the route.
/// - **Custom schemes**: the authority is the FIRST path segment —
///   `myapp://items/42` → `/items/42`. That is how people write these links
///   (`myapp://settings`), and a custom scheme has no domain for the
///   authority to be. `myapp:///items/42` (empty authority) routes the same.
/// - An already app-relative input (`/items/42`) passes through.
///
/// No percent-decoding: navigators match the raw segments, exactly as web
/// matches `location.pathname`. An empty path routes to `/`.
pub fn route_path(raw_url: &str) -> String {
    let raw = raw_url.trim();
    // Fragment never routes.
    let raw = raw.split('#').next().unwrap_or("");

    let scheme_end = raw.find(':').filter(|&i| {
        // A scheme is ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) — anything
        // else before the first ':' means this isn't one (`/a:b`).
        let s = &raw[..i];
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let (scheme, rest) = match scheme_end {
        Some(i) => (raw[..i].to_ascii_lowercase(), &raw[i + 1..]),
        None => (String::new(), raw),
    };

    let (authority, path_and_query) = match rest.strip_prefix("//") {
        Some(after) => {
            let end = after.find(['/', '?']).unwrap_or(after.len());
            (&after[..end], &after[end..])
        }
        None => ("", rest),
    };
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_and_query, None),
    };

    let web = scheme.is_empty() || scheme == "http" || scheme == "https";
    let mut out = String::new();
    if !web && !authority.is_empty() {
        out.push('/');
        out.push_str(authority);
    }
    if !path.is_empty() && !path.starts_with('/') {
        out.push('/');
    }
    out.push_str(path);
    if out.is_empty() {
        out.push('/');
    }
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        out.push('?');
        out.push_str(q);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::navigator::peek_initial_path;
    use std::cell::Cell;

    // Each test gets a fresh thread (and so fresh thread-locals): the
    // ingress is per-thread by design.
    fn fresh(f: impl FnOnce() + Send + 'static) {
        std::thread::spawn(f).join().unwrap();
    }

    #[test]
    fn route_path_web_links_route_on_the_path() {
        assert_eq!(route_path("https://example.com/items/42?tab=2"), "/items/42?tab=2");
        assert_eq!(route_path("https://example.com"), "/");
        assert_eq!(route_path("https://example.com/"), "/");
        assert_eq!(route_path("HTTPS://Example.com/a#frag"), "/a");
        assert_eq!(route_path("https://example.com:8443/a?"), "/a");
    }

    #[test]
    fn route_path_custom_scheme_authority_is_the_first_segment() {
        assert_eq!(route_path("myapp://items/42?x=1"), "/items/42?x=1");
        assert_eq!(route_path("myapp://settings"), "/settings");
        assert_eq!(route_path("myapp:///items/42"), "/items/42");
        assert_eq!(route_path("myapp://"), "/");
        assert_eq!(route_path("myapp:items/42"), "/items/42");
        assert_eq!(route_path("myapp://?ref=mail"), "/?ref=mail");
    }

    #[test]
    fn route_path_app_relative_input_passes_through() {
        assert_eq!(route_path("/items/42?x=1"), "/items/42?x=1");
        assert_eq!(route_path("/a:b"), "/a:b");
        assert_eq!(route_path(""), "/");
    }

    #[test]
    fn launch_records_the_url_and_seeds_the_launch_slot() {
        fresh(|| {
            launch("myapp://items/42?x=1");
            assert_eq!(launch_url().as_deref(), Some("myapp://items/42?x=1"));
            assert_eq!(peek_initial_path().as_deref(), Some("/items/42?x=1"));
            // First launch wins.
            record_launch("myapp://other");
            assert_eq!(launch_url().as_deref(), Some("myapp://items/42?x=1"));
        });
    }

    #[test]
    fn deliver_observes_then_routes() {
        fresh(|| {
            let seen = Rc::new(RefCell::new(Vec::<String>::new()));
            let routed = Rc::new(RefCell::new(Vec::<String>::new()));
            let s = seen.clone();
            let _obs = observe(move |u| s.borrow_mut().push(u.to_string()));
            let r = routed.clone();
            install_router(Rc::new(move |p| {
                r.borrow_mut().push(p.to_string());
                true
            }));
            assert!(deliver("myapp://items/7"));
            assert_eq!(*seen.borrow(), vec!["myapp://items/7".to_string()]);
            assert_eq!(*routed.borrow(), vec!["/items/7".to_string()]);
            // A warm link is not the launch link.
            assert_eq!(launch_url(), None);
        });
    }

    #[test]
    fn an_interceptor_claiming_the_link_stops_routing_but_not_observers() {
        fresh(|| {
            let routed = Rc::new(Cell::new(0));
            let r = routed.clone();
            install_router(Rc::new(move |_| {
                r.set(r.get() + 1);
                true
            }));
            let observed = Rc::new(Cell::new(0));
            let o = observed.clone();
            let _obs = observe(move |_| o.set(o.get() + 1));
            let held = Rc::new(RefCell::new(None::<String>));
            let h = held.clone();
            let gate = intercept(move |u| {
                *h.borrow_mut() = Some(u.to_string());
                true
            });
            assert!(!deliver("myapp://secret"));
            assert_eq!(routed.get(), 0);
            assert_eq!(observed.get(), 1);
            // The holder routes it later (after login).
            let pending = held.borrow_mut().take().unwrap();
            assert!(route(&pending));
            assert_eq!(routed.get(), 1);
            // Dropping the interceptor lets links route again.
            drop(gate);
            deliver("myapp://open");
            assert_eq!(routed.get(), 2);
        });
    }

    #[test]
    fn a_declining_interceptor_lets_the_link_route() {
        fresh(|| {
            let routed = Rc::new(Cell::new(0));
            let r = routed.clone();
            install_router(Rc::new(move |_| {
                r.set(r.get() + 1);
                true
            }));
            let _gate = intercept(|u| u.contains("admin"));
            deliver("myapp://items/1");
            assert_eq!(routed.get(), 1);
            deliver("myapp://admin");
            assert_eq!(routed.get(), 1);
        });
    }

    #[test]
    fn no_router_means_nothing_routes() {
        fresh(|| {
            assert!(!deliver("myapp://items/1"));
            assert!(!route("myapp://items/1"));
        });
    }

    #[test]
    fn callbacks_may_register_and_unregister_reentrantly() {
        fresh(|| {
            let inner = Rc::new(RefCell::new(None::<Registration>));
            let i = inner.clone();
            let _outer = observe(move |_| {
                // Register (and replace/drop) from inside a dispatch — must
                // not double-borrow the ingress.
                *i.borrow_mut() = Some(observe(|_| {}));
            });
            deliver("myapp://a");
            deliver("myapp://b");
            assert!(inner.borrow().is_some());
        });
    }
}
