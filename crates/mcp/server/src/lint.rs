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
//! - **Unscoped component** — a first-party component resolves to no
//!   `doc_scope!` (Warning, or Error under `strict_scopes`). Only
//!   emitted once the project has declared at least one scope.
//!
//! The unscoped check lives here — not in the `doc_scope!` macro —
//! because "is this component covered by a scope?" is a whole-graph fact
//! a per-item proc-macro can't see (see `docs/catalog-scopes-spec.md`
//! §7). Scopes are flat, so there's no parent graph to validate.
//!
//! Each finding is a [`LintFinding`] with severity + actionable
//! message. [`run`] / [`run_with`] return the full list so callers can
//! print, exit-code, or marshal to JSON.

use mcp_catalog::{EdgeStatus, EntryRef, ResolvedCatalog};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// Knobs for the lint pass. Default is lenient: scope-membership issues
/// are warnings, applied to every entry.
#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    /// Upgrade unscoped-component findings from Warning to Error,
    /// failing `--check`. Structural scope issues (cycle, dangling
    /// parent) are always errors regardless of this flag.
    pub strict_scopes: bool,
    /// Crate roots considered first-party (the first `::` segment of an
    /// entry's `module_path`). Empty = treat every entry as first-party
    /// (the common single-project case). Scope-membership findings are
    /// emitted only for first-party entries, so a strict project never
    /// fails on unscoped third-party dependencies it can't fix.
    pub first_party_crates: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LintFinding {
    pub severity: Severity,
    pub fqn: String,
    pub message: String,
}

/// Scan the catalog with the default (lenient) options.
pub fn run(cat: &ResolvedCatalog) -> Vec<LintFinding> {
    run_with(cat, &LintOptions::default())
}

/// Scan the catalog and produce one finding per integrity issue.
/// Findings are sorted by `(fqn, message)` for deterministic output.
pub fn run_with(cat: &ResolvedCatalog, opts: &LintOptions) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    scope_membership_findings(cat, opts, &mut findings);

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

/// Flag first-party components that resolve to no scope. Only runs once
/// the project has declared at least one scope (so projects not yet
/// using scopes get no noise). Warning by default, Error under
/// `strict_scopes`.
fn scope_membership_findings(
    cat: &ResolvedCatalog,
    opts: &LintOptions,
    findings: &mut Vec<LintFinding>,
) {
    if cat.scopes().is_empty() {
        return;
    }
    let severity = if opts.strict_scopes {
        Severity::Error
    } else {
        Severity::Warning
    };
    for entry in cat.entries() {
        if !is_first_party(entry.module_path, &opts.first_party_crates) {
            continue;
        }
        if cat.scope_for(entry.module_path).is_none() {
            findings.push(LintFinding {
                severity: severity.clone(),
                fqn: EntryRef::of(entry).fqn(),
                message:
                    "component is not in any doc_scope! — declare one in this module or an ancestor"
                        .to_string(),
            });
        }
    }
}

/// First-party iff the entry's root crate (first `::` segment) is in the
/// configured list — or the list is empty (single-project default).
fn is_first_party(module_path: &str, first_party: &[String]) -> bool {
    if first_party.is_empty() {
        return true;
    }
    let root = module_path.split("::").next().unwrap_or(module_path);
    first_party.iter().any(|c| c == root)
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
    fn unscoped_component_warns_then_errors_under_strict() {
        let json = r#"{"components":[
            {"name":"Lonely","module_path":"crate::elsewhere","file":"f.rs","line":1,
             "docs":"d","composes":[],"params":[]}
        ],"scopes":[
            {"slug":"a","title":"A","module_path":"crate::featurea","order":0}
        ]}"#;
        let cat = ResolvedCatalog::build_from_json(json).unwrap();

        let lenient = run(&cat);
        assert!(
            lenient
                .iter()
                .any(|x| x.severity == Severity::Warning
                    && x.message.contains("not in any doc_scope")),
            "lenient should warn, got {:?}",
            lenient
        );

        let strict = run_with(
            &cat,
            &LintOptions { strict_scopes: true, first_party_crates: Vec::new() },
        );
        assert!(
            strict
                .iter()
                .any(|x| x.severity == Severity::Error
                    && x.message.contains("not in any doc_scope")),
            "strict should error, got {:?}",
            strict
        );
    }

    #[test]
    fn unscoped_not_flagged_when_no_scopes_declared() {
        // A project that hasn't adopted scopes at all gets no scope noise.
        let json = r#"{"components":[
            {"name":"Lonely","module_path":"crate::x","file":"f.rs","line":1,
             "docs":"d","composes":[],"params":[]}
        ],"scopes":[]}"#;
        let cat = ResolvedCatalog::build_from_json(json).unwrap();
        let f = run(&cat);
        assert!(
            !f.iter().any(|x| x.message.contains("doc_scope")),
            "expected no scope findings, got {:?}",
            f
        );
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
