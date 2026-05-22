//! Walk-through of the framework-mcp catalog.
//!
//! Two run modes:
//!
//! ```text
//! cargo run -p mcp-demo              # prints catalog (default)
//! cargo run -p mcp-demo -- --serve   # launches stdio MCP server
//! cargo run -p mcp-demo -- --watch   # MCP server + file-watch reload
//! cargo run -p mcp-demo -- --check   # lint pass (phase 6)
//! ```
//!
//! The print mode (default) shows two views of the same catalog:
//!
//! 1. **Flat catalog** — exactly what `framework_mcp::catalog_json()`
//!    emits. Each component carries its bare composes idents, not
//!    yet resolved to other entries. This is the wire shape `cargo
//!    idealyst mcp --json-catalog` is destined to produce.
//!
//! 2. **Resolved graph** — `ResolvedCatalog::build()` over the same
//!    entries, with composes idents disambiguated to fully-qualified
//!    names per spec §6 (same module → closest ancestor → workspace).
//!    Also includes the reverse adjacency ("used by") because that's
//!    what an MCP `find_uses(name)` tool would call into.

use framework_mcp::{catalog_json, ComponentEntry, EdgeStatus, EntryRef, ResolvedCatalog};

mod components;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--serve") {
        return run_server(false);
    }
    if args.iter().any(|a| a == "--watch") {
        return run_server(true);
    }
    if args.iter().any(|a| a == "--check") {
        return run_check();
    }
    print_catalog();
}

/// Phase 6: run the lint pass and exit non-zero if anything fired.
/// Output is one line per finding, sorted by FQN.
fn run_check() {
    let cat = framework_mcp::ResolvedCatalog::build();
    let findings = mcp_server::lint_catalog(&cat);
    if findings.is_empty() {
        println!("OK — {} components, no catalog-integrity issues", cat.entries().len());
        return;
    }
    for f in &findings {
        let tag = match f.severity {
            mcp_server::Severity::Warning => "warn",
            mcp_server::Severity::Error => "error",
        };
        println!("[{}] {} — {}", tag, f.fqn, f.message);
    }
    println!("\n{} findings", findings.len());
    std::process::exit(1);
}

/// Launch the MCP server on stdio. Blocks until the client
/// disconnects. With `watch = true`, file changes under this
/// crate's `src/` trigger an in-process catalog reload (see
/// [`mcp_server::run_stdio_with_watch`] for the limitations).
fn run_server(watch: bool) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = rt.block_on(async move {
        if watch {
            let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
            mcp_server::run_stdio_with_watch(vec![src]).await
        } else {
            mcp_server::run_stdio().await
        }
    });
    if let Err(e) = result {
        eprintln!("mcp server exited with error: {:?}", e);
        std::process::exit(1);
    }
}

fn print_catalog() {
    print_header("Flat catalog (raw composes, unresolved)");
    let flat = catalog_json();
    println!("{}", serde_json::to_string_pretty(&flat).unwrap());

    print_header("Resolved graph");
    let cat = ResolvedCatalog::build();
    let mut entries: Vec<&ComponentEntry> = cat.entries().to_vec();
    entries.sort_by_key(|e| (e.module_path, e.name));
    for entry in entries {
        print_entry(&cat, entry);
    }
}

fn print_header(title: &str) {
    println!("\n=== {} ===\n", title);
}

fn print_entry(cat: &ResolvedCatalog, entry: &ComponentEntry) {
    let host_ref = EntryRef::of(entry);
    println!("{}", host_ref.fqn());

    if !entry.docs.is_empty() {
        // Indent every doc line so a multi-paragraph block stays
        // visually grouped with the entry header.
        for line in entry.docs.lines() {
            println!("    /// {}", line);
        }
    }

    if !entry.params.is_empty() {
        println!("    params:");
        for p in entry.params {
            println!("      {}: {}", p.name, p.type_str);
        }
    }

    let deps = cat.dependencies(&host_ref);
    if !deps.is_empty() {
        println!("    composes:");
        for edge in deps {
            match &edge.status {
                EdgeStatus::Resolved { target } => {
                    println!("      {:<20} -> {}", edge.raw_name, target.fqn());
                }
                EdgeStatus::NoMatch => {
                    println!("      {:<20} -> (unresolved)", edge.raw_name);
                }
                EdgeStatus::Ambiguous { candidates } => {
                    let cands: Vec<String> = candidates.iter().map(|c| c.fqn()).collect();
                    println!(
                        "      {:<20} -> (ambiguous: {})",
                        edge.raw_name,
                        cands.join(", ")
                    );
                }
            }
        }
    }

    let users = cat.uses(&host_ref);
    if !users.is_empty() {
        println!("    used by:");
        for u in users {
            println!("      {}", u.fqn());
        }
    }

    println!();
}
