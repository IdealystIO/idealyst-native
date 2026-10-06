//! The over-the-air console: one page to manage a release location.
//!
//! The server holds the bucket's credentials (see README.md) and runs the
//! release operations of `ota-publish` — the same ones the CLI runs — so a
//! console action and a CLI publish at the same moment can't overwrite each
//! other (both write the index conditionally). The page:
//!
//! - lists each bundle's releases in the order apps choose them: live,
//!   pinned, the rest, with what each newly requires;
//! - takes any release down, optionally with the kill switch (apps running
//!   it replace it at once instead of at their next launch), with a reason;
//! - restores a taken-down release; pins one ahead of newer ones; unpins;
//! - lists the registered app builds and what each runs, and, before a
//!   take-down, which of them it moves and which it leaves with nothing;
//! - shows the audit log (the console's actions and the CLI's publishes).
//!
//! Apps never talk to the console. The service they can ask is
//! `ota-resolver` (`crates/ota/resolver`), deployed on its own.
//!
//! Publishing stays in the CLI and CI: signing keys never reach the console,
//! so it can rearrange signed releases but never ship code.

#![cfg_attr(feature = "server", allow(dead_code, unused_imports))]

extern crate runtime_core;

use std::rc::Rc;

use idea_ui::{tone, typography_kind, variant, Alert, Badge, Button, Card, Field, Modal, Switch, ToneRef, Tooltip, Typography, VariantRef};
use runtime_core::{component, signal, ui, Element, ReadSignal, Signal};
use serde::{Deserialize, Serialize};
use server::{server, ServerError};

// ===========================================================================
// What crosses between the page and the server.
// ===========================================================================

/// A release location, as the page shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    /// Where (`s3://bucket/prefix at <endpoint>`).
    pub location: String,
    pub bundles: Vec<BundleView>,
    /// The audit log, most recent first.
    pub audit: Vec<EventView>,
    /// The registered app builds, and what each runs.
    pub builds: Vec<BuildView>,
}

/// A registered app build.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildView {
    /// Its manifest id.
    pub id: String,
    pub label: Option<String>,
    /// Captured from its build (`idealyst ota manifest`), not only seen in
    /// the field.
    pub from_build: bool,
    pub registered_at: u64,
    pub bundles: Vec<BuildBundle>,
}

/// What one app build does with one bundle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildBundle {
    pub bundle: String,
    /// The live releases it can run (`sha256`, version), in the order apps
    /// choose: the first is what it runs.
    pub runnable: Vec<(String, String)>,
    /// The version it runs, if any.
    pub runs: Option<String>,
    /// Why it can't run each release ahead of the one it runs:
    /// `1.3.0: the app has no host function `shop::checkout``.
    pub blocked: Vec<String>,
    /// Every live release, in the order apps choose: whether this build can
    /// run it, and why not (the compatibility grid).
    pub cells: Vec<CellView>,
}

/// One build against one release.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CellView {
    pub sha256: String,
    pub version: String,
    pub runs: bool,
    /// The release this build chooses (the first it can run).
    pub chosen: bool,
    /// Why it can't, one line per problem.
    pub reasons: Vec<String>,
    /// What differs without stopping it (a context type it may not read).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BundleView {
    pub name: String,
    /// In the order apps choose them: the first is live.
    pub releases: Vec<ReleaseView>,
    pub withdrawn: Vec<WithdrawnView>,
    pub pinned: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReleaseView {
    pub version: String,
    pub sha256: String,
    pub size: u64,
    pub published: u64,
    pub signed_by: Option<String>,
    /// The remote components it provides.
    pub components: Vec<String>,
    /// What it requires that the release published before it didn't.
    pub new_requirements: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WithdrawnView {
    pub release: ReleaseView,
    pub withdrawn_at: u64,
    pub urgent: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventView {
    pub at: u64,
    pub actor: String,
    /// `publish`, `take-down`, `kill-switch`, `restore`, `pin`, `unpin`.
    pub action: String,
    pub bundle: String,
    pub version: String,
    pub sha256: String,
    pub reason: Option<String>,
}

// ===========================================================================
// The server.
// ===========================================================================

/// The release location the server manages, installed at boot
/// (`src/bin/server.rs`) — or why there is none. The server starts either
/// way and the page says what to set: a server that exits at startup over
/// configuration is one line in `idealyst dev`'s output, and a page that
/// never loads.
#[cfg(feature = "server")]
pub struct Console {
    pub target: Result<ota_publish::Target, String>,
}

/// How to configure the console, for the page to show when it isn't.
pub const SETUP: &str = "Set OTA_CONSOLE_LOCATION to the release location (s3://bucket/prefix, or a directory) and the AWS_* variables for it (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_REGION; AWS_ENDPOINT_URL for MinIO), then restart the console.";

/// Who the console's actions are recorded as. No sign-in yet: the console is
/// self-hosted for one team.
#[cfg(feature = "server")]
const ACTOR: &str = "console";

/// Run a release operation on a blocking thread: they wait on the bucket.
#[cfg(feature = "server")]
async fn on_bucket<T: Send + 'static>(
    work: impl FnOnce(&ota_publish::Target) -> anyhow::Result<T> + Send + 'static,
) -> Result<T, ServerError> {
    let console = server::use_state::<std::sync::Arc<Console>>().ok_or_else(|| ServerError::failed("the console has no release location"))?;
    let target = console.target.clone().map_err(|why| ServerError::failed(format!("{why} {SETUP}")))?;
    tokio::task::spawn_blocking(move || work(&target))
        .await
        .map_err(|e| ServerError::failed(format!("the operation stopped: {e}")))?
        .map_err(|e| ServerError::failed(format!("{e:#}")))
}

/// One registered build's view: what it runs of each bundle in `index`,
/// and why not the releases ahead of that.
#[cfg(feature = "server")]
fn build_view(entry: &ota_index::Registered, provides: &remote_bundle::Provides, index: &ota_index::Index) -> BuildView {
    let answer = ota_index::resolve(index, provides);
    let bundles = answer
        .bundles
        .iter()
        .map(|(name, r)| {
            let runs_at = r.verdicts.iter().position(ota_index::Verdict::runs).unwrap_or(r.verdicts.len());
            BuildBundle {
                bundle: name.clone(),
                runnable: r.verdicts.iter().filter(|v| v.runs()).map(|v| (v.sha256.clone(), v.version.clone())).collect(),
                runs: r.release.as_ref().map(|rel| rel.version.clone()),
                cells: r
                    .verdicts
                    .iter()
                    .enumerate()
                    .map(|(i, v)| CellView {
                        sha256: v.sha256.clone(),
                        version: v.version.clone(),
                        runs: v.runs(),
                        chosen: i == runs_at,
                        reasons: v.problems.iter().filter(|p| p.is_error()).map(|p| p.to_string()).collect(),
                        warnings: v.problems.iter().filter(|p| !p.is_error()).map(|p| p.to_string()).collect(),
                    })
                    .collect(),
                blocked: r.verdicts[..runs_at]
                    .iter()
                    .map(|v| {
                        let errors: Vec<String> = v.problems.iter().filter(|p| p.is_error()).map(|p| p.to_string()).collect();
                        format!("{}: {}", v.version, errors.join("; "))
                    })
                    .collect(),
            }
        })
        .collect();
    BuildView {
        id: entry.id.clone(),
        label: entry.label.clone(),
        from_build: entry.source == ota_index::ManifestSource::Build,
        registered_at: entry.registered_at,
        bundles,
    }
}

#[cfg(feature = "server")]
fn release_view(r: &ota_index::Release, before: Option<&ota_index::Release>) -> ReleaseView {
    ReleaseView {
        version: r.version.clone(),
        sha256: r.sha256.clone(),
        size: r.size,
        published: r.published,
        signed_by: r.signed_by.clone(),
        components: r.components().map(str::to_string).collect(),
        new_requirements: before.map(|b| ota_index::new_requirements(&b.requires, &r.requires)).unwrap_or_default(),
    }
}

/// Everything at the release location.
#[server]
pub async fn overview() -> Result<Overview, ServerError> {
    on_bucket(|target| {
        let index = ota_publish::read_index(target)?;
        let audit = ota_publish::read_audit(target)?;
        let bundles = index
            .bundles
            .iter()
            .map(|(name, b)| {
                // "Newly requires" compares each release with the one
                // published just before it, live or not.
                let mut by_time: Vec<&ota_index::Release> = b.releases.iter().chain(b.withdrawn.iter().map(|w| &w.release)).collect();
                by_time.sort_by_key(|r| r.published);
                let before = |r: &ota_index::Release| {
                    let i = by_time.iter().position(|x| x.sha256 == r.sha256)?;
                    i.checked_sub(1).map(|j| by_time[j])
                };
                BundleView {
                    name: name.clone(),
                    releases: b.releases.iter().map(|r| release_view(r, before(r))).collect(),
                    withdrawn: b
                        .withdrawn
                        .iter()
                        .map(|w| WithdrawnView {
                            release: release_view(&w.release, before(&w.release)),
                            withdrawn_at: w.withdrawn_at,
                            urgent: w.urgent,
                            reason: w.reason.clone(),
                        })
                        .collect(),
                    pinned: b.pinned.clone(),
                }
            })
            .collect();
        let audit = audit
            .into_iter()
            .take(100)
            .map(|e| EventView {
                at: e.at,
                actor: e.actor,
                action: serde_json_action(e.action),
                bundle: e.bundle,
                version: e.version,
                sha256: e.sha256,
                reason: e.reason,
            })
            .collect();
        let mut builds = Vec::new();
        for entry in &ota_publish::read_registry(target)?.manifests {
            if let Some(m) = ota_publish::read_manifest(target, &entry.id)? {
                builds.push(build_view(entry, &m.provides, &index));
            }
        }
        Ok(Overview { location: target.to_string(), bundles, audit, builds })
    })
    .await
}

#[cfg(feature = "server")]
fn serde_json_action(a: ota_publish::Action) -> String {
    match a {
        ota_publish::Action::Publish => "publish",
        ota_publish::Action::TakeDown => "take-down",
        ota_publish::Action::KillSwitch => "kill-switch",
        ota_publish::Action::Restore => "restore",
        ota_publish::Action::Pin => "pin",
        ota_publish::Action::Unpin => "unpin",
    }
    .to_string()
}

/// Take release `sha256` of `bundle` down; `urgent` is the kill switch.
#[server]
pub async fn take_down(bundle: String, sha256: String, urgent: bool, reason: Option<String>) -> Result<(), ServerError> {
    on_bucket(move |t| {
        let reason = reason.filter(|r| !r.trim().is_empty());
        ota_publish::take_down(t, &bundle, &sha256, ota_publish::TakeDown { urgent, reason }, ACTOR).map(drop)
    })
    .await
}

/// Put a taken-down release back.
#[server]
pub async fn restore(bundle: String, sha256: String) -> Result<(), ServerError> {
    on_bucket(move |t| ota_publish::restore(t, &bundle, &sha256, ACTOR).map(drop)).await
}

/// Serve release `sha256` of `bundle` ahead of newer ones.
#[server]
pub async fn pin(bundle: String, sha256: String) -> Result<(), ServerError> {
    on_bucket(move |t| ota_publish::pin(t, &bundle, &sha256, ACTOR)).await
}

/// Serve `bundle`'s newest release again.
#[server]
pub async fn unpin(bundle: String) -> Result<(), ServerError> {
    on_bucket(move |t| ota_publish::unpin(t, &bundle, ACTOR)).await
}

// ===========================================================================
// The page.
// ===========================================================================

/// `secs` since the Unix epoch as `YYYY-MM-DD HH:MM UTC`.
pub fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let rem = secs % 86_400;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rem / 3_600, rem % 3_600 / 60)
}

fn short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

fn build_name(b: &BuildView) -> String {
    b.label.clone().unwrap_or_else(|| format!("build {}", short(&b.id)))
}

/// How one cell of the compatibility grid reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// It runs this release: the first it can.
    Runs,
    /// It could run it, but runs one ahead of it.
    Could,
    Cannot,
    /// The build's answer doesn't list it (registered before it existed and
    /// not re-read since): shown as unknown, never guessed.
    Unknown,
}

/// One cell: its fit and the sentence that explains it.
#[derive(Debug, Clone, PartialEq)]
struct GridCell {
    fit: Fit,
    detail: String,
}

/// Build `b`'s row of `bundle`'s grid, one cell per release in `columns`
/// (`sha256`, version), in the order given.
fn grid_row(b: &BuildView, bundle: &str, columns: &[(String, String)]) -> Vec<GridCell> {
    let mine = b.bundles.iter().find(|x| x.bundle == bundle);
    let name = build_name(b);
    columns
        .iter()
        .map(|(sha, version)| {
            let Some(cell) = mine.and_then(|m| m.cells.iter().find(|c| &c.sha256 == sha)) else {
                return GridCell { fit: Fit::Unknown, detail: format!("{name}: no verdict for {version} yet — refresh") };
            };
            let warn = if cell.warnings.is_empty() { String::new() } else { format!(" (warning: {})", cell.warnings.join("; ")) };
            match (cell.runs, cell.chosen) {
                (true, true) => GridCell { fit: Fit::Runs, detail: format!("{name} runs {version} ({}){warn}", short(sha)) },
                (true, false) => GridCell {
                    fit: Fit::Could,
                    detail: format!("{name} can run {version} ({}), and runs a release ahead of it{warn}", short(sha)),
                },
                (false, _) => GridCell {
                    fit: Fit::Cannot,
                    detail: format!("{name} can't run {version} ({}): {}", short(sha), cell.reasons.join("; ")),
                },
            }
        })
        .collect()
}

/// What taking release `sha256` of `bundle` down does to the registered
/// builds: each one running it, and where it goes.
fn impact(builds: &[BuildView], bundle: &str, sha256: &str) -> Vec<String> {
    builds
        .iter()
        .filter_map(|b| {
            let mine = b.bundles.iter().find(|x| x.bundle == bundle)?;
            if mine.runnable.first().map(|(sha, _)| sha.as_str()) != Some(sha256) {
                return None;
            }
            Some(match mine.runnable.get(1) {
                Some((sha, version)) => format!("{} moves to {version} ({})", build_name(b), short(sha)),
                None => format!("{} is left with nothing it can run", build_name(b)),
            })
        })
        .collect()
}

/// What a take-down is about to do, for the confirmation dialog.
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    bundle: String,
    release: ReleaseView,
    /// It is the live release (apps run it now).
    live: bool,
    /// The release apps go to instead, if any.
    next: Option<String>,
    /// What it does to each registered build running it ([`impact`]).
    impact: Vec<String>,
}

/// The page's state and how it reaches the server.
#[derive(Clone, Copy)]
struct Page {
    overview: Signal<Option<Overview>>,
    bundles: Signal<Vec<BundleView>>,
    audit: Signal<Vec<EventView>>,
    builds: Signal<Vec<BuildView>>,
    error: Signal<Option<String>>,
    busy: Signal<bool>,
    pending: Signal<Option<Pending>>,
}

#[cfg(not(feature = "server"))]
thread_local! {
    /// The page's own lifetime, taken while the root is built: what its
    /// actions' results are tied to ([`Page::act`]).
    static PAGE_ALIVE: std::cell::RefCell<Option<runtime_core::ScopeAlive>> = const { std::cell::RefCell::new(None) };
}

#[cfg(not(feature = "server"))]
fn page() -> Page {
    runtime_core::inject::<Page>().expect("the console provides its Page")
}

#[cfg(not(feature = "server"))]
impl Page {
    fn new() -> Page {
        Page {
            overview: signal(None),
            bundles: signal(Vec::new()),
            audit: signal(Vec::new()),
            builds: signal(Vec::new()),
            error: signal(None),
            busy: signal(false),
            pending: signal(None),
        }
    }

    fn refresh(self) {
        Page::on_page(|| runtime_core::spawn_then(overview(), move |r| self.loaded(r)));
    }

    /// Start async work whose result belongs to the PAGE, not to the
    /// element that asked. `spawn_then` ties a result to the scope it was
    /// started in — from a click handler, the clicked element — and drops
    /// it if that scope is gone. The take-down dialog closes as soon as it
    /// confirms, so its result (and the page's refresh) was dropped and
    /// the page stayed on "Working…". Anchored to the page's root instead.
    fn on_page(start: impl FnOnce()) {
        match PAGE_ALIVE.with(|a| a.borrow().clone()) {
            Some(alive) => alive.run_within(start),
            None => start(),
        }
    }

    fn loaded(self, r: Result<Overview, ServerError>) {
        match r {
            Ok(o) => {
                self.bundles.set(o.bundles.clone());
                self.audit.set(o.audit.clone());
                self.builds.set(o.builds.clone());
                self.overview.set(Some(o));
                self.error.set(None);
            }
            Err(e) => self.error.set(Some(format!("Couldn't read the release location: {}", server_message(&e)))),
        }
    }

    /// Run an action, then reload everything from the bucket.
    fn act(self, what: &'static str, call: impl std::future::Future<Output = Result<(), ServerError>> + 'static) {
        self.busy.set(true);
        Page::on_page(|| {
            runtime_core::spawn_then(
                async move {
                    let done = call.await;
                    (done, overview().await)
                },
                move |(done, fresh)| {
                    self.busy.set(false);
                    self.loaded(fresh);
                    if let Err(e) = done {
                        self.error.set(Some(format!("{what} failed: {}", server_message(&e))));
                    }
                },
            )
        });
    }
}

/// A server error's own words, without the error type's framing.
fn server_message(e: &ServerError) -> String {
    match e {
        ServerError::Failed(m) => m.clone(),
        other => other.to_string(),
    }
}

/// The console page.
#[cfg(not(feature = "server"))]
pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    configure_server();
    ui! { Console() }
}

#[cfg(not(feature = "server"))]
fn configure_server() {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_else(|| "http://127.0.0.1:3100".into());
        server::configure(server::ClientConfig::new(origin));
    }
    #[cfg(not(target_arch = "wasm32"))]
    server::configure(server::ClientConfig::new("http://127.0.0.1:3100"));
}

fn px(v: f32) -> Option<runtime_core::Tokenized<runtime_core::Length>> {
    Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(v)))
}

fn column(gap: f32) -> runtime_core::StyleRules {
    runtime_core::StyleRules {
        flex_direction: Some(runtime_core::FlexDirection::Column),
        gap: px(gap),
        ..Default::default()
    }
}

fn row(gap: f32) -> runtime_core::StyleRules {
    runtime_core::StyleRules {
        flex_direction: Some(runtime_core::FlexDirection::Row),
        align_items: Some(runtime_core::AlignItems::Center),
        flex_wrap: Some(runtime_core::FlexWrap::Wrap),
        gap: px(gap),
        ..Default::default()
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn Console() -> Element {
    // The page's state lives in this component, and so do its actions'
    // results: the token is taken HERE, in the component's own build, not
    // in `app()`, whose surrounding scope is the host's business (under
    // `idealyst dev` it doesn't last as long as the page, and every result
    // was dropped: the page sat on "Loading…").
    let page = Page::new();
    PAGE_ALIVE.with(|a| *a.borrow_mut() = Some(runtime_core::ScopeAlive::current()));
    // The components below reach the page's state and actions through context.
    runtime_core::provide(page);
    page.refresh();
    let (overview, error, busy) = (page.overview, page.error, page.busy);
    ui! {
        scroll_view(style = runtime_core::StyleRules {
            background: Some(runtime_core::Tokenized::token("color-background", runtime_core::Color("#f6f7f9".into()))),
            flex_grow: Some(runtime_core::Tokenized::Literal(1.0)),
            ..Default::default()
        }) {
            view(style = runtime_core::StyleRules {
                padding_top: px(28.0), padding_bottom: px(28.0), padding_left: px(28.0), padding_right: px(28.0),
                max_width: px(980.0),
                ..column(18.0)
            }) {
                view(style = row(12.0)) {
                    view(style = runtime_core::StyleRules { flex_grow: Some(runtime_core::Tokenized::Literal(1.0)), ..column(2.0) }) {
                        Typography(content = "Over-the-air releases".to_string(), kind = typography_kind::H2)
                        text(style = runtime_core::StyleRules { color: Some(runtime_core::Tokenized::Literal(runtime_core::Color("#6b7280".into()))), ..Default::default() }) {
                            move || overview.get().map_or("Loading…".to_string(), |o| o.location)
                        }
                    }
                    Button(
                        label = runtime_core::rx!(if busy.get() { "Working…".to_string() } else { "Refresh".to_string() }),
                        on_click = Rc::new(move || page.refresh()) as Rc<dyn Fn()>,
                        variant = variant::Outlined
                    )
                }
                if error.get().is_some() {
                    Alert(title = runtime_core::rx!(error.get().unwrap_or_default()), tone = tone::Danger)
                }
                if overview.get().is_some_and(|o| o.bundles.is_empty()) {
                    Card() {
                        Typography(content = "Nothing published here yet. `idealyst ota publish` publishes an app's bundles.".to_string())
                    }
                }
                for bundle in page.bundles, key = format!("{bundle:?}") {
                    BundleCard(bundle = bundle)
                }
                AppBuilds()
                AuditLog(events = page.audit.read_only())
                TakeDownDialog()
            }
        }
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn BundleCard(#[prop(static)] bundle: BundleView) -> Element {
    let page = page();
    let live = bundle.releases.first().cloned();
    let headline = match &live {
        Some(r) => format!("live: {} ({})", r.version, short(&r.sha256)),
        None => "no live release — apps keep what they run".to_string(),
    };
    let headline_tone = if live.is_some() { ToneRef::from(tone::Success) } else { ToneRef::from(tone::Warning) };
    let name = bundle.name.clone();
    let rows: Vec<(usize, ReleaseView, bool, Option<String>)> = bundle
        .releases
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let pinned = bundle.pinned.as_deref() == Some(r.sha256.as_str());
            (i, r.clone(), pinned, bundle.releases.get(i + 1).map(|n| format!("{} ({})", n.version, short(&n.sha256))))
        })
        .collect();
    let withdrawn = bundle.withdrawn.clone();
    let columns: Vec<(String, String)> = bundle.releases.iter().map(|r| (r.sha256.clone(), r.version.clone())).collect();
    let has_columns = !columns.is_empty();
    let grid_name = name.clone();
    let has_pin = bundle.pinned.is_some();
    let has_withdrawn = !withdrawn.is_empty();
    let unpin_name = name.clone();
    let withdrawn_name = name.clone();
    let on_unpin = Rc::new(move || page.act("Unpinning", unpin(unpin_name.clone()))) as Rc<dyn Fn()>;
    ui! {
        Card() {
            view(style = row(10.0)) {
                Typography(content = format!("Bundle `{name}`"), kind = typography_kind::H3)
                Badge(label = headline, tone = headline_tone)
                if has_pin {
                    Badge(label = "pinned".to_string(), tone = tone::Info)
                    Button(label = "Unpin".to_string(), on_click = on_unpin.clone(), variant = variant::Ghost)
                }
            }
            for (i, r, pinned, next) in rows {
                ReleaseRow(bundle = name.clone(), release = r, position = i, pinned = pinned, next = next)
            }
            if has_columns {
                CompatGrid(bundle = grid_name.clone(), columns = columns.clone())
            }
            if has_withdrawn {
                Typography(content = "Taken down".to_string(), kind = typography_kind::H3)
                for w in withdrawn.clone() {
                    WithdrawnRow(bundle = withdrawn_name.clone(), withdrawn = w)
                }
            }
        }
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn ReleaseRow(
    #[prop(static)] bundle: String,
    #[prop(static)] release: ReleaseView,
    #[prop(static)] position: usize,
    #[prop(static)] pinned: bool,
    #[prop(static)] next: Option<String>,
) -> Element {
    let page = page();
    let r = release.clone();
    let detail = release_detail(&release);
    let (pin_bundle, pin_sha) = (bundle.clone(), release.sha256.clone());
    let pending = Pending { bundle, release: release.clone(), live: position == 0, next, impact: Vec::new() };
    ui! {
        view(style = runtime_core::StyleRules {
            padding_top: px(10.0), padding_bottom: px(10.0),
            border_top_width: Some(runtime_core::Tokenized::Literal(1.0)),
            border_top_color: Some(runtime_core::Tokenized::token("color-border", runtime_core::Color("#e5e7eb".into()))),
            ..column(4.0)
        }) {
            view(style = row(8.0)) {
                text(style = runtime_core::StyleRules { font_weight: Some(runtime_core::FontWeight::Bold), ..Default::default() }) { format!("{} · {}", r.version, short(&r.sha256)) }
                if position == 0 {
                    Badge(label = "live".to_string(), tone = tone::Success)
                }
                if pinned {
                    Badge(label = "pinned".to_string(), tone = tone::Info)
                }
            }
            text(style = runtime_core::StyleRules { color: Some(runtime_core::Tokenized::Literal(runtime_core::Color("#4b5563".into()))), ..Default::default() }) { detail }
            view(style = row(8.0)) {
                if !pinned {
                    Button(
                        label = "Pin".to_string(),
                        on_click = Rc::new(move || page.act("Pinning", pin(pin_bundle.clone(), pin_sha.clone()))) as Rc<dyn Fn()>,
                        variant = variant::Outlined
                    )
                }
                Button(
                    label = "Take down…".to_string(),
                    on_click = Rc::new(move || {
                        let builds = runtime_core::untrack(|| page.builds.get());
                        let impact = impact(&builds, &pending.bundle, &pending.release.sha256);
                        page.pending.set(Some(Pending { impact, ..pending.clone() }));
                    }) as Rc<dyn Fn()>,
                    tone = tone::Danger,
                    variant = variant::Outlined
                )
            }
        }
    }
}

fn release_detail(r: &ReleaseView) -> String {
    let mut parts = vec![
        format!("published {}", utc(r.published)),
        format!("{:.1} KB", r.size as f64 / 1024.0),
        r.signed_by.as_ref().map_or("unsigned".to_string(), |k| format!("signed by {k}")),
    ];
    if !r.components.is_empty() {
        parts.push(format!("provides {}", r.components.join(", ")));
    }
    let mut out = parts.join(" · ");
    if !r.new_requirements.is_empty() {
        out.push_str(&format!("\nnewly requires: {}", r.new_requirements.join("; ")));
    }
    out
}

#[cfg(not(feature = "server"))]
#[component]
fn WithdrawnRow(#[prop(static)] bundle: String, #[prop(static)] withdrawn: WithdrawnView) -> Element {
    let page = page();
    let w = withdrawn.clone();
    let how = if w.urgent { "with the kill switch" } else { "" };
    let why = w.reason.as_ref().map_or(String::new(), |r| format!(" — {r}"));
    let line = format!("{} · {} · taken down {} {how}{why}", w.release.version, short(&w.release.sha256), utc(w.withdrawn_at));
    let sha = w.release.sha256.clone();
    ui! {
        view(style = row(10.0)) {
            if w.urgent {
                Badge(label = "killed".to_string(), tone = tone::Danger)
            }
            text(style = runtime_core::StyleRules { flex_grow: Some(runtime_core::Tokenized::Literal(1.0)), ..Default::default() }) { line }
            Button(
                label = "Restore".to_string(),
                on_click = Rc::new(move || page.act("Restoring", restore(bundle.clone(), sha.clone()))) as Rc<dyn Fn()>,
                variant = variant::Outlined
            )
        }
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn TakeDownDialog() -> Element {
    let page = page();
    let pending = page.pending;
    let urgent = signal(false);
    let reason = signal(String::new());
    let close = Rc::new(move || {
        pending.set(None);
        urgent.set(false);
        reason.set(String::new());
    }) as Rc<dyn Fn()>;
    let cancel = close.clone();
    let after = close.clone();
    let confirm = Rc::new(move || {
        let Some(p) = pending.get() else { return };
        let why = Some(reason.get());
        page.act("Taking down", take_down(p.bundle, p.release.sha256, urgent.get(), why));
        after();
    }) as Rc<dyn Fn()>;
    let explain = move || match pending.get() {
        None => String::new(),
        Some(p) if !p.live => format!(
            "{} isn't live: apps don't choose it now unless they can't run the newer releases. Taking it down removes it from their choices.",
            p.release.version
        ),
        Some(p) => match p.next {
            Some(next) => format!("Apps go to {next}. Without the kill switch, they switch at their next launch (or at once, if they apply updates immediately)."),
            None => "It's the only release: without the kill switch apps keep running it; with it, they drop it (or return to their built-in copy).".to_string(),
        },
    };
    let affected = move || match pending.get() {
        None => String::new(),
        Some(p) if p.impact.is_empty() => "No registered app build runs it.".to_string(),
        Some(p) => format!("Registered app builds running it: {}.", p.impact.join("; ")),
    };
    ui! {
        Modal(
            open = runtime_core::rx!(pending.get().is_some()),
            on_dismiss = Some(close.clone()),
            content = move || {
                let title = pending.get().map_or(String::new(), |p| format!("Take down `{}` {}?", p.bundle, p.release.version));
                let explain = explain.clone();
                let affected = affected.clone();
                let (cancel, confirm) = (cancel.clone(), confirm.clone());
                ui! {
                    view(style = column(14.0)) {
                        Typography(content = title, kind = typography_kind::H3)
                        Typography(content = explain())
                        Typography(content = affected(), muted = true)
                        Switch(
                            label = Some("Kill switch: apps running it replace it at once".to_string()),
                            value = urgent,
                            on_change = Rc::new(move |v| urgent.set(v)) as Rc<dyn Fn(bool)>
                        )
                        Field(
                            label = Some("Reason (for the audit log)".to_string()),
                            value = reason,
                            on_change = Rc::new(move |v| reason.set(v)) as Rc<dyn Fn(String)>,
                            placeholder = Some("e.g. crashes on launch".to_string())
                        )
                        view(style = row(8.0)) {
                            Button(label = "Cancel".to_string(), on_click = cancel, variant = variant::Ghost)
                            Button(
                                label = runtime_core::rx!(if urgent.get() { "Kill it now".to_string() } else { "Take down".to_string() }),
                                on_click = confirm,
                                tone = tone::Danger
                            )
                        }
                    }
                }
            }
        )
    }
}

/// Width of the grid's first column (the build's name), and of each cell.
const GRID_NAME_WIDTH: f32 = 200.0;
const GRID_CELL_WIDTH: f32 = 104.0;

/// `bundle`'s compatibility grid: each registered build against each live
/// release, in the order apps choose them. A cell's tooltip — or, tapped,
/// the line under the grid — says why.
#[cfg(not(feature = "server"))]
#[component]
fn CompatGrid(#[prop(static)] bundle: String, #[prop(static)] columns: Vec<(String, String)>) -> Element {
    let builds = page().builds;
    ui! {
        view(style = column(6.0)) {
            Typography(content = "Which app builds can run each release".to_string(), kind = typography_kind::H3)
            if builds.get().is_empty() {
                Typography(content = "No app builds registered: `idealyst ota manifest` registers one.".to_string(), muted = true)
            }
            if !builds.get().is_empty() {
                GridTable(bundle = bundle.clone(), columns = columns.clone())
            }
        }
    }
}

/// The grid itself: a header of releases, a row per build, and the
/// selected cell's reason.
#[cfg(not(feature = "server"))]
#[component]
fn GridTable(#[prop(static)] bundle: String, #[prop(static)] columns: Vec<(String, String)>) -> Element {
    let builds = page().builds;
    let selected = signal(None::<String>);
    let headers: Vec<(String, String)> = columns.iter().map(|(sha, version)| (short(sha), version.clone())).collect();
    ui! {
        view(style = column(6.0)) {
            scroll_view(style = runtime_core::StyleRules {
                flex_direction: Some(runtime_core::FlexDirection::Row),
                ..Default::default()
            }, horizontal = true) {
                view(style = column(4.0)) {
                    view(style = runtime_core::StyleRules { flex_direction: Some(runtime_core::FlexDirection::Row), gap: px(4.0), ..Default::default() }) {
                        view(style = runtime_core::StyleRules { width: px(GRID_NAME_WIDTH), ..Default::default() }) {}
                        for (sha, version) in headers {
                            view(style = runtime_core::StyleRules { width: px(GRID_CELL_WIDTH), ..column(0.0) }) {
                                text(style = runtime_core::StyleRules { font_weight: Some(runtime_core::FontWeight::Bold), ..Default::default() }) { version }
                                text(style = runtime_core::StyleRules { color: Some(runtime_core::Tokenized::Literal(runtime_core::Color("#6b7280".into()))), ..Default::default() }) { sha }
                            }
                        }
                    }
                    for b in builds, key = format!("{b:?}") {
                        GridRow(build = b, bundle = bundle.clone(), columns = columns.clone(), selected = selected)
                    }
                }
            }
            text(style = runtime_core::StyleRules { color: Some(runtime_core::Tokenized::Literal(runtime_core::Color("#374151".into()))), ..Default::default() }) {
                move || selected.get().unwrap_or_else(|| "Hover or tap a cell for the reason.".to_string())
            }
        }
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn GridRow(
    #[prop(static)] build: BuildView,
    #[prop(static)] bundle: String,
    #[prop(static)] columns: Vec<(String, String)>,
    #[prop(static)] selected: Signal<Option<String>>,
) -> Element {
    let name = build_name(&build);
    let cells = grid_row(&build, &bundle, &columns);
    ui! {
        view(style = runtime_core::StyleRules {
            flex_direction: Some(runtime_core::FlexDirection::Row),
            align_items: Some(runtime_core::AlignItems::Center),
            gap: px(4.0),
            ..Default::default()
        }) {
            view(style = runtime_core::StyleRules { width: px(GRID_NAME_WIDTH), ..Default::default() }) {
                text() { name }
            }
            for cell in cells {
                GridCellView(cell_fit = FitProp(cell.fit), detail = cell.detail, selected = selected)
            }
        }
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn GridCellView(#[prop(static)] cell_fit: FitProp, #[prop(static)] detail: String, #[prop(static)] selected: Signal<Option<String>>) -> Element {
    let (label, cell_tone, cell_variant) = match cell_fit.0 {
        Fit::Runs => ("✓ runs", ToneRef::from(tone::Success), VariantRef::from(variant::Solid)),
        Fit::Could => ("✓", ToneRef::from(tone::Success), VariantRef::from(variant::Soft)),
        Fit::Cannot => ("✗", ToneRef::from(tone::Danger), VariantRef::from(variant::Soft)),
        Fit::Unknown => ("?", ToneRef::from(tone::Neutral), VariantRef::from(variant::Ghost)),
    };
    let tip = detail.clone();
    ui! {
        view(style = runtime_core::StyleRules { width: px(GRID_CELL_WIDTH), ..Default::default() }) {
            Tooltip(text = tip) {
                Button(
                    label = label.to_string(),
                    on_click = Rc::new(move || selected.set(Some(detail.clone()))) as Rc<dyn Fn()>,
                    tone = cell_tone,
                    variant = cell_variant
                )
            }
        }
    }
}

/// [`Fit`] as a prop (`#[component]` props need `Default`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct FitProp(Fit);

impl Default for FitProp {
    fn default() -> FitProp {
        FitProp(Fit::Unknown)
    }
}

impl From<Fit> for FitProp {
    fn from(f: Fit) -> FitProp {
        FitProp(f)
    }
}

/// The registered app builds, and what each runs.
#[cfg(not(feature = "server"))]
#[component]
fn AppBuilds() -> Element {
    let builds = page().builds;
    ui! {
        Card() {
            Typography(content = "App builds".to_string(), kind = typography_kind::H3)
            if builds.get().is_empty() {
                Typography(
                    content = "None registered. `idealyst ota manifest` registers an app build; builds that report themselves to the resolver appear here too.".to_string(),
                    muted = true
                )
            }
            for b in builds, key = format!("{b:?}") {
                BuildRow(build = b)
            }
        }
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn BuildRow(#[prop(static)] build: BuildView) -> Element {
    let name = build_name(&build);
    let (badge, badge_tone) = if build.from_build {
        ("from its build".to_string(), ToneRef::from(tone::Info))
    } else {
        ("seen in the field".to_string(), ToneRef::from(tone::Neutral))
    };
    let header = format!("{} · registered {}", short(&build.id), utc(build.registered_at));
    let lines: Vec<(String, Vec<String>)> = build
        .bundles
        .iter()
        .map(|b| {
            let runs = match &b.runs {
                Some(v) => format!("`{}` {v}", b.bundle),
                None => format!("`{}`: nothing it can run", b.bundle),
            };
            (runs, b.blocked.iter().map(|why| format!("can't run {why}")).collect())
        })
        .collect();
    ui! {
        view(style = runtime_core::StyleRules {
            padding_top: px(10.0), padding_bottom: px(10.0),
            border_top_width: Some(runtime_core::Tokenized::Literal(1.0)),
            border_top_color: Some(runtime_core::Tokenized::token("color-border", runtime_core::Color("#e5e7eb".into()))),
            ..column(4.0)
        }) {
            view(style = row(8.0)) {
                text(style = runtime_core::StyleRules { font_weight: Some(runtime_core::FontWeight::Bold), ..Default::default() }) { name }
                Badge(label = badge, tone = badge_tone)
                text(style = runtime_core::StyleRules { color: Some(runtime_core::Tokenized::Literal(runtime_core::Color("#4b5563".into()))), ..Default::default() }) { header }
            }
            for (runs, blocked) in lines {
                text() { runs }
                for why in blocked {
                    text(style = runtime_core::StyleRules { color: Some(runtime_core::Tokenized::Literal(runtime_core::Color("#92400e".into()))), ..Default::default() }) { why }
                }
            }
        }
    }
}

#[cfg(not(feature = "server"))]
#[component]
fn AuditLog(events: ReadSignal<Vec<EventView>>) -> Element {
    ui! {
        Card() {
            Typography(content = "Audit log".to_string(), kind = typography_kind::H3)
            if events.get().is_empty() {
                Typography(content = "Nothing yet.".to_string(), muted = true)
            }
            for e in events, key = format!("{}:{}:{}:{}", e.at, e.action, e.bundle, e.sha256) {
                text() {
                    format!(
                        "{} · {} · {} `{}` {} ({}){}",
                        utc(e.at),
                        e.actor,
                        e.action,
                        e.bundle,
                        e.version,
                        short(&e.sha256),
                        e.reason.as_ref().map_or(String::new(), |r| format!(" — {r}"))
                    )
                }
            }
        }
    }
}

/// SDK-handler registration seam: only builtins and idea-ui here.
pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The take-down dialog names each registered build running the
    /// release and where it goes; builds running another aren't named.
    #[test]
    fn a_take_down_names_the_builds_it_moves() {
        let build = |label: &str, runnable: &[(&str, &str)]| BuildView {
            id: "0123456789abcdef".into(),
            label: Some(label.into()),
            from_build: true,
            registered_at: 0,
            bundles: vec![BuildBundle {
                bundle: "shop".into(),
                runnable: runnable.iter().map(|(s, v)| (s.to_string(), v.to_string())).collect(),
                runs: runnable.first().map(|(_, v)| v.to_string()),
                blocked: vec![],
                cells: vec![],
            }],
        };
        let builds = [
            build("ios 2.4", &[("b", "1.2.0"), ("a", "1.1.0")]),
            build("ios 2.3", &[("b", "1.2.0")]),
            build("ios 2.5", &[("c", "1.3.0"), ("b", "1.2.0")]),
        ];
        assert_eq!(impact(&builds, "shop", "b"), ["ios 2.4 moves to 1.1.0 (a)", "ios 2.3 is left with nothing it can run"]);
        assert!(impact(&builds, "other", "b").is_empty());
    }

    /// The grid's cells: the release a build runs, ones it could run, ones
    /// it can't (with the reasons), and an unknown for a release its answer
    /// doesn't cover yet.
    #[test]
    fn the_grid_says_what_each_build_runs_and_why_not() {
        let cell = |sha: &str, runs: bool, chosen: bool, reasons: &[&str]| CellView {
            sha256: sha.into(),
            version: format!("v{sha}"),
            runs,
            chosen,
            reasons: reasons.iter().map(|r| r.to_string()).collect(),
            warnings: vec![],
        };
        let build = BuildView {
            id: "0123456789abcdef".into(),
            label: Some("ios 2.4".into()),
            from_build: true,
            registered_at: 0,
            bundles: vec![BuildBundle {
                bundle: "shop".into(),
                cells: vec![
                    cell("c", false, false, &["the app has no host function `shop::checkout`"]),
                    cell("b", true, true, &[]),
                    cell("a", true, false, &[]),
                ],
                ..Default::default()
            }],
        };
        let columns: Vec<(String, String)> = ["c", "b", "a", "z"].iter().map(|s| (s.to_string(), format!("v{s}"))).collect();
        let row = grid_row(&build, "shop", &columns);
        assert_eq!(row.iter().map(|c| c.fit).collect::<Vec<_>>(), [Fit::Cannot, Fit::Runs, Fit::Could, Fit::Unknown]);
        assert_eq!(row[0].detail, "ios 2.4 can't run vc (c): the app has no host function `shop::checkout`");
        assert_eq!(row[1].detail, "ios 2.4 runs vb (b)");
        assert!(grid_row(&build, "other", &columns).iter().all(|c| c.fit == Fit::Unknown));
    }

    #[test]
    fn dates_and_details() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        let r = ReleaseView {
            version: "1.2.0".into(),
            sha256: "abcdef0123456789".into(),
            size: 2048,
            published: 0,
            signed_by: None,
            components: vec!["shop::Offer".into()],
            new_requirements: vec!["prop `Card.glow`".into()],
        };
        assert_eq!(
            release_detail(&r),
            "published 1970-01-01 00:00 UTC · 2.0 KB · unsigned · provides shop::Offer\nnewly requires: prop `Card.glow`"
        );
    }
}
