//! The source scan against small on-disk crates: module-tree resolution,
//! `cfg`, files mid-edit, `macro_rules!` expansion, and the cases that
//! must refuse (and send the caller to the compiled extractor) rather
//! than guess. Exactness of the entries themselves against a real build
//! is pinned by `mcp-catalog`'s `source_scan_reads_exactly_what_the_
//! compiled_macros_register`.

use std::path::{Path, PathBuf};

use catalog_scan::{scan, Cfg, MacroCrate, ScanCrate};

/// A fresh directory with `files` (path → contents) written into it.
fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("scan-tests").join(name);
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    root
}

fn host() -> Cfg {
    Cfg::from_print_cfg("unix\ntarget_os=\"linux\"\ntarget_arch=\"x86_64\"\n")
}

fn krate(name: &str, root: &Path) -> ScanCrate {
    ScanCrate { name: name.into(), root: root.join("src/lib.rs"), cfg: host().with_features(["on"]) }
}

fn components(parts: &mcp_catalog::CatalogParts) -> Vec<String> {
    let mut v: Vec<String> = parts.components.iter().map(|c| format!("{}::{}@{}", c.module_path, c.name, c.line)).collect();
    v.sort();
    v
}

const COMPONENT: &str = "#[component]\npub fn NAME() -> Element { ui! { view() } }\n";

fn component(name: &str) -> String {
    COMPONENT.replace("NAME", name)
}

#[test]
fn follows_the_module_tree_the_way_rustc_does() {
    let root = tree(
        "modtree",
        &[
            ("src/lib.rs", "mod flat;\nmod nested;\n#[path = \"elsewhere/odd.rs\"]\nmod renamed;\nmod inline {\n    mod child;\n}\n#[cfg(feature = \"off\")]\nmod gated;\n#[cfg(feature = \"on\")]\nmod enabled;\n"),
            ("src/flat.rs", &format!("mod grandchild;\n{}", component("Flat"))),
            ("src/flat/grandchild.rs", &component("Grandchild")),
            ("src/nested/mod.rs", &component("Nested")),
            ("src/elsewhere/odd.rs", &component("Renamed")),
            ("src/inline/child.rs", &component("InlineChild")),
            ("src/gated.rs", &component("Gated")),
            ("src/enabled.rs", &component("Enabled")),
            // Not reachable from the module tree: never read.
            ("src/orphan.rs", &component("Orphan")),
        ],
    );
    let out = scan(&[krate("app", &root)], &[]);
    assert!(out.refused.is_empty(), "{:?}", out.refused);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(
        components(&out.parts),
        [
            "app::enabled::Enabled@1",
            "app::flat::Flat@2",
            "app::flat::grandchild::Grandchild@1",
            "app::inline::child::InlineChild@1",
            "app::nested::Nested@1",
            "app::renamed::Renamed@1",
        ]
    );
    let file = out.parts.components.iter().find(|c| c.name == "Grandchild").unwrap().file;
    assert_eq!(file, root.join("src/flat/grandchild.rs").to_string_lossy());
}

/// A file mid-edit is skipped and reported; its siblings still count, and
/// so do its child modules (its `mod` lines are recovered).
#[test]
fn a_file_that_does_not_parse_is_skipped_not_fatal() {
    let root = tree(
        "midedit",
        &[
            ("src/lib.rs", "mod broken;\nmod fine;\n"),
            ("src/broken.rs", &format!("mod kid;\n{}\nfn half( {{", component("Lost"))),
            ("src/broken/kid.rs", &component("Kid")),
            ("src/fine.rs", &component("Fine")),
        ],
    );
    let out = scan(&[krate("app", &root)], &[]);
    assert!(out.refused.is_empty(), "{:?}", out.refused);
    assert_eq!(components(&out.parts), ["app::broken::kid::Kid@1", "app::fine::Fine@1"]);
    assert_eq!(out.skipped.len(), 1);
    assert!(out.skipped[0].file.ends_with("src/broken.rs"), "{:?}", out.skipped);
}

/// CrewForge's `state_props!` shape: a local `macro_rules!` stamping out
/// `#[props] #[derive(IdealystSchema)]` structs. Their schemas must
/// appear, and a component's `line!()` inside a macro is the
/// invocation's line, as rustc reports it.
#[test]
fn expands_local_macro_rules_that_emit_catalog_macros() {
    let root = tree(
        "localmacro",
        &[(
            "src/lib.rs",
            r#"
macro_rules! state_props {
    ($name:ident) => {
        #[runtime_core::props]
        #[derive(IdealystSchema)]
        struct $name {
            #[prop(static)]
            state: AccessState,
        }
    };
}

macro_rules! screen {
    ($name:ident) => {
        #[component]
        fn $name() -> Element { ui! { view() } }
    };
}

// Not a catalog macro: never expanded (its input would not match).
macro_rules! unrelated { (x) => {}; }

state_props!(DoorsProps);
state_props!(WhoProps);
screen!(Doors);
unrelated!(this does not match);
"#,
        )],
    );
    let out = scan(&[krate("app", &root)], &[]);
    assert!(out.refused.is_empty(), "{:?}", out.refused);
    let mut schemas: Vec<&str> = out.parts.props_schemas.iter().map(|s| s.short_name).collect();
    schemas.sort();
    assert_eq!(schemas, ["DoorsProps", "WhoProps"]);
    // `#[prop(static)]` kept the field type unwrapped.
    assert_eq!(out.parts.props_schemas[0].fields[0].type_str, "AccessState");
    assert_eq!(components(&out.parts), ["app::Doors@25"]);
}

/// An app invoking a dependency's exported catalog macro (idea-theme's
/// `tone!`), bare or through a renamed crate path.
#[test]
fn expands_a_dependencys_exported_macro_even_through_a_renamed_crate() {
    let dep = tree(
        "depmacro-dep",
        &[(
            "src/macros.rs",
            r#"
#[macro_export]
macro_rules! tone {
    ($vis:vis $name:ident) => {
        #[derive(Copy, Clone, Default, ::runtime_core::IdealystSchema)]
        #[schema(value_of = "ToneRef")]
        $vis struct $name;
    };
}
"#,
        )],
    );
    let app = tree("depmacro-app", &[("src/lib.rs", "tone!(pub Brand);\ntheme::tone!(Accent);\n")]);
    let deps = [MacroCrate { name: "idea_theme".into(), src_dir: dep.join("src") }];
    let out = scan(&[krate("app", &app)], &deps);
    assert!(out.refused.is_empty(), "{:?}", out.refused);
    let mut values: Vec<(&str, &str)> = out.parts.values.iter().map(|v| (v.short_name, v.value_of)).collect();
    values.sort();
    assert_eq!(values, [("Accent", "ToneRef"), ("Brand", "ToneRef")]);
}

/// What the scan cannot reproduce refuses the crate holding it — never
/// silently missing entries — and only that crate: a catalog type
/// registered by hand, a catalog macro invocation no rule matches.
#[test]
fn refuses_what_it_cannot_reproduce_crate_by_crate() {
    let handwritten = tree(
        "handwritten",
        &[(
            "src/lib.rs",
            "inventory::submit! { ::runtime_core::__mcp::IconSetEntry { name: \"mine\", icons: ICONS } }\n",
        )],
    );
    let nomatch = tree(
        "nomatch",
        &[(
            "src/lib.rs",
            "macro_rules! m { ($n:ident) => { #[component] fn $n() -> Element { todo!() } }; }\nm!(1 2 3);\n",
        )],
    );
    // A dependency's token macro, invoked through a local wrapper, the
    // way idea-theme's `catalog_token!` reaches `register_style_token!`.
    let tokens = tree(
        "tokens",
        &[(
            "src/lib.rs",
            "macro_rules! catalog_token { ($n:expr) => { ::runtime_core::register_style_token!($n, {}); }; }\ncatalog_token!(\"gap\");\n",
        )],
    );
    let vocab = tree(
        "tokens-dep",
        &[(
            "src/lib.rs",
            "#[macro_export]\nmacro_rules! register_style_token { ($n:expr, $d:block) => { $crate::glue::__mcp::inventory::submit! { $crate::glue::__mcp::StyleTokenEntry { name: $n, default_value: $crate::glue::__mcp::TokenDefault::Resolver(|| $d) } } }; }\n",
        )],
    );
    let fine = tree("fine", &[("src/lib.rs", &component("Fine"))]);
    let deps = [MacroCrate { name: "runtime_vocabulary".into(), src_dir: vocab.join("src") }];
    let out = scan(
        &[krate("handwritten", &handwritten), krate("nomatch", &nomatch), krate("tokens", &tokens), krate("fine", &fine)],
        &deps,
    );
    let refused: Vec<(usize, String)> = out.refused.iter().map(|r| (r.krate, r.error.message.clone())).collect();
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert!(refused[0].0 == 0 && refused[0].1.contains("IconSetEntry"), "{refused:?}");
    assert!(refused[1].0 == 1 && refused[1].1.contains("no rule"), "{refused:?}");
    assert!(refused[2].0 == 2 && refused[2].1.contains("StyleTokenEntry"), "{refused:?}");
    // The scannable crate is read, and nothing of the refused ones is.
    assert_eq!(components(&out.parts), ["fine::Fine@1"]);

    // A catalog type registered under its bare (imported) name is still a
    // hand registration — refused, not skipped as someone else's.
    let bare = tree("bare", &[("src/lib.rs", "use mcp_catalog::ScopeEntry;\ninventory::submit! { ScopeEntry { slug: \"core\", title: \"Core\", docs: \"\", module_path: \"x\", order: CORE_ORDER } }\n")]);
    let out = scan(&[krate("bare", &bare)], &[]);
    assert_eq!(out.refused.len(), 1, "{:?}", out.refused);

    // An inventory registration of something else entirely is not ours.
    let root = tree("otherinventory", &[("src/lib.rs", "inventory::submit! { crate::Plugin { name: \"x\" } }\n")]);
    let out = scan(&[krate("app", &root)], &[]);
    assert!(out.refused.is_empty() && out.parts.components.is_empty());
}

/// A file with no catalog code is only tokenized for its `mod`
/// declarations; those must still be read exactly — attributes over
/// several lines, visibility, two declarations on one line, nested
/// inline modules.
#[test]
fn a_lightly_read_file_still_yields_every_module() {
    let root = tree(
        "lightmods",
        &[
            (
                "src/lib.rs",
                "//! No catalog code here.\npub(crate) mod a; mod b;\n#[cfg(all(\n    unix,\n    feature = \"on\"\n))]\npub mod c;\n#[cfg(windows)]\nmod gone;\nmod outer {\n    pub(super) mod d;\n}\nfn helper() -> u32 { 1 }\n",
            ),
            ("src/a.rs", &component("A")),
            ("src/b.rs", &component("B")),
            ("src/c.rs", &component("C")),
            ("src/gone.rs", &component("Gone")),
            ("src/outer/d.rs", &component("D")),
        ],
    );
    let out = scan(&[krate("app", &root)], &[]);
    assert!(out.refused.is_empty() && out.skipped.is_empty(), "{:?} {:?}", out.refused, out.skipped);
    assert_eq!(components(&out.parts), ["app::a::A@1", "app::b::B@1", "app::c::C@1", "app::outer::d::D@1"]);
}
