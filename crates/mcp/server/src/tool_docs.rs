//! Long-form documentation for this server's own tools, served by the
//! `get_docs` tool.
//!
//! Every tool definition is sent to the client up front and sits in the
//! model's context for the whole session, so a definition carries only what
//! a caller needs to pick the tool: a one-line description and one-line
//! parameter docs. Everything else (return shapes, platform caveats,
//! precedence rules, workflows) lives in the markdown topics under
//! `docs/` and is fetched on demand.
//!
//! A topic is one markdown file. Its `## ` headings split it into sections;
//! a section whose heading names tools (`## tap` or
//! `## get_logs, clear_logs`) is the doc for those tools. The
//! `every_tool_has_a_docs_section` test pins that each registered tool has
//! exactly one such section, so adding a tool without docs fails CI.

use rmcp::model::Tool;
use serde_json::Value;

/// A docs topic: a markdown file under `docs/`.
pub(crate) struct Topic {
    pub name: &'static str,
    /// One line for the `get_docs()` index.
    pub summary: &'static str,
    pub body: &'static str,
}

pub(crate) const TOPICS: &[Topic] = &[
    Topic {
        name: "overview",
        summary: "What this server does, its tool groups, and how get_docs works.",
        body: include_str!("../docs/overview.md"),
    },
    Topic {
        name: "app",
        summary: "The `app` argument: running-app selector (Robot) vs crate filter (catalog).",
        body: include_str!("../docs/app.md"),
    },
    Topic {
        name: "catalog",
        summary: "Catalog tools: list/describe flow, filter syntax, name resolution, freshness, search.",
        body: include_str!("../docs/catalog.md"),
    },
    Topic {
        name: "robot",
        summary: "Driving a running app: elements, input, navigation, signals, screenshots, native parity.",
        body: include_str!("../docs/robot.md"),
    },
    Topic {
        name: "dev",
        summary: "Launching and watching `idealyst dev` sessions.",
        body: include_str!("../docs/dev.md"),
    },
    Topic {
        name: "project",
        summary: "Linting and configuring a project (devcontainer, VS Code).",
        body: include_str!("../docs/project.md"),
    },
];

/// Search results returned in full; the rest are listed by name only.
const MAX_SEARCH_SECTIONS: usize = 5;

/// One heading-delimited chunk of a markdown doc.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Section<'a> {
    /// Heading text without the `#`s; empty for the preamble before the
    /// first heading.
    pub title: &'a str,
    /// The heading line plus everything up to the next heading of the same
    /// or higher level.
    pub text: &'a str,
}

impl<'a> Section<'a> {
    /// Tool names this section documents: its heading split on commas, when
    /// every part is a bare identifier. `## name resolution` names none.
    pub fn keys(&self) -> Vec<&'a str> {
        let parts: Vec<&str> = self.title.split(',').map(str::trim).collect();
        if parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            parts
        } else {
            Vec::new()
        }
    }

    /// The section body without its heading line.
    fn body(&self) -> &'a str {
        if self.title.is_empty() {
            return self.text;
        }
        self.text.split_once('\n').map(|(_, rest)| rest).unwrap_or("").trim_start_matches('\n')
    }
}

/// Split `text` at headings of exactly `level` (`2` → `## `). Deeper
/// headings stay inside their parent section. A non-empty preamble before
/// the first heading becomes an untitled section.
pub(crate) fn split_sections(text: &str, level: usize) -> Vec<Section<'_>> {
    let marker = format!("{} ", "#".repeat(level));
    let mut starts: Vec<usize> = Vec::new();
    let mut offset = 0;
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && line.starts_with(&marker) {
            starts.push(offset);
        }
        offset += line.len();
    }
    let mut out = Vec::new();
    let first = starts.first().copied().unwrap_or(text.len());
    if !text[..first].trim().is_empty() {
        out.push(Section { title: "", text: text[..first].trim_end() });
    }
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(text.len());
        let chunk = &text[start..end];
        let title = chunk.lines().next().unwrap_or("")[marker.len()..].trim();
        out.push(Section { title, text: chunk.trim_end() });
    }
    out
}

pub(crate) fn topic(name: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

/// The topic and `## ` section documenting tool `name`.
pub(crate) fn tool_section(name: &str) -> Option<(&'static Topic, Section<'static>)> {
    TOPICS.iter().find_map(|t| {
        split_sections(t.body, 2)
            .into_iter()
            .find(|s| s.keys().contains(&name))
            .map(|s| (t, s))
    })
}

/// `get_docs` with neither a name nor a query: the topic list, each with the
/// tools it documents.
pub(crate) fn render_index(tools: &[Tool]) -> String {
    let mut out = String::from(
        "# Docs\n\nCall `get_docs(name)` with a topic or tool name; add `query` to keep only matching sections.\n",
    );
    for t in TOPICS {
        out.push_str(&format!("\n- `{}`: {}", t.name, t.summary));
        let covered: Vec<&str> = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .filter(|n| tool_section(n).is_some_and(|(owner, _)| owner.name == t.name))
            .collect();
        if !covered.is_empty() {
            out.push_str(&format!(" Tools: {}.", covered.join(", ")));
        }
    }
    out.push('\n');
    out
}

/// One tool's doc page: its summary and parameters (read from the tool's own
/// advertised schema, so they never drift from what the client was sent),
/// then its section of the owning topic.
pub(crate) fn render_tool(tool: &Tool, query: Option<&str>) -> String {
    let mut out = format!("# {}\n\n", tool.name);
    if let Some(desc) = &tool.description {
        out.push_str(desc);
        out.push_str("\n\n");
    }
    let params = render_params(&Value::Object((*tool.input_schema).clone()));
    if !params.is_empty() {
        out.push_str("Parameters:\n");
        out.push_str(&params);
        out.push('\n');
    }
    if let Some((topic, section)) = tool_section(&tool.name) {
        let body = section.body();
        match query.filter(|q| !q.trim().is_empty()) {
            // A tool section is short; a query narrows to its `### `
            // subsections when it has them.
            Some(q) => out.push_str(&render_matches(q, &[(topic.name, body)], 3)),
            None => out.push_str(body),
        }
        out.push_str(&format!("\n\nTopic: `{}`.\n", topic.name));
    }
    out
}

/// A whole topic, or only its sections matching `query`.
pub(crate) fn render_topic(topic: &Topic, query: Option<&str>) -> String {
    match query.filter(|q| !q.trim().is_empty()) {
        Some(q) => render_matches(q, &[(topic.name, topic.body)], 2),
        None => topic.body.to_string(),
    }
}

/// `get_docs(query)` with no name: search every topic.
pub(crate) fn render_search_all(query: &str) -> String {
    let docs: Vec<(&str, &str)> = TOPICS.iter().map(|t| (t.name, t.body)).collect();
    render_matches(query, &docs, 2)
}

/// Rank the sections of `docs` (split at `level`) by how many distinct
/// query terms each contains, OR-matched and case-insensitive, the same
/// ranking the catalog `search` tool uses. The best few come back in full;
/// the rest are named so the caller can fetch them.
fn render_matches(query: &str, docs: &[(&str, &str)], level: usize) -> String {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut hits: Vec<(usize, &str, Section<'_>)> = Vec::new();
    for &(doc, body) in docs {
        for s in split_sections(body, level) {
            let hay = s.text.to_lowercase();
            let score = terms.iter().filter(|t| hay.contains(t.as_str())).count();
            if score > 0 {
                hits.push((score, doc, s));
            }
        }
    }
    if hits.is_empty() {
        return format!("No sections match `{query}`. `get_docs()` lists every topic.\n");
    }
    // Stable sort keeps document order among equal scores.
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    let mut out = String::new();
    for (_, doc, s) in hits.iter().take(MAX_SEARCH_SECTIONS) {
        if docs.len() > 1 {
            out.push_str(&format!("<!-- {doc} -->\n"));
        }
        out.push_str(s.text);
        out.push_str("\n\n");
    }
    if hits.len() > MAX_SEARCH_SECTIONS {
        let rest: Vec<String> = hits[MAX_SEARCH_SECTIONS..]
            .iter()
            .map(|(_, doc, s)| {
                if s.title.is_empty() { (*doc).to_string() } else { format!("{doc} › {}", s.title) }
            })
            .collect();
        out.push_str(&format!("Also matching: {}.\n", rest.join("; ")));
    }
    out
}

/// One bullet per property: name, type, whether required, and its one-line
/// description.
fn render_params(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return String::new();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = String::new();
    for (name, prop) in props {
        let ty = type_label(prop);
        let req = if required.contains(&name.as_str()) { ", required" } else { "" };
        // Robot tools leave `app` undocumented in their schemas (one
        // explanation in the server instructions instead of forty copies);
        // the doc page links the topic that explains it.
        let desc = match prop.get("description").and_then(Value::as_str) {
            Some(d) => d,
            None if name == "app" => "Running-app selector from `list_apps`; see `get_docs(\"app\")`.",
            None => "",
        };
        let sep = if desc.is_empty() { "" } else { ": " };
        out.push_str(&format!("- `{name}` ({ty}{req}){sep}{desc}\n"));
    }
    out
}

fn type_label(prop: &Value) -> String {
    if let Some(r) = prop.get("$ref").and_then(Value::as_str) {
        return r.rsplit('/').next().unwrap_or(r).to_string();
    }
    match prop.get("type") {
        Some(Value::String(t)) if t == "array" => match prop.get("items") {
            Some(items) => format!("array of {}", type_label(items)),
            None => "array".into(),
        },
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(ts)) => {
            ts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | ")
        }
        _ => "any JSON".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sections_keeps_subsections_inside_their_parent() {
        let doc = "# T\n\nintro\n\n## a\n\none\n\n### deep\n\nx\n\n## b, c\n\ntwo\n";
        let s = split_sections(doc, 2);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].title, "");
        assert_eq!(s[1].title, "a");
        assert!(s[1].text.contains("### deep"));
        assert_eq!(s[2].keys(), vec!["b", "c"]);
    }

    #[test]
    fn headings_inside_code_fences_do_not_split() {
        let doc = "## a\n\n```md\n## not a heading\n```\n\n## b\n";
        let titles: Vec<_> = split_sections(doc, 2).iter().map(|s| s.title).collect();
        assert_eq!(titles, vec!["a", "b"]);
    }

    #[test]
    fn prose_headings_name_no_tools() {
        let doc = "## name resolution\n\nx\n";
        assert!(split_sections(doc, 2)[0].keys().is_empty());
    }

    #[test]
    fn search_ranks_sections_by_distinct_terms() {
        let out = render_search_all("hold_ms long press");
        let first = out.lines().find(|l| l.starts_with("## ")).unwrap();
        assert_eq!(first, "## drag", "got:\n{out}");
    }

    #[test]
    fn search_with_no_match_says_so() {
        assert!(render_search_all("zzqqxxnotaword").starts_with("No sections match"));
    }

    #[test]
    fn search_lists_overflow_sections_by_name() {
        // `app` appears in nearly every topic, far past the full-text cap.
        let out = render_search_all("app");
        assert!(out.contains("Also matching:"), "got:\n{out}");
    }

    #[test]
    fn topic_query_keeps_only_matching_sections() {
        let robot = topic("robot").unwrap();
        let out = render_topic(robot, Some("adb"));
        assert!(out.contains("## tap\n"));
        assert!(!out.contains("## compare_native_parity"));
    }
}
