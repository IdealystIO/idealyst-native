//! Pure `Snapshot -> String` renderers for the inspector panels. Each is
//! called inside a reactive `text(...)`, so the panel re-renders whenever
//! the snapshot signal updates. Defensive against missing/var-shaped JSON
//! (the bridge hand-rolls its JSON).

use serde_json::Value;

use crate::client::Snapshot;

pub fn header(s: &Snapshot) -> String {
    if let Some(err) = &s.error {
        format!("● disconnected — {err}")
    } else if s.connected {
        format!(
            "● connected — {} navigator(s), {} component(s)",
            s.navigators.len(),
            s.components.len()
        )
    } else {
        "○ connecting…".to_string()
    }
}

pub fn navigators(s: &Snapshot) -> String {
    if s.navigators.is_empty() {
        return "(none)".to_string();
    }
    let mut out = String::new();
    for n in &s.navigators {
        let current = if n["is_current"].as_bool().unwrap_or(false) {
            "▶ "
        } else {
            "  "
        };
        let kind = short_kind(n["type_name"].as_str().unwrap_or("?"));
        out.push_str(&format!(
            "{current}{kind}  route={}  depth={}  back={}\n",
            n["active_route"].as_str().unwrap_or("?"),
            n["depth"].as_u64().unwrap_or(0),
            n["can_go_back"].as_bool().unwrap_or(false),
        ));
        if let Some(stack) = n["stack"].as_array() {
            for (i, e) in stack.iter().enumerate() {
                out.push_str(&format!(
                    "      {}. {}  {}\n",
                    i,
                    e["route"].as_str().unwrap_or("?"),
                    e["path"].as_str().unwrap_or(""),
                ));
            }
        }
    }
    out
}

pub fn raw_elements(s: &Snapshot) -> String {
    if s.elements.is_empty() {
        return "(none registered)".to_string();
    }
    let mut out = format!("{} registered:\n", s.elements.len());
    for e in &s.elements {
        let kind = e["kind"].as_str().unwrap_or("?");
        let id = e["id"].as_u64().unwrap_or(0);
        let tid = e["test_id"].as_str().map(|t| format!(" #{t}")).unwrap_or_default();
        let label = e["label"]
            .as_str()
            .filter(|l| !l.is_empty())
            .map(|l| format!("  \"{}\"", truncate(l, 30)))
            .unwrap_or_default();
        out.push_str(&format!("  [{id}] {kind}{tid}{label}\n"));
    }
    out
}

pub fn tree(s: &Snapshot) -> String {
    if s.tree.is_empty() {
        return "(empty)".to_string();
    }
    let mut out = String::new();
    for root in &s.tree {
        render_node(root, 0, &mut out);
    }
    out
}

fn render_node(node: &Value, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let kind = node["kind"].as_str().unwrap_or("?");
    let mut line = format!("{indent}{kind}");
    if let Some(tid) = node["test_id"].as_str() {
        line.push_str(&format!(" #{tid}"));
    }
    if let Some(label) = node["label"].as_str() {
        if !label.is_empty() {
            line.push_str(&format!("  \"{}\"", truncate(label, 40)));
        }
    }
    out.push_str(&line);
    out.push('\n');
    if let Some(children) = node["children"].as_array() {
        for c in children {
            render_node(c, depth + 1, out);
        }
    }
}

pub fn components(s: &Snapshot) -> String {
    if s.components.is_empty() {
        return "(none — components register via `methods! { … }`)".to_string();
    }
    let mut out = String::new();
    for c in &s.components {
        let id = c["instance_id"].as_u64().unwrap_or(0);
        let name = c["name"].as_str().unwrap_or("?");
        out.push_str(&format!("#{id}  {name}\n"));
        if let Some(methods) = c["methods"].as_array() {
            for m in methods {
                let mname = m["name"].as_str().unwrap_or("?");
                let args: Vec<String> = m["args"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|arg| {
                                format!(
                                    "{}: {}",
                                    arg["name"].as_str().unwrap_or("?"),
                                    arg["type"].as_str().unwrap_or("?")
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out.push_str(&format!("      .{}({})\n", mname, args.join(", ")));
            }
        }
    }
    out
}

pub fn perf(s: &Snapshot) -> String {
    let mut out = String::new();
    match &s.arena {
        Some(a) => out.push_str(&format!(
            "arena: signals {}/{}  effects {}/{}  refs {}/{}  subs {}  deps {}\n",
            a["signals_in_use"].as_u64().unwrap_or(0),
            a["signals_total"].as_u64().unwrap_or(0),
            a["effects_in_use"].as_u64().unwrap_or(0),
            a["effects_total"].as_u64().unwrap_or(0),
            a["refs_in_use"].as_u64().unwrap_or(0),
            a["refs_total"].as_u64().unwrap_or(0),
            a["total_subscribers"].as_u64().unwrap_or(0),
            a["total_deps"].as_u64().unwrap_or(0),
        )),
        None => out.push_str("arena: (unavailable)\n"),
    }
    out.push_str("phase counters:\n");
    if let Some(err) = &s.perf_error {
        out.push_str(&format!("  {err}\n"));
    } else if s.perf.is_empty() {
        out.push_str("  (none recorded this interval)\n");
    } else {
        for p in &s.perf {
            out.push_str(&format!(
                "  {:<28} calls={:<6} total={}µs max={}µs\n",
                p["phase"].as_str().unwrap_or("?"),
                p["call_count"].as_u64().unwrap_or(0),
                p["total_us"].as_u64().unwrap_or(0),
                p["max_us"].as_u64().unwrap_or(0),
            ));
        }
    }
    out
}

pub fn signals(s: &Snapshot) -> String {
    if s.signals.is_empty() {
        return "(none — values from `signal!`/`watch_signal` appear here)".to_string();
    }
    let mut out = String::new();
    for sig in &s.signals {
        let name = sig["name"].as_str().unwrap_or("?");
        let value = match &sig["value"] {
            Value::String(st) => st.clone(),
            other => other.to_string(),
        };
        out.push_str(&format!("{name} = {value}\n"));
    }
    out
}

pub fn logs(s: &Snapshot) -> String {
    if s.logs.is_empty() {
        return "(no logs captured)".to_string();
    }
    let mut out = String::new();
    // Most recent last; show the tail.
    for e in s.logs.iter().rev().take(60).collect::<Vec<_>>().iter().rev() {
        out.push_str(&format!(
            "[{}] {}\n",
            e["source"].as_str().unwrap_or("?"),
            e["text"].as_str().unwrap_or(""),
        ));
    }
    out
}

/// `stack_navigator::Presentation` → `stack`, etc. — the leading path
/// segment of the SDK presentation type name.
fn short_kind(type_name: &str) -> &str {
    type_name
        .rsplit("::")
        .nth(1)
        .or_else(|| type_name.split("::").next())
        .unwrap_or(type_name)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}
