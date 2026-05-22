//! End-to-end test: a `#[component]`-annotated function in this test
//! file should appear in `framework_mcp::entries()` at runtime, with
//! its doc comment captured and its `composes` edges extracted from
//! any `ui!` / `jsx!` invocations in the body. Validates the full
//! emission + `inventory` distributed slice round-trip.
//!
//! Test components return `Primitive` so the real `ui!` / `jsx!`
//! expansions inside the host components actually typecheck — the
//! macro emission has to walk a parseable, compilable AST. Stubs
//! return an empty view; bodies are never invoked at runtime.

use framework_core::Primitive;
use framework_macros::{component, jsx, ui};

#[allow(dead_code)]
pub struct DemoProps {}

/// Doc comment whose text we'll look for in the catalog.
#[component]
pub fn democomponent(_props: &DemoProps) -> u32 {
    unreachable!("integration test body should not run")
}

// ---------------------------------------------------------------------
// Stub child components used by the ui!/jsx! host bodies below. They
// take no props so the generated invocation macros use the zero-arg
// shape (`child_a!()` → `child_a()`) and the macro expansion stays
// self-contained without needing a real props struct. They register
// in the catalog as legitimate `#[component]` entries with empty
// composes.

// `ui!` dispatches by lowercasing the call ident, so the function
// names here are snake_case and the call sites use PascalCase
// (`ChildA()` → macro `childa!()` → `childa()`).
#[component]
pub fn child_a() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

#[component]
pub fn child_b() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

#[component]
pub fn child_c() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

// JSX dispatches by lowercasing the tag name (see jsx.rs:577). All
// lowercase ident names sidestep the case issue: `<jsx_outer>` →
// `jsx_outer!()` → calls `jsx_outer()`.
#[component]
pub fn jsx_outer() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

#[component]
pub fn jsx_inner() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

#[component]
pub fn jsx_fragmented() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

fn nondescript_helper() -> u32 {
    0
}

// ---------------------------------------------------------------------
// Host components — the ones whose `composes` edges the test asserts on.

/// Host with a `ui!` body. The macro should walk the body, find the
/// `ui!`, and record every component-position ident. `nondescript_helper()`
/// is in expression position (a Rust statement, not a JSX child) and
/// must NOT show up — see spec §6.3.
///
/// PascalCase call sites: `ui!`'s parser requires an uppercase first
/// letter to recognize an ident as a component invocation (lowercase
/// idents fall through to plain Rust expressions — see
/// `next_is_component_invocation` in `ui.rs`). The component name
/// recorded in `composes` is the *exact* ident written here.
#[allow(non_snake_case)]
#[component]
pub fn ui_host(_props: &DemoProps) -> u32 {
    let _ = nondescript_helper();
    let _ = ui! {
        ChildA()
        ChildB()
        ChildC()
    };
    0
}

/// Host with a `jsx!` body. Element names from `<name ...>` should be
/// captured; the fragment `<>...</>` has no name and is skipped, but
/// its children are still walked.
#[component]
pub fn jsx_host(_props: &DemoProps) -> u32 {
    let _ = jsx! {
        <jsx_outer>
            <jsx_inner />
        </jsx_outer>
        <>
            <jsx_fragmented />
        </>
    };
    0
}

/// Host that nests children inside a ui! component slot
/// (`ChildA() { ChildB() ChildC() }`). The collector should recurse
/// into the children block and capture all three idents.
#[allow(non_snake_case)]
#[component]
pub fn nested_host(_props: &DemoProps) -> u32 {
    let _ = ui! {
        ChildA() {
            ChildB()
            ChildC()
        }
    };
    0
}

/// Host whose `ui!` body uses `for` to iterate. The visitor walks
/// `UiNode::For.body` so the iterated `ChildA()` is captured.
/// `View` wraps the for-loop because the top-level coercion expects
/// a single `Primitive`, not a `Vec<Primitive>`.
#[allow(non_snake_case)]
#[component]
pub fn for_host(_props: &DemoProps) -> u32 {
    let _ = ui! {
        View() {
            for _i in 0..3 {
                ChildA()
            }
        }
    };
    0
}

/// Host with TWO separate `ui!` invocations. The visitor recurses
/// through the block-statement list and both calls should contribute
/// edges, in source order.
#[allow(non_snake_case)]
#[component]
pub fn multi_ui_host(_props: &DemoProps) -> u32 {
    let _ = ui! { ChildA() };
    let _ = ui! { ChildB() };
    0
}

/// Host whose `ui!` lives at statement position with a trailing
/// semicolon and no `let _ = ...` wrapper. `syn` represents this as
/// `Stmt::Macro`, which the visitor handles via its
/// `visit_stmt_macro` override (separate from `visit_expr_macro`).
#[allow(non_snake_case)]
#[component]
pub fn stmt_macro_host(_props: &DemoProps) -> u32 {
    ui! { ChildA() };
    0
}

// The `docless` component has no `///` lines; the catalog should
// record an empty docs string. Use `//` here so this explanatory
// note doesn't become the fn's docs.
#[component]
pub fn docless(_props: &DemoProps) -> u32 {
    0
}

/// Component
/// with three
/// doc lines.
///
/// Including a blank-line paragraph break.
#[component]
pub fn multiline_docs(_props: &DemoProps) -> u32 {
    0
}

// ---------------------------------------------------------------------
// Cross-module proximity: real `module_path!()` values inside a real
// submodule. Verifies the resolver picks the same-module candidate
// over the root-level duplicate when both share a name.

mod submodule {
    use framework_core::Primitive;
    use framework_macros::{component, ui};

    #[component]
    pub fn ambiguousname() -> Primitive {
        ::framework_core::view(::std::vec::Vec::new())
    }

    #[allow(non_snake_case)]
    #[component]
    pub fn submodule_host(_props: &super::DemoProps) -> u32 {
        let _ = ui! { Ambiguousname() };
        0
    }
}

/// Root-level duplicate of `submodule::ambiguousname`. Resolver must
/// disambiguate per spec §6.
#[component]
pub fn ambiguousname() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

#[allow(non_snake_case)]
#[component]
pub fn root_host_with_dupe(_props: &DemoProps) -> u32 {
    let _ = ui! { Ambiguousname() };
    0
}

// ---------------------------------------------------------------------
// Assertions.

#[test]
fn democomponent_registers_in_catalog() {
    let entries: Vec<_> = framework_mcp::entries().collect();
    let demo = entries.iter().find(|e| e.name == "democomponent");
    let names: Vec<&str> = entries.iter().map(|e| e.name).collect();
    assert!(demo.is_some(), "expected 'democomponent' in catalog; found: {:?}", names);

    let demo = demo.unwrap();
    assert!(
        demo.docs.contains("Doc comment whose text we'll look for"),
        "doc comment not captured: {:?}",
        demo.docs
    );
    assert!(!demo.module_path.is_empty(), "module_path empty");
    assert!(!demo.file.is_empty(), "file empty");
    assert!(demo.line > 0, "line was {}", demo.line);
    assert!(
        demo.composes.is_empty(),
        "democomponent has no ui!/jsx! body, expected empty composes; got {:?}",
        demo.composes
    );
}

#[test]
fn catalog_json_has_versioned_envelope() {
    let json = framework_mcp::catalog_json();
    assert_eq!(json["catalog_version"], 1);
    let components = json["components"].as_array().expect("components is an array");
    let found_demo = components.iter().any(|c| c["name"] == "democomponent");
    assert!(found_demo, "democomponent missing from catalog json: {}", json);
}

#[test]
fn ui_host_records_composed_idents() {
    let entries: Vec<_> = framework_mcp::entries().collect();
    let host = entries
        .iter()
        .find(|e| e.name == "ui_host")
        .expect("ui_host registered");

    let edge_names: Vec<&str> = host.composes.iter().map(|e| e.name).collect();
    assert!(
        edge_names.contains(&"ChildA"),
        "expected ChildA edge; got {:?}",
        edge_names
    );
    assert!(
        edge_names.contains(&"ChildB"),
        "expected ChildB edge; got {:?}",
        edge_names
    );
    assert!(
        edge_names.contains(&"ChildC"),
        "expected ChildC edge; got {:?}",
        edge_names
    );
    assert!(
        !edge_names.contains(&"nondescript_helper"),
        "nondescript_helper is in expression position, must NOT be captured; got {:?}",
        edge_names
    );

    // Per-edge line numbers: on stable Rust the proc_macro crate
    // doesn't expose source spans, so `Span::start().line` is always
    // 0 from inside a real proc-macro. The field is still populated
    // (the catalog shape is forward-compatible) — just don't assert
    // a specific value here. Future stable Rust will light this up.
    for edge in host.composes {
        let _ = edge.line;
    }
}

#[test]
fn jsx_host_records_element_idents() {
    let entries: Vec<_> = framework_mcp::entries().collect();
    let host = entries
        .iter()
        .find(|e| e.name == "jsx_host")
        .expect("jsx_host registered");

    let edge_names: Vec<&str> = host.composes.iter().map(|e| e.name).collect();
    assert!(
        edge_names.contains(&"jsx_outer"),
        "expected jsx_outer edge; got {:?}",
        edge_names
    );
    assert!(
        edge_names.contains(&"jsx_inner"),
        "expected jsx_inner edge (child of jsx_outer); got {:?}",
        edge_names
    );
    assert!(
        edge_names.contains(&"jsx_fragmented"),
        "expected jsx_fragmented edge (child of fragment); got {:?}",
        edge_names
    );
}

#[test]
fn resolver_links_ui_host_to_actual_child_entries() {
    // End-to-end: the real global catalog (populated by `#[component]`
    // emissions) goes through `ResolvedCatalog::build()`. Each edge
    // from `ui_host` should resolve to a real entry, and the reverse
    // lookup on a child should mention `ui_host` as a user.
    let cat = framework_mcp::ResolvedCatalog::build();

    let ui_host = cat
        .entries()
        .iter()
        .find(|e| e.name == "ui_host")
        .copied()
        .expect("ui_host in catalog");
    let ui_host_ref = framework_mcp::EntryRef::of(ui_host);

    let edges = cat.dependencies(&ui_host_ref);
    assert!(!edges.is_empty(), "ui_host has composes edges");
    for edge in edges {
        match &edge.status {
            framework_mcp::EdgeStatus::Resolved { target } => {
                // After normalization (PascalCase → snake_case), the
                // edge's call-site ident and the target entry name
                // must match. `pascal_to_snake` is idempotent on
                // already-snake input, so both sides converge.
                assert_eq!(
                    pascal_to_snake(target.name),
                    pascal_to_snake(edge.raw_name),
                    "edge {:?} resolved to wrong target {:?}",
                    edge.raw_name,
                    target.name,
                );
            }
            other => panic!(
                "edge {:?} expected Resolved (stub component is in same crate), got {:?}",
                edge.raw_name, other
            ),
        }
    }

    // Reverse: `child_a` should report `ui_host` among its users.
    let child_a = cat
        .entries()
        .iter()
        .find(|e| e.name == "child_a")
        .copied()
        .expect("child_a in catalog");
    let users = cat.uses(&framework_mcp::EntryRef::of(child_a));
    assert!(
        users.iter().any(|r| r.name == "ui_host"),
        "expected ui_host in child_a's reverse adjacency; got {:?}",
        users
    );
}

/// `democomponent` has a single `&DemoProps` parameter — the macro
/// should record one `ParamSpec` whose `type_str` names the struct.
#[test]
fn single_struct_param_captured() {
    let entry = find_entry("democomponent");
    assert_eq!(entry.params.len(), 1, "expected one param; got {:?}", entry.params);
    let p = &entry.params[0];
    assert_eq!(p.name, "_props");
    // `quote!` stringifies a borrow with a space; tolerate both
    // forms so future formatter changes don't break the test.
    let normalized: String = p.type_str.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(normalized, "&DemoProps", "got {:?}", p.type_str);
}

/// Zero-arg components should record an empty `params` slice — not
/// panic, not record a sentinel entry.
#[test]
fn zero_arg_records_empty_params() {
    let entry = find_entry("child_a");
    assert!(entry.params.is_empty(), "expected empty; got {:?}", entry.params);
}

/// Positional multi-parameter signature — record every parameter in
/// declaration order. Declared inside this test for tight locality;
/// it registers in the global catalog like any other `#[component]`.
#[allow(non_snake_case)]
#[component]
pub fn positional_host(idx: u32, _label: &'static str) -> u32 {
    let _ = (idx, _label);
    0
}

#[test]
fn positional_params_captured_in_order() {
    let entry = find_entry("positional_host");
    assert_eq!(entry.params.len(), 2, "got {:?}", entry.params);
    assert_eq!(entry.params[0].name, "idx");
    assert_eq!(entry.params[1].name, "_label");
    assert!(
        entry.params[0].type_str.contains("u32"),
        "type_str {:?}",
        entry.params[0].type_str
    );
    assert!(
        entry.params[1].type_str.contains("str"),
        "type_str {:?}",
        entry.params[1].type_str
    );
}

#[test]
fn catalog_json_includes_params_array() {
    let json = framework_mcp::catalog_json();
    let components = json["components"].as_array().expect("components");
    let demo = components
        .iter()
        .find(|c| c["name"] == "democomponent")
        .expect("democomponent in JSON");
    let params = demo["params"].as_array().expect("params is an array");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["name"], "_props");
    assert!(params[0]["type"].is_string());
}

#[test]
fn catalog_json_includes_composes_array() {
    let json = framework_mcp::catalog_json();
    let components = json["components"].as_array().expect("components is an array");
    let host = components
        .iter()
        .find(|c| c["name"] == "ui_host")
        .expect("ui_host present in JSON");

    let composes = host["composes"].as_array().expect("composes is an array");
    assert!(!composes.is_empty(), "ui_host composes should be non-empty in JSON");
    let first = &composes[0];
    assert!(first["name"].is_string(), "edge name should be a string");
    // `line` field is present but value is 0 on stable Rust (see note
    // in `ui_host_records_composed_idents`).
    assert!(first["line"].is_number(), "edge line should be a number");
}

/// `Foo() { Bar() }` in a `ui!` body — the collector must recurse
/// into nested children to catch `Bar`.
#[test]
fn nested_children_are_captured() {
    let entry = find_entry("nested_host");
    let names: Vec<&str> = entry.composes.iter().map(|e| e.name).collect();
    assert!(names.contains(&"ChildA"), "got {:?}", names);
    assert!(names.contains(&"ChildB"), "nested child not captured: {:?}", names);
    assert!(names.contains(&"ChildC"), "nested child not captured: {:?}", names);
}

/// `for i in iter { Foo() }` — visitor must descend into `UiNode::For.body`.
#[test]
fn for_loop_body_is_captured() {
    let entry = find_entry("for_host");
    let names: Vec<&str> = entry.composes.iter().map(|e| e.name).collect();
    assert!(
        names.contains(&"ChildA"),
        "for-body child not captured: {:?}",
        names
    );
}

/// Two separate `ui!` blocks in one body — both should contribute edges.
#[test]
fn multiple_ui_invocations_all_captured() {
    let entry = find_entry("multi_ui_host");
    let names: Vec<&str> = entry.composes.iter().map(|e| e.name).collect();
    assert!(names.contains(&"ChildA"), "first ui! lost: {:?}", names);
    assert!(names.contains(&"ChildB"), "second ui! lost: {:?}", names);
}

/// `ui! { ... };` at statement position (no `let _ = ...` wrapper)
/// must still be picked up — the visitor has a `visit_stmt_macro`
/// arm for this case.
#[test]
fn stmt_macro_position_is_captured() {
    let entry = find_entry("stmt_macro_host");
    let names: Vec<&str> = entry.composes.iter().map(|e| e.name).collect();
    assert!(
        names.contains(&"ChildA"),
        "stmt-position ui! lost: {:?}",
        names
    );
}

/// A `#[component]` with no `///` lines should produce an empty docs
/// string — no panic, no placeholder text.
#[test]
fn docless_component_records_empty_docs() {
    let entry = find_entry("docless");
    assert_eq!(entry.docs, "", "expected empty docs; got {:?}", entry.docs);
}

/// Multi-line doc comments — the macro joins them with `\n`. The
/// trailing-space stripper trims the rustc-injected leading space on
/// each line but preserves the line break.
#[test]
fn multiline_docs_preserve_newlines() {
    let entry = find_entry("multiline_docs");
    assert!(entry.docs.contains('\n'), "expected newlines; got {:?}", entry.docs);
    assert!(entry.docs.contains("Component"));
    assert!(entry.docs.contains("with three"));
    assert!(entry.docs.contains("doc lines"));
    // The blank-line paragraph break round-trips as `\n\n`.
    assert!(
        entry.docs.contains("\n\n"),
        "expected blank-line in docs; got {:?}",
        entry.docs
    );
}

/// Real cross-module proximity: root and `submodule` each declare
/// `ambiguousname`. The host inside `submodule` should resolve to
/// `submodule::ambiguousname`, the root host to the root one. This
/// exercises the resolver's same-module-wins rule against actual
/// `module_path!()` values rather than the unit tests' synthetic
/// entries.
#[test]
fn cross_module_resolution_picks_same_module() {
    let cat = framework_mcp::ResolvedCatalog::build();

    let root_host = find_entry("root_host_with_dupe");
    let root_edges = cat.dependencies(&framework_mcp::EntryRef::of(root_host));
    let resolved = match &root_edges[0].status {
        framework_mcp::EdgeStatus::Resolved { target } => *target,
        other => panic!("root host edge not resolved: {:?}", other),
    };
    assert_eq!(
        resolved.module_path, root_host.module_path,
        "root host should resolve to same-module ambiguousname"
    );

    let sub_host = find_entry("submodule_host");
    let sub_edges = cat.dependencies(&framework_mcp::EntryRef::of(sub_host));
    let resolved = match &sub_edges[0].status {
        framework_mcp::EdgeStatus::Resolved { target } => *target,
        other => panic!("submodule host edge not resolved: {:?}", other),
    };
    assert_eq!(
        resolved.module_path, sub_host.module_path,
        "submodule host should resolve to same-module ambiguousname"
    );

    // The two `ambiguousname` entries should be distinct.
    assert_ne!(root_host.module_path, sub_host.module_path);
}

/// `dependencies()` for an entry the catalog never saw should be
/// empty, not panic.
#[test]
fn dependencies_for_unknown_host_is_empty() {
    let cat = framework_mcp::ResolvedCatalog::build();
    let bogus = framework_mcp::EntryRef {
        module_path: "no_such_module",
        name: "no_such_name",
    };
    assert!(cat.dependencies(&bogus).is_empty());
    assert!(cat.uses(&bogus).is_empty());
}

/// Resolver must accept an empty entry list and produce an empty
/// catalog — covers startup before any component has registered.
#[test]
fn empty_catalog_builds() {
    let cat = framework_mcp::ResolvedCatalog::build_from(Vec::new());
    assert!(cat.entries().is_empty());
}

/// Local snake_case conversion mirroring
/// `crates/framework/macros/src/case.rs`. Used only by the
/// `resolver_links_ui_host_to_actual_child_entries` assertion to
/// compare a target entry's name against the edge's call-site ident
/// after both are normalized.
fn pascal_to_snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let lowerish = prev.is_ascii_lowercase() || prev.is_ascii_digit();
            let acronym = prev.is_ascii_uppercase()
                && chars
                    .get(i + 1)
                    .map(|n| n.is_ascii_lowercase())
                    .unwrap_or(false);
            if (lowerish || acronym) && !out.ends_with('_') {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn find_entry(name: &str) -> &'static framework_mcp::ComponentEntry {
    framework_mcp::entries()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("entry {:?} not in catalog", name))
}
