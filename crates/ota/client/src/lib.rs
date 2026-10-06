//! Over-the-air remote-component bundles for an idealyst app.
//!
//! ```ignore
//! // At startup, inside the app's world (it creates a signal):
//! let ota = ota::start(ota::config!(), ota::Options {
//!     host_fns: my_app::host_fns(),
//!     ..Default::default()
//! })?;
//!
//! // Anywhere in the UI:
//! if ota.status().get() == ota::Status::AppUpdateRequired { /* "Update the app for the latest" */ }
//! ```
//!
//! `ota::config!()` reads the release location from the app's `Cargo.toml`
//! (`[package.metadata.idealyst.ota]`, which `idealyst ota init` writes);
//! `idealyst ota publish` writes releases there. Then:
//!
//! - **At launch** the app runs the bundles it ran last time, from its
//!   cache, at once — or, on first launch and right after an app update,
//!   the bundles built into it ([`Options::built_in`]).
//! - **In the background** it reads the release index and, for each bundle
//!   it runs, downloads the newest release it can run: one whose
//!   requirements (components, props, host functions, types) this app
//!   meets. It runs from the next launch ([`Apply::NextLaunch`], so the
//!   screen never changes under the user), or at once ([`Apply::Now`]).
//! - **A remote component no bundle provides yet** shows an empty place
//!   while its bundle downloads, then mounts: small bundles are fetched the
//!   first time one of their components is shown.
//! - **A release this app can't run** is never downloaded; the app keeps
//!   the newest one it can, and [`status`](Ota::status) says
//!   [`Status::AppUpdateRequired`].
//! - **Signatures**: with `public_keys` configured, only bundles signed by
//!   one of them are run or cached.
//! - **Take-downs** (the console, `idealyst ota rollback`): apps move to the
//!   newest remaining release they can run, when they'd apply any update.
//!   With the **kill switch**, apps running the release replace it at once,
//!   whatever [`Apply`] says — with that release, their built-in copy, or
//!   nothing.
//! - **For a status screen**: [`Ota::bundles`] (each bundle's running and
//!   newest release, and what the updater is doing) and [`Ota::checks`];
//!   [`Options::check_every`] checks on a schedule.
//!
//! The release location is static files (see `ota-index`), so it can be a
//! CDN in front of an S3 prefix, any web server, or a `file://` directory.
//!
//! **How a check gets its answer.** The app's manifest (what it offers
//! bundles: components, props, host functions, types) has an id, a hash
//! of it ([`Ota::manifest_id`]). A check asks, in order:
//!
//! 1. the resolution service, when [`Config::resolver`] is set: the id
//!    first, and the whole manifest only if the service has never seen
//!    it;
//! 2. the location's precomputed answer for this build,
//!    `resolved/<id>.json`, there when the build was registered
//!    (`idealyst ota manifest`);
//! 3. the index itself, deciding on the device.
//!
//! All three are the same decision (`ota_index::resolve`), so the answer
//! doesn't depend on which one gave it; an answer made for another
//! manifest or under another compatibility rule is never used. The loader
//! checks every bundle's signature and requirements again before running
//! it, so an answer can hold an update back but never make the app run a
//! bundle it can't.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ota_index::{resolve, resolved_path, Index, Manifest, Release, Resolution, INDEX_FILE};
use remote_bundle::{Provides, PublicKey, Trust};
use remote_host::remote::RemoteApp;
use runtime_world::{ReadSignal, Signal};
use serde::{Deserialize, Serialize};

pub use ota_index as index;
pub use ota_macros::config;

/// Where an app's releases are, and whose signatures it runs. Built by
/// [`config!`] from `Cargo.toml`.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// The release location: `https://…`, or `file://…` for a directory.
    pub url: &'static str,
    /// The keys (hex) a bundle must be signed by. Empty: signatures aren't
    /// required.
    pub public_keys: &'static [&'static str],
    /// The app's package name: what its cache is kept under.
    pub app: &'static str,
    /// A resolution service to ask first (`resolver` in `Cargo.toml`, or
    /// `IDEALYST_OTA_RESOLVER` when the app is built): an `ota-resolver`
    /// deployment, say. `None`: precomputed answers and
    /// the index only.
    pub resolver: Option<&'static str>,
    /// The app's host functions, declared in `Cargo.toml` (`host_fns =
    /// "path::from::crate::root"`): the list bundles may call. Declared
    /// there, the same list builds the manifest `idealyst ota manifest`
    /// registers and the one the app runs with, so the two can't differ.
    /// `None`: [`Options::host_fns`] gives them.
    pub host_fns: Option<fn() -> Vec<runtime_vocabulary::remote::HostFnDef>>,
}

/// When a downloaded release of a bundle the app is already running
/// replaces it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Apply {
    /// At the next launch: nothing on screen changes under the user.
    #[default]
    NextLaunch,
    /// At once: its components remount from the new release.
    Now,
}

#[derive(Default)]
pub struct Options {
    /// The `#[host_fn]`s bundles may call (the app's allowlist), when
    /// [`Config::host_fns`] doesn't declare them.
    pub host_fns: Vec<runtime_vocabulary::remote::HostFnDef>,
    /// Bundles compiled into the app, by name: what it runs on first launch
    /// (and offline), and right after an app update, until a download
    /// replaces them.
    pub built_in: Vec<(&'static str, &'static [u8])>,
    pub apply: Apply,
    /// Where downloads are kept; the app's private files by default.
    pub cache_dir: Option<PathBuf>,
    /// Check again this long after each check ends. `None`: only at start,
    /// and when [`Ota::check`] is called.
    pub check_every: Option<std::time::Duration>,
}

/// Where one bundle stands ([`Ota::bundles`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleState {
    pub name: String,
    /// What runs now; `None` until it's downloaded.
    pub running: Option<Running>,
    /// The newest release in the index this app can run.
    pub latest: Option<Available>,
    /// The newest release in the index needs a newer app.
    pub newer_needs_app_update: bool,
    pub activity: Activity,
}

/// A release in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    pub sha256: String,
}

/// The release a bundle runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// Its version, as the bundle's release build recorded it (`None` for a
    /// development build, which records none).
    pub version: Option<String>,
    pub sha256: String,
    pub from: Source,
}

/// Where the bundle a [`Running`] describes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Compiled into the app ([`Options::built_in`]).
    BuiltIn,
    /// Downloaded on an earlier launch.
    Cache,
    /// Downloaded on this launch.
    Download,
}

/// What the updater is doing with a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Idle,
    /// Downloading this version.
    Downloading(String),
    /// This version is downloaded and runs from the next launch.
    Ready(String),
    /// The last download or load of it failed.
    Failed(String),
}

/// When the updater checks ([`Ota::checks`]), in seconds since the Unix
/// epoch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Checks {
    /// When the last check ended.
    pub last: Option<u64>,
    /// When the next one starts ([`Options::check_every`]).
    pub next: Option<u64>,
    /// Where the last check's answer came from.
    pub answered_by: Option<AnsweredBy>,
}

/// Where a check's answer came from (see the crate docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsweredBy {
    /// The resolution service ([`Config::resolver`]).
    Service,
    /// The location's precomputed answer for this build.
    Precomputed,
    /// The index, decided on the device.
    Index,
}

/// This app's manifest for `host_fns`: what it offers bundles, under its
/// id. What `idealyst ota manifest` registers (through a generated program
/// that calls this with the declared host functions) and what a check
/// sends a resolution service.
pub fn manifest(host_fns: &[runtime_vocabulary::remote::HostFnDef]) -> Manifest {
    Manifest::new(remote_host::remote::provides(host_fns))
}

/// Where the app's bundles stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Running what it has, and looking for updates.
    Checking,
    /// Running the newest release of every bundle it can.
    UpToDate,
    /// A newer release was downloaded; it runs from the next launch.
    UpdateReady,
    /// A newer release exists that this version of the app can't run: it
    /// needs an app update.
    AppUpdateRequired,
    /// The last check failed (offline, a server error); the app runs what
    /// it has.
    Failed(String),
}

/// The running updater. Cheap to clone.
#[derive(Clone)]
pub struct Ota {
    inner: Rc<Inner>,
}

/// What persists between launches (`state.json` in the cache).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// The app build the bundles were chosen for (a hash of what it
    /// provides). An app update starts over from its built-in bundles.
    app: String,
    /// What each bundle runs at launch: its content hash.
    active: BTreeMap<String, String>,
}

struct Inner {
    config: Config,
    /// [`Options::built_in`]: what the kill switch falls back to.
    built_in: Vec<(&'static str, &'static [u8])>,
    apply: Apply,
    trust: Trust,
    remote: RemoteApp,
    provides: Provides,
    /// `provides.id()`.
    manifest_id: String,
    cache: PathBuf,
    saved: RefCell<Saved>,
    /// The latest answer: what each bundle should run.
    answer: RefCell<Option<Resolution>>,
    status: Signal<Status>,
    /// Components shown before the index arrived.
    waiting: RefCell<BTreeSet<String>>,
    /// Bundles being downloaded.
    fetching: RefCell<BTreeSet<String>>,
    checking: Cell<bool>,
    states: RefCell<BTreeMap<String, BundleState>>,
    bundles: Signal<Vec<BundleState>>,
    checks: Signal<Checks>,
    check_every: Option<std::time::Duration>,
    /// Bumped whenever a check is scheduled: an older timer that fires finds
    /// itself stale and does nothing (a manual check re-arms the timer).
    timer: Cell<u64>,
}

thread_local! {
    static CURRENT: RefCell<Option<Ota>> = const { RefCell::new(None) };
}

/// The updater [`start`] installed on this thread.
pub fn current() -> Option<Ota> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Start: install the bundles the app had, then check for updates. Must
/// run inside the app's world (it creates the status signal). `Err` when a
/// built-in bundle doesn't load — a bug in the app's build, not a network
/// condition.
pub fn start(config: Config, options: Options) -> Result<Ota, String> {
    let host_fns = match config.host_fns {
        None => options.host_fns,
        Some(declared) if options.host_fns.is_empty() => declared(),
        Some(_) => {
            return Err("ota: the host functions are declared in Cargo.toml (`host_fns`); leave `Options::host_fns` empty, so the registered manifest is the one the app runs with".into())
        }
    };
    let provides = remote_host::remote::provides(&host_fns);
    let app_id = provides.id();
    let mut trust = Trust::default();
    for key in config.public_keys {
        trust = trust.key(PublicKey::from_hex(key).map_err(|e| format!("ota: public key {key}: {e}"))?);
    }
    if !config.public_keys.is_empty() {
        trust = trust.require_signature();
    }
    let remote = remote_host::remote::install_empty(remote_host::remote::Options {
        host_fns,
        trust: trust.clone(),
    });
    let cache = match options.cache_dir {
        Some(dir) => dir,
        None => files::app_files(config.app)
            .map_err(|e| format!("ota: {e}"))?
            .local_path("ota")
            .ok_or("ota: this platform has no app directory for the cache")?,
    };
    std::fs::create_dir_all(cache.join("bundles")).map_err(|e| format!("ota: create {}: {e}", cache.display()))?;

    let mut saved: Saved = std::fs::read(cache.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if saved.app != app_id {
        saved = Saved { app: app_id.clone(), active: BTreeMap::new() };
    }
    let answer = std::fs::read(cache.join(ANSWER_FILE)).ok().and_then(|b| Resolution::parse(&b).ok()).filter(|a| a.answers(&app_id));

    // What ran last time, from the cache; a file that's gone, damaged or
    // no longer loads falls back to the built-in copy.
    let mut active = BTreeMap::new();
    let mut states = BTreeMap::new();
    let running = |bytes: &[u8], sha: &str, from| Running {
        version: remote_bundle::metadata(bytes).ok().flatten().map(|m| m.version),
        sha256: sha.to_string(),
        from,
    };
    for (name, sha) in &saved.active {
        let Ok(bytes) = std::fs::read(bundle_path(&cache, sha)) else { continue };
        if remote_bundle::content_hash(&bytes) == *sha && remote.set(name, &bytes).is_ok() {
            active.insert(name.clone(), sha.clone());
            states.insert(name.clone(), BundleState::new(name, Some(running(&bytes, sha, Source::Cache))));
        }
    }
    for (name, bytes) in &options.built_in {
        if !active.contains_key(*name) {
            remote.set(name, bytes).map_err(|e| format!("ota: built-in bundle `{name}`: {e}"))?;
            let sha = remote_bundle::content_hash(bytes);
            states.insert(name.to_string(), BundleState::new(name, Some(running(bytes, &sha, Source::BuiltIn))));
            active.insert(name.to_string(), sha);
        }
    }
    saved.active = active;

    let status = runtime_world::unscoped(|| runtime_world::signal(Status::Checking));
    let bundles = runtime_world::unscoped(|| runtime_world::signal(states.values().cloned().collect::<Vec<_>>()));
    let checks = runtime_world::unscoped(|| runtime_world::signal(Checks::default()));
    let inner = Rc::new(Inner {
        config,
        built_in: options.built_in.clone(),
        apply: options.apply,
        trust,
        remote,
        provides,
        manifest_id: app_id,
        cache,
        saved: RefCell::new(saved),
        answer: RefCell::new(answer),
        status,
        waiting: RefCell::default(),
        fetching: RefCell::default(),
        checking: Cell::new(false),
        states: RefCell::new(states),
        bundles,
        checks,
        check_every: options.check_every,
        timer: Cell::new(0),
    });
    inner.save();
    let weak = Rc::downgrade(&inner);
    inner.remote.on_missing(move |component| {
        let (weak, component) = (weak.clone(), component.to_string());
        // Not inside the mount that asked: setting the bundle remounts.
        runtime_shared::scheduling::after_ms_detached(0, move || {
            if let Some(inner) = weak.upgrade() {
                Inner::missing(&inner, &component);
            }
        });
    });
    let ota = Ota { inner };
    CURRENT.with(|c| *c.borrow_mut() = Some(ota.clone()));
    ota.check();
    Ok(ota)
}

impl Ota {
    /// Where the app's bundles stand.
    pub fn status(&self) -> ReadSignal<Status> {
        self.inner.status.read_only()
    }

    /// Look for updates now (one check at a time; `start` runs one).
    pub fn check(&self) {
        Inner::check(&self.inner);
    }

    /// Where each bundle stands — every bundle the app runs or the index
    /// lists — by name.
    pub fn bundles(&self) -> ReadSignal<Vec<BundleState>> {
        self.inner.bundles.read_only()
    }

    /// When the last check ended, and when the next starts.
    pub fn checks(&self) -> ReadSignal<Checks> {
        self.inner.checks.read_only()
    }

    /// The loader the bundles run in.
    pub fn remote(&self) -> &RemoteApp {
        &self.inner.remote
    }

    /// This build's manifest id: what the release console lists it under.
    pub fn manifest_id(&self) -> &str {
        &self.inner.manifest_id
    }
}

/// The cached answer, for the next launch's first components.
const ANSWER_FILE: &str = "answer.json";

impl BundleState {
    fn new(name: &str, running: Option<Running>) -> BundleState {
        BundleState { name: name.to_string(), running, latest: None, newer_needs_app_update: false, activity: Activity::Idle }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn bundle_path(cache: &Path, sha: &str) -> PathBuf {
    cache.join("bundles").join(format!("{sha}.wasm"))
}

impl Inner {
    fn save(&self) {
        let json = serde_json::to_vec_pretty(&*self.saved.borrow()).expect("state serializes");
        // A cache that can't be written costs the next launch a download,
        // nothing more.
        let _ = std::fs::write(self.cache.join("state.json"), json);
    }

    /// Change bundle `name`'s state, and tell the UI.
    fn state(&self, name: &str, change: impl FnOnce(&mut BundleState)) {
        {
            let mut states = self.states.borrow_mut();
            change(states.entry(name.to_string()).or_insert_with(|| BundleState::new(name, None)));
        }
        self.bundles.set(self.states.borrow().values().cloned().collect());
    }

    /// The check ended: record it, and schedule the next.
    fn checked(this: &Rc<Inner>, answered_by: Option<AnsweredBy>) {
        this.checking.set(false);
        let now = now_secs();
        let next = this.check_every.map(|every| now + every.as_secs().max(1));
        this.checks.set(Checks { last: Some(now), next, answered_by });
        let Some(every) = this.check_every else { return };
        let ticket = this.timer.get() + 1;
        this.timer.set(ticket);
        let weak = Rc::downgrade(this);
        let ms = i32::try_from(every.as_millis()).unwrap_or(i32::MAX);
        runtime_shared::scheduling::after_ms_detached(ms, move || {
            if let Some(this) = weak.upgrade() {
                if this.timer.get() == ticket {
                    Inner::check(&this);
                }
            }
        });
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.config.url.trim_end_matches('/'))
    }

    fn check(this: &Rc<Inner>) {
        if this.checking.replace(true) {
            return;
        }
        this.status.set(Status::Checking);
        let this = this.clone();
        runtime_shared::driver::spawn_async(async move {
            let answered_by = match Inner::answer(&this).await {
                Ok((answer, by)) => {
                    let _ = std::fs::write(this.cache.join(ANSWER_FILE), answer.to_json());
                    *this.answer.borrow_mut() = Some(answer);
                    Inner::update(&this).await;
                    Some(by)
                }
                Err(e) => {
                    this.status.set(Status::Failed(e));
                    None
                }
            };
            Inner::checked(&this, answered_by);
        });
    }

    /// This build's answer: from the service, else the precomputed one,
    /// else decided here from the index. Each step that gives nothing
    /// usable — unreachable, absent, made for another manifest or rule —
    /// falls through to the next; only the index failing fails the check.
    async fn answer(this: &Rc<Inner>) -> Result<(Resolution, AnsweredBy), String> {
        let id = &this.manifest_id;
        if let Some(service) = this.config.resolver {
            if let Ok(answer) = ask(service, id, &this.provides).await {
                if answer.answers(id) {
                    return Ok((answer, AnsweredBy::Service));
                }
            }
        }
        // A CDN answers a missing file with 403 or 404, both just "no".
        if let Ok(bytes) = fetch(&this.url(&resolved_path(id))).await {
            if let Some(answer) = Resolution::parse(&bytes).ok().filter(|a| a.answers(id)) {
                return Ok((answer, AnsweredBy::Precomputed));
            }
        }
        let index = fetch(&this.url(INDEX_FILE)).await.and_then(|bytes| Index::parse(&bytes))?;
        Ok((resolve(&index, &this.provides), AnsweredBy::Index))
    }

    /// Bring every bundle the app runs, and every one a shown component
    /// needs, to the newest release it can run.
    async fn update(this: &Rc<Inner>) {
        let Some(answer) = this.answer.borrow().clone() else { return };
        let waiting: Vec<String> = this.waiting.take().into_iter().collect();
        let wanted: BTreeSet<String> = waiting.iter().filter_map(|c| answer.bundle_for(c).map(str::to_string)).collect();
        let (mut ready, mut needs_app, mut failed) = (false, false, None);
        for choice in answer.choices() {
            this.state(&choice.bundle, |s| {
                s.latest = choice.release.as_ref().map(|r| Available { version: r.version.clone(), sha256: r.sha256.clone() });
                s.newer_needs_app_update = choice.needs_app_update;
            });
            let running = this.remote.bundles().contains(&choice.bundle);
            if !running && !wanted.contains(&choice.bundle) {
                continue; // fetched when one of its components is shown
            }
            needs_app |= choice.needs_app_update;
            // What runs NOW (a downloaded update may be waiting for the next
            // launch), and whether it was taken down with the kill switch.
            let running_sha = this.states.borrow().get(&choice.bundle).and_then(|s| s.running.as_ref().map(|r| r.sha256.clone()));
            let killed = running_sha.as_deref().is_some_and(|sha| answer.killed(&choice.bundle, sha));
            let Some(release) = choice.release else {
                if killed {
                    Inner::kill(this, &choice.bundle);
                }
                continue;
            };
            if running_sha.as_deref() == Some(release.sha256.as_str()) {
                continue;
            }
            if !killed && this.saved.borrow().active.get(&choice.bundle) == Some(&release.sha256) {
                ready = true; // downloaded on an earlier check; runs at the next launch
                continue;
            }
            match Inner::install(this, &choice.bundle, &release, !running || this.apply == Apply::Now || killed).await {
                Ok(applied) => ready |= !applied,
                Err(e) => failed = Some(e),
            }
        }
        this.collect_garbage();
        this.status.set(match failed {
            Some(e) => Status::Failed(e),
            None if needs_app => Status::AppUpdateRequired,
            None if ready => Status::UpdateReady,
            None => Status::UpToDate,
        });
    }

    /// Download `release` of `bundle` (or take it from the cache), check
    /// it, cache it, and make it what the bundle runs from the next launch
    /// — and now, when `now`. `Ok(true)` when it runs now.
    async fn install(this: &Rc<Inner>, bundle: &str, release: &Release, now: bool) -> Result<bool, String> {
        if !this.fetching.borrow_mut().insert(bundle.to_string()) {
            return Ok(false);
        }
        let result = async {
            let path = bundle_path(&this.cache, &release.sha256);
            let (bytes, from) = match std::fs::read(&path) {
                Ok(b) if remote_bundle::content_hash(&b) == release.sha256 => (b, Source::Cache),
                _ => {
                    this.state(bundle, |s| s.activity = Activity::Downloading(release.version.clone()));
                    let b = fetch(&this.url(&release.file)).await?;
                    if remote_bundle::content_hash(&b) != release.sha256 {
                        return Err(format!("`{bundle}` {}: the download doesn't match its hash", release.version));
                    }
                    this.trust.check(&b).map_err(|e| format!("`{bundle}` {}: {e}", release.version))?;
                    std::fs::write(&path, &b).map_err(|e| format!("ota: cache {}: {e}", path.display()))?;
                    (b, Source::Download)
                }
            };
            if now {
                this.remote.set(bundle, &bytes).map_err(|e| format!("`{bundle}` {}: {e}", release.version))?;
                this.state(bundle, |s| {
                    s.running = Some(Running { version: Some(release.version.clone()), sha256: release.sha256.clone(), from });
                    s.activity = Activity::Idle;
                });
            } else {
                this.state(bundle, |s| s.activity = Activity::Ready(release.version.clone()));
            }
            this.saved.borrow_mut().active.insert(bundle.to_string(), release.sha256.clone());
            this.save();
            Ok(now)
        }
        .await;
        this.fetching.borrow_mut().remove(bundle);
        if let Err(e) = &result {
            this.state(bundle, |s| s.activity = Activity::Failed(e.clone()));
        }
        result
    }

    /// The release `bundle` runs was taken down with the kill switch, and the
    /// index has none this app can run in its place: go back to the
    /// built-in copy, or stop running the bundle (its components show an
    /// empty place, as before a first download).
    fn kill(this: &Rc<Inner>, bundle: &str) {
        let killed = this.states.borrow().get(bundle).and_then(|s| s.running.as_ref().map(|r| r.sha256.clone()));
        let built_in = this
            .built_in
            .iter()
            .find(|(name, bytes)| *name == bundle && Some(remote_bundle::content_hash(bytes)) != killed)
            .map(|(_, bytes)| *bytes);
        match built_in.map(|bytes| (bytes, this.remote.set(bundle, bytes))) {
            Some((bytes, Ok(()))) => {
                let sha = remote_bundle::content_hash(bytes);
                this.saved.borrow_mut().active.insert(bundle.to_string(), sha.clone());
                let version = remote_bundle::metadata(bytes).ok().flatten().map(|m| m.version);
                this.state(bundle, |s| s.running = Some(Running { version, sha256: sha, from: Source::BuiltIn }));
            }
            _ => {
                this.remote.remove(bundle);
                this.saved.borrow_mut().active.remove(bundle);
                this.state(bundle, |s| s.running = None);
            }
        }
        this.state(bundle, |s| s.activity = Activity::Failed("taken down with the kill switch".into()));
        this.save();
    }

    /// A shown remote component that no bundle provides: fetch the bundle
    /// that does, or wait for the index to say which.
    fn missing(this: &Rc<Inner>, component: &str) {
        if this.remote.provides(component) {
            return;
        }
        let found = this.answer.borrow().as_ref().and_then(|a| {
            let bundle = a.bundle_for(component)?;
            Some((bundle.to_string(), a.bundles[bundle].release.clone()))
        });
        let Some((bundle, release)) = found else {
            this.waiting.borrow_mut().insert(component.to_string());
            return;
        };
        let Some(release) = release else {
            // Every release of it needs a newer app.
            this.status.set(Status::AppUpdateRequired);
            return;
        };
        let this = this.clone();
        runtime_shared::driver::spawn_async(async move {
            if let Err(e) = Inner::install(&this, &bundle, &release, true).await {
                this.status.set(Status::Failed(e));
            }
        });
    }

    /// Remove cached bundles nothing runs.
    fn collect_garbage(&self) {
        let keep: BTreeSet<String> = self.saved.borrow().active.values().map(|s| format!("{s}.wasm")).collect();
        let Ok(entries) = std::fs::read_dir(self.cache.join("bundles")) else { return };
        for entry in entries.flatten() {
            if !keep.contains(&entry.file_name().to_string_lossy().into_owned()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Ask resolution service `service` for manifest `id`'s answer: by id, and
/// with the whole manifest only when the service doesn't know it (404).
async fn ask(service: &str, id: &str, provides: &Provides) -> Result<Resolution, String> {
    let url = format!("{}/v1/resolve", service.trim_end_matches('/'));
    let post = |body: serde_json::Value| {
        let url = url.clone();
        async move { net::Client::new().post(&url).json(&body).send().await.map_err(|e| format!("ask {url}: {e}")) }
    };
    let mut response = post(serde_json::json!({ "manifest": id })).await?;
    if response.status() == 404 {
        let manifest = Manifest::new(provides.clone());
        response = post(serde_json::json!({ "manifest": id, "provides": manifest.provides })).await?;
    }
    let bytes = response.error_for_status().map_err(|e| format!("ask {url}: {e}"))?.bytes().await.map_err(|e| format!("ask {url}: {e}"))?;
    Resolution::parse(&bytes)
}

/// `url`'s bytes: a `file://` path from disk, anything else over HTTP.
async fn fetch(url: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(|e| format!("read {path}: {e}"));
    }
    net::Client::new()
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("fetch {url}: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("fetch {url}: {e}"))
}
