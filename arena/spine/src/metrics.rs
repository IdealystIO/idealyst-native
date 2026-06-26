//! Deterministic transcript pathologies — the cheap, LLM-free fuel for the
//! feedback agent's second pass (process logic). These are mechanically
//! computable from the tool-call log: thrashing, redundant doc re-fetches,
//! flailing. The feedback skill *explains and clusters* them; it doesn't have
//! to *detect* them. Each is simultaneously an MCP-design signal — re-fetching
//! the same doc five times means the doc didn't stick or the tool gave no
//! stable anchor to return to.

use serde::Deserialize;
use std::collections::BTreeMap;

/// One tool invocation from a captured transcript.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Transcript {
    #[serde(default)]
    pub calls: Vec<ToolCall>,
}

impl Transcript {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Pathologies {
    pub total_calls: usize,
    /// Exact `(tool, args)` repeats — the same call made again expecting a
    /// different result. Classic thrashing.
    pub duplicate_calls: usize,
    /// Per documentation lookup `(tool:key)` → times fetched.
    pub doc_fetch_counts: BTreeMap<String, usize>,
    /// Docs fetched more than once, worst offenders first.
    pub repeated_docs: Vec<(String, usize)>,
}

/// Heuristic: which tools are documentation lookups whose repeat-rate is a
/// doc-quality signal. Kept deliberately broad — the MCP surface is
/// `list`/`describe_*`/`search_*`/`*_recipe`/icon catalogs.
fn is_doc_tool(tool: &str) -> bool {
    let t = tool.to_ascii_lowercase();
    t.contains("describe")
        || t.contains("list")
        || t.contains("search")
        || t.contains("recipe")
        || t.contains("docs")
        || t.contains("catalog")
}

/// Primary identifying argument of a doc lookup, used as the fetch key so
/// `describe_component(name=Button)` and `describe_component(name=Card)` count
/// separately while two Button lookups collapse.
fn primary_arg(args: &serde_json::Value) -> String {
    for key in ["name", "id", "component", "query", "tag"] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            return v.to_string();
        }
    }
    String::new()
}

/// Count file reads that escaped the project directory — the agent reaching
/// into the path-dep'd framework source instead of asking the MCP. A high
/// count means the isolation leaked AND the docs didn't satisfy the agent; both
/// are signals. Looks at `Read`/`Glob`/`Grep` paths and `Bash` cat/less/head.
pub fn doc_bypass_reads(t: &Transcript, project_dir: &std::path::Path) -> usize {
    let proj = std::fs::canonicalize(project_dir)
        .unwrap_or_else(|_| project_dir.to_path_buf())
        .to_string_lossy()
        .to_string();

    let mut count = 0;
    for call in &t.calls {
        let outside = match call.tool.as_str() {
            "Read" | "Glob" | "Grep" => call
                .args
                .get("file_path")
                .or_else(|| call.args.get("path"))
                .and_then(|p| p.as_str())
                .map(|p| p.starts_with('/') && !p.starts_with(&proj))
                .unwrap_or(false),
            "Bash" => call
                .args
                .get("command")
                .and_then(|c| c.as_str())
                .map(|c| {
                    let reads_file =
                        ["cat ", "less ", "head ", "tail ", "bat "].iter().any(|p| c.contains(p));
                    // crude: an absolute path that isn't the project, on a read cmd
                    reads_file && c.contains("/crates/") && !c.contains(&proj)
                })
                .unwrap_or(false),
            _ => false,
        };
        if outside {
            count += 1;
        }
    }
    count
}

pub fn analyze(t: &Transcript) -> Pathologies {
    use std::collections::HashSet;

    let mut seen: HashSet<String> = HashSet::new();
    let mut duplicate_calls = 0usize;
    let mut doc_fetch_counts: BTreeMap<String, usize> = BTreeMap::new();

    for call in &t.calls {
        // Canonical key for exact-duplicate detection.
        let args_canon = serde_json::to_string(&call.args).unwrap_or_default();
        let exact_key = format!("{}|{}", call.tool, args_canon);
        if !seen.insert(exact_key) {
            duplicate_calls += 1;
        }

        if is_doc_tool(&call.tool) {
            let key = format!("{}:{}", call.tool, primary_arg(&call.args));
            *doc_fetch_counts.entry(key).or_insert(0) += 1;
        }
    }

    let mut repeated_docs: Vec<(String, usize)> = doc_fetch_counts
        .iter()
        .filter(|(_, &n)| n > 1)
        .map(|(k, &n)| (k.clone(), n))
        .collect();
    repeated_docs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    Pathologies {
        total_calls: t.calls.len(),
        duplicate_calls,
        doc_fetch_counts,
        repeated_docs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(tool: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            tool: tool.into(),
            args,
        }
    }

    #[test]
    fn detects_exact_duplicate_calls() {
        let t = Transcript {
            calls: vec![
                call("describe_component", json!({"name": "Button"})),
                call("describe_component", json!({"name": "Button"})), // dup
                call("describe_component", json!({"name": "Card"})),   // not a dup
            ],
        };
        let p = analyze(&t);
        assert_eq!(p.total_calls, 3);
        assert_eq!(p.duplicate_calls, 1);
    }

    #[test]
    fn flags_repeated_doc_fetches_distinct_from_exact_dupes() {
        // Same doc, fetched via calls that aren't byte-identical still counts
        // as a repeated doc (key is tool + primary arg), but only identical
        // calls count as duplicates.
        let t = Transcript {
            calls: vec![
                call("describe_component", json!({"name": "Button"})),
                call("describe_component", json!({"name": "Button", "verbose": true})),
                call("describe_component", json!({"name": "Button"})),
            ],
        };
        let p = analyze(&t);
        assert_eq!(p.duplicate_calls, 1, "two byte-identical Button calls");
        let button = p.doc_fetch_counts.get("describe_component:Button").copied();
        assert_eq!(button, Some(3), "all three target the Button doc");
        assert_eq!(p.repeated_docs.first().unwrap().1, 3);
    }

    #[test]
    fn non_doc_tools_are_not_counted_as_fetches() {
        let t = Transcript {
            calls: vec![
                call("robot_click", json!({"id": "fab"})),
                call("robot_click", json!({"id": "fab"})),
            ],
        };
        let p = analyze(&t);
        assert!(p.doc_fetch_counts.is_empty());
        assert_eq!(p.duplicate_calls, 1);
    }
}
