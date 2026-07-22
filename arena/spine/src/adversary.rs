//! Adversarial architecture review — the expert skeptic layer.
//!
//! Where the quality judge GRADES fixed dimensions, the adversary HUNTS:
//! a framework-expert reviewer primed with the repo's documented pitfalls
//! (the `idealyst-components` / `idealyst-reactivity` skill corpus,
//! CLAUDE.md §9 component standards) and given Read access to the framework
//! source, tasked with refuting the implementation — finding what the
//! rubric, lint, and judge all missed.
//!
//! Non-scoring, like the judge. Its findings are schema-validated here
//! (severity taxonomy, evidence required, rule named) and persisted as
//! `adversary.json` / `adversary.md`. The loop-closer: any finding that is
//! objectively checkable should graduate into a static/robot rubric item in
//! the next scenario iteration — subjective catches hardening into
//! quantitative checks over time.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Broken or will break: incorrect reactivity, leak, borrow abort path.
    Critical,
    /// Works today but violates intended architecture / documented pitfalls.
    Major,
    /// Idiom drift, fragility, or divergence from component standards.
    Minor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// One-sentence defect statement.
    pub claim: String,
    /// file:line (or file range) in the PRODUCED project. Required.
    pub evidence: String,
    /// The pitfall or architecture rule violated — named, not vibes
    /// (e.g. "dispose-on-hide", "stale-set no-op", "CLAUDE.md 9.3
    /// children-inside-macro", "signal props: narrowest capability").
    pub rule: String,
    /// Optional: what an objective rubric item checking this would assert —
    /// the graduation path into the quantitative layer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rubric_candidate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversaryReport {
    pub findings: Vec<Finding>,
    /// Explicit when clean: "reviewed X files, no findings" beats silence.
    #[serde(default)]
    pub summary: String,
}

/// Validate the adversary's output contract: evidence and rule are required
/// per finding (an unevidenced accusation is discarded like an unevidenced
/// grade), and an empty findings list must carry a summary saying what was
/// reviewed — silence is not a verdict.
pub fn validate(r: &AdversaryReport) -> anyhow::Result<()> {
    for f in &r.findings {
        anyhow::ensure!(
            !f.claim.trim().is_empty(),
            "finding with empty claim"
        );
        anyhow::ensure!(
            !f.evidence.trim().is_empty(),
            "finding {:?} has no evidence — unevidenced findings are rejected",
            f.claim
        );
        anyhow::ensure!(
            !f.rule.trim().is_empty(),
            "finding {:?} names no violated rule/pitfall",
            f.claim
        );
    }
    if r.findings.is_empty() {
        anyhow::ensure!(
            !r.summary.trim().is_empty(),
            "empty findings require a summary of what was reviewed"
        );
    }
    Ok(())
}

/// The adversary's task prompt. Pure, like the judge/feedback builders. The
/// pitfall corpus is NOT inlined — the agent reads the live skill files so
/// the prompt can't drift from the repo's own documentation.
pub fn build_prompt(project_dir: &Path, framework_dir: &Path) -> String {
    format!(
        "You are the arena's ADVERSARY — an expert idealyst framework reviewer whose job is to \
         REFUTE this implementation. Assume it has defects the rubric, linter, and quality judge \
         missed; your job is to find them. You never grade, never praise, never fix — you find.\n\n\
         Implementation under review: `{project}`\n\
         Framework source (ground truth for intended architecture): `{framework}`\n\n\
         PREPARE by reading the pitfall corpus first — these files in the framework repo are the \
         canonical sharp-edges documentation:\n\
         - `{framework}/.claude/skills/idealyst-reactivity/SKILL.md` (stale-set no-op, Ref::with \
         borrow abort, set-in-memo, dispose-on-hide, watch/Subscription lifetimes)\n\
         - `{framework}/.claude/skills/idealyst-components/SKILL.md` (component idiom)\n\
         - `{framework}/CLAUDE.md` section 9 (component standards: ui! usage, children-in-macro, \
         conditional rendering, optional callbacks, signal-prop capability narrowing)\n\n\
         Then HUNT, in priority order:\n\
         1. Reactivity defects: signals read outside tracked contexts, effects that won't re-run, \
         update patterns that hit the stale-set no-op, RefCell/Ref borrow-abort paths, leaks from \
         missing on_cleanup / dispose-on-hide, state that desyncs across rebuild.\n\
         2. Architecture violations: patterns the framework source contradicts — verify against \
         `{framework}/crates/` when unsure what the intended usage is; props silently dropped by \
         ui! (unknown props do NOT error); handlers wired to the wrong prop; capability-wider \
         signal props than needed.\n\
         3. Robustness: unhandled error paths, persistence races, id-reuse after delete, \
         platform-divergent assumptions.\n\n\
         RULES: every finding needs file:line evidence from the project, a NAMED rule/pitfall, \
         and severity critical|major|minor (critical = broken or will break; major = works but \
         violates intended architecture; minor = idiom drift). Do not report style preferences. \
         Do not report things the linter already flags. If you find nothing at a tier, move on; \
         if you find nothing at all, say exactly what you reviewed.\n\n\
         Respond with ONLY one JSON object, no other text:\n\
         {{\"findings\": [{{\"severity\": \"critical|major|minor\", \"claim\": \"<one sentence>\", \
         \"evidence\": \"<file:line>\", \"rule\": \"<named pitfall/rule>\", \
         \"rubric_candidate\": \"<optional: objective check this could become>\"}}], \
         \"summary\": \"<what you reviewed / overall shape, two sentences max>\"}}",
        project = project_dir.display(),
        framework = framework_dir.display(),
    )
}

pub fn render_markdown(r: &AdversaryReport) -> String {
    let mut out = String::new();
    out.push_str("# Adversarial review (non-scoring)\n\n");
    if r.findings.is_empty() {
        out.push_str("_No findings._\n\n");
    } else {
        out.push_str("| severity | claim | evidence | rule |\n|---|---|---|---|\n");
        let mut sorted: Vec<&Finding> = r.findings.iter().collect();
        sorted.sort_by_key(|f| match f.severity {
            Severity::Critical => 0,
            Severity::Major => 1,
            Severity::Minor => 2,
        });
        for f in sorted {
            out.push_str(&format!(
                "| {:?} | {} | `{}` | {} |\n",
                f.severity,
                f.claim.replace('|', "\\|"),
                f.evidence.replace('|', "\\|"),
                f.rule.replace('|', "\\|"),
            ));
        }
        out.push('\n');
        let candidates: Vec<&Finding> =
            r.findings.iter().filter(|f| !f.rubric_candidate.is_empty()).collect();
        if !candidates.is_empty() {
            out.push_str("## Rubric candidates (graduate to objective checks)\n\n");
            for f in candidates {
                out.push_str(&format!("- **{}** → {}\n", f.claim, f.rubric_candidate));
            }
            out.push('\n');
        }
    }
    if !r.summary.is_empty() {
        out.push_str(&format!("{}\n", r.summary));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(sev: Severity, claim: &str, evidence: &str, rule: &str) -> Finding {
        Finding {
            severity: sev,
            claim: claim.into(),
            evidence: evidence.into(),
            rule: rule.into(),
            rubric_candidate: String::new(),
        }
    }

    #[test]
    fn validation_enforces_evidence_rule_and_nonsilent_empty() {
        let ok = AdversaryReport {
            findings: vec![finding(
                Severity::Major,
                "toggle handler mutates without signal set",
                "src/app.rs:42",
                "stale-set no-op",
            )],
            summary: String::new(),
        };
        assert!(validate(&ok).is_ok());

        let unevidenced = AdversaryReport {
            findings: vec![finding(Severity::Critical, "it is broken", " ", "vibes")],
            summary: String::new(),
        };
        assert!(validate(&unevidenced).is_err(), "no evidence → rejected");

        let no_rule = AdversaryReport {
            findings: vec![finding(Severity::Minor, "meh", "src/a.rs:1", "  ")],
            summary: String::new(),
        };
        assert!(validate(&no_rule).is_err(), "no named rule → rejected");

        let silent_clean = AdversaryReport {
            findings: vec![],
            summary: String::new(),
        };
        assert!(validate(&silent_clean).is_err(), "empty findings need a summary");

        let spoken_clean = AdversaryReport {
            findings: vec![],
            summary: "Reviewed all 6 modules; no pitfall violations found.".into(),
        };
        assert!(validate(&spoken_clean).is_ok());
    }

    #[test]
    fn report_roundtrips_and_renders() {
        let r = AdversaryReport {
            findings: vec![finding(
                Severity::Critical,
                "items Vec is not reactive",
                "src/app.rs:14",
                "signals-for-ui-state",
            )],
            summary: "one real bug".into(),
        };
        let back: AdversaryReport =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.findings.len(), 1);
        let md = render_markdown(&back);
        assert!(md.contains("Critical"));
        assert!(md.contains("src/app.rs:14"));
    }
}
