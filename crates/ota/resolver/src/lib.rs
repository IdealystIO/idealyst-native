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
//! The id is checked before anything is read: it must have the form
//! `Provides::id` gives (`ota_index::is_manifest_id`, 64 lowercase hex), or
//! the request is refused (`400`). It names a file at the location
//! (`manifests/<id>.json`), so an id like `../index` must never reach a
//! read.
//!
//! What it caches, per instance: the manifests the location keeps
//! (immutable under their id; see [`Resolver`] for why only those), and
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
    /// Manifests the location keeps, by id: immutable, so kept for good.
    ///
    /// Only ones the location stores — read from it, or stored by this
    /// instance — never one merely received. Anyone can post a manifest,
    /// so caching every one would let a client grow this map without end;
    /// the stored ones are bounded by what the location keeps (reports
    /// capped at [`Settings::max_reported`], builds registered by whoever
    /// publishes), each at most a request body. A manifest that wasn't
    /// stored (reports off, the cap reached, the store failing) is
    /// answered and forgotten: its app gets a `404` next time and sends
    /// it again. A count cap with eviction would bound it as well, but
    /// would also evict the registered builds the cache exists for.
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
        if !ota_index::is_manifest_id(&request.manifest) {
            return Ok(Reply::Refused(format!(
                "`{}` isn't a manifest id (64 lowercase hex characters, `Provides::id`)",
                request.manifest.chars().take(80).collect::<String>()
            )));
        }
        let provides = match request.provides {
            Some(provides) => {
                let manifest = Manifest { rule: remote_bundle::RULE, id: request.manifest.clone(), provides };
                if let Err(why) = manifest.verify() {
                    return Ok(Reply::Refused(why));
                }
                // Full, or the store unwritable: still answered, but not
                // cached (see `known`).
                let stored = self.settings.keep_reports
                    && match ota_publish::register(
                        &self.target,
                        &manifest,
                        ManifestSource::Reported,
                        None,
                        Some(self.settings.max_reported),
                    ) {
                        Ok(_) => true,
                        Err(e) => {
                            eprintln!("ota-resolver: a reported manifest wasn't stored: {e:#}");
                            false
                        }
                    };
                if stored {
                    self.known.lock().expect("not poisoned").insert(manifest.id, manifest.provides.clone());
                }
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
        assert!(matches!(ask(&quiet, false), Reply::Unknown), "reports off: answered, not kept");
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

    /// Regression: the id named a file (`manifests/<id>.json`) unchecked,
    /// so on a directory location `../` walked out of it — here, to a
    /// valid manifest planted beside the store, which was read and
    /// answered. Now any id not of `Provides::id`'s form is refused before
    /// a read: the store below is a FILE, so any read of it would fail
    /// with an error instead of a refusal.
    #[test]
    fn regression_manifest_id_path_traversal_refused_before_any_read() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("store/manifests")).unwrap();
        let planted = Manifest::new(app());
        std::fs::write(root.path().join("planted.json"), planted.to_json()).unwrap();
        let store = Resolver::new(Target::Dir(root.path().join("store")), Settings::default());
        let outside = store.reply(Request { manifest: "../../planted".into(), provides: None }).unwrap();
        assert!(matches!(outside, Reply::Refused(_)), "read outside the store: {outside:?}");

        let not_a_dir = root.path().join("not-a-dir");
        std::fs::write(&not_a_dir, b"").unwrap();
        let untouchable = Resolver::new(Target::Dir(not_a_dir), Settings::default());
        assert!(untouchable.reply(Request { manifest: app().id(), provides: None }).is_err(), "a read here fails");
        let id = app().id();
        for bad in ["../index".to_string(), "abc".into(), id.to_uppercase(), format!("{id}/../{id}"), String::new()] {
            for provides in [None, Some(app())] {
                match untouchable.reply(Request { manifest: bad.clone(), provides }) {
                    Ok(Reply::Refused(why)) => assert!(why.contains("isn't a manifest id"), "{why}"),
                    other => panic!("`{bad}`: {other:?}"),
                }
            }
        }
    }

    /// Regression: every manifest posted was cached for good, stored or
    /// not, so a client posting distinct valid manifests grew the
    /// instance's memory without bound. Only the ones the location keeps
    /// are cached: reports off, or past `max_reported`, nothing is.
    #[test]
    fn regression_unstored_reports_are_not_cached() {
        let manifest = |codec: u32| Provides { codec, ..Default::default() };
        let post = |r: &Resolver, p: Provides| r.reply(Request { manifest: p.id(), provides: Some(p) }).unwrap();
        let cached = |r: &Resolver| r.known.lock().unwrap().len();

        let dir = tempfile::tempdir().unwrap();
        let quiet = Resolver::new(Target::Dir(dir.path().into()), Settings { keep_reports: false, ..Settings::default() });
        for codec in 0..50 {
            assert!(matches!(post(&quiet, manifest(codec)), Reply::Answer(_)));
        }
        assert_eq!(cached(&quiet), 0, "reports off: nothing kept");

        let dir = tempfile::tempdir().unwrap();
        let capped = Resolver::new(Target::Dir(dir.path().into()), Settings { max_reported: 3, ..Settings::default() });
        for codec in 0..50 {
            assert!(matches!(post(&capped, manifest(codec)), Reply::Answer(_)), "past the cap: still answered");
        }
        assert_eq!(cached(&capped), 3, "only the three stored");
        let by_id = |codec: u32| capped.reply(Request { manifest: manifest(codec).id(), provides: None }).unwrap();
        assert!(matches!(by_id(0), Reply::Answer(_)));
        assert!(matches!(by_id(49), Reply::Unknown), "not stored: asked for again");
    }
}
