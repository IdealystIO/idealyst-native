//! Phase 6 — `mcp --check` lint pass.
//!
//! Scans the resolved catalog for integrity issues that would make
//! it less useful as an authoring surface. Per spec §8 / §13 the
//! lint is *narrowly scoped to catalog integrity* — it does not
//! enforce project-wide convention (that's the audit system's job,
//! `.claude/audits/`).
//!
//! Findings:
//! - **Missing docs** — component has no `///` lines.
//! - **Unresolved composes** — bare ident in `composes` matched no
//!   `ComponentEntry` (typo, or referencing a built-in primitive
//!   the catalog doesn't know about).
//! - **Ambiguous composes** — bare ident matched more than one
//!   candidate at the same proximity level (spec §6).
//!
//! Each finding is a [`LintFinding`] with severity + actionable
//! message. [`run`] returns the full list so callers can print,
//! exit-code, or marshal to JSON.

use mcp_catalog::{EdgeStatus, EntryRef, ResolvedCatalog};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Warning,
    /// Reserved for future use — currently every finding is a
    /// warning so projects can flip on the lint incrementally
    /// without a wall of errors.
    #[allow(dead_code)]
    Error,
}

#[derive(Debug, Clone)]
pub struct LintFinding {
    pub severity: Severity,
    pub fqn: String,
    pub message: String,
}

/// Scan the catalog and produce one finding per integrity issue.
/// Findings are sorted by `(fqn, message)` for deterministic output.
pub fn run(cat: &ResolvedCatalog) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    for entry in cat.entries() {
        let host = EntryRef::of(entry);
        let fqn = host.fqn();

        if entry.docs.trim().is_empty() {
            findings.push(LintFinding {
                severity: Severity::Warning,
                fqn: fqn.clone(),
                message: "component has no doc comment".to_string(),
            });
        }

        for edge in cat.dependencies(&host) {
            match &edge.status {
                EdgeStatus::NoMatch => {
                    // Built-in primitives like `View` / `Text` aren't
                    // catalog entries, so they'll always show
                    // unresolved. Suppress those — they're not a user
                    // authoring issue. Any other unresolved reference is
                    // a real typo / dangling edge and should be flagged.
                    if is_framework_primitive(edge.raw_name) {
                        continue;
                    }
                    findings.push(LintFinding {
                        severity: Severity::Warning,
                        fqn: fqn.clone(),
                        message: format!(
                            "composes `{}` which resolves to no catalog entry",
                            edge.raw_name
                        ),
                    });
                }
                EdgeStatus::Ambiguous { candidates } => {
                    let cands: Vec<String> = candidates.iter().map(|c| c.fqn()).collect();
                    findings.push(LintFinding {
                        severity: Severity::Warning,
                        fqn: fqn.clone(),
                        message: format!(
                            "composes `{}` is ambiguous: {}",
                            edge.raw_name,
                            cands.join(", ")
                        ),
                    });
                }
                EdgeStatus::Resolved { .. } => {}
            }
        }
    }

    findings.sort_by(|a, b| a.fqn.cmp(&b.fqn).then(a.message.cmp(&b.message)));
    findings
}

/// Match the framework's built-in primitives — these have no
/// `#[component]` entry and *should* show as unresolved in the
/// catalog without that being flagged as a problem. The framework's
/// dispatch is transform-free, so a primitive call site is its exact
/// PascalCase name; match those directly. Keep this list in sync with
/// `crates/runtime/macros/src/ui.rs`'s primitive match arms.
fn is_framework_primitive(raw_name: &str) -> bool {
    matches!(
        raw_name,
        "Text" | "Button" | "View" | "When"
            | "Image" | "Icon" | "TextInput" | "Toggle" | "ScrollView"
            | "Slider" | "WebView" | "Video" | "ActivityIndicator"
            | "FlatList" | "Link" | "Overlay" | "AnchoredOverlay" | "Presence"
            | "Graphics" | "DrawerNavigator" | "CardTabs"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_catalog::{ComponentEntry, EdgeRef};

    fn leak(
        module_path: &'static str,
        name: &'static str,
        docs: &'static str,
        composes: &'static [EdgeRef],
    ) -> &'static ComponentEntry {
        Box::leak(Box::new(ComponentEntry {
            name,
            module_path,
            file: "synthetic.rs",
            line: 0,
            docs,
            composes,
            params: &[],
        }))
        // params: &[] — lint cares about composes + docs, not props.
    }

    #[test]
    fn missing_docs_flagged() {
        let e = leak("crate", "foo", "", &[]);
        let cat = ResolvedCatalog::build_from(vec![e]);
        let f = run(&cat);
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("no doc comment"));
    }

    #[test]
    fn unresolved_user_component_flagged() {
        let host = leak(
            "crate",
            "host",
            "doc",
            Box::leak(Box::new([EdgeRef { name: "Mystery", line: 0 }])),
        );
        let cat = ResolvedCatalog::build_from(vec![host]);
        let f = run(&cat);
        assert!(
            f.iter().any(|x| x.message.contains("Mystery")),
            "got {:?}",
            f
        );
    }

    #[test]
    fn unresolved_framework_primitive_suppressed() {
        // `View` is a built-in; an unresolved `View` edge should NOT
        // become a finding — those are noise, not authoring issues.
        let host = leak(
            "crate",
            "host",
            "doc",
            Box::leak(Box::new([EdgeRef { name: "View", line: 0 }])),
        );
        let cat = ResolvedCatalog::build_from(vec![host]);
        let f = run(&cat);
        assert!(f.is_empty(), "expected no findings, got {:?}", f);
    }

    #[test]
    fn ambiguous_flagged() {
        let a = leak("crate::x", "card", "doc", &[]);
        let b = leak("crate::y", "card", "doc", &[]);
        let host = leak(
            "crate::host_mod",
            "host",
            "doc",
            Box::leak(Box::new([EdgeRef { name: "card", line: 0 }])),
        );
        let cat = ResolvedCatalog::build_from(vec![a, b, host]);
        let f = run(&cat);
        assert!(
            f.iter().any(|x| x.message.contains("ambiguous")),
            "got {:?}",
            f
        );
    }
}
