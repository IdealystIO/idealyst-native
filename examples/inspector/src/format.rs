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

/// `stack_navigator::Presentation` → `stack`, etc. — the leading path
/// segment of the SDK presentation type name.
pub fn short_kind(type_name: &str) -> &str {
    type_name
        .rsplit("::")
        .nth(1)
        .or_else(|| type_name.split("::").next())
        .unwrap_or(type_name)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}
