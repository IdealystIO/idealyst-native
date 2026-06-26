//! The locator pass that feeds the Playwright tier. For each `outcome` item
//! whose tier is `playwright`, we run a headless `claude` agent equipped with
//! ONLY the Playwright MCP, pointed at the running web build. The agent
//! performs the item's action and asserts the expected observable, then returns
//! a strict JSON verdict that we persist to `<locate_dir>/<item_id>.json` for
//! the deterministic [`crate::verify::playwright`] verifier to consume.
//!
//! The agent locates by accessibility role/name — which doubles as a check that
//! the produced UI is actually accessible. It is a *locator*, not a *judge*:
//! the prompt forces a binary `{passed, evidence}` result, never an opinion.

use crate::rubric::RubricItem;
use crate::verify::playwright::Verdict;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Write the one-server Playwright MCP config used by the locator agent.
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

/// Drive one outcome item against `base_url`. Always writes a verdict file
/// (even on failure) so the verifier has something deterministic to read;
/// returns the parsed verdict, or `Err` only when the agent couldn't be run at
/// all (caller treats that as "tier skipped").
pub fn run_item(
    base_url: &str,
    item: &RubricItem,
    mcp_config: &Path,
    locate_dir: &Path,
    budget_usd: f64,
) -> anyhow::Result<Verdict> {
    std::fs::create_dir_all(locate_dir)?;
    let prompt = build_prompt(base_url, item);

    let output = Command::new("claude")
        .arg("--print")
        .args(["--output-format", "json"])
        .args(["--mcp-config", &mcp_config.to_string_lossy()])
        .arg("--strict-mcp-config")
        .arg("--no-session-persistence")
        .args(["--max-budget-usd", &format!("{budget_usd}")])
        .arg("--allowed-tools")
        .arg("mcp__playwright")
        .arg(&prompt)
        .output()
        .map_err(|e| anyhow::anyhow!("running locator `claude`: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result_text = extract_result_text(&stdout).unwrap_or_else(|| stdout.to_string());
    let verdict = parse_verdict(&result_text).unwrap_or(Verdict {
        passed: false,
        evidence: format!(
            "locator produced no parseable verdict; raw: {}",
            truncate(&result_text, 200)
        ),
    });

    let verdict_path = locate_dir.join(format!("{}.json", item.id));
    std::fs::write(
        &verdict_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "passed": verdict.passed,
            "evidence": verdict.evidence,
        }))?,
    )?;
    Ok(verdict)
}

fn build_prompt(base_url: &str, item: &RubricItem) -> String {
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

/// `claude --output-format json` wraps the agent's final text in a `result`
/// field. Pull it out; fall back to the raw stdout otherwise.
fn extract_result_text(stdout: &str) -> Option<String> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    v.get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
}

/// Parse a `{passed, evidence}` verdict, tolerating prose around the JSON by
/// falling back to the first `{`…`}` span.
fn parse_verdict(text: &str) -> Option<Verdict> {
    if let Ok(v) = serde_json::from_str::<Verdict>(text.trim()) {
        return Some(v);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Verdict>(&text[start..=end]).ok()
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
        let v = parse_verdict(r#"{"passed": true, "evidence": "listitem visible"}"#).unwrap();
        assert!(v.passed);
        assert_eq!(v.evidence, "listitem visible");
    }

    #[test]
    fn parses_a_verdict_wrapped_in_prose() {
        let v =
            parse_verdict("Here is my result:\n{\"passed\": false, \"evidence\": \"not found\"}\nDone")
                .unwrap();
        assert!(!v.passed);
    }

    #[test]
    fn extracts_result_from_claude_json_envelope() {
        let env = r#"{"type":"result","result":"{\"passed\": true, \"evidence\": \"ok\"}","total_cost_usd":0.01}"#;
        let inner = extract_result_text(env).unwrap();
        let v = parse_verdict(&inner).unwrap();
        assert!(v.passed);
    }
}
