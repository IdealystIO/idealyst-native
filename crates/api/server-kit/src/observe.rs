//! Observability: per-request records emitted by the kit's chain hook.
//!
//! The `around` seam holds everything worth measuring — the matched
//! path, the endpoint's tags, and (crucially) the *outcome and
//! duration* of the whole invocation, chain included. Observers are the
//! open end: register any `Fn(&RequestRecord)` — a logger, a metrics
//! counter, a tracing span emitter. The kit ships [`stderr_logger`] as
//! the zero-dependency default.
//!
//! ```ignore
//! server_kit::install_observer(server_kit::stderr_logger());
//! // or your own:
//! server_kit::install_observer(|r: &server_kit::RequestRecord<'_>| {
//!     metrics::histogram!("srv.duration", r.duration, "path" => r.path.to_string());
//! });
//! ```
//!
//! Scope note: records are **transport-level**. A handler that returns
//! the domain error `Err(ServerError::Failed(...))` still encodes to a
//! 200-with-body, so it reports as `Ok` here — observing domain errors
//! is the app's business (its own observer of its own results). What
//! this layer sees precisely: chain rejections (401/403/429/…), handler
//! transport failures, and successful replies with their size.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use server::{Context, TransportError};

/// What kind of invocation a record describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    /// A unary call (single dispatch or one batch entry).
    Call,
    /// A stream open (`#[channel]` / `#[subscription]` upgrade, `#[sse]`).
    Open,
}

/// Transport-level outcome of one invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The handler ran and produced a reply (`reply_bytes` is its wire
    /// size; 0 for an accepted stream open).
    Ok { reply_bytes: usize },
    /// The chain short-circuited or the handler failed at the transport
    /// level; `status` is the HTTP status the dispatcher will send.
    Error { status: u16 },
}

/// One observed invocation. Borrowed — copy out what you keep.
pub struct RequestRecord<'a> {
    pub path: &'a str,
    /// The endpoint's `tags(...)` metadata.
    pub tags: &'a [(&'static str, &'static str)],
    pub kind: RequestKind,
    pub outcome: Outcome,
    /// Wall time across the whole invocation: chain + extractors +
    /// handler body + encode.
    pub duration: Duration,
}

type Observer = Arc<dyn Fn(&RequestRecord<'_>) + Send + Sync>;

fn registry() -> &'static RwLock<Vec<Observer>> {
    static REGISTRY: OnceLock<RwLock<Vec<Observer>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register an observer; every record is delivered to every observer,
/// in registration order, synchronously on the request path — keep them
/// cheap (log line, counter bump) and hand anything slow to a channel.
pub fn install_observer(f: impl Fn(&RequestRecord<'_>) + Send + Sync + 'static) {
    registry().write().unwrap().push(Arc::new(f));
}

/// The HTTP status the dispatcher will map `e` to (mirrors the
/// primitive's `transport_error_response`).
fn status_of(e: &TransportError) -> u16 {
    match e {
        TransportError::Server { status, .. } => *status,
        TransportError::Codec(_) => 400,
        _ => 500,
    }
}

/// Emit a record for one invocation. Zero work when nothing is
/// installed. Called by the chain hook.
pub(crate) fn report(
    ctx: &Context,
    kind: RequestKind,
    result: &Result<Vec<u8>, TransportError>,
    duration: Duration,
) {
    let observers = registry().read().unwrap().clone();
    if observers.is_empty() {
        return;
    }
    let outcome = match result {
        Ok(bytes) => Outcome::Ok { reply_bytes: bytes.len() },
        Err(e) => Outcome::Error { status: status_of(e) },
    };
    let record = RequestRecord {
        path: ctx.path(),
        tags: ctx.route_tags(),
        kind,
        outcome,
        duration,
    };
    for obs in &observers {
        obs(&record);
    }
}

/// The stock observer: one stderr line per invocation —
/// `[srv] payroll 403 0.4ms (open|call) [role=employer]`.
pub fn stderr_logger() -> impl Fn(&RequestRecord<'_>) + Send + Sync + 'static {
    |r: &RequestRecord<'_>| {
        let status = match r.outcome {
            Outcome::Ok { .. } => 200,
            Outcome::Error { status } => status,
        };
        let kind = match r.kind {
            RequestKind::Call => "call",
            RequestKind::Open => "open",
        };
        let tags = if r.tags.is_empty() {
            String::new()
        } else {
            let joined = r
                .tags
                .iter()
                .map(|(n, v)| if v.is_empty() { n.to_string() } else { format!("{n}={v}") })
                .collect::<Vec<_>>()
                .join(",");
            format!(" [{joined}]")
        };
        eprintln!(
            "[srv] {path} {status} {ms:.1}ms ({kind}){tags}",
            path = r.path,
            ms = r.duration.as_secs_f64() * 1000.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use server::ContextBuilder;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq)]
    struct Seen {
        path: String,
        kind: RequestKind,
        outcome: Outcome,
        has_admin_tag: bool,
    }

    fn capture() -> Arc<Mutex<Vec<Seen>>> {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let sink = seen.clone();
        install_observer(move |r| {
            sink.lock().unwrap().push(Seen {
                path: r.path.to_string(),
                kind: r.kind,
                outcome: r.outcome,
                has_admin_tag: r.has_tag_named("admin"),
            });
        });
        seen
    }

    impl RequestRecord<'_> {
        fn has_tag_named(&self, name: &str) -> bool {
            self.tags.iter().any(|(n, _)| *n == name)
        }
    }

    #[test]
    fn report_delivers_outcomes_and_tags_to_observers() {
        let seen = capture();
        let ctx = ContextBuilder::new()
            .path("payroll")
            .tags(&[("admin", "")])
            .build();

        report(&ctx, RequestKind::Call, &Ok(vec![1, 2, 3]), Duration::from_millis(2));
        report(
            &ctx,
            RequestKind::Open,
            &Err(TransportError::Server { status: 403, message: "no".into() }),
            Duration::from_millis(1),
        );
        report(
            &ctx,
            RequestKind::Call,
            &Err(TransportError::Codec("bad".into())),
            Duration::ZERO,
        );

        let seen = seen.lock().unwrap();
        // Other tests may install observers too; filter to our records.
        let ours: Vec<_> = seen.iter().filter(|s| s.path == "payroll").collect();
        assert_eq!(ours.len(), 3);
        assert_eq!(ours[0].outcome, Outcome::Ok { reply_bytes: 3 });
        assert_eq!(ours[0].kind, RequestKind::Call);
        assert!(ours[0].has_admin_tag);
        assert_eq!(ours[1].outcome, Outcome::Error { status: 403 });
        assert_eq!(ours[1].kind, RequestKind::Open);
        assert_eq!(ours[2].outcome, Outcome::Error { status: 400 }, "codec maps to 400");
    }
}
