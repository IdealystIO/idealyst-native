//! Run a headless `claude` agent and capture what it did.
//!
//! The implementation agent runs with `--strict-mcp-config` against the
//! scenario's isolated `.mcp.json`, so the **only** MCP server it can reach is
//! idealyst (docs + Robot). Web search / fetch are denied. We capture the full
//! `stream-json` event log and distill it into:
//!   * a [`crate::metrics::Transcript`] (tool calls) for the pathology pass, and
//!   * token totals, including an estimate of MCP **payload** tokens — the
//!     doc-bloat dial, measured as the size of idealyst tool *results*.

use crate::metrics::{ToolCall, Transcript};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What an agent run produced.
#[derive(Debug, Clone)]
pub struct AgentRun {
    pub transcript: Transcript,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimated tokens of idealyst-MCP tool *results* pulled into context —
    /// the doc-bloat signal, distinct from total spend.
    pub mcp_payload_tokens: u64,
    pub final_text: String,
    pub transcript_path: PathBuf,
}

impl AgentRun {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Tools the implementation agent may use: code editing, the idealyst CLI +
/// cargo via Bash, and every idealyst MCP tool. Everything else (notably web
/// access and any other MCP server) is denied or excluded by
/// `--strict-mcp-config`.
const ALLOWED_TOOLS: &[&str] = &[
    "Edit",
    "Write",
    "Read",
    "Bash",
    "Glob",
    "Grep",
    "mcp__idealyst",
];
const DENIED_TOOLS: &[&str] = &["WebSearch", "WebFetch"];

/// Drive the implementation agent. `prompt` is the full instruction (preamble +
/// scenario prompt). The agent works in `project_dir`; its event log is written
/// to `<run_dir>/transcript.jsonl`.
pub fn implement(
    prompt: &str,
    project_dir: &Path,
    mcp_config: &Path,
    run_dir: &Path,
    max_budget_usd: f64,
) -> anyhow::Result<AgentRun> {
    std::fs::create_dir_all(run_dir)?;
    let transcript_path = run_dir.join("transcript.jsonl");

    let output = Command::new("claude")
        .arg("--print")
        .args(["--output-format", "stream-json"])
        .arg("--verbose") // stream-json requires verbose to emit per-event lines
        .args(["--mcp-config", &mcp_config.to_string_lossy()])
        .arg("--strict-mcp-config")
        .arg("--no-session-persistence")
        .args(["--max-budget-usd", &format!("{max_budget_usd}")])
        .arg("--allowed-tools")
        .arg(ALLOWED_TOOLS.join(" "))
        .arg("--disallowed-tools")
        .arg(DENIED_TOOLS.join(" "))
        .arg(prompt)
        .current_dir(project_dir)
        .output()
        .map_err(|e| anyhow::anyhow!("running `claude`: {e} (is `claude` on PATH?)"))?;

    std::fs::write(&transcript_path, &output.stdout)?;
    if !output.status.success() && output.stdout.is_empty() {
        anyhow::bail!(
            "`claude` exited {:?} with no output:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut run = parse_stream(&output.stdout);
    run.transcript_path = transcript_path;
    Ok(run)
}

/// Parse a `stream-json` event log into a transcript + token totals. Defensive
/// throughout: the schema is broad and we only pull the fields we understand.
pub fn parse_stream(bytes: &[u8]) -> AgentRun {
    let text = String::from_utf8_lossy(bytes);
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut tool_names: HashMap<String, String> = HashMap::new(); // tool_use_id -> name
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut mcp_payload_chars = 0usize;
    let mut final_text = String::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match ev.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                accumulate_usage(&ev, &mut input_tokens, &mut output_tokens);
                if let Some(content) = ev
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            let name = block
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                                tool_names.insert(id.to_string(), name.clone());
                            }
                            calls.push(ToolCall {
                                tool: name,
                                args: block.get("input").cloned().unwrap_or(Value::Null),
                            });
                        }
                    }
                }
            }
            Some("user") => {
                // Tool results come back as user-role messages; size the
                // idealyst ones to estimate doc-payload tokens.
                if let Some(content) = ev
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                            continue;
                        }
                        let from_idealyst = block
                            .get("tool_use_id")
                            .and_then(|i| i.as_str())
                            .and_then(|id| tool_names.get(id))
                            .map(|n| n.starts_with("mcp__idealyst"))
                            .unwrap_or(false);
                        if from_idealyst {
                            mcp_payload_chars += tool_result_len(block.get("content"));
                        }
                    }
                }
            }
            Some("result") => {
                accumulate_usage(&ev, &mut input_tokens, &mut output_tokens);
                if let Some(r) = ev.get("result").and_then(|r| r.as_str()) {
                    final_text = r.to_string();
                }
            }
            _ => {}
        }
    }

    AgentRun {
        transcript: Transcript { calls },
        input_tokens,
        output_tokens,
        // ~4 chars/token is the standard rough conversion.
        mcp_payload_tokens: (mcp_payload_chars / 4) as u64,
        final_text,
        transcript_path: PathBuf::new(),
    }
}

/// Pull `usage.{input_tokens,output_tokens}` from an event if present. The
/// final `result` event carries cumulative totals; assistant events carry
/// per-turn usage — we take the max so a missing `result` still yields a
/// sensible number without double-counting.
fn accumulate_usage(ev: &Value, input: &mut u64, output: &mut u64) {
    let usage = ev
        .get("usage")
        .or_else(|| ev.get("message").and_then(|m| m.get("usage")));
    if let Some(u) = usage {
        if let Some(i) = u.get("input_tokens").and_then(|v| v.as_u64()) {
            *input = (*input).max(i);
        }
        if let Some(o) = u.get("output_tokens").and_then(|v| v.as_u64()) {
            *output = (*output).max(o);
        }
    }
}

/// Length (in chars) of a tool_result's content, which may be a string or an
/// array of `{type:text,text}` blocks.
fn tool_result_len(content: Option<&Value>) -> usize {
    match content {
        Some(Value::String(s)) => s.len(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .map(|s| s.len())
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_calls_tokens_and_mcp_payload() {
        // A minimal stream: one idealyst MCP call + its result, a plain Bash
        // call, and a final result event with cumulative usage.
        let stream = r#"
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__idealyst__list_components","input":{}}],"usage":{"input_tokens":10,"output_tokens":3}}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"AAAAAAAA"}]}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"idealyst build --web"}}]}}
{"type":"result","result":"done","usage":{"input_tokens":120,"output_tokens":45}}
"#;
        let run = parse_stream(stream.as_bytes());
        assert_eq!(run.transcript.calls.len(), 2);
        assert_eq!(run.transcript.calls[0].tool, "mcp__idealyst__list_components");
        assert_eq!(run.input_tokens, 120);
        assert_eq!(run.output_tokens, 45);
        assert_eq!(run.total_tokens(), 165);
        // 8 chars of idealyst result / 4 = 2 payload tokens; the Bash result
        // (none here) must not count.
        assert_eq!(run.mcp_payload_tokens, 2);
        assert_eq!(run.final_text, "done");
    }

    #[test]
    fn non_idealyst_tool_results_dont_count_as_payload() {
        let stream = r#"
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"this is a long bash result that is not docs"}]}]}}
{"type":"result","result":"ok","usage":{"input_tokens":5,"output_tokens":1}}
"#;
        let run = parse_stream(stream.as_bytes());
        assert_eq!(run.mcp_payload_tokens, 0);
    }
}
