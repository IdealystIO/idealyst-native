//! Tiered verifiers. Each [`crate::rubric::Tier`] maps to one [`Verifier`]
//! that turns a rubric item + a produced project into a binary
//! [`VerifyResult`]. No verifier returns an opinion — even the
//! Playwright-driven one (which an LLM locator feeds) reduces to a
//! mechanically-true assertion plus evidence.

pub mod compile;
pub mod playwright;
pub mod robot;
pub mod static_ast;

use crate::rubric::{RubricItem, Tier};
use std::path::PathBuf;

/// Everything a verifier may inspect about a single run.
pub struct RunContext {
    /// Root of the project tree the implementation agent produced.
    pub project_dir: PathBuf,
    /// Captured transcript (tool calls + token counts), if available.
    pub transcript_path: Option<PathBuf>,
    /// Directory holding per-item locator verdicts (`<item_id>.json`) emitted
    /// by the `arena-locate` agent for the Playwright tier. `None` when no
    /// locator pass ran (the Playwright tier then skips).
    pub locate_dir: Option<PathBuf>,
}

impl RunContext {
    /// A context for the source-only tiers (compile/static) — no live app,
    /// no locator verdicts.
    pub fn source_only(project_dir: PathBuf) -> Self {
        Self {
            project_dir,
            transcript_path: None,
            locate_dir: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifyResult {
    pub passed: bool,
    /// Human-readable proof of the verdict (matched line, build error,
    /// DOM snippet, …) — surfaced verbatim in the report.
    pub evidence: String,
    /// The tier couldn't run here (e.g. Robot/Playwright before they're
    /// wired). Skipped items score neither for nor against the agent.
    pub skipped: bool,
}

impl VerifyResult {
    pub fn pass(evidence: impl Into<String>) -> Self {
        Self {
            passed: true,
            evidence: evidence.into(),
            skipped: false,
        }
    }
    pub fn fail(evidence: impl Into<String>) -> Self {
        Self {
            passed: false,
            evidence: evidence.into(),
            skipped: false,
        }
    }
    pub fn skip(evidence: impl Into<String>) -> Self {
        Self {
            passed: false,
            evidence: evidence.into(),
            skipped: true,
        }
    }
}

pub trait Verifier {
    fn verify(&self, item: &RubricItem, ctx: &RunContext) -> VerifyResult;
}

pub fn verifier_for(tier: Tier) -> Box<dyn Verifier> {
    match tier {
        Tier::Compile => Box::new(compile::CompileVerifier),
        Tier::Static => Box::new(static_ast::StaticAstVerifier),
        Tier::Robot => Box::new(robot::RobotVerifier),
        Tier::Playwright => Box::new(playwright::PlaywrightVerifier),
    }
}
