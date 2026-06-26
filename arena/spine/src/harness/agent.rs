//! Distill a subagent's transcript into what the spine scores over.
//!
//! The implementation agent is no longer a `claude --print` subprocess this
//! crate spawns — it's a **subagent** the orchestrating Claude Code session
//! runs (see `.claude/agents/arena-implementer.md` + `.claude/skills/arena-bench`).
//! Driving it that way keeps the run on the subscription instead of pay-as-you-go
//! API billing, and lets the agent be hard-isolated to the idealyst MCP via the
//! subagent's `mcpServers:` frontmatter (the equivalent of
//! `claude -p --strict-mcp-config`).
//!
//! Claude Code writes every subagent's event log to
//! `~/.claude/projects/<proj>/<session-id>/subagents/agent-<agentId>.jsonl`.
//! This module parses one of those files into:
//!   * a [`crate::metrics::Transcript`] (tool calls) for the pathology pass, and
//!   * token totals, including an estimate of MCP **payload** tokens — the
//!     doc-bloat dial, measured as the size of idealyst tool *results*.

use crate::metrics::{ToolCall, Transcript};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What a parsed subagent transcript yields.
#[derive(Debug, Clone)]
pub struct AgentRun {
    pub transcript: Transcript,
    /// Input-side tokens (uncached input + cache creation + cache reads), summed
    /// across the agent's assistant turns. Not a billing figure — a *consistent*
    /// effort proxy. Mid-2026 subagent jsonl token counts are known to undercount
    /// (~2×); fine for the token-bonus tiebreaker, which is relative and strictly
    /// smaller than the smallest rubric item.
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

/// Parse a subagent transcript file (`agent-<id>.jsonl`) from disk.
pub fn load_session_jsonl(path: &Path) -> anyhow::Result<AgentRun> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("reading subagent transcript {}: {e}", path.display()))?;
    let mut run = parse_session_jsonl(&bytes);
    run.transcript_path = path.to_path_buf();
    Ok(run)
}

/// Parse a Claude Code subagent JSONL log into a transcript + token totals.
/// Defensive throughout: the schema is broad and we only pull the fields we
/// understand, so unrelated event types (`queue-operation`, `attachment`,
/// `file-history-snapshot`, …) are simply ignored.
///
/// Schema (per line, one JSON object): an `assistant` event carries
/// `message.content[]` blocks (a `tool_use` block has `name`/`id`/`input`) and a
/// `message.usage` (`input_tokens`/`cache_*`/`output_tokens`); a `user` event
/// carries `tool_result` blocks (`tool_use_id` + `content`, string or array).
/// Unlike `claude -p`'s `stream-json` there is no terminal `result` event with
/// cumulative usage, so token totals are summed over the assistant turns.
pub fn parse_session_jsonl(bytes: &[u8]) -> AgentRun {
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
                let msg = ev.get("message");
                accumulate_usage(msg.and_then(|m| m.get("usage")), &mut input_tokens, &mut output_tokens);
                if let Some(content) = msg
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("tool_use") => {
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
                            Some("text") => {
                                // The agent's last prose is its final message —
                                // keep the most recent so callers can surface it.
                                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                    if !t.trim().is_empty() {
                                        final_text = t.to_string();
                                    }
                                }
                            }
                            _ => {}
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

/// Add one assistant turn's `usage` to the running totals. Input side folds in
/// cache creation + cache reads so a thrashing agent (which re-reads a large
/// cached context every turn) costs more in the proxy — exactly the behavior the
/// token bonus should penalize. There is no cumulative `result` event in the
/// subagent log, so summation is the only way to total it.
fn accumulate_usage(usage: Option<&Value>, input: &mut u64, output: &mut u64) {
    let Some(u) = usage else { return };
    let get = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    *input += get("input_tokens") + get("cache_creation_input_tokens") + get("cache_read_input_tokens");
    *output += get("output_tokens");
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
    fn parses_tool_calls_summed_tokens_and_mcp_payload() {
        // A trimmed subagent log: two assistant turns (one idealyst MCP call,
        // one Bash call) with per-turn usage, the idealyst result returned as a
        // user tool_result, and a final text block.
        let stream = r#"
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__idealyst__list_components","input":{}}],"usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_read_input_tokens":0,"output_tokens":3}}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"AAAAAAAA"}]}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"idealyst build --web"}}],"usage":{"input_tokens":5,"cache_read_input_tokens":200,"output_tokens":42}}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"a long bash result that is NOT idealyst docs"}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"done"}],"usage":{"input_tokens":1,"output_tokens":0}}}
"#;
        let run = parse_session_jsonl(stream.as_bytes());
        assert_eq!(run.transcript.calls.len(), 2);
        assert_eq!(run.transcript.calls[0].tool, "mcp__idealyst__list_components");
        // input = (10+100+0) + (5+0+200) + (1+0+0) = 316
        assert_eq!(run.input_tokens, 316);
        // output = 3 + 42 + 0 = 45
        assert_eq!(run.output_tokens, 45);
        assert_eq!(run.total_tokens(), 361);
        // 8 chars of idealyst result / 4 = 2 payload tokens; the Bash result
        // (a plain string, non-idealyst) must NOT count.
        assert_eq!(run.mcp_payload_tokens, 2);
        assert_eq!(run.final_text, "done");
    }

    #[test]
    fn non_idealyst_tool_results_dont_count_as_payload() {
        let stream = r#"
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}],"usage":{"input_tokens":5,"output_tokens":1}}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"this is a long bash result that is not docs"}]}]}}
"#;
        let run = parse_session_jsonl(stream.as_bytes());
        assert_eq!(run.mcp_payload_tokens, 0);
    }

    #[test]
    fn ignores_unrelated_event_types() {
        // Real subagent logs interleave queue-operation/attachment/etc lines.
        let stream = r#"
{"type":"queue-operation","foo":1}
{"type":"file-history-snapshot","bar":2}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/x"}}],"usage":{"input_tokens":2,"output_tokens":1}}}
{"type":"ai-title","title":"whatever"}
"#;
        let run = parse_session_jsonl(stream.as_bytes());
        assert_eq!(run.transcript.calls.len(), 1);
        assert_eq!(run.transcript.calls[0].tool, "Read");
        assert_eq!(run.total_tokens(), 3);
    }
}
