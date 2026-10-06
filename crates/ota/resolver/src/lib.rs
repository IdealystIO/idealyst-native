//! The over-the-air resolution service: `POST /v1/resolve`, the protocol
//! `ota`'s client speaks when its `resolver` setting is set.
//!
//! The request is `{"manifest": "<id>"}`. A manifest the location knows —
//! registered from its build, or reported before — is answered from the
//! current index; an unknown one gets `404`, and the app sends
//! `{"manifest": "<id>", "provides": {…}}`. That one is checked against its
//! id (`400` if it doesn't match), answered, and — unless reports are off
//! — stored as reported from the field: its manifest and a marker of its
//! own (`ota_publish::register`), so the next request needs only the id and
//! the release console lists the build.
//!
//! The answer is `ota_index::resolve`, the decision the app would make
//! itself from the index, and the app still checks every bundle before
//! running it: this service can delay an update, never make an app run a
//! bundle it can't. It changes nothing at the location but the reports,
//! and keeps nothing it can't read again: run one instance, many, or a
//! Lambda (`src/bin/lambda.rs`).
//!
//! What it caches, per instance: manifests (immutable under their id), and
//! the index for [`Settings::index_ttl`] — a change reaches apps asking
//! here at most that much later.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use ota_index::{Index, Manifest, ManifestSource, Resolution};
use ota_publish::Target;
use remote_bundle::Provides;
use serde::Deserialize;

/// A request's body.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub manifest: String,
    #[serde(default)]
    pub provides: Option<Provides>,
}

/// What to answer.
#[derive(Debug)]
pub enum Reply {
    Answer(Resolution),
    /// Send the manifest (404).
    Unknown,
    /// The manifest isn't the one its id names (400).
    Refused(String),
}

/// How the service behaves, from the environment ([`Settings::from_env`]).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Store reported manifests (`OTA_RESOLVE_REPORTS`: `keep`, the
    /// default, or `ignore`).
    pub keep_reports: bool,
    /// At most this many reported builds (`OTA_RESOLVE_MAX_REPORTED`, 500).
    pub max_reported: usize,
    /// How long an instance reuses the index it read
    /// (`OTA_RESOLVE_INDEX_TTL_SECS`, 5): a publish or take-down reaches
    /// apps asking here at most this much later.
    pub index_ttl: Duration,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { keep_reports: true, max_reported: 500, index_ttl: Duration::from_secs(5) }
    }
}

impl Settings {
    pub fn from_env() -> Settings {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let d = Settings::default();
        Settings {
            keep_reports: var("OTA_RESOLVE_REPORTS").map_or(d.keep_reports, |v| v != "ignore"),
            max_reported: var("OTA_RESOLVE_MAX_REPORTED").and_then(|v| v.parse().ok()).unwrap_or(d.max_reported),
            index_ttl: var("OTA_RESOLVE_INDEX_TTL_SECS").and_then(|v| v.parse().ok()).map_or(d.index_ttl, Duration::from_secs),
        }
    }
}

/// The service's state: the location, and what it has read.
pub struct Resolver {
    target: Target,
    settings: Settings,
    /// Manifests read or received, by id: immutable, so kept for good.
    known: Mutex<HashMap<String, Provides>>,
    /// The index, and when it was read.
    index: Mutex<Option<(Instant, Arc<Index>)>>,
}

impl Resolver {
    pub fn new(target: Target, settings: Settings) -> Resolver {
        Resolver { target, settings, known: Mutex::default(), index: Mutex::default() }
    }

    /// From the environment: `OTA_LOCATION` (`s3://bucket/prefix` with the
    /// AWS_* variables, or a directory) and [`Settings::from_env`].
    pub fn from_env() -> Result<Resolver> {
        let location = std::env::var("OTA_LOCATION").map_err(|_| {
            anyhow!("set OTA_LOCATION to the release location (s3://bucket/prefix, with the AWS_* variables, or a directory)")
        })?;
        let target = ota_publish::connect(&location).with_context(|| format!("OTA_LOCATION={location}"))?;
        Ok(Resolver::new(target, Settings::from_env()))
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The index, read again once it is older than the TTL.
    fn index(&self) -> Result<Arc<Index>> {
        if let Some((at, index)) = &*self.index.lock().expect("not poisoned") {
            if at.elapsed() < self.settings.index_ttl {
                return Ok(index.clone());
            }
        }
        let index = Arc::new(ota_publish::read_index(&self.target)?);
        *self.index.lock().expect("not poisoned") = Some((Instant::now(), index.clone()));
        Ok(index)
    }

    /// Answer one request. Blocking: it may read the location.
    pub fn reply(&self, request: Request) -> Result<Reply> {
        let provides = match request.provides {
            Some(provides) => {
                let manifest = Manifest { rule: remote_bundle::RULE, id: request.manifest.clone(), provides };
                if let Err(why) = manifest.verify() {
                    return Ok(Reply::Refused(why));
                }
                if self.settings.keep_reports {
                    // Full, or the store unwritable: still answered.
                    let stored = ota_publish::register(
                        &self.target,
                        &manifest,
                        ManifestSource::Reported,
                        None,
                        Some(self.settings.max_reported),
                    );
                    if let Err(e) = stored {
                        eprintln!("ota-resolver: a reported manifest wasn't stored: {e:#}");
                    }
                }
                self.known.lock().expect("not poisoned").insert(manifest.id, manifest.provides.clone());
                manifest.provides
            }
            None => {
                let cached = self.known.lock().expect("not poisoned").get(&request.manifest).cloned();
                match cached {
                    Some(p) => p,
                    None => match ota_publish::read_manifest(&self.target, &request.manifest)? {
                        None => return Ok(Reply::Unknown),
                        Some(m) => {
                            self.known.lock().expect("not poisoned").insert(m.id, m.provides.clone());
                            m.provides
                        }
                    },
                }
            }
        };
        let index = self.index()?;
        Ok(Reply::Answer(ota_index::resolve(&index, &provides)))
    }
}

/// The largest request accepted: a manifest is tens of KB.
const BODY_LIMIT: usize = 1 << 20;

/// `POST /v1/resolve`, and `GET /health` for a load balancer.
pub fn router(resolver: Arc<Resolver>) -> axum::Router {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let handle = move |axum::Json(request): axum::Json<Request>| {
        let resolver = resolver.clone();
        async move {
            match tokio::task::spawn_blocking(move || resolver.reply(request)).await {
                Ok(Ok(Reply::Answer(a))) => ([(axum::http::header::CONTENT_TYPE, "application/json")], a.to_json()).into_response(),
                Ok(Ok(Reply::Unknown)) => (StatusCode::NOT_FOUND, "{\"error\":\"unknown-manifest\"}").into_response(),
                Ok(Ok(Reply::Refused(why))) => (StatusCode::BAD_REQUEST, why).into_response(),
                Ok(Err(e)) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
    };
    axum::Router::new()
        .route("/v1/resolve", axum::routing::post(handle))
        .route("/health", axum::routing::get(|| async { "ok" }))
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Provides {
        let mut p = Provides { codec: 2, ..Default::default() };
        p.components.insert("ui::Card".into(), Default::default());
        p
    }

    fn ask(r: &Resolver, with: bool) -> Reply {
        r.reply(Request { manifest: app().id(), provides: with.then(app) }).unwrap()
    }

    /// An unknown id asks for the manifest; the manifest is checked against
    /// its id, answered and stored as reported (unless reports are off), so
    /// a fresh instance answers the id alone; the answer is `resolve`'s.
    #[test]
    fn answers_by_id_once_it_has_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let target = Target::Dir(dir.path().into());
        let quiet = Resolver::new(target.clone(), Settings { keep_reports: false, ..Settings::default() });
        assert!(matches!(ask(&quiet, false), Reply::Unknown));
        assert!(matches!(ask(&quiet, true), Reply::Answer(_)));
        assert!(matches!(ask(&quiet, false), Reply::Answer(_)), "kept in memory");
        assert!(ota_publish::read_registry(&target).unwrap().manifests.is_empty(), "reports off: nothing stored");

        let keeping = Resolver::new(target.clone(), Settings::default());
        let Reply::Answer(a) = ask(&keeping, true) else { panic!() };
        assert_eq!(a, ota_index::resolve(&ota_publish::read_index(&target).unwrap(), &app()));
        assert_eq!(ota_publish::read_registry(&target).unwrap().manifests[0].source, ManifestSource::Reported);
        let fresh = Resolver::new(target.clone(), Settings::default());
        assert!(matches!(ask(&fresh, false), Reply::Answer(_)), "read from the location");

        let mut other = app();
        other.codec = 3;
        let forged = fresh.reply(Request { manifest: app().id(), provides: Some(other) }).unwrap();
        assert!(matches!(forged, Reply::Refused(_)));
    }

    /// Within the TTL an instance reuses the index it read; past it, it
    /// reads the change.
    #[test]
    fn the_index_is_reused_for_its_ttl_only() {
        let dir = tempfile::tempdir().unwrap();
        let target = Target::Dir(dir.path().into());
        let index = |generation: u64| {
            let mut i = Index::new();
            i.generation = generation;
            std::fs::write(dir.path().join(ota_index::INDEX_FILE), i.to_json()).unwrap();
        };
        let generation = |r: &Resolver| match ask(r, true) {
            Reply::Answer(a) => a.generation,
            other => panic!("{other:?}"),
        };
        index(1);
        let cached = Resolver::new(target.clone(), Settings { index_ttl: Duration::from_secs(3600), ..Settings::default() });
        assert_eq!(generation(&cached), 1);
        index(2);
        assert_eq!(generation(&cached), 1, "still the one it read");
        let uncached = Resolver::new(target, Settings { index_ttl: Duration::ZERO, ..Settings::default() });
        assert_eq!(generation(&uncached), 2);
    }
}
