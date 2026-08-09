//! Query parameters — the durable half of a screen's navigation state.
//!
//! # Why this is separate from `RouteParams`
//!
//! [`RouteParams`](super::shared::RouteParams) fills the `:placeholder`
//! segments of a route pattern: it is part of the route's *identity*, so
//! `/items/5` and `/items/6` are different screens and switching between
//! them mounts a new subtree.
//!
//! Query params are the opposite: they describe how the *same* screen is
//! configured — which tab is open, what the list is filtered to, what the
//! user typed in the search box. Two URLs differing only in their query
//! resolve to the same route, the same cache key, and the same mounted
//! screen. Nothing remounts when the query changes.
//!
//! That distinction is load-bearing. Folding the query into the routing
//! axis would make every filter toggle tear down and rebuild the screen it
//! is filtering, which is both slow and visibly wrong (scroll position and
//! focus would reset on each keystroke).
//!
//! # Why query params rather than an opaque payload
//!
//! A screen's initial state can arrive from two directions:
//!
//! 1. An in-app navigation — `handle.push_with_state(route, params, state)`.
//! 2. A cold load: the user pasted a URL, hit reload, or restored a tab.
//!
//! Only a *serializable* channel can serve both. An opaque `Rc<dyn Any>`
//! rides case 1 fine and evaporates in case 2, which forces every screen to
//! carry a second, divergent "but what if there's no state" path. Encoding
//! the state as query params means one representation covers both: the
//! navigation writes it, the URL stores it, and a cold load parses the same
//! bytes back into the same struct.
//!
//! On platforms with no address bar (iOS, Android, desktop, terminal) there
//! is nowhere durable to store it, so the URL round-trip is a no-op there —
//! but the in-memory path is byte-for-byte identical, so a screen reads its
//! state exactly the same way on every backend. That uniformity is the
//! point (project rule 7): backends differ in what they can *persist*, not
//! in what the author writes.
//!
//! # Encoding
//!
//! `application/x-www-form-urlencoded`, hand-rolled rather than pulled from
//! a crate: `runtime-shared` is a core dependency of every backend
//! including wasm, and this is ~40 lines of table-free ASCII work. Space
//! encodes as `+` on the way out and decodes from both `+` and `%20` on the
//! way in, matching what browsers put in `location.search`.
//!
//! Order is preserved. A `HashMap` here would make the URL for a given
//! state non-deterministic, which breaks SSR/premint golden comparisons and
//! makes browser history entries compare unequal at random.

use std::fmt;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Percent-coding
// ---------------------------------------------------------------------------

/// Unreserved per RFC 3986 plus the sub-delims browsers leave alone in a
/// query. Everything else is percent-escaped.
fn is_query_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'*')
}

fn encode_into(out: &mut String, s: &str) {
    for &b in s.as_bytes() {
        if is_query_safe(b) {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push(char::from_digit((b >> 4) as u32, 16).expect("nibble").to_ascii_uppercase());
            out.push(char::from_digit((b & 0xf) as u32, 16).expect("nibble").to_ascii_uppercase());
        }
    }
}

/// Percent-decode one query component. Invalid escapes are passed through
/// literally rather than dropped — a malformed URL from the address bar
/// should degrade to a weird-looking value, never to a panic or a silently
/// truncated string.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    // Lossy: a hand-edited URL can carry bytes that aren't valid UTF-8, and
    // the replacement char is a better outcome than refusing to route.
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// QueryParams
// ---------------------------------------------------------------------------

/// An ordered set of `key=value` query parameters.
///
/// Insertion order is preserved and duplicate keys are allowed on parse
/// (the raw URL may contain them), but [`set`](Self::set) replaces in place
/// so round-tripping a typed state never accumulates duplicates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryParams {
    entries: Vec<(String, String)>,
}

impl QueryParams {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a query string, with or without a leading `?`. An empty or
    /// all-separator input yields an empty set. Keys with no `=` map to an
    /// empty value, matching browser behavior for `?debug`.
    pub fn parse(query: &str) -> Self {
        let query = query.strip_prefix('?').unwrap_or(query);
        let entries = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((k, v)) => (decode(k), decode(v)),
                None => (decode(pair), String::new()),
            })
            .filter(|(k, _)| !k.is_empty())
            .collect();
        Self { entries }
    }

    /// Serialize to a query string with NO leading `?`. Empty when there
    /// are no entries.
    pub fn to_query_string(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.entries {
            if !out.is_empty() {
                out.push('&');
            }
            encode_into(&mut out, k);
            out.push('=');
            encode_into(&mut out, v);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// First value for `key`, or `None`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Parse the value for `key` as `T`. `None` when the key is absent OR
    /// the value doesn't parse — a garbled URL falls back to the screen's
    /// default rather than failing the navigation.
    pub fn get_as<T: FromStr>(&self, key: &str) -> Option<T> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    /// Every value for `key`, in order (for genuinely repeated keys such as
    /// `?tag=a&tag=b`).
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.entries.iter().filter(move |(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Set `key` to `value`, replacing the first existing entry in place
    /// (preserving its position) or appending.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        let key = key.into();
        let value = value.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
        self
    }

    /// Append `key=value` without replacing an existing entry.
    pub fn push(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.entries.push((key.into(), value.into()));
        self
    }

    /// Builder form of [`set`](Self::set), for `QueryParams::new().with(..)`
    /// chains inside a `to_query` impl.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.set(key, value);
        self
    }

    /// Builder form that skips `None` — the idiomatic way to encode an
    /// optional field without emitting `?filter=`.
    pub fn with_opt(self, key: impl Into<String>, value: Option<impl ToString>) -> Self {
        match value {
            Some(v) => self.with(key, v.to_string()),
            None => self,
        }
    }

    /// Remove every entry for `key`; returns whether anything was removed.
    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(k, _)| k != key);
        before != self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl fmt::Display for QueryParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_query_string())
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for QueryParams {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self { entries: iter.into_iter().map(|(k, v)| (k.into(), v.into())).collect() }
    }
}

// ---------------------------------------------------------------------------
// URL splitting / joining
// ---------------------------------------------------------------------------

/// Split a URL into its path and its parsed query.
///
/// Every routing decision in the navigator substrate runs on the path half
/// ONLY. This is called at the boundaries where a URL enters the framework
/// (a cold-start deep link, a browser `popstate`, an author-supplied route
/// path) so that no query string ever reaches `match_prefix`, the nav-base
/// publication, or a screen cache key. A `?` leaking into any of those
/// silently corrupts the value it lands in: `match_prefix("/items/5?tab=a",
/// "/items/:id")` binds `id` to the string `5?tab=a`.
///
/// A fragment (`#…`) is dropped: it is client-side-only by definition and
/// the navigator never routes on it.
pub fn split_query(url: &str) -> (&str, QueryParams) {
    let url = url.split('#').next().unwrap_or(url);
    match url.split_once('?') {
        Some((path, query)) => (path, QueryParams::parse(query)),
        None => (url, QueryParams::new()),
    }
}

/// The path half of a URL — [`split_query`] discarding the query. Use at
/// call sites that only need to route.
pub fn strip_query(url: &str) -> &str {
    split_query(url).0
}

/// Recompose `path` and `query` into a URL. An empty query yields `path`
/// unchanged (never a bare trailing `?`, which would make two equivalent
/// URLs compare unequal in browser history).
pub fn with_query(path: &str, query: &QueryParams) -> String {
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{}?{}", path, query.to_query_string())
    }
}

// ---------------------------------------------------------------------------
// ScreenState
// ---------------------------------------------------------------------------

/// A screen's durable initial state, encoded as query parameters.
///
/// Implement this on the struct a screen wants pre-populated, then hand it
/// to `NavHandle::push_with_state` / `select_with_state` / … at the
/// navigation site and read it back with
/// [`screen_state`](super::shared::screen_state) inside the screen builder.
/// The same value arrives whether the screen was reached by an in-app
/// navigation or by a cold load of the URL that navigation produced.
///
/// [`from_query`](Self::from_query) returns `Option` so a hand-edited or
/// truncated URL degrades to the screen's own default instead of panicking;
/// prefer filling missing fields with defaults over returning `None`, and
/// reserve `None` for a query that is genuinely for a different shape.
///
/// ```ignore
/// struct Filters { tab: String, archived: bool }
///
/// impl ScreenState for Filters {
///     fn to_query(&self) -> QueryParams {
///         QueryParams::new()
///             .with("tab", &self.tab)
///             .with("archived", self.archived.to_string())
///     }
///     fn from_query(q: &QueryParams) -> Option<Self> {
///         Some(Filters {
///             tab: q.get("tab").unwrap_or("all").to_string(),
///             archived: q.get_as("archived").unwrap_or(false),
///         })
///     }
/// }
/// ```
pub trait ScreenState: 'static + Sized {
    fn to_query(&self) -> QueryParams;
    fn from_query(query: &QueryParams) -> Option<Self>;
}

/// The no-state case — `push` is `push_with_state(.., ())`.
impl ScreenState for () {
    fn to_query(&self) -> QueryParams {
        QueryParams::new()
    }
    fn from_query(_: &QueryParams) -> Option<Self> {
        Some(())
    }
}

/// Untyped passthrough, for a screen that reads raw keys rather than
/// declaring a struct.
impl ScreenState for QueryParams {
    fn to_query(&self) -> QueryParams {
        self.clone()
    }
    fn from_query(query: &QueryParams) -> Option<Self> {
        Some(query.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_serializes_round_trip() {
        let q = QueryParams::parse("?tab=notes&page=3");
        assert_eq!(q.get("tab"), Some("notes"));
        assert_eq!(q.get_as::<u32>("page"), Some(3));
        assert_eq!(q.to_query_string(), "tab=notes&page=3");
    }

    #[test]
    fn preserves_insertion_order() {
        // A HashMap here would make the emitted URL non-deterministic,
        // breaking SSR goldens and browser-history equality.
        let q = QueryParams::new().with("z", "1").with("a", "2").with("m", "3");
        assert_eq!(q.to_query_string(), "z=1&a=2&m=3");
    }

    #[test]
    fn percent_codes_reserved_characters() {
        let q = QueryParams::new().with("q", "a&b=c d/e?f");
        let s = q.to_query_string();
        assert_eq!(s, "q=a%26b%3Dc+d%2Fe%3Ff");
        assert_eq!(QueryParams::parse(&s).get("q"), Some("a&b=c d/e?f"));
    }

    #[test]
    fn decodes_space_from_both_plus_and_hex() {
        assert_eq!(QueryParams::parse("a=x+y").get("a"), Some("x y"));
        assert_eq!(QueryParams::parse("a=x%20y").get("a"), Some("x y"));
    }

    #[test]
    fn round_trips_non_ascii() {
        let q = QueryParams::new().with("name", "café ☕");
        assert_eq!(QueryParams::parse(&q.to_query_string()).get("name"), Some("café ☕"));
    }

    #[test]
    fn malformed_escapes_pass_through_instead_of_panicking() {
        // A hand-edited URL must degrade, never abort.
        assert_eq!(QueryParams::parse("a=100%").get("a"), Some("100%"));
        assert_eq!(QueryParams::parse("a=%zz").get("a"), Some("%zz"));
    }

    #[test]
    fn valueless_key_parses_as_empty() {
        let q = QueryParams::parse("debug&tab=x");
        assert_eq!(q.get("debug"), Some(""));
        assert_eq!(q.get("tab"), Some("x"));
    }

    #[test]
    fn empty_forms_are_empty() {
        assert!(QueryParams::parse("").is_empty());
        assert!(QueryParams::parse("?").is_empty());
        assert!(QueryParams::parse("&&").is_empty());
    }

    #[test]
    fn set_replaces_in_place_and_push_appends() {
        let mut q = QueryParams::new().with("a", "1").with("b", "2");
        q.set("a", "9");
        assert_eq!(q.to_query_string(), "a=9&b=2");
        q.push("a", "10");
        assert_eq!(q.get_all("a").collect::<Vec<_>>(), vec!["9", "10"]);
        assert!(q.remove("a"));
        assert_eq!(q.to_query_string(), "b=2");
    }

    #[test]
    fn with_opt_skips_none() {
        let q = QueryParams::new().with_opt("a", Some(1)).with_opt("b", None::<i32>);
        assert_eq!(q.to_query_string(), "a=1");
    }

    #[test]
    fn get_as_returns_none_for_unparseable() {
        let q = QueryParams::parse("n=banana");
        assert_eq!(q.get_as::<u32>("n"), None);
    }

    #[test]
    fn split_query_separates_path_from_query() {
        let (path, q) = split_query("/items/5?tab=notes");
        assert_eq!(path, "/items/5");
        assert_eq!(q.get("tab"), Some("notes"));

        let (path, q) = split_query("/items/5");
        assert_eq!(path, "/items/5");
        assert!(q.is_empty());
    }

    #[test]
    fn split_query_drops_the_fragment() {
        let (path, q) = split_query("/docs?section=api#heading");
        assert_eq!(path, "/docs");
        assert_eq!(q.get("section"), Some("api"));
        assert_eq!(strip_query("/docs#heading"), "/docs");
    }

    #[test]
    fn with_query_omits_the_separator_when_empty() {
        assert_eq!(with_query("/a", &QueryParams::new()), "/a");
        assert_eq!(with_query("/a", &QueryParams::new().with("b", "1")), "/a?b=1");
    }

    #[test]
    fn url_round_trips_through_split_and_join() {
        let url = "/items/5?tab=notes&q=hello+world";
        let (path, q) = split_query(url);
        assert_eq!(with_query(path, &q), url);
    }

    #[test]
    fn screen_state_round_trips() {
        #[derive(Debug, PartialEq)]
        struct Filters {
            tab: String,
            archived: bool,
        }
        impl ScreenState for Filters {
            fn to_query(&self) -> QueryParams {
                QueryParams::new()
                    .with("tab", self.tab.clone())
                    .with("archived", self.archived.to_string())
            }
            fn from_query(q: &QueryParams) -> Option<Self> {
                Some(Filters {
                    tab: q.get("tab").unwrap_or("all").to_string(),
                    archived: q.get_as("archived").unwrap_or(false),
                })
            }
        }

        let original = Filters { tab: "notes".into(), archived: true };
        let decoded = Filters::from_query(&original.to_query()).expect("round trip");
        assert_eq!(original, decoded);

        // A cold load with a partial query falls back to per-field defaults
        // rather than losing the whole state.
        let partial = Filters::from_query(&QueryParams::parse("tab=labs")).expect("partial");
        assert_eq!(partial, Filters { tab: "labs".into(), archived: false });
    }
}
