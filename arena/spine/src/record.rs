//! Longitudinal results ledger — one CSV pair per scenario, append-only.
//!
//! `arena record` folds a run's artifacts (`scored.json` required;
//! `process.json` / `quality.json` optional) into:
//!
//! * `<out>/<scenario>.csv` — one SUMMARY row per run: score, tokens,
//!   process metrics, judge dimensions. Fixed, flat columns so the file can
//!   be lifted into a database table unchanged.
//! * `<out>/<scenario>-items.csv` — LONG format, one row per rubric item per
//!   run. This is the table per-item pass-rate-over-time queries want.
//!
//! Design constraints: append-only (history is the point), duplicate-guarded
//! by `(run_id)` (re-recording a re-scored run needs `--force`), timestamps
//! as epoch seconds (timezone-free, DB-trivial), no CSV crate (fields are
//! numeric or slug-like; the one free-text-ish field, model, is quoted).

use crate::quality::QualityReport;
use crate::score::ScoredRun;
use std::path::Path;

pub const SUMMARY_HEADER: &str = "recorded_at_epoch,scenario_id,run_id,framework_commit,model,\
rubric_points,max_points,final_score,agent_total_tokens,mcp_payload_tokens,\
total_calls,duplicate_calls,mcp_calls,mcp_errors,mcp_tools_used,doc_bypass_reads,\
items_passed,items_failed,items_skipped,items_neutralized,\
lint_clean,lint_diagnostics,judge_visual_polish,judge_layout_sanity,judge_accessibility,\
judge_error_handling,judge_code_organization";

pub const ITEMS_HEADER: &str =
    "recorded_at_epoch,scenario_id,run_id,item_id,passed,skipped,neutralized,awarded,points";

/// Everything a record needs, pre-loaded by the CLI from the run dir.
pub struct RunRecord<'a> {
    pub scenario_id: &'a str,
    pub run_id: &'a str,
    pub framework_commit: &'a str,
    pub recorded_at_epoch: u64,
    pub scored: &'a ScoredRun,
    pub process: Option<&'a serde_json::Value>,
    pub quality: Option<&'a QualityReport>,
}

fn csv_quote(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn opt_u64(v: Option<&serde_json::Value>, path: &[&str]) -> String {
    let mut cur = match v {
        Some(x) => x,
        None => return String::new(),
    };
    for p in path {
        match cur.get(p) {
            Some(x) => cur = x,
            None => return String::new(),
        }
    }
    cur.as_u64().map(|n| n.to_string()).unwrap_or_default()
}

pub fn summary_row(r: &RunRecord) -> String {
    let s = r.scored;
    let passed = s.outcomes.iter().filter(|o| o.passed).count();
    let skipped = s.outcomes.iter().filter(|o| o.skipped).count();
    let neutralized = s.outcomes.iter().filter(|o| o.neutralized).count();
    let failed = s.outcomes.len() - passed - skipped - neutralized;

    let judge = |id: &str| -> String {
        r.quality
            .and_then(|q| q.judge.as_ref())
            .and_then(|j| j.dimensions.iter().find(|d| d.id == id))
            .map(|d| d.score.to_string())
            .unwrap_or_default()
    };
    let (lint_clean, lint_diags) = match r.quality {
        Some(q) => (
            if q.lint.clean { "true" } else { "false" }.to_string(),
            q.lint.diagnostics.to_string(),
        ),
        None => (String::new(), String::new()),
    };

    [
        r.recorded_at_epoch.to_string(),
        csv_quote(r.scenario_id),
        csv_quote(r.run_id),
        csv_quote(r.framework_commit),
        csv_quote(s.model.as_deref().unwrap_or("")),
        s.rubric_points.to_string(),
        s.max_points.to_string(),
        format!("{:.3}", s.final_score),
        s.agent_total_tokens.to_string(),
        s.mcp_payload_tokens.to_string(),
        opt_u64(r.process, &["pathologies", "total_calls"]),
        opt_u64(r.process, &["pathologies", "duplicate_calls"]),
        opt_u64(r.process, &["pathologies", "mcp_calls"]),
        opt_u64(r.process, &["pathologies", "mcp_errors"]),
        r.process
            .and_then(|p| p.pointer("/pathologies/mcp_tools_used"))
            .and_then(|m| m.as_object())
            .map(|m| m.len().to_string())
            .unwrap_or_default(),
        opt_u64(r.process, &["doc_bypass_reads"]),
        passed.to_string(),
        failed.to_string(),
        skipped.to_string(),
        neutralized.to_string(),
        lint_clean,
        lint_diags,
        judge("visual-polish"),
        judge("layout-sanity"),
        judge("accessibility"),
        judge("error-handling"),
        judge("code-organization"),
    ]
    .join(",")
}

pub fn item_rows(r: &RunRecord) -> Vec<String> {
    r.scored
        .outcomes
        .iter()
        .map(|o| {
            [
                r.recorded_at_epoch.to_string(),
                csv_quote(r.scenario_id),
                csv_quote(r.run_id),
                csv_quote(&o.item_id),
                o.passed.to_string(),
                o.skipped.to_string(),
                o.neutralized.to_string(),
                o.awarded.to_string(),
                o.points.to_string(),
            ]
            .join(",")
        })
        .collect()
}

/// Append `row` (and `items`) to the scenario's CSV pair under `out_dir`,
/// creating files + headers as needed. Returns `false` (and writes nothing)
/// when `run_id` is already recorded and `force` is off — the ledger is
/// append-only, so re-records must be explicit.
pub fn append(
    out_dir: &Path,
    scenario_id: &str,
    run_id: &str,
    summary: &str,
    items: &[String],
    force: bool,
) -> anyhow::Result<bool> {
    std::fs::create_dir_all(out_dir)?;
    let summary_path = out_dir.join(format!("{scenario_id}.csv"));
    let items_path = out_dir.join(format!("{scenario_id}-items.csv"));

    if !force && summary_path.exists() {
        let existing = std::fs::read_to_string(&summary_path)?;
        let needle = format!(",{},", csv_quote(run_id));
        if existing.lines().any(|l| l.contains(&needle)) {
            return Ok(false);
        }
    }

    let mut summary_out = std::fs::read_to_string(&summary_path).unwrap_or_default();
    if summary_out.is_empty() {
        summary_out.push_str(SUMMARY_HEADER);
        summary_out.push('\n');
    }
    summary_out.push_str(summary);
    summary_out.push('\n');
    std::fs::write(&summary_path, summary_out)?;

    let mut items_out = std::fs::read_to_string(&items_path).unwrap_or_default();
    if items_out.is_empty() {
        items_out.push_str(ITEMS_HEADER);
        items_out.push('\n');
    }
    for row in items {
        items_out.push_str(row);
        items_out.push('\n');
    }
    std::fs::write(&items_path, items_out)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::{Outcome, ScoredRun};

    fn scored() -> ScoredRun {
        ScoredRun {
            outcomes: vec![
                Outcome {
                    item_id: "a".into(),
                    points: 10,
                    passed: true,
                    awarded: 10,
                    neutralized: false,
                    skipped: false,
                    evidence: "ok".into(),
                },
                Outcome {
                    item_id: "b".into(),
                    points: 20,
                    passed: false,
                    awarded: 0,
                    neutralized: false,
                    skipped: true,
                    evidence: "tier not wired".into(),
                },
            ],
            rubric_points: 10,
            max_points: 10,
            agent_total_tokens: 1000,
            mcp_payload_tokens: 50,
            final_score: 12.5,
            framework_findings: vec![],
            model: Some("claude-opus-4-8".into()),
        }
    }

    fn record<'a>(s: &'a ScoredRun) -> RunRecord<'a> {
        RunRecord {
            scenario_id: "todo-app",
            run_id: "run-9",
            framework_commit: "abc1234",
            recorded_at_epoch: 1_784_700_000,
            scored: s,
            process: None,
            quality: None,
        }
    }

    #[test]
    fn summary_row_has_header_arity_and_core_fields() {
        let s = scored();
        let row = summary_row(&record(&s));
        assert_eq!(
            row.split(',').count(),
            SUMMARY_HEADER.split(',').count(),
            "row arity must match header (DB-readiness invariant)"
        );
        assert!(row.contains("todo-app"));
        assert!(row.contains("claude-opus-4-8"));
        assert!(row.contains("12.500"));
    }

    #[test]
    fn append_creates_headers_and_guards_duplicates() {
        let tmp = std::env::temp_dir().join(format!("arena-record-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let s = scored();
        let r = record(&s);
        let row = summary_row(&r);
        let items = item_rows(&r);

        assert!(append(&tmp, "todo-app", "run-9", &row, &items, false).unwrap());
        // Duplicate run_id without --force: refused, nothing appended.
        assert!(!append(&tmp, "todo-app", "run-9", &row, &items, false).unwrap());
        let summary = std::fs::read_to_string(tmp.join("todo-app.csv")).unwrap();
        assert_eq!(summary.lines().count(), 2, "header + exactly one row");
        assert!(summary.starts_with(SUMMARY_HEADER));
        // --force appends the re-record.
        assert!(append(&tmp, "todo-app", "run-9", &row, &items, true).unwrap());
        let items_csv = std::fs::read_to_string(tmp.join("todo-app-items.csv")).unwrap();
        assert_eq!(items_csv.lines().count(), 1 + 4, "header + 2 items × 2 records");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
