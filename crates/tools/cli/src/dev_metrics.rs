//! `idealyst dev --metrics`: a record of what every save cost.
//!
//! A save can land in milliseconds (an overlay patch), seconds (a hot
//! patch, or a rebuild that recompiles the app crate) or minutes (a
//! rebuild that recompiles half the dependency tree). The session's event
//! stream already says which tier a save took and how long each stage ran;
//! what it cannot say on its own is *which crates cargo recompiled and
//! why*. So `--metrics` also turns on rebuild explanation
//! ([`dev_events::process::set_explain_rebuilds`]): every recompiled unit
//! arrives as a `unit_built` with its time, and every dirty one as a
//! `unit_dirty` with cargo's reason.
//!
//! [`start`] subscribes a sink to the session reporter. It groups each
//! target's events into one *episode* per save (opened by
//! `change_detected`, or by `build_started` for the initial and forced
//! builds; closed by the event that ends the save's work) and appends one
//! record per episode to two files beside the session log, kept across
//! sessions so a slow rebuild can be compared with a fast one:
//!
//! - `build-metrics.md`: one section per save, for reading;
//! - `build-metrics.jsonl`: the same records as JSON, one per line, for
//!   analysis.
//!
//! `--metrics-diffs` adds each save's diff, taken against what the file
//! held at the previous save (or when the session started watching it).
//! That needs a snapshot of every watched text file, held in memory for
//! the session, so it is a flag of its own.
//!
//! The sink runs under the reporter's fan-out lock, so it only forwards
//! envelopes to a worker thread; reading files, diffing and writing all
//! happen there.

use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use dev_events::{
    BuildCause, BuildOutcome, CrateTiming, Decision, DevEvent, DirtyReason, Envelope, Reporter,
    SidecarUpdate, Sink, Timing,
};
use serde::Serialize;

/// The Markdown record's file name, beside the session log.
pub const MARKDOWN_FILE: &str = "build-metrics.md";
/// The JSON-lines record's file name, beside the session log.
pub const JSONL_FILE: &str = "build-metrics.jsonl";

/// Recompiled packages listed in a Markdown section, slowest first. The
/// JSON record lists every one.
const MAX_LISTED_PACKAGES: usize = 25;
/// Packages named per root cause in a Markdown section.
const MAX_CAUSE_PACKAGES: usize = 12;
/// Lines of one file's diff kept in a record.
const MAX_DIFF_LINES: usize = 2000;
/// A file larger than this is not snapshotted or diffed.
const MAX_SNAPSHOT_BYTES: u64 = 1 << 20;
/// Context lines around each diff hunk.
const DIFF_CONTEXT: usize = 3;
/// Above this many cells the line diff gives up on a minimal edit script
/// and reports the changed middle as removed-then-added (see [`line_diff`]).
const MAX_DIFF_CELLS: usize = 4_000_000;

/// What [`start`] needs.
#[derive(Debug, Clone)]
pub struct Config {
    /// Paths under it are shown relative to it.
    pub project_dir: PathBuf,
    /// Where the two files go: the session log's directory.
    pub out_dir: PathBuf,
    /// `--metrics-diffs`.
    pub diffs: bool,
    /// The recording session's process id, written into every record:
    /// sessions of one project share the files, and this is what tells
    /// their records apart.
    pub session: u32,
}

impl Config {
    pub fn markdown(&self) -> PathBuf {
        self.out_dir.join(MARKDOWN_FILE)
    }

    pub fn jsonl(&self) -> PathBuf {
        self.out_dir.join(JSONL_FILE)
    }
}

/// Start recording: rebuild explanation on, the sink subscribed. Call
/// before the session's `session_started`, which heads each session's
/// records.
pub fn start(config: Config, reporter: &Reporter) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.out_dir)?;
    dev_events::process::set_explain_rebuilds(true);
    let (tx, rx) = mpsc::channel::<Envelope>();
    let mut recorder = Recorder::new(config);
    std::thread::Builder::new().name("dev-metrics".into()).spawn(move || {
        for envelope in rx {
            recorder.handle(&envelope);
        }
    })?;
    reporter.subscribe(Box::new(MetricsSink { tx }));
    Ok(())
}

struct MetricsSink {
    tx: mpsc::Sender<Envelope>,
}

impl Sink for MetricsSink {
    fn emit(&self, envelope: &Envelope) {
        if wanted(&envelope.event) {
            let _ = self.tx.send(envelope.clone());
        }
    }
}

/// The events a record is built from. Everything else (subprocess lines
/// above all, thousands a build) is not worth the clone.
fn wanted(event: &DevEvent) -> bool {
    matches!(
        event,
        DevEvent::SessionStarted { .. }
            | DevEvent::Watching { .. }
            | DevEvent::ChangeDetected { .. }
            | DevEvent::Decided { .. }
            | DevEvent::OverlayPushed { .. }
            | DevEvent::PatchBuilt { .. }
            | DevEvent::PatchFailed { .. }
            | DevEvent::PatchStep { .. }
            | DevEvent::Superseded { .. }
            | DevEvent::BuildStarted { .. }
            | DevEvent::StageFinished { .. }
            | DevEvent::CargoProgress { .. }
            | DevEvent::UnitBuilt { .. }
            | DevEvent::UnitDirty { .. }
            | DevEvent::Diagnostic { .. }
            | DevEvent::BuildTimed { .. }
            | DevEvent::BuildFinished { .. }
            | DevEvent::SidecarApplied { .. }
    )
}

/// One save's record, as written to `build-metrics.jsonl`.
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub app: String,
    /// The pid of the `idealyst dev` session that wrote it ([`Config::session`]).
    pub session: u32,
    /// Local time the episode opened, RFC 3339.
    pub started: String,
    pub target: String,
    /// `save`, `initial`, `forced`, `one_shot`, or `unknown` (a build with
    /// no `build_started` seen).
    pub trigger: String,
    /// Extra watcher batches folded into the save.
    pub folded: usize,
    /// The saved files, project-relative where they are under the project.
    pub changes: Vec<ChangedFile>,
    /// The packages the watcher attributed the save to.
    pub changed_crates: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    /// A hot patch that could not be built, so the save rebuilt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_failed: Option<String>,
    /// How the episode ended, in words.
    pub outcome: String,
    pub failed: bool,
    /// From the episode's first event to the one that closed it.
    pub total_ms: u64,
    pub stages: Vec<Timing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cargo: Option<CargoRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hot_patch: Option<HotPatchRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<FileDiff>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    /// `rust source`, `manifest`, `lockfile`, `build script`, `config`,
    /// `asset`, `deleted`, `other`.
    pub kind: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CargoRecord {
    /// Distinct packages cargo reported, fresh or not, and the closure size.
    pub packages_seen: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packages_total: Option<u32>,
    /// Every package with a recompiled unit, slowest first.
    pub recompiled: Vec<RecompiledPackage>,
    /// The reasons that are not "a dependency was rebuilt", grouped: what
    /// actually started the cascade.
    pub root_causes: Vec<RootCause>,
    pub errors: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecompiledPackage {
    pub name: String,
    pub version: String,
    /// The package's longest unit (see `unit_built`'s `ms`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// Its recompiled units' kinds (`lib`, `custom-build`, …).
    pub units: Vec<String>,
    /// Why cargo said each unit was dirty.
    pub reasons: Vec<DirtyReason>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RootCause {
    pub kind: String,
    pub detail: String,
    pub packages: Vec<String>,
}

/// A hot patch the save attempted. Only a built one has `crates`,
/// `redirected` and `bytes`; one that failed or was superseded has the
/// steps it finished, and `total_ms` says how long it ran.
#[derive(Debug, Clone, Serialize)]
pub struct HotPatchRecord {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub crates: Vec<CrateTiming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirected: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// In order, from the decision to the push: `decide`, `base-index`,
    /// `seed-wait`, `cargo`, `diff`, `verify-additive`, `link`, `resolve`,
    /// `jump-table`, `strip+write`, `scan-wait`, `install`, each present
    /// when the patch took it.
    pub steps: Vec<Timing>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub path: String,
    /// What the diff is against: `previous save`, `session start`,
    /// `new file`, or why there is no diff (`binary`, `too large`).
    pub against: &'static str,
    /// Unified diff text; empty when `against` says why there is none.
    pub diff: String,
}

/// An open episode.
#[derive(Debug, Default)]
struct Episode {
    opened_at_ms: u64,
    started: String,
    trigger: String,
    folded: usize,
    changes: Vec<ChangedFile>,
    changed_crates: Vec<String>,
    decision: Option<Decision>,
    patch_failed: Option<String>,
    /// A build or a patch started: the episode is past the point where a
    /// second `change_detected` could still be the same save.
    working: bool,
    stages: Vec<Timing>,
    cargo: Option<CargoRecord>,
    units: BTreeMap<(String, String), UnitAcc>,
    hot_patch: Option<HotPatchRecord>,
    /// `patch_step`s so far: all a hot patch that is never built leaves.
    patch_steps: Vec<Timing>,
    diffs: Vec<FileDiff>,
}

#[derive(Debug, Default)]
struct UnitAcc {
    ms: Option<u64>,
    units: Vec<String>,
    reasons: Vec<DirtyReason>,
    built: bool,
}

/// The worker's state. Synchronous, so tests drive it event by event.
struct Recorder {
    config: Config,
    app: String,
    open: HashMap<String, Episode>,
    /// Watched file contents, for `--metrics-diffs`. `None` values are
    /// files that are not diffable (binary, too large).
    baseline: Option<HashMap<PathBuf, Snapshot>>,
}

#[derive(Debug, Clone)]
enum Snapshot {
    Text { text: String, from_save: bool },
    Binary,
    TooLarge,
}

impl Recorder {
    fn new(config: Config) -> Self {
        let baseline = config.diffs.then(HashMap::new);
        Self { config, app: String::new(), open: HashMap::new(), baseline }
    }

    fn handle(&mut self, envelope: &Envelope) {
        let at = envelope.at_ms;
        match &envelope.event {
            DevEvent::SessionStarted { app, targets, mode, hot_tier, .. } => {
                self.app = app.clone();
                self.open.clear();
                let tier = match hot_tier {
                    dev_events::HotTier::Armed => "armed".to_string(),
                    dev_events::HotTier::Off { reason } => format!("off ({reason})"),
                };
                self.write_markdown(&format!(
                    "\n---\n\n# Session {} — {app}\n\n{} mode · targets: {} · hot patch tier: {tier}{}\n",
                    now_local(),
                    mode.as_str(),
                    targets.join(", "),
                    if self.config.diffs { " · diffs on" } else { "" },
                ));
            }
            DevEvent::Watching { roots, .. } => {
                if let Some(baseline) = &mut self.baseline {
                    for root in roots {
                        snapshot_into(baseline, Path::new(root));
                    }
                }
            }
            DevEvent::ChangeDetected { target, paths, crates, folded } => {
                let merge = self.open.get(target).is_some_and(|e| !e.working && e.decision.is_none());
                if !merge {
                    if self.open.contains_key(target) {
                        self.close(target, at, "interrupted by a newer save".into(), false);
                    }
                    self.open.insert(target.clone(), self.episode(at, "save"));
                }
                let changes: Vec<ChangedFile> = paths
                    .iter()
                    .map(|p| ChangedFile { path: self.display(p), kind: change_kind(Path::new(p)) })
                    .collect();
                let diffs = self.diffs_for(paths);
                let ep = self.open.get_mut(target).expect("opened above");
                // Under `--no-watch` each report is the whole queue so far:
                // the latest replaces the earlier ones.
                ep.changes = changes;
                ep.changed_crates = crates.clone();
                ep.folded = *folded;
                ep.diffs.extend(diffs);
            }
            DevEvent::Decided { target, decision } => {
                let ep = self.open_or_new(target, at);
                ep.decision = Some(decision.clone());
                if matches!(decision, Decision::Unchanged) {
                    self.close(target, at, "no change that reaches the program".into(), false);
                }
            }
            DevEvent::OverlayPushed { target, sites, .. } => {
                self.close(target, at, format!("overlay patch: {sites} site(s) updated"), false);
            }
            DevEvent::PatchBuilt { target, crates, redirected, steps, bytes, .. } => {
                if let Some(ep) = self.open.get_mut(target) {
                    ep.hot_patch = Some(HotPatchRecord {
                        crates: crates.clone(),
                        redirected: Some(*redirected),
                        bytes: Some(*bytes),
                        steps: steps.clone(),
                    });
                }
                self.close(target, at, format!("hot patch: {redirected} function(s) redirected"), false);
            }
            DevEvent::PatchFailed { target, reason, .. } => {
                self.open_or_new(target, at).patch_failed = Some(reason.clone());
            }
            DevEvent::PatchStep { target, step, ms } => {
                if let Some(ep) = self.open.get_mut(target) {
                    ep.patch_steps.push(Timing { name: step.clone(), ms: *ms });
                }
            }
            DevEvent::Superseded { target, work, ms } => {
                let outcome = format!("superseded ({}) after {}", work.as_str(), fmt_ms(*ms));
                self.close(target, at, outcome, false);
            }
            DevEvent::BuildStarted { target, cause } => {
                let (trigger, folded) = match cause {
                    BuildCause::Initial => ("initial", 0),
                    BuildCause::Save { folded } => ("save", *folded),
                    BuildCause::Forced => ("forced", 0),
                    BuildCause::OneShot => ("one_shot", 0),
                };
                // A save's build continues its save; the others are
                // episodes of their own.
                let continues = trigger == "save" && self.open.contains_key(target);
                if !continues {
                    if self.open.contains_key(target) {
                        self.close(target, at, "interrupted by another build".into(), false);
                    }
                    let mut ep = self.episode(at, trigger);
                    ep.folded = folded;
                    self.open.insert(target.clone(), ep);
                }
                self.open.get_mut(target).expect("present").working = true;
            }
            DevEvent::StageFinished { target, stage, ms } => {
                if let Some(ep) = self.open.get_mut(target) {
                    ep.stages.push(Timing { name: stage.clone(), ms: *ms });
                }
            }
            DevEvent::BuildTimed { target, stages, .. } => {
                // The bundler's own summary is the complete list; the
                // stage events seen so far were its parts.
                if let Some(ep) = self.open.get_mut(target) {
                    if !stages.is_empty() {
                        ep.stages = stages.clone();
                    }
                }
            }
            DevEvent::CargoProgress { target, compiled, total, .. } => {
                if let Some(ep) = self.open.get_mut(target) {
                    let cargo = ep.cargo.get_or_insert_with(CargoRecord::default);
                    cargo.packages_seen = cargo.packages_seen.max(*compiled);
                    if total.is_some() {
                        cargo.packages_total = *total;
                    }
                }
            }
            DevEvent::UnitBuilt { target, package, version, kind, ms } => {
                if let Some(ep) = self.open.get_mut(target) {
                    ep.cargo.get_or_insert_with(CargoRecord::default);
                    let acc = ep.units.entry((package.clone(), version.clone())).or_default();
                    acc.built = true;
                    acc.units.push(kind.clone());
                    acc.ms = match (acc.ms, *ms) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                }
            }
            DevEvent::UnitDirty { target, package, version, reason, .. } => {
                if let Some(ep) = self.open.get_mut(target) {
                    ep.cargo.get_or_insert_with(CargoRecord::default);
                    let acc = ep.units.entry((package.clone(), version.clone())).or_default();
                    if !acc.reasons.contains(reason) {
                        acc.reasons.push(reason.clone());
                    }
                }
            }
            DevEvent::Diagnostic { target, diagnostic } if diagnostic.is_error() => {
                if let Some(ep) = self.open.get_mut(target) {
                    ep.cargo.get_or_insert_with(CargoRecord::default).errors += 1;
                }
            }
            DevEvent::BuildFinished { target, outcome, .. } => {
                let (text, failed) = match outcome {
                    BuildOutcome::Ready { gen } => (format!("ready (gen {gen})"), false),
                    BuildOutcome::Reloaded { gen } => (format!("rebuilt, pages reloaded (gen {gen})"), false),
                    BuildOutcome::Unchanged => ("rebuilt, output unchanged".into(), false),
                    BuildOutcome::PremintRefreshed { gen } => {
                        (format!("rebuilt, premint refreshed (gen {gen})"), false)
                    }
                    BuildOutcome::Failed { error } => {
                        (format!("failed: {}", error.lines().next().unwrap_or_default()), true)
                    }
                };
                self.close(target, at, text, failed);
            }
            DevEvent::SidecarApplied { target, how, .. } => {
                let text = match how {
                    SidecarUpdate::HotPatch => "sidecar hot patched",
                    SidecarUpdate::Respawn => "sidecar rebuilt and respawned",
                };
                self.close(target, at, text.into(), false);
            }
            _ => {}
        }
    }

    fn episode(&self, at: u64, trigger: &str) -> Episode {
        Episode { opened_at_ms: at, started: now_local(), trigger: trigger.into(), ..Episode::default() }
    }

    fn open_or_new(&mut self, target: &str, at: u64) -> &mut Episode {
        if !self.open.contains_key(target) {
            let ep = self.episode(at, "unknown");
            self.open.insert(target.to_string(), ep);
        }
        self.open.get_mut(target).expect("inserted above")
    }

    /// Close `target`'s episode, if one is open, and write its record.
    fn close(&mut self, target: &str, at: u64, outcome: String, failed: bool) {
        let Some(ep) = self.open.remove(target) else { return };
        let record = finish(&self.app, self.config.session, target, ep, at, outcome, failed);
        self.write(&record);
    }

    fn display(&self, path: &str) -> String {
        Path::new(path)
            .strip_prefix(&self.config.project_dir)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.to_string())
    }

    /// Each saved file's diff against the baseline, which then moves to
    /// the file's new contents. Empty unless `--metrics-diffs`.
    fn diffs_for(&mut self, paths: &[String]) -> Vec<FileDiff> {
        let Some(baseline) = self.baseline.as_mut() else { return Vec::new() };
        let mut out = Vec::new();
        for p in paths {
            let path = PathBuf::from(p);
            let now = read_snapshot(&path);
            let shown = Path::new(p)
                .strip_prefix(&self.config.project_dir)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| p.clone());
            let (against, diff) = match (baseline.get(&path), &now) {
                (_, Some(Snapshot::Binary)) => ("binary", String::new()),
                (_, Some(Snapshot::TooLarge)) => ("too large", String::new()),
                (Some(Snapshot::Text { text, from_save }), Some(Snapshot::Text { text: new, .. })) => {
                    (if *from_save { "previous save" } else { "session start" }, line_diff(text, new))
                }
                (Some(Snapshot::Text { text, from_save }), None) => {
                    (if *from_save { "previous save" } else { "session start" }, line_diff(text, ""))
                }
                (None | Some(Snapshot::Binary | Snapshot::TooLarge), Some(Snapshot::Text { text: new, .. })) => {
                    ("new file", line_diff("", new))
                }
                (_, None) => ("deleted, no baseline", String::new()),
            };
            match now {
                Some(Snapshot::Text { text, .. }) => {
                    baseline.insert(path, Snapshot::Text { text, from_save: true });
                }
                Some(other) => {
                    baseline.insert(path, other);
                }
                None => {
                    baseline.remove(&path);
                }
            }
            out.push(FileDiff { path: shown, against, diff });
        }
        out
    }

    fn write(&self, record: &Record) {
        if let Ok(line) = serde_json::to_string(record) {
            append(&self.config.jsonl(), &format!("{line}\n"));
        }
        self.write_markdown(&markdown(record));
    }

    fn write_markdown(&self, text: &str) {
        let path = self.config.markdown();
        let fresh = std::fs::metadata(&path).map(|m| m.len() == 0).unwrap_or(true);
        if fresh {
            append(
                &path,
                "# Build metrics\n\nWritten by `idealyst dev --metrics`: one section per save, \
                 newest last. The same records are in `build-metrics.jsonl`.\n",
            );
        }
        append(&path, text);
    }
}

/// Write errors are dropped: a metrics file that cannot be written must not
/// take the dev loop down. The first one is reported.
fn append(path: &Path, text: &str) {
    let result = OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| f.write_all(text.as_bytes()));
    if let Err(e) = result {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            dev_events::global().warn("dev", format!("cannot write {}: {e}", path.display()));
        });
    }
}

fn finish(app: &str, session: u32, target: &str, ep: Episode, at: u64, outcome: String, failed: bool) -> Record {
    let cargo = ep.cargo.map(|mut cargo| {
        let mut causes: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for ((name, version), acc) in &ep.units {
            for reason in acc.reasons.iter().filter(|r| !r.is_propagated()) {
                causes
                    .entry((reason.kind.clone(), cause_detail(reason)))
                    .or_default()
                    .push(format!("{name} {version}"));
            }
        }
        let mut root_causes: Vec<RootCause> = causes
            .into_iter()
            .map(|((kind, detail), mut packages)| {
                packages.dedup();
                RootCause { kind, detail, packages }
            })
            .collect();
        root_causes.sort_by(|a, b| b.packages.len().cmp(&a.packages.len()));
        cargo.root_causes = root_causes;
        cargo.recompiled = ep
            .units
            .into_iter()
            .filter(|(_, acc)| acc.built)
            .map(|((name, version), acc)| RecompiledPackage {
                name,
                version,
                ms: acc.ms,
                units: acc.units,
                reasons: acc.reasons,
            })
            .collect();
        cargo.recompiled.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.name.cmp(&b.name)));
        cargo
    });
    Record {
        app: app.to_string(),
        session,
        started: ep.started,
        target: target.to_string(),
        trigger: ep.trigger,
        folded: ep.folded,
        changes: ep.changes,
        changed_crates: ep.changed_crates,
        decision: ep.decision,
        patch_failed: ep.patch_failed,
        outcome,
        failed,
        total_ms: at.saturating_sub(ep.opened_at_ms),
        stages: ep.stages,
        cargo,
        hot_patch: ep.hot_patch.or_else(|| {
            (!ep.patch_steps.is_empty()).then(|| HotPatchRecord {
                crates: Vec::new(),
                redirected: None,
                bytes: None,
                steps: ep.patch_steps,
            })
        }),
        diffs: ep.diffs,
    }
}

/// What groups units under one root cause. A `no_prior_build`'s detail
/// names each unit's own fingerprint file, so it would make one cause per
/// unit; they share the explanation instead.
fn cause_detail(reason: &DirtyReason) -> String {
    if reason.kind == dev_events::cargo::NO_PRIOR_BUILD {
        "no fingerprint at this unit's hash: the first build with these features, flags or \
         profile, or its artifacts were removed"
            .into()
    } else {
        reason.detail.clone()
    }
}

/// One record as a Markdown section.
pub fn markdown(r: &Record) -> String {
    let mut s = String::new();
    let tier = r.decision.as_ref().map(Decision::tier).unwrap_or(match r.trigger.as_str() {
        "initial" => "initial build",
        "forced" => "forced rebuild",
        _ => "build",
    });
    let time = r.started.get(11..19).unwrap_or(&r.started);
    s.push_str(&format!("\n## {time} · {} · {tier} · {}\n\n", r.target, fmt_ms(r.total_ms)));
    let mut facts: Vec<String> = Vec::new();
    if !r.changes.is_empty() {
        let files: Vec<String> = r.changes.iter().map(|c| format!("`{}` ({})", c.path, c.kind)).collect();
        let folded = if r.folded > 0 { format!(", {} batches folded in", r.folded) } else { String::new() };
        facts.push(format!("**Saved:** {}{folded}", files.join(", ")));
    }
    if !r.changed_crates.is_empty() {
        facts.push(format!("**Crates edited:** {}", r.changed_crates.join(", ")));
    }
    if let Some(d) = &r.decision {
        facts.push(format!("**Decision:** {}", decision_text(d)));
    }
    if let Some(why) = &r.patch_failed {
        facts.push(format!("**Hot patch failed:** {why}"));
    }
    facts.push(format!("**Outcome:** {}", r.outcome));
    if !r.stages.is_empty() {
        let stages: Vec<String> = r.stages.iter().map(|t| format!("{} {}", t.name, fmt_ms(t.ms))).collect();
        facts.push(format!("**Stages:** {}", stages.join(" · ")));
    }
    if let Some(p) = &r.hot_patch {
        if !p.steps.is_empty() {
            let steps: Vec<String> = p.steps.iter().map(|t| format!("{} {}", t.name, fmt_ms(t.ms))).collect();
            facts.push(format!("**Hot patch steps:** {}", steps.join(" · ")));
        }
        if !p.crates.is_empty() {
            let crates: Vec<String> = p
                .crates
                .iter()
                .map(|c| match c.ms {
                    Some(ms) => format!("{} {}", c.name, fmt_ms(ms)),
                    None => format!("{} (reused)", c.name),
                })
                .collect();
            let bytes = p.bytes.map(|b| format!(" · {b} bytes")).unwrap_or_default();
            facts.push(format!("**Patched crates:** {}{bytes}", crates.join(", ")));
        }
    }
    s.push_str(&facts.join("  \n"));
    s.push('\n');

    if let Some(c) = &r.cargo {
        let total = c.packages_total.map(|t| format!(" of {t}")).unwrap_or_default();
        s.push_str(&format!(
            "\n**Cargo:** {} package(s) recompiled, {}{total} seen{}\n",
            c.recompiled.len(),
            c.packages_seen,
            if c.errors > 0 { format!(", {} error(s)", c.errors) } else { String::new() },
        ));
        if !c.root_causes.is_empty() {
            s.push_str("\nWhy cargo recompiled (causes other than a rebuilt dependency):\n\n");
            for cause in &c.root_causes {
                let shown: Vec<&str> =
                    cause.packages.iter().take(MAX_CAUSE_PACKAGES).map(String::as_str).collect();
                let more = cause.packages.len().saturating_sub(MAX_CAUSE_PACKAGES);
                let more = if more > 0 { format!(" and {more} more") } else { String::new() };
                s.push_str(&format!(
                    "- **{}** `{}` → {}{more}\n",
                    cause.kind,
                    cause.detail.replace('`', "'"),
                    shown.join(", "),
                ));
            }
        } else if !c.recompiled.is_empty() && c.recompiled.iter().all(|p| p.reasons.is_empty()) {
            s.push_str("\n(No fingerprint log: cargo's reasons were not captured for this build.)\n");
        }
        if !c.recompiled.is_empty() {
            s.push_str("\n| crate | time | units | why |\n|---|---|---|---|\n");
            for p in c.recompiled.iter().take(MAX_LISTED_PACKAGES) {
                // A propagated reason or a missing fingerprint says all it
                // has to in its kind; the detail of the latter is only the
                // unit's own fingerprint path.
                let mut why: Vec<String> = Vec::new();
                for r in &p.reasons {
                    let text = if r.is_propagated() || r.kind == dev_events::cargo::NO_PRIOR_BUILD {
                        r.kind.clone()
                    } else {
                        format!("{} {}", r.kind, short(&r.detail))
                    };
                    if !why.contains(&text) {
                        why.push(text);
                    }
                }
                s.push_str(&format!(
                    "| {} {} | {} | {} | {} |\n",
                    p.name,
                    p.version,
                    p.ms.map(fmt_ms).unwrap_or_else(|| "?".into()),
                    p.units.join(", "),
                    why.join("; ").replace('|', "\\|"),
                ));
            }
            let more = c.recompiled.len().saturating_sub(MAX_LISTED_PACKAGES);
            if more > 0 {
                s.push_str(&format!("\n…and {more} more in `{JSONL_FILE}`.\n"));
            }
        }
    }
    for d in &r.diffs {
        if d.diff.is_empty() {
            s.push_str(&format!("\n`{}`: no diff ({})\n", d.path, d.against));
            continue;
        }
        s.push_str(&format!(
            "\n<details><summary>diff <code>{}</code> (against {})</summary>\n\n```diff\n{}```\n\n</details>\n",
            d.path, d.against, d.diff
        ));
    }
    s
}

fn decision_text(d: &Decision) -> String {
    match d {
        Decision::Overlay { sites } => format!("overlay patch ({sites} site(s))"),
        Decision::HotPatch { crates, .. } => format!("hot patch ({})", crates.join(", ")),
        Decision::Rebuild { reason: Some(why) } => format!("rebuild — {why}"),
        Decision::Rebuild { reason: None } => "rebuild — the save named no file the archives describe".into(),
        Decision::Unchanged => "unchanged".into(),
    }
}

/// A reason's detail, shortened for a table cell: a path to its last two
/// components.
fn short(detail: &str) -> String {
    let p = Path::new(detail);
    if p.is_absolute() {
        let parts: Vec<_> = p.components().rev().take(2).collect();
        let tail: PathBuf = parts.into_iter().rev().collect();
        return format!("…/{}", tail.display());
    }
    if detail.chars().count() > 80 {
        format!("{}…", detail.chars().take(80).collect::<String>())
    } else {
        detail.to_string()
    }
}

/// What kind of file a save touched: the first thing that says how
/// expensive it can be. A manifest or lockfile edit can re-resolve
/// features for the whole graph; a build script re-runs.
fn change_kind(path: &Path) -> &'static str {
    if !path.exists() {
        return "deleted";
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    match (name, ext) {
        ("Cargo.toml", _) => "manifest",
        ("Cargo.lock", _) => "lockfile",
        ("build.rs", _) => "build script",
        (_, "rs") => "rust source",
        (_, "toml") => "config",
        (_, "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "ttf" | "otf" | "woff" | "woff2") => "asset",
        (_, "css" | "html" | "js") => "web asset",
        _ => "other",
    }
}

fn read_snapshot(path: &Path) -> Option<Snapshot> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_SNAPSHOT_BYTES {
        return Some(Snapshot::TooLarge);
    }
    let bytes = std::fs::read(path).ok()?;
    Some(match String::from_utf8(bytes) {
        Ok(text) => Snapshot::Text { text, from_save: false },
        Err(_) => Snapshot::Binary,
    })
}

/// Read every file under `root` (or `root` itself) into `baseline`,
/// skipping paths already there, hidden entries and build output.
fn snapshot_into(baseline: &mut HashMap<PathBuf, Snapshot>, root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            let Ok(entries) = std::fs::read_dir(&path) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                stack.push(entry.path());
            }
        } else if !baseline.contains_key(&path) {
            if let Some(snapshot) = read_snapshot(&path) {
                baseline.insert(path, snapshot);
            }
        }
    }
}

/// `old` → `new` as unified-diff hunks (no file header), each line ending
/// in a newline.
///
/// A minimal edit script by LCS over the lines between the common prefix
/// and suffix — a save is usually a small edit in a large file, so the
/// table stays small. Past [`MAX_DIFF_CELLS`] (a rewrite of a big file)
/// the middle is reported as removed and then added: still correct, just
/// not minimal.
pub fn line_diff(old: &str, new: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Op {
        Same,
        Del,
        Add,
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (ma, mb) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    if ma.is_empty() && mb.is_empty() {
        return String::new();
    }
    // (op, index into a, index into b) for every line of the merged script.
    let mut ops: Vec<(Op, usize, usize)> = (0..prefix).map(|i| (Op::Same, i, i)).collect();
    if ma.len().saturating_mul(mb.len()) <= MAX_DIFF_CELLS {
        let (n, m) = (ma.len(), mb.len());
        // lcs[i][j]: LCS length of ma[i..] and mb[j..].
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if ma[i] == mb[j] {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && ma[i] == mb[j] {
                ops.push((Op::Same, prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
                // On a tie the removal goes first, as unified diffs read.
                ops.push((Op::Del, prefix + i, prefix + j));
                i += 1;
            } else {
                ops.push((Op::Add, prefix + i, prefix + j));
                j += 1;
            }
        }
    } else {
        ops.extend((0..ma.len()).map(|i| (Op::Del, prefix + i, prefix)));
        ops.extend((0..mb.len()).map(|j| (Op::Add, prefix + ma.len(), prefix + j)));
    }
    let (sa, sb) = (a.len() - suffix, b.len() - suffix);
    ops.extend((0..suffix).map(|k| (Op::Same, sa + k, sb + k)));

    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != Op::Same).collect();
    let mut out = String::new();
    let mut lines = 0usize;
    let mut k = 0;
    while k < changed.len() {
        // Extend the hunk while the next change is within two contexts.
        let mut last = k;
        while last + 1 < changed.len() && changed[last + 1] - changed[last] <= 2 * DIFF_CONTEXT {
            last += 1;
        }
        let start = changed[k].saturating_sub(DIFF_CONTEXT);
        let end = (changed[last] + DIFF_CONTEXT + 1).min(ops.len());
        let hunk = &ops[start..end];
        let old_count = hunk.iter().filter(|o| o.0 != Op::Add).count();
        let new_count = hunk.iter().filter(|o| o.0 != Op::Del).count();
        let old_start = hunk[0].1 + usize::from(old_count > 0);
        let new_start = hunk[0].2 + usize::from(new_count > 0);
        out.push_str(&format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"));
        for &(op, i, j) in hunk {
            if lines == MAX_DIFF_LINES {
                out.push_str(&format!("… diff truncated at {MAX_DIFF_LINES} lines\n"));
                return out;
            }
            lines += 1;
            match op {
                Op::Same => out.push_str(&format!(" {}\n", a[i])),
                Op::Del => out.push_str(&format!("-{}\n", a[i])),
                Op::Add => out.push_str(&format!("+{}\n", b[j])),
            }
        }
        k = last + 1;
    }
    out
}

fn now_local() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// `640 ms`, `12.3 s`, `4m 52s`.
fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(seq: u64, at_ms: u64, event: DevEvent) -> Envelope {
        Envelope { v: dev_events::SCHEMA_VERSION, seq, at_ms, event }
    }

    fn recorder(dir: &Path, diffs: bool) -> Recorder {
        Recorder::new(Config { project_dir: dir.to_path_buf(), out_dir: dir.join("out"), diffs, session: 7 })
    }

    fn records(dir: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(dir.join("out").join(JSONL_FILE))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn feed(r: &mut Recorder, events: Vec<(u64, DevEvent)>) {
        for (i, (at, e)) in events.into_iter().enumerate() {
            r.handle(&env(i as u64 + 1, at, e));
        }
    }

    fn web() -> String {
        "web".into()
    }

    fn reason(kind: &str, detail: &str) -> DirtyReason {
        DirtyReason { kind: kind.into(), detail: detail.into() }
    }

    /// A rebuild's record says which file started it, which crates cargo
    /// recompiled (slowest first, each with its time and reason), and what
    /// the root cause was — the question `--metrics` exists to answer.
    #[test]
    fn a_rebuild_record_names_its_recompiled_crates_and_their_root_cause() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        let saved = dir.path().join("src/app.rs");
        std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
        std::fs::write(&saved, "fn main() {}\n").unwrap();
        let mut r = recorder(dir.path(), false);
        let unit = |package: &str, kind: &str, ms| DevEvent::UnitBuilt {
            target: web(),
            package: package.into(),
            version: "0.1.0".into(),
            kind: kind.into(),
            ms,
        };
        let dirty = |package: &str, why| DevEvent::UnitDirty {
            target: web(),
            package: package.into(),
            version: "0.1.0".into(),
            unit: package.into(),
            reason: why,
        };
        feed(
            &mut r,
            vec![
                (0, DevEvent::SessionStarted {
                    app: "Lab".into(),
                    targets: vec![web()],
                    mode: dev_events::Mode::Local,
                    hot_tier: dev_events::HotTier::Armed,
                    log_file: None,
                    server: None,
                }),
                (1_000, DevEvent::ChangeDetected {
                    target: web(),
                    paths: vec![saved.display().to_string()],
                    crates: vec!["lab".into()],
                    folded: 1,
                }),
                (1_010, DevEvent::Decided {
                    target: web(),
                    decision: Decision::Rebuild { reason: Some("a signature changed".into()) },
                }),
                (1_020, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 1 } }),
                (1_100, dirty("lab", reason("file_changed", &saved.display().to_string()))),
                (1_100, dirty("lab-app", reason(dev_events::cargo::DEPENDENCY_REBUILT, ""))),
                (5_000, unit("lab", "lib", Some(3_900))),
                (9_000, unit("lab-app", "lib", Some(4_000))),
                (9_100, DevEvent::CargoProgress { target: web(), compiled: 300, total: Some(310), current: None }),
                (9_500, DevEvent::BuildTimed {
                    target: web(),
                    stages: vec![Timing { name: "cargo".into(), ms: 8_000 }, Timing { name: "wasm-bindgen".into(), ms: 400 }],
                    total_ms: 8_400,
                }),
                (9_600, DevEvent::BuildFinished {
                    target: web(),
                    outcome: BuildOutcome::Reloaded { gen: 2 },
                    ms: 8_600,
                }),
            ],
        );
        let recs = records(dir.path());
        assert_eq!(recs.len(), 1, "{recs:?}");
        let rec = &recs[0];
        assert_eq!(rec["trigger"], "save");
        // Sessions of one project share the file; this is how their
        // records are told apart (`read_build_metrics` filters on it).
        assert_eq!(rec["session"], 7);
        assert_eq!(rec["total_ms"], 8_600);
        assert_eq!(rec["changes"][0]["path"], "src/app.rs");
        assert_eq!(rec["changes"][0]["kind"], "rust source");
        assert_eq!(rec["decision"]["tier"], "rebuild");
        let names: Vec<&str> =
            rec["cargo"]["recompiled"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["lab-app", "lab"], "slowest first");
        let causes = rec["cargo"]["root_causes"].as_array().unwrap();
        assert_eq!(causes.len(), 1, "the propagated reason is not a cause: {causes:?}");
        assert_eq!(causes[0]["kind"], "file_changed");
        assert_eq!(causes[0]["packages"][0], "lab 0.1.0");
        assert_eq!(rec["stages"][1]["name"], "wasm-bindgen");

        let md = std::fs::read_to_string(dir.path().join("out").join(MARKDOWN_FILE)).unwrap();
        assert!(md.starts_with("# Build metrics"), "{md}");
        assert!(md.contains("# Session") && md.contains("— Lab"), "{md}");
        assert!(md.contains("· web · rebuild · 8.6 s"), "{md}");
        assert!(md.contains("**Decision:** rebuild — a signature changed"), "{md}");
        assert!(md.contains("| lab-app 0.1.0 | 4.0 s | lib | dependency_rebuilt |"), "{md}");
        assert!(md.contains("**Cargo:** 2 package(s) recompiled, 300 of 310 seen"), "{md}");
    }

    /// Units with no fingerprint at their hash each name their own
    /// fingerprint file; grouped by that they would read as hundreds of
    /// causes. They are one cause.
    #[test]
    fn units_with_no_prior_build_are_one_root_cause() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        let mut r = recorder(dir.path(), false);
        let mut events = vec![(0, DevEvent::BuildStarted { target: web(), cause: BuildCause::Forced })];
        for p in ["a", "b", "c"] {
            events.push((1, DevEvent::UnitDirty {
                target: web(),
                package: p.into(),
                version: "1.0.0".into(),
                unit: p.into(),
                reason: reason(dev_events::cargo::NO_PRIOR_BUILD, &format!("failed to read `/t/.fingerprint/{p}-1/lib-{p}`")),
            }));
        }
        events.push((2, DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Unchanged, ms: 2 }));
        feed(&mut r, events);
        let rec = &records(dir.path())[0];
        assert_eq!(rec["trigger"], "forced");
        let causes = rec["cargo"]["root_causes"].as_array().unwrap();
        assert_eq!(causes.len(), 1, "{causes:?}");
        assert_eq!(causes[0]["packages"].as_array().unwrap().len(), 3);
    }

    /// A package whose every unit had no fingerprint reads as that kind
    /// once in the table, not as each unit's fingerprint path.
    #[test]
    fn regression_a_no_prior_build_row_names_the_kind_once() {
        let rec = Record {
            app: "Lab".into(),
            session: 7,
            started: "2026-10-26T10:59:59-07:00".into(),
            target: web(),
            trigger: "initial".into(),
            folded: 0,
            changes: vec![],
            changed_crates: vec![],
            decision: None,
            patch_failed: None,
            outcome: "ready (gen 1)".into(),
            failed: false,
            total_ms: 23_000,
            stages: vec![],
            cargo: Some(CargoRecord {
                recompiled: vec![RecompiledPackage {
                    name: "serde".into(),
                    version: "1.0.0".into(),
                    ms: Some(4_000),
                    units: vec!["custom-build".into(), "lib".into()],
                    reasons: vec![
                        reason(dev_events::cargo::NO_PRIOR_BUILD, "failed to read `/t/.fingerprint/serde-1/build`"),
                        reason(dev_events::cargo::NO_PRIOR_BUILD, "failed to read `/t/.fingerprint/serde-2/lib`"),
                    ],
                }],
                ..CargoRecord::default()
            }),
            hot_patch: None,
            diffs: vec![],
        };
        let md = markdown(&rec);
        assert!(md.contains("| serde 1.0.0 | 4.0 s | custom-build, lib | no_prior_build |"), "{md}");
        assert!(md.contains("## 10:59:59 · web · initial build · 23.0 s"), "{md}");
    }

    /// Each way a save's work ends closes its episode, and a superseded
    /// save is recorded apart from the restart that follows it.
    #[test]
    fn every_ending_closes_one_record() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        let mut r = recorder(dir.path(), false);
        let change = || DevEvent::ChangeDetected {
            target: web(),
            paths: vec!["/elsewhere/src/lib.rs".into()],
            crates: vec![],
            folded: 0,
        };
        feed(
            &mut r,
            vec![
                (0, change()),
                (5, DevEvent::Decided { target: web(), decision: Decision::Overlay { sites: 2 } }),
                (9, DevEvent::OverlayPushed { target: web(), sites: 2, ms: 4 }),
                (10, change()),
                (11, DevEvent::Decided { target: web(), decision: Decision::Unchanged }),
                (20, change()),
                (21, DevEvent::Decided {
                    target: web(),
                    decision: Decision::HotPatch { crates: vec!["a".into()], files: vec![] },
                }),
                (300, DevEvent::Superseded {
                    target: web(),
                    work: dev_events::SupersededWork::HotPatch,
                    ms: 279,
                }),
                (301, change()),
                (302, DevEvent::Decided {
                    target: web(),
                    decision: Decision::HotPatch { crates: vec!["a".into()], files: vec![] },
                }),
                (900, DevEvent::PatchBuilt {
                    target: web(),
                    files: vec![],
                    crates: vec![CrateTiming { name: "a".into(), ms: Some(550) }],
                    redirected: 3,
                    steps: vec![],
                    skipped: vec![],
                    bytes: 2048,
                    ms: 598,
                }),
            ],
        );
        let recs = records(dir.path());
        let outcomes: Vec<&str> = recs.iter().map(|r| r["outcome"].as_str().unwrap()).collect();
        assert_eq!(
            outcomes,
            vec![
                "overlay patch: 2 site(s) updated",
                "no change that reaches the program",
                "superseded (hot patch) after 279 ms",
                "hot patch: 3 function(s) redirected",
            ]
        );
        assert_eq!(recs[3]["total_ms"], 599);
        assert_eq!(recs[3]["hot_patch"]["crates"][0]["ms"], 550);
        assert_eq!(recs[0]["changes"][0]["path"], "/elsewhere/src/lib.rs", "outside the project: absolute");
    }

    /// Regression: a hot patch that was superseded, or that failed and fell
    /// back to a rebuild, left no timing in its record (CrewForge: a 17.9 s
    /// superseded patch with nothing but its total, and a 15.2 s save whose
    /// stages accounted for 4 s). Its `patch_step`s are the record's
    /// `hot_patch.steps` whatever ended it, beside the rebuild's own stages.
    #[test]
    fn regression_an_unbuilt_hot_patch_keeps_its_steps() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        let mut r = recorder(dir.path(), false);
        let change = || DevEvent::ChangeDetected {
            target: web(),
            paths: vec!["/elsewhere/src/lib.rs".into()],
            crates: vec![],
            folded: 0,
        };
        let decided = || DevEvent::Decided {
            target: web(),
            decision: Decision::HotPatch { crates: vec!["a".into()], files: vec![] },
        };
        let step = |name: &str, ms| DevEvent::PatchStep { target: web(), step: name.into(), ms };
        feed(
            &mut r,
            vec![
                (0, change()),
                (1, decided()),
                (3, step("decide", 3)),
                (1_500, step("seed-wait", 1_497)),
                (17_900, DevEvent::Superseded {
                    target: web(),
                    work: dev_events::SupersededWork::HotPatch,
                    ms: 17_900,
                }),
                (18_000, change()),
                (18_001, decided()),
                (18_004, step("decide", 3)),
                (29_000, step("scan-wait", 10_996)),
                (29_001, DevEvent::PatchFailed { target: web(), files: vec![], reason: "no capture".into() }),
                (29_002, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } }),
                (31_000, DevEvent::BuildTimed {
                    target: web(),
                    stages: vec![Timing { name: "cargo".into(), ms: 1_500 }],
                    total_ms: 1_998,
                }),
                (31_001, DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Unchanged, ms: 1_999 }),
            ],
        );
        let recs = records(dir.path());
        assert_eq!(recs.len(), 2);
        let names = |rec: &serde_json::Value| -> Vec<String> {
            rec["hot_patch"]["steps"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(names(&recs[0]), ["decide", "seed-wait"]);
        assert_eq!(recs[0]["hot_patch"]["steps"][1]["ms"], 1_497);
        assert!(recs[0]["hot_patch"].get("bytes").is_none(), "nothing was built: {}", recs[0]);
        assert_eq!(names(&recs[1]), ["decide", "scan-wait"]);
        assert_eq!(recs[1]["stages"][0]["name"], "cargo", "the rebuild's stages stay its own");
        let md = std::fs::read_to_string(dir.path().join("out").join(MARKDOWN_FILE)).unwrap();
        assert!(md.contains("**Hot patch steps:** decide 3 ms · seed-wait 1.5 s"), "{md}");
    }

    /// Under `--metrics-diffs` each save records what changed since the
    /// previous save of that file, starting from the session's snapshot.
    #[test]
    fn diffs_are_taken_against_the_previous_save() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("app.rs");
        std::fs::write(&file, "a\nb\nc\n").unwrap();
        let mut r = recorder(dir.path(), true);
        let save = |r: &mut Recorder, at, paths: Vec<String>| {
            r.handle(&env(at, at, DevEvent::ChangeDetected { target: web(), paths, crates: vec![], folded: 0 }));
            r.handle(&env(at + 1, at + 1, DevEvent::Decided { target: web(), decision: Decision::Unchanged }));
        };
        r.handle(&env(1, 0, DevEvent::Watching { target: web(), roots: vec![src.display().to_string()], rewatch: false }));
        std::fs::write(&file, "a\nB\nc\n").unwrap();
        save(&mut r, 10, vec![file.display().to_string()]);
        std::fs::write(&file, "a\nB\nc\nd\n").unwrap();
        let added = src.join("new.rs");
        std::fs::write(&added, "fresh\n").unwrap();
        save(&mut r, 20, vec![file.display().to_string(), added.display().to_string()]);

        let recs = records(dir.path());
        assert_eq!(recs[0]["diffs"][0]["against"], "session start");
        assert_eq!(recs[0]["diffs"][0]["diff"], "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
        assert_eq!(recs[1]["diffs"][0]["against"], "previous save");
        assert_eq!(recs[1]["diffs"][0]["diff"], "@@ -1,3 +1,4 @@\n a\n B\n c\n+d\n");
        assert_eq!(recs[1]["diffs"][1]["path"], "src/new.rs");
        assert_eq!(recs[1]["diffs"][1]["against"], "new file");
        let md = std::fs::read_to_string(dir.path().join("out").join(MARKDOWN_FILE)).unwrap();
        assert!(md.contains("```diff\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n```"), "{md}");
    }

    #[test]
    fn without_diffs_nothing_is_snapshotted_or_diffed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        let mut r = recorder(dir.path(), false);
        r.handle(&env(1, 0, DevEvent::Watching { target: web(), roots: vec![dir.path().display().to_string()], rewatch: false }));
        assert!(r.baseline.is_none());
        assert!(r.diffs_for(&[dir.path().join("x.rs").display().to_string()]).is_empty());
    }

    #[test]
    fn line_diff_groups_hunks_with_context() {
        let mut lines: Vec<String> = (1..=20).map(|i| i.to_string()).collect();
        let old: String = lines.iter().map(|l| format!("{l}\n")).collect();
        lines[1] = "two".into();
        lines[17] = "eighteen".into();
        let new: String = lines.iter().map(|l| format!("{l}\n")).collect();
        assert_eq!(
            line_diff(&old, &new),
            "@@ -1,5 +1,5 @@\n 1\n-2\n+two\n 3\n 4\n 5\n@@ -15,6 +15,6 @@\n 15\n 16\n 17\n-18\n+eighteen\n 19\n 20\n"
        );
        assert_eq!(line_diff("same\n", "same\n"), "");
        assert_eq!(line_diff("", "x\n"), "@@ -0,0 +1,1 @@\n+x\n");
        assert_eq!(line_diff("x\n", ""), "@@ -1,1 +0,0 @@\n-x\n");
    }

    #[test]
    fn changed_files_are_classified() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Cargo.toml", "Cargo.lock", "build.rs", "lib.rs", "dev.toml", "icon.svg", "notes.txt"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let k = |n: &str| change_kind(&dir.path().join(n));
        assert_eq!(
            ["Cargo.toml", "Cargo.lock", "build.rs", "lib.rs", "dev.toml", "icon.svg", "notes.txt", "gone.rs"].map(k),
            ["manifest", "lockfile", "build script", "rust source", "config", "asset", "other", "deleted"]
        );
    }

    #[test]
    fn durations_read_at_every_scale() {
        assert_eq!(fmt_ms(640), "640 ms");
        assert_eq!(fmt_ms(12_340), "12.3 s");
        assert_eq!(fmt_ms(292_000), "4m 52s");
    }

    /// Through the real sink and worker thread: the reporter's events
    /// reach the files.
    #[test]
    fn the_sink_writes_through_its_worker() {
        let dir = tempfile::tempdir().unwrap();
        let reporter = Reporter::new();
        let config = Config { project_dir: dir.path().into(), out_dir: dir.path().join("out"), diffs: false, session: 7 };
        start(config, &reporter).unwrap();
        dev_events::process::set_explain_rebuilds(false);
        reporter.emit(DevEvent::BuildStarted { target: web(), cause: BuildCause::Initial });
        reporter.emit(DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 5 });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while records(dir.path()).is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let recs = records(dir.path());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["trigger"], "initial");
        assert_eq!(recs[0]["outcome"], "ready (gen 1)");
    }
}
