//! The feedback agent — the arena's real product for improving the MCP. It
//! does NOT change any score. Two passes, exactly as designed:
//!   1. **Rubric-anchored:** for each lost/neutralized item, trace the
//!      transcript to the navigation or comprehension failure that caused it.
//!   2. **Process logic:** explain the deterministic pathologies (thrashing,
//!      repeated doc fetches, doc-bypass reads) into concrete MCP-doc fixes.
//!
//! Pass 2's *detection* is already done in pure code ([`crate::metrics`]); the
//! agent only has to cluster and explain it.
//!
//! The reviewer is an `arena-feedback` **subagent** the orchestrating session
//! runs (see `.claude/skills/arena-bench`) — not a `claude` subprocess this
//! crate spawns — so it stays on the subscription. What lives here is the pure
//! prompt the orchestrator hands that subagent ([`build_feedback_prompt`]); the
//! orchestrator writes the returned Markdown to `<run_dir>/feedback.md`.

use crate::metrics::Pathologies;
use crate::score::ScoredRun;
use std::path::Path;

/// Inputs the feedback agent reasons over for one run.
pub struct FeedbackInputs<'a> {
    pub scenario_id: &'a str,
    pub scenario_prompt: &'a str,
    pub scored: &'a ScoredRun,
    pub pathologies: &'a Pathologies,
    pub doc_bypass_reads: usize,
    pub transcript_path: &'a Path,
}

/// Build the (LLM-free) prompt for the feedback reviewer. Pure: no spawning, no
/// I/O — the orchestrator passes the result to the `arena-feedback` subagent.
pub fn build_feedback_prompt(inputs: &FeedbackInputs) -> String {
    let lost: Vec<String> = inputs
        .scored
        .outcomes
        .iter()
        .filter(|o| !o.passed && !o.skipped)
        .map(|o| {
            let tag = if o.neutralized { "NEUTRALIZED" } else { "LOST" };
            format!("- [{tag}] {} ({} pts): {}", o.item_id, o.points, o.evidence)
        })
        .collect();

    let repeated: Vec<String> = inputs
        .pathologies
        .repeated_docs
        .iter()
        .map(|(d, n)| format!("- {d} ×{n}"))
        .collect();

    format!(
        "You are the arena's FEEDBACK reviewer. Your job is NOT to score — it is to \
         improve the idealyst MCP server's documentation and tools so a future agent does \
         better. Read the transcript at `{transcript}` (a JSONL stream of the implementation \
         agent's tool calls and results) and produce a Markdown report with exactly two sections.\n\n\
         ## Pass 1 — Rubric-anchored\n\
         For each item below the agent lost or that was neutralized, trace the transcript to the \
         specific MCP navigation or comprehension failure that caused it (e.g. \"never called \
         describe_sdk for storage, so it guessed an API that doesn't exist\"). Tie each finding to \
         a concrete doc/tool change.\n\n\
         Items lost/neutralized:\n{lost}\n\n\
         ## Pass 2 — Process logic\n\
         Explain these mechanically-detected pathologies and what they imply about the MCP \
         (a repeated doc fetch means the doc didn't stick or there was no stable anchor to return \
         to; a doc-bypass read means the docs failed to answer something the agent needed):\n\
         - total tool calls: {total}\n\
         - exact duplicate calls: {dupes}\n\
         - doc-bypass reads (framework source read instead of MCP): {bypass}\n\
         - repeated doc fetches:\n{repeated}\n\n\
         Be specific and actionable. Output only the Markdown report.",
        transcript = inputs.transcript_path.display(),
        lost = if lost.is_empty() { "(none)".into() } else { lost.join("\n") },
        total = inputs.pathologies.total_calls,
        dupes = inputs.pathologies.duplicate_calls,
        bypass = inputs.doc_bypass_reads,
        repeated = if repeated.is_empty() { "(none)".into() } else { repeated.join("\n") },
    )
}
