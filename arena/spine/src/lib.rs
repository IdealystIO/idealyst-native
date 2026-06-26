//! Deterministic evaluation spine for the idealyst MCP arena.
//!
//! The arena measures how well an LLM, given **only** the idealyst MCP server
//! as its documentation + introspection surface, can fulfil a coding task.
//! This crate is the objective half: it never asks an LLM for a judgement.
//! Every score falls out of a compile gate, a static-source assertion, a
//! Robot self-report, or a platform-truth check driven by a locator skill.
//!
//! Pipeline per run:
//!   1. An isolated implementation agent produces a project tree + transcript.
//!   2. [`score::score_run`] verifies each [`rubric::RubricItem`] against that
//!      tree at the item's declared [`rubric::Tier`].
//!   3. [`score`] applies divergence neutralization (an *outcome* item that
//!      fails while its *decision* dependency passed is a framework finding,
//!      not the agent's fault) and computes the final score.
//!   4. [`aggregate`] folds N runs into per-item pass-rates — the headline
//!      signal for what the MCP docs should fix.
//!
//! The two epistemologies the arena keeps strictly separate:
//!   * **Robot** = the framework's self-report (what the agent can see).
//!   * **Playwright** = platform truth (what the evaluator can see).
//! When they disagree, the agent isn't penalized — the gap is surfaced as a
//! framework finding for the feedback pass.

pub mod harness;
pub mod metrics;
pub mod report;
pub mod rubric;
pub mod scenario;
pub mod score;
pub mod verify;

pub use harness::agent::AgentRun;
pub use rubric::{ItemClass, Rubric, RubricItem, Tier};
pub use scenario::{Platform, Scenario};
pub use score::{score_from_results, score_run, Outcome, ScoredRun};
pub use verify::RunContext;

use std::collections::BTreeMap;

/// Cross-run aggregate for one scenario. Per-item pass-rate is the value:
/// an item that passes 8/8 is well-documented; 3/8 points at a doc
/// *ambiguity*, not mere model variance.
#[derive(Debug, Clone)]
pub struct Aggregate {
    pub runs: usize,
    pub mean_points: f64,
    pub mean_final: f64,
    /// item_id -> (passes, total_runs_that_scored_it)
    pub per_item_pass_rate: BTreeMap<String, (usize, usize)>,
}

/// Fold a set of scored runs (same scenario, same rubric) into an aggregate.
pub fn aggregate(runs: &[ScoredRun]) -> Aggregate {
    let n = runs.len();
    let mut per_item: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for run in runs {
        for o in &run.outcomes {
            // Skipped/neutralized items didn't really get scored — don't let
            // them drag a pass-rate denominator they never contributed to.
            if o.skipped || o.neutralized {
                continue;
            }
            let e = per_item.entry(o.item_id.clone()).or_insert((0, 0));
            e.1 += 1;
            if o.passed {
                e.0 += 1;
            }
        }
    }
    let mean_points = if n == 0 {
        0.0
    } else {
        runs.iter().map(|r| r.rubric_points as f64).sum::<f64>() / n as f64
    };
    let mean_final = if n == 0 {
        0.0
    } else {
        runs.iter().map(|r| r.final_score).sum::<f64>() / n as f64
    };
    Aggregate {
        runs: n,
        mean_points,
        mean_final,
        per_item_pass_rate: per_item,
    }
}
