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
//!
//! The release location is static files (see `ota-index`), so it can be a
//! CDN in front of an S3 prefix, any web server, or a `file://` directory.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ota_index::{choose, Index, Release, INDEX_FILE};
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
    /// The `#[host_fn]`s bundles may call (the app's allowlist).
    pub host_fns: Vec<runtime_vocabulary::remote::HostFnDef>,
    /// Bundles compiled into the app, by name: what it runs on first launch
    /// (and offline), and right after an app update, until a download
    /// replaces them.
    pub built_in: Vec<(&'static str, &'static [u8])>,
    pub apply: Apply,
    /// Where downloads are kept; the app's private files by default.
    pub cache_dir: Option<PathBuf>,
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
    apply: Apply,
    trust: Trust,
    remote: RemoteApp,
    provides: Provides,
    cache: PathBuf,
    saved: RefCell<Saved>,
    index: RefCell<Option<Index>>,
    status: Signal<Status>,
    /// Components shown before the index arrived.
    waiting: RefCell<BTreeSet<String>>,
    /// Bundles being downloaded.
    fetching: RefCell<BTreeSet<String>>,
    checking: Cell<bool>,
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
    let provides = remote_host::remote::provides(&options.host_fns);
    let app_id = remote_bundle::content_hash(&serde_json::to_vec(&provides).expect("provides serializes"));
    let mut trust = Trust::default();
    for key in config.public_keys {
        trust = trust.key(PublicKey::from_hex(key).map_err(|e| format!("ota: public key {key}: {e}"))?);
    }
    if !config.public_keys.is_empty() {
        trust = trust.require_signature();
    }
    let remote = remote_host::remote::install_empty(remote_host::remote::Options {
        host_fns: options.host_fns,
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
        saved = Saved { app: app_id, active: BTreeMap::new() };
    }
    let index = std::fs::read(cache.join(INDEX_FILE)).ok().and_then(|b| Index::parse(&b).ok());

    // What ran last time, from the cache; a file that's gone, damaged or
    // no longer loads falls back to the built-in copy.
    let mut active = BTreeMap::new();
    for (name, sha) in &saved.active {
        let Ok(bytes) = std::fs::read(bundle_path(&cache, sha)) else { continue };
        if remote_bundle::content_hash(&bytes) == *sha && remote.set(name, &bytes).is_ok() {
            active.insert(name.clone(), sha.clone());
        }
    }
    for (name, bytes) in &options.built_in {
        if !active.contains_key(*name) {
            remote.set(name, bytes).map_err(|e| format!("ota: built-in bundle `{name}`: {e}"))?;
            active.insert(name.to_string(), remote_bundle::content_hash(bytes));
        }
    }
    saved.active = active;

    let status = runtime_world::unscoped(|| runtime_world::signal(Status::Checking));
    let inner = Rc::new(Inner {
        config,
        apply: options.apply,
        trust,
        remote,
        provides,
        cache,
        saved: RefCell::new(saved),
        index: RefCell::new(index),
        status,
        waiting: RefCell::default(),
        fetching: RefCell::default(),
        checking: Cell::new(false),
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

    /// The loader the bundles run in.
    pub fn remote(&self) -> &RemoteApp {
        &self.inner.remote
    }
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
            match fetch(&this.url(INDEX_FILE)).await.and_then(|bytes| Index::parse(&bytes).map(|i| (i, bytes))) {
                Ok((index, bytes)) => {
                    let _ = std::fs::write(this.cache.join(INDEX_FILE), bytes);
                    *this.index.borrow_mut() = Some(index);
                    Inner::update(&this).await;
                }
                Err(e) => this.status.set(Status::Failed(e)),
            }
            this.checking.set(false);
        });
    }

    /// Bring every bundle the app runs, and every one a shown component
    /// needs, to the newest release it can run.
    async fn update(this: &Rc<Inner>) {
        let Some(index) = this.index.borrow().clone() else { return };
        let waiting: Vec<String> = this.waiting.take().into_iter().collect();
        let wanted: BTreeSet<String> = waiting.iter().filter_map(|c| index.bundle_for(c).map(str::to_string)).collect();
        let (mut ready, mut needs_app, mut failed) = (false, false, None);
        for choice in choose(&index, &this.provides) {
            let running = this.remote.bundles().contains(&choice.bundle);
            if !running && !wanted.contains(&choice.bundle) {
                continue; // fetched when one of its components is shown
            }
            needs_app |= choice.needs_app_update;
            let Some(release) = choice.release else { continue };
            if this.saved.borrow().active.get(&choice.bundle) == Some(&release.sha256) {
                continue;
            }
            match Inner::install(this, &choice.bundle, &release, !running || this.apply == Apply::Now).await {
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
            let bytes = match std::fs::read(&path) {
                Ok(b) if remote_bundle::content_hash(&b) == release.sha256 => b,
                _ => {
                    let b = fetch(&this.url(&release.file)).await?;
                    if remote_bundle::content_hash(&b) != release.sha256 {
                        return Err(format!("`{bundle}` {}: the download doesn't match its hash", release.version));
                    }
                    this.trust.check(&b).map_err(|e| format!("`{bundle}` {}: {e}", release.version))?;
                    std::fs::write(&path, &b).map_err(|e| format!("ota: cache {}: {e}", path.display()))?;
                    b
                }
            };
            if now {
                this.remote.set(bundle, &bytes).map_err(|e| format!("`{bundle}` {}: {e}", release.version))?;
            }
            this.saved.borrow_mut().active.insert(bundle.to_string(), release.sha256.clone());
            this.save();
            Ok(now)
        }
        .await;
        this.fetching.borrow_mut().remove(bundle);
        result
    }

    /// A shown remote component that no bundle provides: fetch the bundle
    /// that does, or wait for the index to say which.
    fn missing(this: &Rc<Inner>, component: &str) {
        if this.remote.provides(component) {
            return;
        }
        let bundle = match this.index.borrow().as_ref() {
            None => None,
            Some(index) => index.bundle_for(component).map(str::to_string),
        };
        let Some(bundle) = bundle else {
            this.waiting.borrow_mut().insert(component.to_string());
            return;
        };
        let index = this.index.borrow().clone().expect("checked above");
        let Some(release) = choose(&index, &this.provides).into_iter().find(|c| c.bundle == bundle).and_then(|c| c.release)
        else {
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
