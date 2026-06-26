//! Render a scored run (and the optional transcript pathologies) as Markdown.
//! This is the per-run artifact a human reads; the feedback agent consumes the
//! same data structures directly.

use crate::metrics::Pathologies;
use crate::score::ScoredRun;
use crate::Aggregate;

pub fn render_markdown(scenario_id: &str, scored: &ScoredRun) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Arena run — `{scenario_id}`\n\n"));
    out.push_str(&format!(
        "**Score:** {} / {} rubric points (final {:.3})\n\n",
        scored.rubric_points, scored.max_points, scored.final_score
    ));
    out.push_str(&format!(
        "**Tokens:** {} agent · {} MCP payload\n\n",
        scored.agent_total_tokens, scored.mcp_payload_tokens
    ));

    out.push_str("## Rubric\n\n");
    out.push_str("| item | pts | result | evidence |\n|---|---|---|---|\n");
    for o in &scored.outcomes {
        let status = if o.skipped {
            "skip"
        } else if o.neutralized {
            "framework"
        } else if o.passed {
            "pass"
        } else {
            "FAIL"
        };
        let pts = if o.passed {
            format!("{}/{}", o.awarded, o.points)
        } else {
            format!("0/{}", o.points)
        };
        let evidence = o.evidence.replace('\n', " ").replace('|', "\\|");
        let evidence = truncate(&evidence, 90);
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            o.item_id, pts, status, evidence
        ));
    }
    out.push('\n');

    if !scored.framework_findings.is_empty() {
        out.push_str("## Framework findings (not scored against the agent)\n\n");
        for f in &scored.framework_findings {
            out.push_str(&format!("- {f}\n"));
        }
        out.push('\n');
    }

    out
}

/// Append the transcript pathology section (feedback pass-2 fuel).
pub fn render_pathologies(p: &Pathologies) -> String {
    let mut out = String::new();
    out.push_str("## Process pathologies\n\n");
    out.push_str(&format!(
        "- {} tool calls, {} exact duplicates\n",
        p.total_calls, p.duplicate_calls
    ));
    if p.repeated_docs.is_empty() {
        out.push_str("- no docs re-fetched\n");
    } else {
        out.push_str("- re-fetched docs (doc retention / navigation signal):\n");
        for (doc, n) in &p.repeated_docs {
            out.push_str(&format!("  - `{doc}` ×{n}\n"));
        }
    }
    out.push('\n');
    out
}

/// Cross-run aggregate — the per-item pass-rate is the headline: a low rate is
/// a documentation ambiguity, not mere model variance.
pub fn render_aggregate(scenario_id: &str, agg: &Aggregate) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Aggregate — `{scenario_id}` ({} runs)\n\n",
        agg.runs
    ));
    out.push_str(&format!(
        "**Mean rubric points:** {:.1}  ·  **Mean final:** {:.3}\n\n",
        agg.mean_points, agg.mean_final
    ));
    out.push_str("## Per-item pass-rate\n\n");
    out.push_str("| item | pass-rate | |\n|---|---|---|\n");
    for (item, (passes, total)) in &agg.per_item_pass_rate {
        let rate = if *total == 0 {
            0.0
        } else {
            *passes as f64 / *total as f64
        };
        let flag = if rate < 0.5 {
            "← doc ambiguity?"
        } else if rate < 1.0 {
            "← flaky"
        } else {
            ""
        };
        out.push_str(&format!(
            "| `{item}` | {passes}/{total} ({:.0}%) | {flag} |\n",
            rate * 100.0
        ));
    }
    out.push('\n');
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}
