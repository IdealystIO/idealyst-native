//! `read_build_metrics`: the records `idealyst dev --metrics` writes.
//!
//! A session started with `--metrics` (or `--metrics-diffs`) appends one
//! JSON record per save, and per initial build, to
//! `<project>/target/idealyst/<package>/build-metrics.jsonl`, beside the
//! session log `dev.log`. Records accumulate across sessions. The CLI's
//! `dev_metrics` module writes them; this reads them back.
//!
//! A record of a full rebuild lists every crate cargo recompiled (hundreds
//! on an initial build) and, with diffs on, each saved file's diff. So the
//! default answer is a summary per record — what kind of work the save
//! took, what was saved, the outcome and time, the root causes and the
//! slowest crates — and `detail` returns the records as written.
//!
//! Every session of a project appends to the same file, so the records
//! interleave, and a record is written when its work ends, not when it
//! starts. Records are therefore ordered by `started`, and a query for
//! one session keeps only the records that session wrote (each carries
//! its pid as `session`). Whatever the query, an answer stays under
//! [`MAX_ANSWER_CHARS`]: the oldest records it asked for are left out
//! first, and the answer says how many.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The records file's name, as the CLI writes it beside `dev.log`.
pub const JSONL_FILE: &str = "build-metrics.jsonl";

/// Records returned by default, and at most.
pub const DEFAULT_LAST: usize = 10;
pub const MAX_LAST: usize = 200;

/// Recompiled crates a summary lists, slowest first.
const SUMMARY_CRATES: usize = 10;
/// Packages a summary names per root cause.
const SUMMARY_CAUSE_PACKAGES: usize = 8;
/// Saved files a summary names. A branch switch saves thousands.
const SUMMARY_CHANGES: usize = 10;
/// The longest answer, in characters of pretty-printed JSON, kept well
/// under an MCP client's tool-result limit.
pub const MAX_ANSWER_CHARS: usize = 24_000;

/// The records file of the session whose log is `dev_log`.
pub fn beside_log(dev_log: &Path) -> Option<PathBuf> {
    Some(dev_log.parent()?.join(JSONL_FILE))
}

/// The records file of the project at `dir`: under its own `target/`, in
/// the folder named after its package, as the CLI names it.
pub fn for_project(dir: &Path) -> Result<PathBuf, String> {
    let manifest = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", manifest.display()))?;
    let name = parsed
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| format!("{} has no [package] name", manifest.display()))?;
    Ok(dir.join("target").join("idealyst").join(name).join(JSONL_FILE))
}

/// What to return from a records file.
#[derive(Debug, Clone, Copy)]
pub struct Query {
    /// The newest this many matching records, oldest first.
    pub last: usize,
    /// Only records whose work was a cargo build: rebuilds, initial and
    /// forced builds. Overlay and hot patches are left out.
    pub rebuilds_only: bool,
    /// The records as written, instead of summaries.
    pub detail: bool,
    /// Only the records of the session with this pid.
    pub session: Option<u32>,
}

/// Read `path` for `query`. A missing file is not an error: it means no
/// session of this project has recorded yet, and the answer says how to
/// start one.
pub fn read(path: &Path, query: Query) -> Value {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => {
            return json!({
                "path": path.display().to_string(),
                "total": 0,
                "records": [],
                "note": "no records yet: start the session with `run_dev` and `metrics: true` \
                         (or `idealyst dev --metrics`); each save then adds one",
            })
        }
    };
    // (line number, record), 1-based as an editor shows the file.
    let all: Vec<(usize, Value)> = text
        .lines()
        .enumerate()
        .filter_map(|(i, l)| serde_json::from_str(l).ok().map(|r| (i + 1, r)))
        .collect();
    let mut matching: Vec<&(usize, Value)> = all
        .iter()
        .filter(|(_, r)| !query.rebuilds_only || is_build(r))
        .filter(|(_, r)| query.session.is_none_or(|pid| r["session"].as_u64() == Some(u64::from(pid))))
        .collect();
    // Stable: records whose `started` does not parse keep their file order.
    matching.sort_by_key(|(_, r)| started_ms(r));
    let last = query.last.clamp(1, MAX_LAST);
    let start = matching.len().saturating_sub(last);

    // Newest first into the budget, then back to oldest first.
    let mut records: Vec<Value> = Vec::new();
    let mut used = 0;
    let mut too_large: Option<usize> = None;
    for (line, r) in matching[start..].iter().rev() {
        let mut rec = if query.detail { r.clone() } else { summary(r) };
        rec["line"] = json!(line);
        let size = serde_json::to_string_pretty(&rec).map(|t| t.len()).unwrap_or(0);
        if used + size > MAX_ANSWER_CHARS {
            if records.is_empty() {
                too_large = Some(*line);
            }
            break;
        }
        used += size;
        records.push(rec);
    }
    records.reverse();
    let omitted = matching.len() - start - records.len();

    let mut out = json!({
        "path": path.display().to_string(),
        "total": all.len(),
        "matching": matching.len(),
        "records": records,
    });
    if let Some(pid) = query.session {
        out["session"] = json!(pid);
    }
    let mut notes: Vec<String> = Vec::new();
    if omitted > 0 {
        out["omitted_for_size"] = json!(omitted);
        notes.push(match too_large {
            Some(line) => format!(
                "the newest record (line {line}) alone is over {MAX_ANSWER_CHARS} characters: \
                 read it from the file, or ask without `detail` for its summary"
            ),
            None => format!(
                "the {omitted} oldest of the records asked for are left out to keep the answer \
                 under {MAX_ANSWER_CHARS} characters: ask for fewer with `last`"
            ),
        });
    }
    if query.session.is_some() && matching.is_empty() && !all.is_empty() {
        notes.push(
            "none of the file's records are this session's (a record names its session as \
             `session`; an idealyst from before that wrote none): pass the project's `dir` \
             instead of the session to read every record"
                .into(),
        );
    }
    if !notes.is_empty() {
        out["note"] = json!(notes.join("; "));
    }
    out
}

/// When the record's work started, as epoch milliseconds.
fn started_ms(r: &Value) -> Option<i64> {
    let started = r["started"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(started).ok().map(|t| t.timestamp_millis())
}

/// Whether the record's work ran cargo: a rebuild decision, a hot patch
/// that fell back to a rebuild, or a build with no decision (the initial
/// and forced builds, and every save of a target that only rebuilds).
fn is_build(r: &Value) -> bool {
    r["cargo"].is_object()
        || r["decision"]["tier"].as_str() == Some("rebuild")
        || (r["decision"]["tier"].is_null() && matches!(r["trigger"].as_str(), Some("initial" | "forced")))
}

/// What work the record's save or build took, in words. The decision's
/// tier alone does not say: an initial or requested build, and every save
/// of a target that only rebuilds (a full-stack server), have none, and a
/// hot patch that could not be built rebuilt instead.
fn kind(r: &Value) -> &'static str {
    let built = r["cargo"].is_object();
    match r["decision"]["tier"].as_str() {
        Some("overlay") => "overlay patch",
        Some("hot_patch") if built || r["patch_failed"].is_string() => "rebuild after a failed hot patch",
        Some("hot_patch") => "hot patch",
        Some("rebuild") => "rebuild",
        Some("unchanged") => "no change",
        Some(_) => "other",
        None => match r["trigger"].as_str() {
            Some("initial") => "initial build",
            Some("forced") => "requested rebuild",
            Some("one_shot") => "one-shot build",
            Some("save") if built => "rebuild",
            Some("save") => "save, not built",
            _ => "build",
        },
    }
}

/// One record cut down to what explains its time.
fn summary(r: &Value) -> Value {
    let changes = r["changes"].as_array().cloned().unwrap_or_default();
    let mut out = json!({
        "started": r["started"],
        "target": r["target"],
        "kind": kind(r),
        "trigger": r["trigger"],
        "outcome": r["outcome"],
        "failed": r["failed"],
        "total_ms": r["total_ms"],
        "changes": changes.iter().take(SUMMARY_CHANGES).collect::<Vec<_>>(),
    });
    let o = out.as_object_mut().expect("an object");
    if changes.len() > SUMMARY_CHANGES {
        o.insert("changes_total".into(), json!(changes.len()));
    }
    if let Some(session) = r["session"].as_u64() {
        o.insert("session".into(), json!(session));
    }
    if let Some(tier) = r["decision"]["tier"].as_str() {
        o.insert("tier".into(), json!(tier));
    }
    if let Some(reason) = r["decision"]["reason"].as_str() {
        o.insert("reason".into(), json!(reason));
    }
    if let Some(why) = r["patch_failed"].as_str() {
        o.insert("patch_failed".into(), json!(why));
    }
    if let Some(stages) = r["stages"].as_array().filter(|s| !s.is_empty()) {
        o.insert("stages".into(), json!(stages));
    }
    if let Some(cargo) = r["cargo"].as_object() {
        let recompiled = cargo.get("recompiled").and_then(Value::as_array).cloned().unwrap_or_default();
        let slowest: Vec<Value> = recompiled
            .iter()
            .take(SUMMARY_CRATES)
            .map(|p| {
                json!({
                    "name": p["name"],
                    "ms": p["ms"],
                    "reasons": p["reasons"].as_array().map(|rs| {
                        rs.iter().filter_map(|x| x["kind"].as_str()).collect::<Vec<_>>()
                    }),
                })
            })
            .collect();
        let causes: Vec<Value> = cargo
            .get("root_causes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|c| {
                let packages = c["packages"].as_array().cloned().unwrap_or_default();
                json!({
                    "kind": c["kind"],
                    "detail": c["detail"],
                    "packages": packages.iter().take(SUMMARY_CAUSE_PACKAGES).collect::<Vec<_>>(),
                    "package_count": packages.len(),
                })
            })
            .collect();
        o.insert(
            "cargo".into(),
            json!({
                "recompiled": recompiled.len(),
                "packages_seen": cargo.get("packages_seen"),
                "root_causes": causes,
                "slowest": slowest,
            }),
        );
    }
    if let Some(patch) = r["hot_patch"].as_object() {
        o.insert("hot_patch".into(), json!(patch));
    }
    if let Some(diffs) = r["diffs"].as_array().filter(|d| !d.is_empty()) {
        let files: Vec<&Value> = diffs.iter().take(SUMMARY_CHANGES).map(|d| &d["path"]).collect();
        o.insert("diffed_files".into(), json!(files));
        if diffs.len() > SUMMARY_CHANGES {
            o.insert("diffed_files_total".into(), json!(diffs.len()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(target: &str, tier: Option<&str>, trigger: &str, total_ms: u64) -> Value {
        let mut r = json!({
            "app": "Lab",
            "started": "2026-10-27T10:00:00-04:00",
            "target": target,
            "trigger": trigger,
            "folded": 0,
            "changes": [{ "path": "src/app.rs", "kind": "rust source" }],
            "changed_crates": ["lab"],
            "outcome": "x",
            "failed": false,
            "total_ms": total_ms,
            "stages": [],
        });
        if let Some(tier) = tier {
            r["decision"] = json!({ "tier": tier, "reason": "a signature changed" });
        }
        r
    }

    fn write(dir: &Path, records: &[Value]) -> PathBuf {
        let path = dir.join(JSONL_FILE);
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn q(last: usize, rebuilds_only: bool, detail: bool) -> Query {
        Query { last, rebuilds_only, detail, session: None }
    }

    /// The project's records live under its package's folder, as the CLI
    /// names it — the package name, not the directory's.
    #[test]
    fn a_project_dir_locates_its_records_by_package_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"kiosk-app\"\nversion = \"0.1.0\"\n").unwrap();
        assert_eq!(
            for_project(dir.path()).unwrap(),
            dir.path().join("target/idealyst/kiosk-app").join(JSONL_FILE)
        );
        assert_eq!(
            beside_log(Path::new("/p/target/idealyst/kiosk-app/dev.log")).unwrap(),
            Path::new("/p/target/idealyst/kiosk-app").join(JSONL_FILE)
        );
        let err = for_project(&dir.path().join("nowhere")).unwrap_err();
        assert!(err.contains("Cargo.toml"), "{err}");
    }

    #[test]
    fn the_last_records_are_returned_oldest_first_and_rebuilds_can_be_picked_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut initial = record("web", None, "initial", 23_000);
        initial["cargo"] = json!({ "recompiled": [], "root_causes": [] });
        let path = write(
            dir.path(),
            &[
                initial,
                record("web", Some("hot_patch"), "save", 769),
                record("web", Some("rebuild"), "save", 2_944),
                record("web", Some("overlay"), "save", 9),
            ],
        );
        let out = read(&path, q(2, false, false));
        assert_eq!(out["total"], 4);
        let ms: Vec<u64> = out["records"].as_array().unwrap().iter().map(|r| r["total_ms"].as_u64().unwrap()).collect();
        assert_eq!(ms, vec![2_944, 9], "the newest two, oldest first");

        let out = read(&path, q(10, true, false));
        assert_eq!(out["matching"], 2);
        let triggers: Vec<&str> =
            out["records"].as_array().unwrap().iter().map(|r| r["trigger"].as_str().unwrap()).collect();
        assert_eq!(triggers, vec!["initial", "save"], "the initial build and the rebuild");
        assert_eq!(out["records"][1]["reason"], "a signature changed");
    }

    /// A summary keeps what explains the time and drops the bulk: the
    /// full crate list and the diff text.
    #[test]
    fn a_summary_keeps_the_causes_and_the_slowest_crates_and_drops_the_bulk() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = record("web", Some("rebuild"), "save", 290_000);
        let recompiled: Vec<Value> = (0..40)
            .map(|i| json!({ "name": format!("c{i}"), "version": "1.0.0", "ms": 40_000 - i * 100, "units": ["lib"],
                             "reasons": [{ "kind": "dependency_rebuilt", "detail": "" }] }))
            .collect();
        r["cargo"] = json!({
            "packages_seen": 480,
            "recompiled": recompiled,
            "root_causes": [{ "kind": "features_changed", "detail": "old: [], new: [x]",
                              "packages": (0..20).map(|i| format!("c{i} 1.0.0")).collect::<Vec<_>>() }],
            "errors": 0,
        });
        r["diffs"] = json!([{ "path": "Cargo.toml", "against": "previous save", "diff": "@@ -1 +1 @@\n-a\n+b\n" }]);
        let path = write(dir.path(), &[r]);

        let s = &read(&path, q(1, false, false))["records"][0];
        assert_eq!(s["cargo"]["recompiled"], 40);
        assert_eq!(s["cargo"]["slowest"].as_array().unwrap().len(), SUMMARY_CRATES);
        assert_eq!(s["cargo"]["slowest"][0]["name"], "c0");
        assert_eq!(s["cargo"]["slowest"][0]["reasons"][0], "dependency_rebuilt");
        assert_eq!(s["cargo"]["root_causes"][0]["kind"], "features_changed");
        assert_eq!(s["cargo"]["root_causes"][0]["package_count"], 20);
        assert_eq!(s["cargo"]["root_causes"][0]["packages"].as_array().unwrap().len(), SUMMARY_CAUSE_PACKAGES);
        assert_eq!(s["diffed_files"][0], "Cargo.toml");
        assert!(s.get("diffs").is_none(), "no diff text in a summary");

        let full = &read(&path, q(1, false, true))["records"][0];
        assert_eq!(full["cargo"]["recompiled"].as_array().unwrap().len(), 40);
        assert!(full["diffs"][0]["diff"].as_str().unwrap().contains("+b"));
    }

    /// Regression: two sessions of one project append to one file, and a
    /// record lands when its work ends. "The last 10" were yesterday's
    /// session's, out of order, and missed the rebuild just requested.
    #[test]
    fn regression_records_are_ordered_by_start_and_a_session_reads_only_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let at = |session: u32, started: &str, ms: u64| {
            let mut r = record("web", Some("rebuild"), "save", ms);
            r["session"] = json!(session);
            r["started"] = json!(started);
            r
        };
        let mut untagged = record("web", Some("rebuild"), "save", 1);
        untagged["started"] = json!("2026-10-27T09:00:00-04:00");
        let path = write(
            dir.path(),
            &[
                untagged,
                // Written after the next one, started before it.
                at(100, "2026-10-28T22:05:00+00:00", 2),
                at(100, "2026-10-28T17:59:00-04:00", 3),
                at(200, "2026-10-29T12:26:52+00:00", 4),
                at(100, "2026-10-28T23:37:05+00:00", 5),
            ],
        );
        let ms = |out: &Value| -> Vec<u64> {
            out["records"].as_array().unwrap().iter().map(|r| r["total_ms"].as_u64().unwrap()).collect()
        };

        let every = read(&path, q(10, false, false));
        assert_eq!(ms(&every), vec![1, 3, 2, 5, 4], "by start, across offsets");

        let mine = read(&path, Query { session: Some(200), ..q(10, false, false) });
        assert_eq!(ms(&mine), vec![4]);
        assert_eq!(mine["records"][0]["session"], 200);
        assert_eq!(mine["records"][0]["line"], 4, "where the full record is in the file");

        let none = read(&path, Query { session: Some(300), ..q(10, false, false) });
        assert_eq!(none["matching"], 0);
        assert!(none["note"].as_str().unwrap().contains("`dir`"), "{none}");
    }

    /// Regression: a summary carried every saved file, and the answer had
    /// no bound — one branch-switch save listing 22 000 characters of
    /// paths took "the last 10" past the client's tool-result limit.
    #[test]
    fn regression_an_answer_stays_under_the_limit_whatever_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut switch = record("web", None, "save", 9_999);
        switch["changes"] = (0..3000).map(|i| json!({ "path": format!("crates/c{i}/src/lib.rs"), "kind": "rust source" })).collect();
        switch["diffs"] = (0..3000).map(|i| json!({ "path": format!("crates/c{i}/src/lib.rs"), "against": "previous save", "diff": "" })).collect();
        let mut records: Vec<Value> = (0..30).map(|i| record("web", Some("rebuild"), "save", i)).collect();
        records.insert(20, switch);
        let path = write(dir.path(), &records);

        let out = read(&path, q(MAX_LAST, false, false));
        let text = serde_json::to_string_pretty(&out).unwrap();
        assert!(text.len() < MAX_ANSWER_CHARS + 2_000, "{} characters", text.len());
        let switch = out["records"].as_array().unwrap().iter().find(|r| r["total_ms"] == 9_999).unwrap();
        assert_eq!(switch["changes"].as_array().unwrap().len(), SUMMARY_CHANGES);
        assert_eq!(switch["changes_total"], 3000);
        assert_eq!(switch["diffed_files"].as_array().unwrap().len(), SUMMARY_CHANGES);
        assert_eq!(switch["diffed_files_total"], 3000);

        // In detail the switch's record is too large to fit beside the rest:
        // the newest come first, and the answer says what it left out.
        let out = read(&path, q(MAX_LAST, false, true));
        let text = serde_json::to_string_pretty(&out).unwrap();
        assert!(text.len() < MAX_ANSWER_CHARS + 2_000, "{} characters", text.len());
        let kept = out["records"].as_array().unwrap();
        assert_eq!(kept.last().unwrap()["total_ms"], 29, "the newest is kept");
        assert_eq!(out["omitted_for_size"].as_u64().unwrap() as usize, 31 - kept.len());
        assert!(out["note"].as_str().unwrap().contains("`last`"), "{out}");

        // Asked for alone, it is pointed at in the file instead.
        let only = write(dir.path(), &records[20..21]);
        let out = read(&only, q(1, false, true));
        assert_eq!(out["records"].as_array().unwrap().len(), 0);
        assert!(out["note"].as_str().unwrap().contains("line 1"), "{out}");
    }

    /// Regression: a summary showed only the decision's tier, which most
    /// records lack (initial and requested builds, a server's saves) and
    /// which says "hot patch" for a save that fell back to a rebuild.
    #[test]
    fn regression_every_record_says_what_kind_of_work_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let mut fell_back = record("web", Some("hot_patch"), "save", 1);
        fell_back["patch_failed"] = json!("a signature changed");
        fell_back["cargo"] = json!({ "recompiled": [], "root_causes": [] });
        let mut server_save = record("server", None, "save", 1);
        server_save["cargo"] = json!({ "recompiled": [], "root_causes": [] });
        let path = write(
            dir.path(),
            &[
                record("web", None, "initial", 1),
                record("web", None, "forced", 1),
                server_save,
                record("web", Some("hot_patch"), "save", 1),
                fell_back,
                record("web", Some("overlay"), "save", 1),
                record("web", Some("unchanged"), "save", 1),
                record("web", None, "save", 1),
            ],
        );
        let out = read(&path, q(10, false, false));
        let kinds: Vec<&str> = out["records"].as_array().unwrap().iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            vec![
                "initial build",
                "requested rebuild",
                "rebuild",
                "hot patch",
                "rebuild after a failed hot patch",
                "overlay patch",
                "no change",
                "save, not built",
            ]
        );
        let builds = read(&path, q(10, true, false));
        let kinds: Vec<&str> = builds["records"].as_array().unwrap().iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["initial build", "requested rebuild", "rebuild", "rebuild after a failed hot patch"]);
    }

    #[test]
    fn a_project_that_never_recorded_says_how_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let out = read(&dir.path().join(JSONL_FILE), q(10, false, false));
        assert_eq!(out["total"], 0);
        assert!(out["note"].as_str().unwrap().contains("metrics: true"), "{out}");
    }
}

