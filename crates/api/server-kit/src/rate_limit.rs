//! Token-bucket rate limiting driven by `tags(limit = "...")`.
//!
//! The declaration lives on the endpoint (its self-description); the
//! enforcement lives here; the *key* — what "one caller" means — is the
//! app's vocabulary, registered via [`RateLimit::key_by`]. Same split as
//! `role_guard`: kit machinery, app vocabulary.
//!
//! ```ignore
//! #[server(tags(limit = "30/min"))]
//! async fn send_message(msg: NewMessage, who: Role<Member>) -> Result<(), ServerError> { … }
//!
//! server_kit::install_middleware(
//!     server_kit::rate_limit()
//!         // Key by your principal when signed in…
//!         .key_by(|ctx| ctx.get::<Principal>().map(|p| format!("user:{}", p.username))),
//!         // …falling back to x-forwarded-for, then a per-route global bucket.
//! );
//! ```
//!
//! Semantics:
//! - Limits are per `(route, key)` token buckets with continuous refill:
//!   `"30/min"` = capacity 30, refilling at 30/min — bursts up to the
//!   capacity, sustained rate bounded by it.
//! - A rejected call is **429** with a `Retry-After: <secs>` header
//!   (rounded up), attached via the primitive's response-header jar — on
//!   unary calls, per batch entry (header joins the batch response), and
//!   refused stream opens alike.
//! - A **malformed `limit` tag fails closed**: 500 naming the tag and the
//!   accepted grammar, never an unlimited route.
//!
//! # Where the buckets live: [`LimitStore`]
//!
//! By default in-process ([`MemoryLimitStore`]) — N server instances ≈
//! N× the nominal limit. For shared limits across instances, provide a
//! Redis connection **as app context, exactly like a database**, and
//! point the limiter's store at it (feature `redis`):
//!
//! ```ignore
//! let redis = redis::Client::open(cfg.redis_url)?;
//! server::install_state(redis.clone());     // fns can use it as context too
//! server_kit::install_middleware(
//!     server_kit::rate_limit()
//!         .key_by(…)
//!         .store(server_kit::RedisLimitStore::new(redis)),
//!     // or `.store(server_kit::RedisLimitStore::from_installed())` to
//!     // read the client you already installed.
//! );
//! ```
//!
//! Failure policy: a *config* error (malformed tag) fails **closed**; a
//! *store outage* (Redis down) fails **open** with a stderr warning —
//! rate limiting protects capacity, and a limiter dependency outage must
//! not become a self-inflicted total outage. Override by wrapping the
//! store if your threat model says otherwise.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use server::{Context, ResponseHeaderJar, TransportError};

use crate::server_impl::{Middleware, MiddlewareFuture};

/// A parsed `"N/window"` limit: `capacity` tokens per `window`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    capacity: u32,
    window: Duration,
}

impl Limit {
    /// Parse a `"30/min"`-style spec — the same grammar the `limit` tag
    /// uses. Public so apps and custom [`LimitStore`] tests can build
    /// limits without going through a request.
    pub fn parse(spec: &str) -> Result<Self, String> {
        parse_limit(spec)
    }

    /// Bucket capacity (the burst size).
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The declared window (`"30/min"` → 60s).
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Refill rate in tokens per second.
    pub fn per_second(&self) -> f64 {
        self.capacity as f64 / self.window.as_secs_f64()
    }
}

/// Parse `"30/min"`-style limit declarations. Accepted windows:
/// `sec`/`s`, `min`/`m`, `hour`/`h`, `day`/`d`. Count must be ≥ 1.
pub(crate) fn parse_limit(s: &str) -> Result<Limit, String> {
    let err = || {
        format!(
            "malformed rate limit '{s}': expected \"<count>/<window>\" with window one of \
             sec|s, min|m, hour|h, day|d (e.g. \"30/min\")"
        )
    };
    let (count, window) = s.split_once('/').ok_or_else(err)?;
    let capacity: u32 = count.trim().parse().map_err(|_| err())?;
    if capacity == 0 {
        return Err(err());
    }
    let window = match window.trim() {
        "sec" | "s" => Duration::from_secs(1),
        "min" | "m" => Duration::from_secs(60),
        "hour" | "h" => Duration::from_secs(3600),
        "day" | "d" => Duration::from_secs(86_400),
        _ => return Err(err()),
    };
    Ok(Limit { capacity, window })
}

// ---------------------------------------------------------------------------
// The store seam.
// ---------------------------------------------------------------------------

/// One admit decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Admitted {
    Allowed,
    /// Rejected; `retry_secs` is the (rounded-up) wait until a token is
    /// available — becomes the `Retry-After` header.
    Denied { retry_secs: u64 },
}

/// The store could not answer (backend outage). The limiter fails OPEN
/// on this — see the module docs.
#[derive(Debug)]
pub struct StoreUnavailable(pub String);

/// Future returned by [`LimitStore::admit`].
pub type StoreFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Admitted, StoreUnavailable>> + Send + 'a>>;

/// Where `(route, key)` buckets live. [`MemoryLimitStore`] is the
/// in-process default; `RedisLimitStore` (feature `redis`) shares
/// buckets across instances. Implement this to bring your own backend —
/// the contract is one atomic take-or-report-wait per call.
pub trait LimitStore: Send + Sync + 'static {
    fn admit<'a>(&'a self, route: &'a str, key: &'a str, limit: Limit) -> StoreFuture<'a>;
}

// ---------------------------------------------------------------------------
// In-process store (the default).
// ---------------------------------------------------------------------------

/// One caller's bucket. `tokens` refills continuously toward `capacity`.
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    fn full(limit: Limit, now: Instant) -> Self {
        Self { tokens: limit.capacity as f64, last: now }
    }

    /// Refill for elapsed time, then try to take one token. `Ok(())`
    /// admits the call; `Err(secs)` is the (rounded-up) wait until a
    /// token is available.
    fn admit(&mut self, limit: Limit, now: Instant) -> Result<(), u64> {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * limit.per_second()).min(limit.capacity as f64);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(((1.0 - self.tokens) / limit.per_second()).ceil() as u64)
        }
    }
}

/// Above this many live buckets, idle (full) ones are evicted on insert.
/// A full bucket re-materializes full, so eviction never loosens or
/// tightens anyone's limit.
const BUCKET_SWEEP_THRESHOLD: usize = 10_000;

/// The in-process [`LimitStore`]: per-`(route, key)` buckets in a map.
/// Correct for a single instance; N instances ≈ N× the nominal limit.
#[derive(Default)]
pub struct MemoryLimitStore {
    buckets: Mutex<HashMap<(String, String), Bucket>>,
}

impl MemoryLimitStore {
    /// Synchronous core, factored out for deterministic unit tests on
    /// fabricated instants.
    fn admit_at(&self, route: &str, key: &str, limit: Limit, now: Instant) -> Admitted {
        let mut buckets = self.buckets.lock().unwrap();
        if buckets.len() > BUCKET_SWEEP_THRESHOLD {
            // Drop idle buckets (refilled to capacity) — they carry no
            // state a fresh bucket wouldn't have.
            buckets.retain(|_, b| {
                let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
                (b.tokens + elapsed * limit.per_second()) < limit.capacity as f64
            });
        }
        match buckets
            .entry((route.to_string(), key.to_string()))
            .or_insert_with(|| Bucket::full(limit, now))
            .admit(limit, now)
        {
            Ok(()) => Admitted::Allowed,
            Err(retry_secs) => Admitted::Denied { retry_secs },
        }
    }
}

impl LimitStore for MemoryLimitStore {
    fn admit<'a>(&'a self, route: &'a str, key: &'a str, limit: Limit) -> StoreFuture<'a> {
        let decision = self.admit_at(route, key, limit, Instant::now());
        Box::pin(async move { Ok(decision) })
    }
}

// ---------------------------------------------------------------------------
// The middleware.
// ---------------------------------------------------------------------------

/// Build the rate-limit middleware (in-process store by default — see
/// [`RateLimit::store`]). Install once via
/// [`install_middleware`](crate::install_middleware), after any
/// fact-producing guards (so `key_by` can read the principal).
pub fn rate_limit() -> RateLimit {
    RateLimit {
        keys: Vec::new(),
        default_limit: None,
        store: Arc::new(MemoryLimitStore::default()),
    }
}

/// See [`rate_limit`].
pub struct RateLimit {
    /// App-registered key fns, tried in order; first `Some` wins.
    #[allow(clippy::type_complexity)]
    keys: Vec<Arc<dyn Fn(&Context) -> Option<String> + Send + Sync>>,
    /// Applied to routes with NO `limit` tag when set.
    default_limit: Option<Limit>,
    store: Arc<dyn LimitStore>,
}

impl RateLimit {
    /// Register how to identify "one caller" — e.g. by your principal
    /// type, an API key, or a tenant id. Tried in registration order;
    /// the first `Some` wins. When none matches, the fallback chain is
    /// the first `x-forwarded-for` hop, else one shared per-route
    /// bucket.
    pub fn key_by(
        mut self,
        f: impl Fn(&Context) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.keys.push(Arc::new(f));
        self
    }

    /// Also limit routes that declare NO `limit` tag. Panics at boot on
    /// a malformed spec (misconfiguration fails fast, not at request
    /// time).
    pub fn default_limit(mut self, spec: &str) -> Self {
        self.default_limit =
            Some(parse_limit(spec).unwrap_or_else(|e| panic!("rate_limit default_limit: {e}")));
        self
    }

    /// Replace the bucket store (default: [`MemoryLimitStore`]). Use
    /// `RedisLimitStore` (feature `redis`) for limits shared across
    /// server instances.
    pub fn store(mut self, store: impl LimitStore) -> Self {
        self.store = Arc::new(store);
        self
    }

    fn resolve_key(&self, ctx: &Context) -> String {
        for f in &self.keys {
            if let Some(k) = f(ctx) {
                return k;
            }
        }
        if let Some(ip) = ctx
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return format!("ip:{ip}");
        }
        "global".to_string()
    }
}

impl Middleware for RateLimit {
    fn handle<'a>(&'a self, ctx: &'a mut Context) -> MiddlewareFuture<'a> {
        // Resolve config + key synchronously; only the store I/O is async.
        let limit = match ctx.tag("limit") {
            Some(spec) => match parse_limit(spec) {
                Ok(l) => Some(l),
                // Fail CLOSED: a typo'd limit must never mean "unlimited".
                Err(e) => {
                    return Box::pin(async move {
                        Err(TransportError::Server { status: 500, message: e })
                    });
                }
            },
            None => self.default_limit,
        };
        let Some(limit) = limit else {
            return Box::pin(async { Ok(()) }); // untagged + no default
        };
        let key = self.resolve_key(ctx);
        let jar = ctx.get::<ResponseHeaderJar>();
        let route = ctx.path().to_string();
        Box::pin(async move {
            match self.store.admit(&route, &key, limit).await {
                Ok(Admitted::Allowed) => Ok(()),
                Ok(Admitted::Denied { retry_secs }) => {
                    // Annotate the rejection — the jar rides error
                    // responses (and refused stream opens) by design.
                    if let Some(jar) = jar {
                        jar.append("retry-after", retry_secs.to_string());
                    }
                    Err(TransportError::Server {
                        status: 429,
                        message: format!("rate limit exceeded; retry in {retry_secs}s"),
                    })
                }
                // Store outage: fail OPEN (see module docs) but loudly.
                Err(StoreUnavailable(why)) => {
                    eprintln!(
                        "[server-kit] rate-limit store unavailable ({why}); admitting '{route}' \
                         unlimited until the store recovers"
                    );
                    Ok(())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_the_documented_grammar() {
        assert_eq!(
            parse_limit("30/min").unwrap(),
            Limit { capacity: 30, window: Duration::from_secs(60) }
        );
        assert_eq!(
            parse_limit("2/s").unwrap(),
            Limit { capacity: 2, window: Duration::from_secs(1) }
        );
        assert_eq!(
            parse_limit(" 100 / hour ").unwrap(),
            Limit { capacity: 100, window: Duration::from_secs(3600) }
        );
        assert_eq!(
            parse_limit("1/day").unwrap(),
            Limit { capacity: 1, window: Duration::from_secs(86_400) }
        );
        for bad in ["", "30", "/min", "0/min", "-1/min", "30/fortnight", "x/min"] {
            let err = parse_limit(bad).expect_err(bad);
            assert!(err.contains("malformed rate limit"), "{bad}: {err}");
        }
    }

    /// Deterministic bucket math on fabricated instants — no sleeps.
    #[test]
    fn bucket_bursts_to_capacity_then_refills_at_rate() {
        let limit = parse_limit("2/min").unwrap(); // 1 token / 30s
        let t0 = Instant::now();
        let mut b = Bucket::full(limit, t0);

        // Burst: capacity admits back-to-back calls…
        assert!(b.admit(limit, t0).is_ok());
        assert!(b.admit(limit, t0).is_ok());
        // …then the empty bucket reports the time until one token: 30s.
        assert_eq!(b.admit(limit, t0), Err(30));

        // 29s later: still short (and closer).
        assert_eq!(b.admit(limit, t0 + Duration::from_secs(29)), Err(1));
        // 31s after t0: one token has refilled.
        assert!(b.admit(limit, t0 + Duration::from_secs(31)).is_ok());
        // But only one — the next call must wait again.
        assert!(b.admit(limit, t0 + Duration::from_secs(31)).is_err());

        // A long idle period refills to capacity, not beyond: exactly
        // two calls admitted after an hour.
        let later = t0 + Duration::from_secs(3600);
        assert!(b.admit(limit, later).is_ok());
        assert!(b.admit(limit, later).is_ok());
        assert!(b.admit(limit, later).is_err());
    }

    #[test]
    fn memory_store_isolates_by_route_and_key() {
        let store = MemoryLimitStore::default();
        let limit = parse_limit("1/min").unwrap();
        let now = Instant::now();
        assert_eq!(store.admit_at("send", "user:a", limit, now), Admitted::Allowed);
        // Same route, same key: exhausted.
        assert!(matches!(
            store.admit_at("send", "user:a", limit, now),
            Admitted::Denied { .. }
        ));
        // Same route, different key: fresh bucket.
        assert_eq!(store.admit_at("send", "user:b", limit, now), Admitted::Allowed);
        // Different route, same key: fresh bucket.
        assert_eq!(store.admit_at("other", "user:a", limit, now), Admitted::Allowed);
    }
}
