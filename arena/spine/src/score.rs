//! Scoring. Rubric points dominate; token efficiency is a strictly-secondary
//! bonus that can never flip two runs with different rubric scores but always
//! breaks ties in favour of the cheaper run.
//!
//!   final = rubric_points + token_bonus,   token_bonus ∈ [0, ε),  ε < min(item.points)
//!
//! Divergence neutralization: an *outcome* item that fails while its
//! `depends_on` *decision* item passed is removed from both numerator and
//! denominator and emitted as a framework finding — the agent wrote the right
//! code; the platform didn't render it.

use crate::rubric::{ItemClass, Rubric};
use crate::verify::{verifier_for, RunContext, VerifyResult};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub item_id: String,
    /// The item's maximum point value.
    pub points: u32,
    pub passed: bool,
    /// Points actually credited (== points iff passed and not neutralized).
    pub awarded: u32,
    /// Outcome failed but its decision dependency passed → framework finding,
    /// excluded from the denominator.
    pub neutralized: bool,
    /// Tier couldn't run → excluded from both numerator and denominator.
    pub skipped: bool,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoredRun {
    pub outcomes: Vec<Outcome>,
    pub rubric_points: u32,
    /// Sum of points for items that actually counted (not skipped/neutralized).
    pub max_points: u32,
    pub agent_total_tokens: u64,
    pub mcp_payload_tokens: u64,
    pub final_score: f64,
    /// Outcome failures attributable to the framework, not the agent.
    pub framework_findings: Vec<String>,
}

/// Verify every item against a produced project, then score.
pub fn score_run(
    rubric: &Rubric,
    ctx: &RunContext,
    agent_total_tokens: u64,
    mcp_payload_tokens: u64,
    budget: u64,
) -> ScoredRun {
    let mut results: HashMap<String, VerifyResult> = HashMap::new();
    for item in &rubric.items {
        let verifier = verifier_for(item.tier);
        results.insert(item.id.clone(), verifier.verify(item, ctx));
    }
    score_from_results(
        rubric,
        &results,
        agent_total_tokens,
        mcp_payload_tokens,
        budget,
    )
}

/// Pure scoring core: combine pre-computed verifier results into a score.
/// Separated from [`score_run`] so the neutralization + token-bonus logic is
/// unit-testable without touching the filesystem or spawning a compiler.
pub fn score_from_results(
    rubric: &Rubric,
    results: &HashMap<String, VerifyResult>,
    agent_total_tokens: u64,
    mcp_payload_tokens: u64,
    budget: u64,
) -> ScoredRun {
    let mut outcomes = Vec::with_capacity(rubric.items.len());
    let mut findings = Vec::new();

    for item in &rubric.items {
        let res = results
            .get(&item.id)
            .cloned()
            .unwrap_or_else(|| VerifyResult::skip("no verifier result"));
        let passed = res.passed && !res.skipped;
        let mut neutralized = false;

        if !passed && !res.skipped && item.class == ItemClass::Outcome {
            if let Some(dep) = &item.depends_on {
                let dep_passed = results
                    .get(dep)
                    .map(|d| d.passed && !d.skipped)
                    .unwrap_or(false);
                if dep_passed {
                    neutralized = true;
                    findings.push(format!(
                        "{}: decision '{}' passed but outcome failed — framework finding: {}",
                        item.id, dep, res.evidence
                    ));
                }
            }
        }

        let awarded = if passed { item.points } else { 0 };
        outcomes.push(Outcome {
            item_id: item.id.clone(),
            points: item.points,
            passed,
            awarded,
            neutralized,
            skipped: res.skipped,
            evidence: res.evidence,
        });
    }

    let rubric_points: u32 = outcomes.iter().map(|o| o.awarded).sum();
    let max_points: u32 = outcomes
        .iter()
        .filter(|o| !o.neutralized && !o.skipped)
        .map(|o| o.points)
        .sum();

    let bonus = token_bonus(rubric, agent_total_tokens, budget);
    let final_score = rubric_points as f64 + bonus;

    ScoredRun {
        outcomes,
        rubric_points,
        max_points,
        agent_total_tokens,
        mcp_payload_tokens,
        final_score,
        framework_findings: findings,
    }
}

/// Token bonus in `[0, ε)` with `ε` strictly below the smallest rubric item
/// value, so it orders equal-rubric runs without ever outranking a single
/// extra rubric point. Linear in remaining budget: a run that spends nothing
/// approaches ε; one that spends its whole budget gets 0.
fn token_bonus(rubric: &Rubric, used: u64, budget: u64) -> f64 {
    let min_points = rubric
        .items
        .iter()
        .map(|i| i.points)
        .min()
        .unwrap_or(1)
        .max(1) as f64;
    // Half the smallest item keeps the bonus strictly < any rubric point.
    let epsilon = min_points * 0.5;
    if budget == 0 {
        return 0.0;
    }
    let remaining_frac = (1.0 - used as f64 / budget as f64).clamp(0.0, 1.0);
    epsilon * remaining_frac
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rubric::{Assertion, ItemClass, RubricItem, Tier};

    fn item(id: &str, points: u32, class: ItemClass, depends_on: Option<&str>) -> RubricItem {
        RubricItem {
            id: id.into(),
            description: String::new(),
            points,
            class,
            tier: Tier::Static,
            verifier: "static_ast".into(),
            depends_on: depends_on.map(String::from),
            assertion: Assertion::default(),
        }
    }

    fn rubric(items: Vec<RubricItem>) -> Rubric {
        Rubric {
            scenario_id: "t".into(),
            items,
        }
    }

    #[test]
    fn passed_items_award_full_points() {
        let r = rubric(vec![
            item("a", 10, ItemClass::Decision, None),
            item("b", 20, ItemClass::Decision, None),
        ]);
        let mut res = HashMap::new();
        res.insert("a".to_string(), VerifyResult::pass("ok"));
        res.insert("b".to_string(), VerifyResult::fail("nope"));
        let scored = score_from_results(&r, &res, 0, 0, 1_000_000);
        assert_eq!(scored.rubric_points, 10);
        assert_eq!(scored.max_points, 30);
    }

    #[test]
    fn outcome_failure_with_passing_decision_is_neutralized() {
        // decision passed, outcome failed → framework finding, not a deduction.
        let r = rubric(vec![
            item("uses-x", 10, ItemClass::Decision, None),
            item("x-renders", 20, ItemClass::Outcome, Some("uses-x")),
        ]);
        let mut res = HashMap::new();
        res.insert("uses-x".to_string(), VerifyResult::pass("found x"));
        res.insert("x-renders".to_string(), VerifyResult::fail("not on screen"));
        let scored = score_from_results(&r, &res, 0, 0, 1_000_000);

        assert_eq!(scored.rubric_points, 10, "only the decision point counts");
        assert_eq!(
            scored.max_points, 10,
            "neutralized outcome leaves the denominator"
        );
        assert_eq!(scored.framework_findings.len(), 1);
        let neutralized = scored.outcomes.iter().find(|o| o.item_id == "x-renders").unwrap();
        assert!(neutralized.neutralized);
    }

    #[test]
    fn outcome_failure_with_failing_decision_is_the_agents_fault() {
        // both failed → the agent never wrote it right; full deduction, no finding.
        let r = rubric(vec![
            item("uses-x", 10, ItemClass::Decision, None),
            item("x-renders", 20, ItemClass::Outcome, Some("uses-x")),
        ]);
        let mut res = HashMap::new();
        res.insert("uses-x".to_string(), VerifyResult::fail("no x in source"));
        res.insert("x-renders".to_string(), VerifyResult::fail("not on screen"));
        let scored = score_from_results(&r, &res, 0, 0, 1_000_000);

        assert_eq!(scored.rubric_points, 0);
        assert_eq!(scored.max_points, 30, "nothing neutralized");
        assert!(scored.framework_findings.is_empty());
    }

    #[test]
    fn skipped_items_leave_both_numerator_and_denominator() {
        let r = rubric(vec![
            item("a", 10, ItemClass::Decision, None),
            item("b", 20, ItemClass::Outcome, None),
        ]);
        let mut res = HashMap::new();
        res.insert("a".to_string(), VerifyResult::pass("ok"));
        res.insert("b".to_string(), VerifyResult::skip("tier not wired"));
        let scored = score_from_results(&r, &res, 0, 0, 1_000_000);
        assert_eq!(scored.rubric_points, 10);
        assert_eq!(scored.max_points, 10);
    }

    #[test]
    fn token_bonus_never_outranks_the_smallest_rubric_item() {
        // Smallest item is 10 points → ε = 5, so the max possible bonus (5)
        // can never lift a run past one that earned that 10-point item.
        let r = rubric(vec![item("a", 10, ItemClass::Decision, None)]);
        let mut res = HashMap::new();
        res.insert("a".to_string(), VerifyResult::pass("ok"));
        // Spend literally zero tokens — bonus is at its maximum.
        let best = score_from_results(&r, &res, 0, 0, 1_000_000);
        let bonus = best.final_score - best.rubric_points as f64;
        let smallest_item = 10.0;
        assert!(bonus > 0.0, "zero-token run earns a positive bonus");
        assert!(
            bonus < smallest_item,
            "bonus {bonus} must stay strictly below the smallest rubric item ({smallest_item})"
        );
    }

    #[test]
    fn fewer_tokens_scores_higher_at_equal_rubric() {
        let r = rubric(vec![item("a", 10, ItemClass::Decision, None)]);
        let mut res = HashMap::new();
        res.insert("a".to_string(), VerifyResult::pass("ok"));
        let cheap = score_from_results(&r, &res, 100_000, 0, 1_000_000);
        let pricey = score_from_results(&r, &res, 900_000, 0, 1_000_000);
        assert_eq!(cheap.rubric_points, pricey.rubric_points);
        assert!(cheap.final_score > pricey.final_score);
    }
}
