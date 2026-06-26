//! Playwright tier: **platform truth**. The actual browser driving happens in
//! the `arena-locate` agent (which has the Playwright MCP); that agent writes a
//! per-item verdict file. This verifier is the deterministic consumer of those
//! verdicts — it never drives a browser itself, so scoring stays reproducible
//! and the LLM stays a *locator*, not a *judge*: the verdict it emits is a
//! binary observable plus evidence, validated here against a fixed schema.
//!
//! Verdict file: `<locate_dir>/<item_id>.json`
//! ```json
//! { "passed": true, "evidence": "listitem \"Buy milk\" visible at (12,40)" }
//! ```

use super::{RunContext, VerifyResult, Verifier};
use crate::rubric::RubricItem;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Verdict {
    pub passed: bool,
    #[serde(default)]
    pub evidence: String,
}

pub struct PlaywrightVerifier;

impl Verifier for PlaywrightVerifier {
    fn verify(&self, item: &RubricItem, ctx: &RunContext) -> VerifyResult {
        let Some(dir) = &ctx.locate_dir else {
            return VerifyResult::skip("no locator pass ran (locate_dir unset)");
        };
        let path = dir.join(format!("{}.json", item.id));
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(_) => {
                return VerifyResult::skip(format!(
                    "no locator verdict at {} — item not driven",
                    path.display()
                ))
            }
        };
        match serde_json::from_str::<Verdict>(&raw) {
            Ok(v) if v.passed => VerifyResult::pass(v.evidence),
            Ok(v) => VerifyResult::fail(if v.evidence.is_empty() {
                "locator reported not-passed (no evidence)".into()
            } else {
                v.evidence
            }),
            Err(e) => VerifyResult::fail(format!("malformed locator verdict {}: {e}", path.display())),
        }
    }
}
