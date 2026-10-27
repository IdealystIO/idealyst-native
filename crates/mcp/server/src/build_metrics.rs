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
//! default answer is a summary per record — what was saved, the tier and
//! why, the outcome and time, the root causes and the slowest crates — and
//! `detail` returns the records as written.

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
    let all: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let matching: Vec<&Value> = all.iter().filter(|r| !query.rebuilds_only || is_build(r)).collect();
    let last = query.last.clamp(1, MAX_LAST);
    let start = matching.len().saturating_sub(last);
    let records: Vec<Value> = matching[start..]
        .iter()
        .map(|r| if query.detail { (*r).clone() } else { summary(r) })
        .collect();
    json!({
        "path": path.display().to_string(),
        "total": all.len(),
        "matching": matching.len(),
        "records": records,
    })
}

/// Whether the record's work ran cargo: a rebuild decision, or a build with
/// no decision (the initial and forced builds).
fn is_build(r: &Value) -> bool {
    match r["decision"]["tier"].as_str() {
        Some(tier) => tier == "rebuild",
        None => r["cargo"].is_object() || matches!(r["trigger"].as_str(), Some("initial" | "forced")),
    }
}

/// One record cut down to what explains its time.
fn summary(r: &Value) -> Value {
    let mut out = json!({
        "started": r["started"],
        "target": r["target"],
        "trigger": r["trigger"],
        "tier": r["decision"]["tier"],
        "outcome": r["outcome"],
        "failed": r["failed"],
        "total_ms": r["total_ms"],
        "changes": r["changes"],
    });
    let o = out.as_object_mut().expect("an object");
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
        let files: Vec<&Value> = diffs.iter().map(|d| &d["path"]).collect();
        o.insert("diffed_files".into(), json!(files));
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
        Query { last, rebuilds_only, detail }
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
                record("web", Some("hot patch"), "save", 769),
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

    #[test]
    fn a_project_that_never_recorded_says_how_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let out = read(&dir.path().join(JSONL_FILE), q(10, false, false));
        assert_eq!(out["total"], 0);
        assert!(out["note"].as_str().unwrap().contains("metrics: true"), "{out}");
    }
}
