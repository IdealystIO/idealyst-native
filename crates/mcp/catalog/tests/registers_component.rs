//! End-to-end test: a `#[component]`-annotated function in this test
//! file should appear in `mcp_catalog::entries()` at runtime, with
//! its doc comment captured and its `composes` edges extracted from
//! any `ui!` / `jsx!` invocations in the body. Validates the full
//! emission + `inventory` distributed slice round-trip.
//!
//! Test components return `Element` so the real `ui!` / `jsx!`
//! expansions inside the host components actually typecheck — the
//! macro emission has to walk a parseable, compilable AST. Stubs
//! return an empty view; bodies are never invoked at runtime.

use runtime_core::{ChildList, Element};
use runtime_macros::{component, idealyst_tool, jsx, recipe, ui, IdealystSchema};

#[allow(dead_code)]
#[derive(Default)]
pub struct DemoProps {}

/// Doc comment whose text we'll look for in the catalog.
#[component]
pub fn democomponent(_props: &DemoProps) -> Element {
    // Stub body — never invoked; the tests assert only on catalog metadata.
    ::runtime_core::view(::std::vec::Vec::new())
}

// ---------------------------------------------------------------------
// Stub child components used by the ui!/jsx! host bodies below. They
// take no props so the generated invocation macros use the zero-arg
// shape (`child_a!()` → `child_a()`) and the macro expansion stays
// self-contained without needing a real props struct. They register
// in the catalog as legitimate `#[component]` entries with empty
// composes.

// Dispatch is transform-free: a call site `ChildA()` lowers to
// `ChildA!()` → `ChildA()`, so the fn name equals the call ident
// verbatim. Components are PascalCase; `#[component]` suppresses the
// `non_snake_case` lint.
#[component]
pub fn ChildA() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

#[component]
pub fn ChildB() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

#[component]
pub fn ChildC() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

// JSX dispatch is transform-free too. These keep snake_case names to
// prove a lowercase tag still resolves verbatim: `<jsx_outer>` →
// `jsx_outer!()` → `jsx_outer()`.
/// Container variant — holds children so the jsx host can nest
/// `<jsx_inner/>` inside `<jsx_outer>…</jsx_outer>`. (Container
/// components move `children` out of props via `#[component(children)]`.)
#[derive(Default)]
pub struct JsxOuterProps {
    pub children: Vec<Element>,
}

#[component(children)]
pub fn jsx_outer(props: JsxOuterProps) -> Element {
    let mut kids: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut kids);
    }
    ui! { view() { kids } }
}

#[component]
pub fn jsx_inner() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

#[component]
pub fn jsx_fragmented() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Generic container used by `nested_host` to exercise the walker's
/// recursion into a component's children block (`Nest() { ChildA() … }`).
#[derive(Default)]
pub struct NestProps {
    pub children: Vec<Element>,
}

#[component(children)]
pub fn Nest(props: NestProps) -> Element {
    let mut kids: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut kids);
    }
    ui! { view() { kids } }
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
#[component]
pub fn ui_host() -> Element {
    let _ = nondescript_helper();
    let _ = ui! {
        ChildA()
        ChildB()
        ChildC()
    };
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Host with a `jsx!` body. Element names from `<name ...>` should be
/// captured; the fragment `<>...</>` has no name and is skipped, but
/// its children are still walked.
#[component]
pub fn jsx_host() -> Element {
    let _ = jsx! {
        <jsx_outer>
            <jsx_inner />
        </jsx_outer>
        <>
            <jsx_fragmented />
        </>
    };
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Host that nests children inside a ui! component slot
/// (`ChildA() { ChildB() ChildC() }`). The collector should recurse
/// into the children block and capture all three idents.
#[component]
pub fn nested_host() -> Element {
    let _ = ui! {
        Nest() {
            ChildA()
            ChildB()
            ChildC()
        }
    };
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Host whose `ui!` body uses `for` to iterate. The visitor walks
/// `UiNode::For.body` so the iterated `ChildA()` is captured.
/// `View` wraps the for-loop because the top-level coercion expects
/// a single `Element`, not a `Vec<Element>`.
#[component]
pub fn for_host() -> Element {
    let _ = ui! {
        view() {
            for _i in 0..3 {
                ChildA()
            }
        }
    };
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Host with TWO separate `ui!` invocations. The visitor recurses
/// through the block-statement list and both calls should contribute
/// edges, in source order.
#[component]
pub fn multi_ui_host() -> Element {
    let _ = ui! { ChildA() };
    let _ = ui! { ChildB() };
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Host whose `ui!` lives at statement position with a trailing
/// semicolon and no `let _ = ...` wrapper. `syn` represents this as
/// `Stmt::Macro`, which the visitor handles via its
/// `visit_stmt_macro` override (separate from `visit_expr_macro`).
#[component]
pub fn stmt_macro_host() -> Element {
    ui! { ChildA() };
    ::runtime_core::view(::std::vec::Vec::new())
}

// The `docless` component has no `///` lines; the catalog should
// record an empty docs string. Use `//` here so this explanatory
// note doesn't become the fn's docs.
#[component]
pub fn docless() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Component
/// with three
/// doc lines.
///
/// Including a blank-line paragraph break.
#[component]
pub fn multiline_docs() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

// ---------------------------------------------------------------------
// Cross-module proximity: real `module_path!()` values inside a real
// submodule. Verifies the resolver picks the same-module candidate
// over the root-level duplicate when both share a name.

mod submodule {
    use runtime_core::Element;
    use runtime_macros::{component, ui};

    // Dispatched via the generated type alias inside `ui!`, never called
    // as a fn — only registered in the catalog. Same for the host below.
    #[allow(dead_code)]
    #[component]
    pub fn Ambiguousname() -> Element {
        ::runtime_core::view(::std::vec::Vec::new())
    }

    #[allow(dead_code)]
    #[component]
    pub fn submodule_host() -> Element {
        let _ = ui! { Ambiguousname() };
        ::runtime_core::view(::std::vec::Vec::new())
    }
}

/// Root-level duplicate of `submodule::Ambiguousname`. Resolver must
/// disambiguate per spec §6.
#[component]
pub fn Ambiguousname() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

#[component]
pub fn root_host_with_dupe() -> Element {
    let _ = ui! { Ambiguousname() };
    ::runtime_core::view(::std::vec::Vec::new())
}

// ---------------------------------------------------------------------
// doc_scope! — the catalog spine. A two-level scope tree with a
// component in each level exercises the ambient module-proximity join
// (`ResolvedCatalog::scope_for`): same-module wins, else nearest ancestor.

mod scoped {
    use runtime_core::{component, doc_scope, Element};

    doc_scope!(Demo = "Demo Feature", docs = "Scope for demo components.", order = 5);

    #[allow(dead_code)]
    #[component]
    pub fn DemoLeaf() -> Element {
        ::runtime_core::view(::std::vec::Vec::new())
    }

    pub mod inner {
        use runtime_core::{component, doc_scope, Element};

        // Custom slug here to prove the `slug =` override path. Scopes are
        // flat — a nearer-module scope just wins assignment, no parent.
        doc_scope!(Inner = "Inner Feature", slug = "inner-feature");

        #[allow(dead_code)]
        #[component]
        pub fn InnerLeaf() -> Element {
            ::runtime_core::view(::std::vec::Vec::new())
        }
    }
}

#[test]
fn doc_scope_registers_with_slug_and_title() {
    let demo = mcp_catalog::lookup_scope("demo").expect("demo scope registered");
    assert_eq!(demo.title, "Demo Feature");
    assert!(demo.docs.contains("demo components"), "docs = {:?}", demo.docs);
    assert_eq!(demo.order, 5);

    // Explicit `slug =` override is honored.
    let inner = mcp_catalog::lookup_scope("inner-feature")
        .expect("inner scope registered under its custom slug");
    assert_eq!(inner.title, "Inner Feature");
}

#[test]
fn scope_for_picks_nearest_enclosing_scope() {
    let cat = mcp_catalog::ResolvedCatalog::build();

    // Same-module declaration wins for the inner leaf.
    let inner_leaf = find_entry("InnerLeaf");
    let s = cat
        .scope_for(inner_leaf.module_path)
        .expect("InnerLeaf resolves to a scope");
    assert_eq!(s.slug, "inner-feature", "nearest (same-module) scope wins");

    // The outer leaf falls to the outer scope.
    let demo_leaf = find_entry("DemoLeaf");
    let s = cat
        .scope_for(demo_leaf.module_path)
        .expect("DemoLeaf resolves to a scope");
    assert_eq!(s.slug, "demo");

    // A component at the crate root with no `doc_scope!` ancestor has no
    // scope — the default/root fallback lives at a higher layer, not here.
    let unscoped = find_entry("democomponent");
    assert!(
        cat.scope_for(unscoped.module_path).is_none(),
        "unscoped module should yield None, got {:?}",
        cat.scope_for(unscoped.module_path).map(|s| s.slug),
    );
}

// ---------------------------------------------------------------------
// Assertions.

#[test]
fn democomponent_registers_in_catalog() {
    let entries: Vec<_> = mcp_catalog::entries().collect();
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
    let json = mcp_catalog::catalog_json();
    // v2 — added primitives, utilities, states, guides, methods,
    // animations, types, tools alongside the original components
    // slice. Bumped in the catalog-extension work.
    assert_eq!(json["catalog_version"], 2);
    let components = json["components"].as_array().expect("components is an array");
    let found_demo = components.iter().any(|c| c["name"] == "democomponent");
    assert!(found_demo, "democomponent missing from catalog json: {}", json);
    // Locked slices the framework ships unconditionally — should
    // appear in every catalog regardless of the consumer crate's
    // contents.
    assert!(
        json["primitives"].as_array().map_or(false, |a| !a.is_empty()),
        "primitives slice missing/empty: {}",
        json,
    );
    assert!(
        json["utilities"].as_array().map_or(false, |a| !a.is_empty()),
        "utilities slice missing/empty: {}",
        json,
    );
    assert!(
        json["states"].as_array().map_or(false, |a| a.len() == 4),
        "states slice should have exactly 4 entries; got: {:?}",
        json["states"],
    );
    assert!(
        json["guides"].as_array().map_or(false, |a| !a.is_empty()),
        "guides slice missing/empty: {}",
        json,
    );
}

/// Locked slices are framework-supplied and visible to every
/// consuming crate via `inventory`. These tests verify the
/// hand-curated tables landed and the entries carry the expected
/// metadata.
#[test]
fn primitives_table_includes_core_set() {
    let names: Vec<&str> = mcp_catalog::primitives().map(|p| p.name).collect();
    for required in &["view", "text", "button", "scroll_view", "toggle", "when"] {
        assert!(
            names.contains(required),
            "primitives table missing {:?}; got {:?}",
            required,
            names,
        );
    }
}

#[test]
fn states_table_has_exactly_the_four_interaction_states() {
    let mut names: Vec<&str> = mcp_catalog::states().map(|s| s.name).collect();
    names.sort();
    assert_eq!(names, vec!["disabled", "focused", "hovered", "pressed"]);
}

#[test]
fn utilities_table_includes_platform_accessor() {
    let utils: Vec<&mcp_catalog::UtilityEntry> = mcp_catalog::utilities().collect();
    let platform = utils
        .iter()
        .find(|u| u.name == "platform")
        .expect("`platform` utility registered");
    assert_eq!(platform.return_type_short, "Platform");
}

#[test]
fn guides_table_includes_getting_started() {
    let getting = mcp_catalog::lookup_guide("getting-started")
        .expect("getting-started guide registered");
    assert!(getting.body.contains("Idealyst"));
    assert!(getting.order < 999, "front-matter `order` should be parsed");
}

#[test]
fn macros_table_documents_effect_and_signal() {
    // Regression for the gap this slice closes: the authoring macros
    // were undocumented in the catalog, so `effect!` was invisible and
    // authors fell back to a bare `Effect::new`. `describe_macro` must
    // surface `effect`, mark it the recommended form, and show what it
    // expands to (the primitive underneath).
    let effect = mcp_catalog::lookup_macro("effect").expect("`effect!` macro registered");
    assert_eq!(effect.module_path, "runtime_core");
    assert_eq!(effect.kind, mcp_catalog::MacroKind::Reactive);
    assert!(
        effect.expansion.contains("Effect::new"),
        "effect! expansion should name the primitive it lowers to: {:?}",
        effect.expansion,
    );
    assert!(
        effect.docs.to_lowercase().contains("recommend"),
        "effect! docs should steer authors to it over a bare Effect::new",
    );

    // A trailing `!` resolves the same entry — `list_macros` consumers
    // pass either spelling.
    assert!(mcp_catalog::lookup_macro("effect!").is_some());
    assert!(mcp_catalog::lookup_macro("signal").is_some());

    // The attribute macros live here too, keyed by bare name.
    let component = mcp_catalog::lookup_macro("component").expect("`#[component]` registered");
    assert_eq!(component.invocation, "#[component]");
    assert_eq!(component.kind, mcp_catalog::MacroKind::Component);
}

#[test]
fn sdks_table_indexes_non_ui_capability_crates() {
    // A1 regression: non-UI SDK crates (net / storage / credentials /
    // server) were invisible to every catalog surface, so an agent
    // couldn't discover how to make a network request or persist data.
    // The `sdks` slice closes that — assert the data-layer crates are
    // present with a usable dep line and the right classification.
    let by_name = |needle: &str| {
        mcp_catalog::sdks()
            .find(|s| s.name == needle)
            .unwrap_or_else(|| panic!("`{needle}` SDK registered; got {:?}",
                mcp_catalog::sdks().map(|s| s.name).collect::<Vec<_>>()))
    };

    let net = by_name("net");
    assert_eq!(net.category, mcp_catalog::SdkCategory::Data);
    assert_eq!(net.kind, mcp_catalog::SdkKind::Api);
    assert!(net.dep_line.contains("net ="), "dep line is copy-pasteable: {:?}", net.dep_line);
    assert!(net.summary.to_lowercase().contains("http"), "net summary names HTTP");

    // The other capabilities the field report flagged as undiscoverable.
    for required in &["storage", "credentials", "server", "files"] {
        let s = by_name(required);
        assert_eq!(s.category, mcp_catalog::SdkCategory::Data, "{required} is a data crate");
    }

    // The component library + an Element::External primitive are tagged.
    assert_eq!(by_name("idea-ui").category, mcp_catalog::SdkCategory::Ui);
    assert_eq!(by_name("webview").kind, mcp_catalog::SdkKind::External);

    // Lookup helper resolves by crate name.
    assert!(mcp_catalog::lookup_sdk("net").is_some());
    assert!(mcp_catalog::lookup_sdk("no-such-crate").is_none());
}

#[test]
fn sdks_guide_enumerates_the_data_crates() {
    // The structured slice's prose home: the `sdks` guide must exist and
    // name the data crates so `read_guide("sdks")` is a real fallback.
    let guide = mcp_catalog::lookup_guide("sdks").expect("sdks guide registered");
    for required in &["net", "storage", "credentials", "server"] {
        assert!(
            guide.body.contains(required),
            "sdks guide should mention `{required}`; body len {}",
            guide.body.len(),
        );
    }
    // And the server-functions guide exists for the #[server] flow.
    let srv = mcp_catalog::lookup_guide("server-functions")
        .expect("server-functions guide registered");
    assert!(srv.body.contains("#[server]"), "server-functions guide covers the macro");
}

#[test]
fn primitive_entry_is_construct_locked_to_this_crate() {
    // Compile-time check: external callers can read every pub field
    // but cannot construct one. This test asserts the read surface
    // works; the lock itself is enforced by `_seal: ()` being a
    // private field (struct-literal construction is rejected at
    // compile time from outside `mcp_catalog`).
    let view = mcp_catalog::lookup_primitive("view").expect("view primitive registered");
    let _name: &str = view.name;
    let _docs: &str = view.docs;
    let _category = view.category;
    let _backends: &[&str] = view.backends;
    // The seal exists. Construction from this crate's own integration
    // test isn't blocked (the test compiles inside `mcp_catalog`'s
    // test target — same crate, same privacy boundary).
}

#[test]
fn ui_host_records_composed_idents() {
    let entries: Vec<_> = mcp_catalog::entries().collect();
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
    let entries: Vec<_> = mcp_catalog::entries().collect();
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
    let cat = mcp_catalog::ResolvedCatalog::build();

    let ui_host = cat
        .entries()
        .iter()
        .find(|e| e.name == "ui_host")
        .copied()
        .expect("ui_host in catalog");
    let ui_host_ref = mcp_catalog::EntryRef::of(ui_host);

    let edges = cat.dependencies(&ui_host_ref);
    assert!(!edges.is_empty(), "ui_host has composes edges");
    for edge in edges {
        match &edge.status {
            mcp_catalog::EdgeStatus::Resolved { target } => {
                // Transform-free dispatch: the call-site ident equals
                // the target entry name verbatim. Exact match, no case
                // folding.
                assert_eq!(
                    target.name, edge.raw_name,
                    "edge {:?} resolved to wrong target {:?}",
                    edge.raw_name, target.name,
                );
            }
            other => panic!(
                "edge {:?} expected Resolved (stub component is in same crate), got {:?}",
                edge.raw_name, other
            ),
        }
    }

    // Reverse: `ChildA` should report `ui_host` among its users.
    let child_a = cat
        .entries()
        .iter()
        .find(|e| e.name == "ChildA")
        .copied()
        .expect("ChildA in catalog");
    let users = cat.uses(&mcp_catalog::EntryRef::of(child_a));
    assert!(
        users.iter().any(|r| r.name == "ui_host"),
        "expected ui_host in ChildA's reverse adjacency; got {:?}",
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
    let entry = find_entry("ChildA");
    assert!(entry.params.is_empty(), "expected empty; got {:?}", entry.params);
}

/// Positional multi-parameter signature — record every parameter in
/// declaration order. Declared inside this test for tight locality;
/// it registers in the global catalog like any other `#[component]`.
#[allow(non_snake_case)]
#[component]
pub fn positional_host(idx: u32, _label: &'static str) -> Element {
    let _ = (idx, _label);
    ::runtime_core::view(::std::vec::Vec::new())
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

/// Phase 3b: `IdealystSchema` derive should produce a
/// `PropsSchemaEntry` whose fields carry per-field docs + the
/// `#[schema(constraint = "...")]` hint.
#[allow(dead_code)]
#[derive(Default, IdealystSchema)]
pub struct BadgeProps {
    /// Visible label text.
    pub label: String,
    pub count: u32,
    #[schema(constraint = "valid CSS color")]
    pub color: String,
}

#[test]
fn props_schema_records_fields_with_docs_and_constraints() {
    let s = mcp_catalog::lookup_schema("BadgeProps")
        .expect("BadgeProps schema registered");
    assert_eq!(s.fields.len(), 3);

    let label = s.fields.iter().find(|f| f.name == "label").unwrap();
    assert!(
        label.doc.contains("Visible label"),
        "got {:?}",
        label.doc
    );

    let color = s.fields.iter().find(|f| f.name == "color").unwrap();
    assert_eq!(color.constraint, "valid CSS color");

    let count = s.fields.iter().find(|f| f.name == "count").unwrap();
    assert_eq!(count.doc, "", "no doc → empty string");
    assert_eq!(count.constraint, "");
}

/// `ParamSpec.type_short_name` should give the bare ident of the
/// parameter type so the catalog can join `&BadgeProps` →
/// `BadgeProps` → its schema fields.
#[allow(non_snake_case)]
#[component]
pub fn badge_host(_props: &BadgeProps) -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

#[test]
fn param_records_type_short_name_for_join() {
    let entry = find_entry("badge_host");
    assert_eq!(entry.params.len(), 1);
    assert_eq!(entry.params[0].type_short_name, "BadgeProps");
}

/// Phase 3c: a `#[idealyst_tool]` fn should land in the
/// `ToolEntry` slice with its parameters captured the same way a
/// `#[component]` records them.
#[idealyst_tool]
/// Returns a hex color darkened by `amount` (linear-light).
pub fn darken(_hex: &str, _amount: f32) -> String {
    String::new()
}

#[test]
fn idealyst_tool_registers_with_params_and_return() {
    let entry = mcp_catalog::tools()
        .find(|t| t.name == "darken")
        .expect("darken tool registered");
    assert!(entry.docs.contains("hex color darkened"));
    assert_eq!(entry.params.len(), 2);
    assert_eq!(entry.params[0].name, "_hex");
    assert_eq!(entry.params[1].name, "_amount");
    assert!(entry.return_type.contains("String"));
}

#[test]
fn catalog_json_inlines_schema_for_param() {
    let json = mcp_catalog::catalog_json();
    let components = json["components"].as_array().unwrap();
    let host = components
        .iter()
        .find(|c| c["name"] == "badge_host")
        .expect("badge_host in JSON");
    let param = &host["params"][0];
    assert_eq!(param["type_short_name"], "BadgeProps");
    let schema = param["schema"].as_array().expect("schema inlined");
    assert_eq!(schema.len(), 3);
    let names: Vec<&str> = schema.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"label"));
    assert!(names.contains(&"count"));
    assert!(names.contains(&"color"));
}

#[test]
fn catalog_json_includes_params_array() {
    let json = mcp_catalog::catalog_json();
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
    let json = mcp_catalog::catalog_json();
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
    let cat = mcp_catalog::ResolvedCatalog::build();

    let root_host = find_entry("root_host_with_dupe");
    let root_edges = cat.dependencies(&mcp_catalog::EntryRef::of(root_host));
    let resolved = match &root_edges[0].status {
        mcp_catalog::EdgeStatus::Resolved { target } => *target,
        other => panic!("root host edge not resolved: {:?}", other),
    };
    assert_eq!(
        resolved.module_path, root_host.module_path,
        "root host should resolve to same-module ambiguousname"
    );

    let sub_host = find_entry("submodule_host");
    let sub_edges = cat.dependencies(&mcp_catalog::EntryRef::of(sub_host));
    let resolved = match &sub_edges[0].status {
        mcp_catalog::EdgeStatus::Resolved { target } => *target,
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
    let cat = mcp_catalog::ResolvedCatalog::build();
    let bogus = mcp_catalog::EntryRef {
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
    let cat = mcp_catalog::ResolvedCatalog::build_from(Vec::new());
    assert!(cat.entries().is_empty());
}

fn find_entry(name: &str) -> &'static mcp_catalog::ComponentEntry {
    mcp_catalog::entries()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("entry {:?} not in catalog", name))
}

// =============================================================================
// New-slice emission tests — added with the catalog v2 extension.
// =============================================================================

/// `#[derive(IdealystSchema)]` on an enum produces a `TypeEntry`
/// with shape `Enum`, including per-variant docs and payload.
#[allow(dead_code)]
#[derive(IdealystSchema)]
/// Outer enum doc that should land on `TypeEntry.docs`.
pub enum DemoSize {
    /// The smallest variant.
    Small,
    Medium,
    /// Tuple variant — payload is positional.
    Custom(u32),
    /// Struct variant — payload has named fields.
    Named {
        /// The width override.
        width: u32,
        height: u32,
    },
}

#[test]
fn enum_schema_emits_type_entry_with_variants() {
    let t = mcp_catalog::lookup_type("DemoSize").expect("DemoSize TypeEntry registered");
    let variants = match &t.shape {
        mcp_catalog::TypeShape::Enum { variants } => *variants,
        _ => panic!("expected enum shape; got {:?}", t.shape),
    };
    let names: Vec<&str> = variants.iter().map(|v| v.name).collect();
    assert_eq!(names, vec!["Small", "Medium", "Custom", "Named"]);

    let small = variants.iter().find(|v| v.name == "Small").unwrap();
    assert!(small.docs.contains("smallest"), "small.docs = {:?}", small.docs);
    assert!(small.payload.is_empty(), "unit variant has empty payload");

    let custom = variants.iter().find(|v| v.name == "Custom").unwrap();
    assert_eq!(custom.payload.len(), 1);
    assert_eq!(custom.payload[0].name, "", "tuple variant payload uses empty names");
    assert_eq!(custom.payload[0].type_str.trim(), "u32");

    let named = variants.iter().find(|v| v.name == "Named").unwrap();
    let field_names: Vec<&str> = named.payload.iter().map(|f| f.name).collect();
    assert_eq!(field_names, vec!["width", "height"]);
    let width = named.payload.iter().find(|f| f.name == "width").unwrap();
    assert!(width.doc.contains("width override"), "got {:?}", width.doc);
}

/// `#[derive(IdealystSchema)]` on a struct still emits the legacy
/// `PropsSchemaEntry` AND a new `TypeEntry` with shape `Struct`.
#[test]
fn struct_schema_also_emits_type_entry() {
    let t = mcp_catalog::lookup_type("BadgeProps")
        .expect("BadgeProps registered as TypeEntry too (alongside PropsSchemaEntry)");
    let fields = match &t.shape {
        mcp_catalog::TypeShape::Struct { fields } => *fields,
        _ => panic!("BadgeProps should be Struct shape"),
    };
    assert_eq!(fields.len(), 3);
}

// -----------------------------------------------------------------------------
// MethodEntry — emitted from `methods! { fn name(&self, …) { … } }`
// blocks inside `#[component]` bodies.
// -----------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Default)]
pub struct CounterProps {
    pub initial: i32,
}

/// Counter component with two imperative methods. The catalog should
/// pick up both `reset` and `bump_by` with their docs and parameters.
#[component]
pub fn counter(_props: &CounterProps) -> runtime_core::Element {
    let value = runtime_core::signal!(0_i32);
    methods! {
        /// Reset the counter to zero.
        fn reset(&self) {
            value.set(0);
        }
        /// Bump the counter by `n` (positive or negative).
        fn bump_by(&self, n: i32) {
            value.update(|v| *v += n);
        }
    }
    ::runtime_core::view(::std::vec::Vec::new())
}

#[test]
fn methods_block_emits_method_entries() {
    let methods: Vec<&mcp_catalog::MethodEntry> = mcp_catalog::methods()
        .filter(|m| m.parent_name == "counter")
        .collect();
    let names: Vec<&str> = methods.iter().map(|m| m.name).collect();
    assert!(names.contains(&"reset"), "missing reset; got {:?}", names);
    assert!(names.contains(&"bump_by"), "missing bump_by; got {:?}", names);

    let reset = methods.iter().find(|m| m.name == "reset").unwrap();
    assert!(reset.docs.contains("Reset the counter"), "got {:?}", reset.docs);
    assert_eq!(reset.params.len(), 0);

    let bump = methods.iter().find(|m| m.name == "bump_by").unwrap();
    assert_eq!(bump.params.len(), 1);
    assert_eq!(bump.params[0].name, "n");
    assert_eq!(bump.params[0].type_str.trim(), "i32");
}

// -----------------------------------------------------------------------------
// AnimationEntry — emitted from `animated!(...)` calls inside
// `#[component]` bodies.
// -----------------------------------------------------------------------------

#[component]
pub fn fader_demo() -> runtime_core::Element {
    let _opacity = runtime_core::animated!(0.0_f32);
    let _scale = runtime_core::animated!(1.0_f32);
    ::runtime_core::view(::std::vec::Vec::new())
}

#[test]
fn animated_macros_emit_animation_entries() {
    let anims: Vec<&mcp_catalog::AnimationEntry> = mcp_catalog::animations()
        .filter(|a| a.parent_name == "fader_demo")
        .collect();
    let bindings: Vec<&str> = anims.iter().map(|a| a.binding).collect();
    assert!(
        bindings.contains(&"_opacity"),
        "expected `_opacity` binding; got {:?}",
        bindings,
    );
    assert!(
        bindings.contains(&"_scale"),
        "expected `_scale` binding; got {:?}",
        bindings,
    );

    let opacity = anims.iter().find(|a| a.binding == "_opacity").unwrap();
    assert!(
        opacity.initial.contains("0.0") && opacity.initial.contains("f32"),
        "initial should capture the literal; got {:?}",
        opacity.initial,
    );
}

// -----------------------------------------------------------------------------
// Recipes target ANY entity (phase 3) — not just components. A recipe
// for a free function must surface via `recipes_for`, and the cross-kind
// `resolve_entity` must tag names with their kind.
// -----------------------------------------------------------------------------

/// A free function (not a component) that a recipe can target.
#[allow(dead_code)]
pub fn compute_thing(x: u32) -> u32 {
    x * 2
}

recipe!(
    compute_thing,
    /// Doubling helper — call it with the input you want doubled.
    fn compute_thing_example() {
        let _ = compute_thing(21);
    }
);

#[test]
fn recipe_can_target_a_free_function() {
    let cat = mcp_catalog::ResolvedCatalog::build();
    let recs = cat.recipes_for("compute_thing");
    let r = recs
        .iter()
        .find(|r| r.name == "compute_thing_example")
        .unwrap_or_else(|| {
            panic!(
                "recipe targeting a free fn should surface via recipes_for; got {:?}",
                recs.iter().map(|r| r.name).collect::<Vec<_>>(),
            )
        });
    assert_eq!(r.target, "compute_thing", "recipe target should be the fn name");
    assert!(r.docs.contains("Doubling helper"), "recipe docs = {:?}", r.docs);
}

#[test]
fn resolve_entity_tags_kinds() {
    use mcp_catalog::EntityKind;
    let cat = mcp_catalog::ResolvedCatalog::build();

    // A component resolves as Component.
    let m = cat.resolve_entity("ui_host");
    assert!(
        m.iter().any(|e| e.kind == EntityKind::Component && e.name == "ui_host"),
        "ui_host should resolve as Component; got {:?}",
        m,
    );

    // The framework `platform` utility resolves as Utility.
    let m = cat.resolve_entity("platform");
    assert!(
        m.iter().any(|e| e.kind == EntityKind::Utility),
        "platform should resolve as Utility; got {:?}",
        m,
    );

    // A primitive resolves as Primitive (snake_case or pascal name).
    let m = cat.resolve_entity("view");
    assert!(
        m.iter().any(|e| e.kind == EntityKind::Primitive),
        "view should resolve as Primitive; got {:?}",
        m,
    );

    // An unknown name yields no matches (not a panic).
    assert!(cat.resolve_entity("definitely_not_a_thing_xyz").is_empty());
}

// -----------------------------------------------------------------------------
// catalog_version + cross-slice presence.
// -----------------------------------------------------------------------------

#[test]
fn catalog_json_v2_includes_every_new_slice() {
    let json = mcp_catalog::catalog_json();
    for slice in &[
        "primitives",
        "utilities",
        "macros",
        "states",
        "guides",
        "methods",
        "animations",
        "types",
        "tools",
        "scopes",
        "sdks",
    ] {
        assert!(
            json[slice].is_array(),
            "catalog v2 missing `{}` slice: {}",
            slice,
            json,
        );
    }
}

// -----------------------------------------------------------------------------
// Round-trip guard: catalog_json() → build_from_json() must reproduce every
// slice the in-process `build()` sees. This pins the *equivalence* of the
// serialize (to_json) and deserialize (leak-from-json) halves so the
// CatalogSlice trait migration is provably behavior-preserving — if either
// half drifts, the per-slice counts or the spot-checked fields diverge.
// -----------------------------------------------------------------------------

#[test]
fn catalog_json_round_trips_through_build_from_json() {
    use mcp_catalog::ResolvedCatalog;

    let json_str = serde_json::to_string(&mcp_catalog::catalog_json()).unwrap();
    let rebuilt = ResolvedCatalog::build_from_json(&json_str)
        .expect("build_from_json should accept catalog_json output");
    let direct = ResolvedCatalog::build();

    // Every slice must survive the round-trip with the same cardinality.
    assert_eq!(
        rebuilt.entries().len(),
        direct.entries().len(),
        "component count drifted through JSON round-trip",
    );
    assert_eq!(rebuilt.primitives().len(), direct.primitives().len(), "primitives");
    assert_eq!(rebuilt.utilities().len(), direct.utilities().len(), "utilities");
    assert_eq!(rebuilt.macros().len(), direct.macros().len(), "macros");
    assert_eq!(rebuilt.states().len(), direct.states().len(), "states");
    assert_eq!(rebuilt.guides().len(), direct.guides().len(), "guides");
    assert_eq!(rebuilt.methods().len(), direct.methods().len(), "methods");
    assert_eq!(rebuilt.animations().len(), direct.animations().len(), "animations");
    assert_eq!(rebuilt.types().len(), direct.types().len(), "types");
    assert_eq!(rebuilt.tools().len(), direct.tools().len(), "tools");
    assert_eq!(rebuilt.recipes().len(), direct.recipes().len(), "recipes");
    assert_eq!(rebuilt.scopes().len(), direct.scopes().len(), "scopes");
    assert_eq!(rebuilt.sdks().len(), direct.sdks().len(), "sdks");

    // A scope survives the JSON boundary with its fields intact.
    let rebuilt_inner = rebuilt
        .scopes()
        .iter()
        .find(|s| s.slug == "inner-feature")
        .expect("inner scope survives round-trip");
    assert_eq!(rebuilt_inner.title, "Inner Feature", "scope title lost in round-trip");

    // A recipe keeps its target across the boundary.
    let rebuilt_recipe = rebuilt
        .recipes()
        .iter()
        .find(|r| r.name == "compute_thing_example")
        .expect("recipe survives round-trip");
    assert_eq!(rebuilt_recipe.target, "compute_thing", "recipe target lost in round-trip");

    // An SDK keeps its dep line + classification across the wire — the
    // fields an agent needs to actually add the crate.
    let rebuilt_net = rebuilt
        .sdks()
        .iter()
        .find(|s| s.name == "net")
        .expect("net SDK survives round-trip");
    assert!(rebuilt_net.dep_line.contains("net ="), "sdk dep_line lost in round-trip");
    assert_eq!(rebuilt_net.category, mcp_catalog::SdkCategory::Data, "sdk category lost");
    assert_eq!(rebuilt_net.kind, mcp_catalog::SdkKind::Api, "sdk kind lost");

    // Field-level fidelity: a component carries its docs, composes, and
    // params across the boundary intact.
    let demo = rebuilt
        .entries()
        .iter()
        .find(|e| e.name == "democomponent")
        .expect("democomponent survives round-trip");
    assert!(!demo.docs.is_empty(), "component docs lost in round-trip");
    assert_eq!(demo.params.len(), 1, "component params lost in round-trip");

    let ui_host = rebuilt
        .entries()
        .iter()
        .find(|e| e.name == "ui_host")
        .expect("ui_host survives round-trip");
    assert!(
        !ui_host.composes.is_empty(),
        "composes edges lost in round-trip",
    );

    // An enum TypeEntry keeps its variants through the boundary.
    let direct_enum_variants: usize = direct
        .types()
        .iter()
        .filter_map(|t| match &t.shape {
            mcp_catalog::TypeShape::Enum { variants } => Some(variants.len()),
            _ => None,
        })
        .sum();
    let rebuilt_enum_variants: usize = rebuilt
        .types()
        .iter()
        .filter_map(|t| match &t.shape {
            mcp_catalog::TypeShape::Enum { variants } => Some(variants.len()),
            _ => None,
        })
        .sum();
    assert_eq!(
        rebuilt_enum_variants, direct_enum_variants,
        "enum variants lost in round-trip",
    );

    // A tool keeps its params + return type.
    if let Some(direct_tool) = direct.tools().first() {
        let rebuilt_tool = rebuilt
            .tools()
            .iter()
            .find(|t| t.name == direct_tool.name)
            .expect("tool survives round-trip");
        assert_eq!(
            rebuilt_tool.params.len(),
            direct_tool.params.len(),
            "tool params lost in round-trip",
        );
        assert_eq!(
            rebuilt_tool.return_type, direct_tool.return_type,
            "tool return type lost in round-trip",
        );
    }
}
