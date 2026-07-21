//! Qualitative layer — assesses the OUTPUT beyond the rubric, and never
//! touches `final_score`. Two sub-layers, kept strictly apart:
//!
//! * **Deterministic base**: `idealyst lint` over the produced tree (the
//!   idiom-drift linter — "subjective issues made mechanical"). Pure code,
//!   no LLM.
//! * **Judge residue**: an `arena-quality` subagent grades the fixed
//!   dimensions the linter can't see (visual polish, layout sanity,
//!   accessibility labelling, error-state handling) on an anchored 0–4
//!   scale with evidence REQUIRED per grade. The judge's JSON is validated
//!   here against a fixed schema — out-of-range scores or missing evidence
//!   are rejected, so an opinionated judge can't smuggle in a number.
//!
//! Artifacts: `quality.json` (merged) + `quality.md` (human view), both in
//! the run dir, separate from `report.md`/`scored.json` — the quantitative
//! score stays LLM-free by construction.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// Judge dimensions are FIXED — the judge fills in scores, it doesn't invent
/// axes. Keep ids stable; aggregation keys on them.
pub const JUDGE_DIMENSIONS: &[&str] = &[
    "visual-polish",
    "layout-sanity",
    "accessibility",
    "error-handling",
    "code-organization",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintSummary {
    /// `idealyst lint` exit success (no diagnostics).
    pub clean: bool,
    /// Diagnostic count (compiler-message lines in `--format json`).
    pub diagnostics: usize,
    /// First few rendered diagnostics for the report.
    pub sample: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeDimension {
    pub id: String,
    /// Anchored 0–4: 0 broken · 1 poor · 2 acceptable · 3 good · 4 excellent.
    pub score: u8,
    /// Concrete observation justifying the score. Required — an unevidenced
    /// grade is rejected at validation.
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeVerdict {
    pub dimensions: Vec<JudgeDimension>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub lint: LintSummary,
    /// None when no judge pass ran — the deterministic base stands alone.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub judge: Option<JudgeVerdict>,
}

/// Run `idealyst lint --format json` over the produced tree. Lenient about
/// the line stream: anything that isn't the `build-finished` marker counts as
/// a diagnostic line; the marker's `success` field is authoritative.
pub fn run_lint(project_dir: &Path) -> anyhow::Result<LintSummary> {
    let output = Command::new("idealyst")
        .args(["lint", "--format", "json"])
        .current_dir(project_dir)
        .output()
        .map_err(|e| anyhow::anyhow!("running `idealyst lint`: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut clean = output.status.success();
    let mut diagnostics = 0usize;
    let mut sample = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("reason").and_then(|r| r.as_str()) {
            Some("build-finished") => {
                if let Some(s) = v.get("success").and_then(|s| s.as_bool()) {
                    clean = s && diagnostics == 0;
                }
            }
            _ => {
                diagnostics += 1;
                if sample.len() < 5 {
                    let rendered = v
                        .pointer("/message/rendered")
                        .and_then(|r| r.as_str())
                        .map(|s| s.lines().next().unwrap_or(s).to_string())
                        .unwrap_or_else(|| truncate(line, 160));
                    sample.push(rendered);
                }
            }
        }
    }
    Ok(LintSummary {
        clean,
        diagnostics,
        sample,
    })
}

/// Validate a judge verdict against the fixed contract: known dimension ids
/// only, scores in 0..=4, evidence non-empty. Rejecting here is what keeps
/// the LLM a *grader on rails* rather than a free-form judge.
pub fn validate_judge(v: &JudgeVerdict) -> anyhow::Result<()> {
    for d in &v.dimensions {
        anyhow::ensure!(
            JUDGE_DIMENSIONS.contains(&d.id.as_str()),
            "unknown judge dimension {:?}; allowed: {JUDGE_DIMENSIONS:?}",
            d.id
        );
        anyhow::ensure!(d.score <= 4, "dimension {:?} score {} out of 0..=4", d.id, d.score);
        anyhow::ensure!(
            !d.evidence.trim().is_empty(),
            "dimension {:?} has no evidence — unevidenced grades are rejected",
            d.id
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for d in &v.dimensions {
        anyhow::ensure!(seen.insert(&d.id), "duplicate judge dimension {:?}", d.id);
    }
    Ok(())
}

/// The prompt contract for the `arena-quality` judge subagent. Pure, like
/// [`crate::harness::feedback::build_feedback_prompt`].
pub fn build_judge_prompt(project_dir: &Path, screenshot_dir: Option<&Path>) -> String {
    let screenshots = screenshot_dir
        .map(|d| format!("Screenshots of the running app are in `{}`.", d.display()))
        .unwrap_or_else(|| "No screenshots are available; grade visual dimensions from source only and say so in the evidence.".into());
    format!(
        "You are the arena's QUALITY judge. You grade the produced app on FIXED dimensions — \
         you never invent axes, never comment on the score, and every grade needs concrete \
         evidence.\n\n\
         Project source: `{project}`\n{screenshots}\n\n\
         Grade each of these dimensions on the anchored scale \
         0 broken · 1 poor · 2 acceptable · 3 good · 4 excellent:\n{dims}\n\n\
         Respond with ONLY one JSON object, no other text:\n\
         {{\"dimensions\": [{{\"id\": \"<dimension>\", \"score\": <0-4>, \"evidence\": \"<one concrete observation>\"}}, …], \
         \"summary\": \"<two sentences at most>\"}}",
        project = project_dir.display(),
        dims = JUDGE_DIMENSIONS
            .iter()
            .map(|d| format!("- {d}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn render_markdown(q: &QualityReport) -> String {
    let mut out = String::new();
    out.push_str("# Quality (non-scoring)\n\n");
    out.push_str(&format!(
        "**Lint:** {} ({} diagnostic(s))\n\n",
        if q.lint.clean { "clean" } else { "ISSUES" },
        q.lint.diagnostics
    ));
    for s in &q.lint.sample {
        out.push_str(&format!("- {s}\n"));
    }
    if let Some(j) = &q.judge {
        out.push_str("\n## Judge (anchored 0–4, evidence-required)\n\n");
        out.push_str("| dimension | score | evidence |\n|---|---|---|\n");
        for d in &j.dimensions {
            out.push_str(&format!(
                "| {} | {}/4 | {} |\n",
                d.id,
                d.score,
                truncate(&d.evidence.replace('|', "\\|"), 100)
            ));
        }
        if !j.summary.is_empty() {
            out.push_str(&format!("\n{}\n", j.summary));
        }
    } else {
        out.push_str("\n_No judge pass ran — deterministic base only._\n");
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dim(id: &str, score: u8, evidence: &str) -> JudgeDimension {
        JudgeDimension {
            id: id.into(),
            score,
            evidence: evidence.into(),
        }
    }

    #[test]
    fn judge_validation_enforces_the_rails() {
        let ok = JudgeVerdict {
            dimensions: vec![dim("visual-polish", 3, "consistent spacing, clear done-state")],
            summary: String::new(),
        };
        assert!(validate_judge(&ok).is_ok());

        let out_of_range = JudgeVerdict {
            dimensions: vec![dim("visual-polish", 5, "x")],
            summary: String::new(),
        };
        assert!(validate_judge(&out_of_range).is_err(), "score 5 must be rejected");

        let invented_axis = JudgeVerdict {
            dimensions: vec![dim("vibes", 4, "great vibes")],
            summary: String::new(),
        };
        assert!(validate_judge(&invented_axis).is_err(), "unknown dimension must be rejected");

        let unevidenced = JudgeVerdict {
            dimensions: vec![dim("accessibility", 2, "  ")],
            summary: String::new(),
        };
        assert!(validate_judge(&unevidenced).is_err(), "empty evidence must be rejected");

        let dup = JudgeVerdict {
            dimensions: vec![
                dim("accessibility", 2, "labels ok"),
                dim("accessibility", 4, "labels great"),
            ],
            summary: String::new(),
        };
        assert!(validate_judge(&dup).is_err(), "duplicate dimension must be rejected");
    }

    #[test]
    fn quality_report_roundtrips_json() {
        let q = QualityReport {
            lint: LintSummary {
                clean: true,
                diagnostics: 0,
                sample: vec![],
            },
            judge: Some(JudgeVerdict {
                dimensions: vec![dim("layout-sanity", 4, "fills viewport, no overflow")],
                summary: "solid".into(),
            }),
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: QualityReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.judge.unwrap().dimensions[0].score, 4);
    }
}
