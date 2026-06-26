//! Helpers for the Playwright (platform-truth) tier.
//!
//! The locator is no longer a `claude` subprocess this crate spawns — it's an
//! `arena-locator` **subagent** the orchestrating session runs against the
//! served web build (see `.claude/skills/arena-bench`). That keeps it on the
//! subscription and lets it be hard-isolated to the Playwright MCP via the
//! subagent's `mcpServers:` frontmatter.
//!
//! What stays here is the deterministic contract both sides agree on: how to
//! phrase an item's locator task ([`build_prompt`]), how to parse the binary
//! `{passed, evidence}` verdict the locator must return ([`parse_verdict`]), and
//! how to persist it ([`write_verdict`]) so [`crate::verify::playwright`] can
//! read it. The locator is a *locator*, not a *judge*: the prompt forces a
//! binary observable, never an opinion.

use crate::rubric::RubricItem;
use crate::verify::playwright::Verdict;
use std::path::{Path, PathBuf};

/// Write the one-server Playwright MCP config the locator subagent can reference.
pub fn write_playwright_mcp_config(run_dir: &Path) -> anyhow::Result<PathBuf> {
    let cfg = serde_json::json!({
        "mcpServers": {
            "playwright": { "command": "npx", "args": ["@playwright/mcp@latest", "--headless"] }
        }
    });
    let path = run_dir.join("playwright.mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(path)
}

/// Phrase one outcome item as a locator task against `base_url`. The orchestrator
/// hands this to the `arena-locator` subagent verbatim.
pub fn build_prompt(base_url: &str, item: &RubricItem) -> String {
    let a = &item.assertion;
    let mut checks = Vec::new();
    if let Some(role) = &a.expect_role {
        checks.push(format!("an element with accessibility role `{role}`"));
    }
    if let Some(name) = &a.expect_name {
        checks.push(format!("an element whose accessible name is `{name}`"));
    }
    let check = if checks.is_empty() {
        "the action completes without error and the page is in the expected state".to_string()
    } else {
        format!("{} is present and visible", checks.join(" and "))
    };
    let action = a
        .action
        .clone()
        .unwrap_or_else(|| "observe the initial page state".to_string());

    format!(
        "Open {base_url} in the browser using the Playwright MCP tools.\n\
         Perform this action: {action}.\n\
         Then verify: {check}.\n\
         Locate elements by their accessibility role and name (the snapshot's roles), not by CSS.\n\
         Respond with ONLY a single JSON object and no other text, in exactly this shape:\n\
         {{\"passed\": <true|false>, \"evidence\": \"<one sentence describing what you observed>\"}}"
    )
}

/// Persist a parsed verdict to `<locate_dir>/<item_id>.json` for the
/// deterministic [`crate::verify::playwright`] verifier to consume. Always
/// writes something (even a failure) so the verifier has a file to read.
pub fn write_verdict(locate_dir: &Path, item_id: &str, verdict: &Verdict) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(locate_dir)?;
    let verdict_path = locate_dir.join(format!("{item_id}.json"));
    std::fs::write(
        &verdict_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "passed": verdict.passed,
            "evidence": verdict.evidence,
        }))?,
    )?;
    Ok(verdict_path)
}

/// Parse a `{passed, evidence}` verdict, tolerating prose around the JSON by
/// falling back to the first `{`…`}` span. Returns a deterministic failure
/// verdict if nothing parseable is present, so a flaky locator can't crash the run.
pub fn parse_verdict(text: &str) -> Verdict {
    if let Ok(v) = serde_json::from_str::<Verdict>(text.trim()) {
        return v;
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if end > start {
            if let Ok(v) = serde_json::from_str::<Verdict>(&text[start..=end]) {
                return v;
            }
        }
    }
    Verdict {
        passed: false,
        evidence: format!("locator produced no parseable verdict; raw: {}", truncate(text, 200)),
    }
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

    #[test]
    fn parses_a_clean_verdict() {
        let v = parse_verdict(r#"{"passed": true, "evidence": "listitem visible"}"#);
        assert!(v.passed);
        assert_eq!(v.evidence, "listitem visible");
    }

    #[test]
    fn parses_a_verdict_wrapped_in_prose() {
        let v = parse_verdict("Here is my result:\n{\"passed\": false, \"evidence\": \"not found\"}\nDone");
        assert!(!v.passed);
    }

    #[test]
    fn unparseable_text_yields_a_failure_verdict() {
        let v = parse_verdict("the page looked fine to me, trust me");
        assert!(!v.passed);
        assert!(v.evidence.contains("no parseable verdict"));
    }
}
