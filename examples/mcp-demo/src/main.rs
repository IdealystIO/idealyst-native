//! Walk-through of the framework-mcp catalog.
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p mcp-demo
//! ```
//!
//! Prints two views of the same catalog:
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
