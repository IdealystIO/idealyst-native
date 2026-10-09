//! `idealyst catalog-scan DIR` — one crate's own catalog entries from
//! its SOURCE, without compiling anything.
//!
//! The editor extension's fast half: it runs this on every save and
//! merges the result over the compiled `catalog-json --deps-only`
//! catalog, so the author's own components are there before (and
//! whether or not) anything compiles. The entries are read by the
//! `catalog-scan` library — the catalog macros' own expansion run over
//! the source, so they are exactly what a compiled catalog would hold
//! (see that crate's docs); `catalog-json --scan` and `idealyst mcp` read
//! the workspace the same way.
//!
//! The document is the catalog JSON (`catalog-json`'s shape) holding only
//! entries registered from this crate, plus `scanned_crate` (the crate's
//! name as a module path root) so a consumer can replace that crate's
//! entries with these. Workspace crates this one depends on are read too,
//! so a component whose props struct lives in one of them still gets the
//! struct's fields inlined; their own entries are left out.
//!
//! Forgiving by design: a file that does not parse is reported on stderr
//! and skipped, every other file still contributes. A crate the scan
//! cannot read in full (a catalog entry registered by hand, say) yields
//! what was read up to that point, with the reason on stderr — still more
//! useful to completion than nothing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mcp_catalog::{origin_crate, CatalogParts};
use serde_json::Value;

use super::scan_plan::ScanPlan;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Crate directory (the one holding `Cargo.toml` + `src/`).
    #[arg(default_value = ".")]
    pub dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve crate dir {}", args.dir.display()))?;
    let plan = super::scan_plan::plan(std::slice::from_ref(&dir)).context("plan the source scan")?;
    let json = crate_json(&plan, &dir)?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

/// The document for the planned crate at `dir` (see the module docs).
pub fn crate_json(plan: &ScanPlan, dir: &Path) -> Result<Value> {
    let idx = plan
        .member_dirs
        .iter()
        .position(|d| d == dir)
        .with_context(|| format!("{} is not a crate that uses the framework", dir.display()))?;
    let name = plan.crates[idx].name.clone();
    let mut scanned = ::catalog_scan::scan(&plan.crates, &plan.macro_deps);
    for skipped in &scanned.skipped {
        eprintln!("[catalog-scan] {skipped}");
    }
    let mut parts = std::mem::take(&mut scanned.parts);
    for refused in scanned.refused {
        if refused.krate == idx {
            eprintln!(
                "[catalog-scan] {name} cannot be read in full ({}); its entries up to there are listed",
                refused.error
            );
        }
        parts.extend(refused.partial);
    }

    // This crate's entries; every crate's props schemas, for the join.
    let ours = |m: &str| origin_crate(m) == name;
    let own = CatalogParts {
        components: parts.components.into_iter().filter(|e| ours(e.module_path)).collect(),
        props_schemas: parts.props_schemas,
        methods: parts.methods.into_iter().filter(|e| ours(e.parent_module_path)).collect(),
        animations: parts.animations.into_iter().filter(|e| ours(e.parent_module_path)).collect(),
        types: parts.types.into_iter().filter(|e| ours(e.module_path)).collect(),
        values: parts.values.into_iter().filter(|e| ours(e.module_path)).collect(),
        tools: parts.tools.into_iter().filter(|e| ours(e.module_path)).collect(),
        recipes: parts.recipes.into_iter().filter(|e| ours(e.module_path)).collect(),
        scopes: parts.scopes.into_iter().filter(|e| ours(e.module_path)).collect(),
        ..Default::default()
    };
    let mut json = own.to_json();
    json["scanned_crate"] = Value::String(name);
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::catalog_scan::{Cfg, ScanCrate};
    use std::fs;

    /// A crate at a fresh temp dir, and the plan `run` would make for it
    /// (built by hand: no `cargo metadata` for a fake crate).
    fn fake_crate(tag: &str, files: &[(&str, &str)]) -> (PathBuf, ScanPlan) {
        let dir = std::env::temp_dir().join(format!("idealyst-scan-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (path, text) in files {
            let p = dir.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        let plan = ScanPlan {
            crates: vec![ScanCrate { name: "my_app".into(), root: dir.join("src/lib.rs"), cfg: Cfg::default() }],
            packages: vec!["my-app".into()],
            macro_deps: Vec::new(),
            member_dirs: vec![dir.clone()],
            target_dir: dir.join("target"),
            deps_inputs: None,
        };
        (dir, plan)
    }

    #[test]
    fn scans_components_props_enums_and_values_with_module_paths() {
        let (dir, plan) = fake_crate(
            "basic",
            &[
                (
                    "src/lib.rs",
                    r#"
pub mod components;

/// Counts clicks.
#[component]
fn Counter(start: i32, #[prop(default = "x".into())] label: String) -> Element { todo!() }

/// Presentation style.
#[derive(Clone, IdealystSchema)]
pub enum Mode {
    /// Plain.
    Plain,
    Sized(u32),
}

/// A loud tone.
#[derive(Copy, Clone, Default, IdealystSchema)]
#[schema(value_of = "ToneRef")]
pub struct Hype;
"#,
                ),
                (
                    "src/components/mod.rs",
                    r#"
pub mod card;
mod inner {
    /// Nested.
    #[component]
    fn Deep(props: &DeepProps) -> Element { todo!() }
    #[props]
    #[derive(IdealystSchema)]
    pub struct DeepProps {
        /// Shown.
        pub title: String,
        #[prop(static)]
        pub fixed: u32,
        pub on_click: Rc<dyn Fn()>,
        pub tone: Option<ToneRef>,
    }
}
"#,
                ),
                (
                    "src/components/card.rs",
                    r#"
/// A card.
#[runtime_core::component]
pub fn Card(props: &CardProps) -> Element { todo!() }
#[derive(Default, IdealystSchema)]
pub struct CardProps {
    /// Card title.
    #[schema(constraint = "max 80 chars")]
    pub title: String,
}
"#,
                ),
            ],
        );

        let json = crate_json(&plan, &dir).unwrap();
        assert_eq!(json["scanned_crate"], "my_app");
        let comps = json["components"].as_array().unwrap();
        let by_name = |n: &str| comps.iter().find(|c| c["name"] == n).unwrap_or_else(|| panic!("{n} scanned"));

        // Inline props: the params are the props, with the type the macro
        // gives them (data wrapped `Reactive<…>`), spelled as a compiled
        // catalog spells it — the param string is taken before the macro's
        // `runtime_core` → `runtime_vocabulary::glue` retarget.
        let counter = by_name("Counter");
        assert_eq!(counter["module_path"], "my_app");
        assert_eq!(counter["docs"], "Counts clicks.");
        assert_eq!(counter["params"][1]["name"], "label");
        assert_eq!(counter["params"][1]["type"], ":: runtime_core :: Reactive < String >");
        assert!(counter["params"][0].get("schema").is_none());

        // Explicit props in a nested inline mod, `#[props]` wrapping applied
        // by the macro itself.
        let deep = by_name("Deep");
        assert_eq!(deep["module_path"], "my_app::components::inner");
        let schema = deep["params"][0]["schema"].as_array().unwrap();
        let field = |n: &str| schema.iter().find(|f| f["name"] == n).unwrap();
        assert_eq!(field("title")["type"], ":: runtime_vocabulary :: glue :: Reactive < String >");
        assert_eq!(field("title")["doc"], "Shown.");
        assert_eq!(field("fixed")["type"], "u32", "#[prop(static)] stays bare");
        assert_eq!(field("on_click")["type"], "Rc < dyn Fn() >", "handlers never wrap");
        assert_eq!(
            field("tone")["type"],
            ":: runtime_vocabulary :: glue :: Reactive < Option < ToneRef > >",
            "Option<data> wraps"
        );

        // Path-qualified attribute, file-module path, constraint hint.
        let card = by_name("Card");
        assert_eq!(card["module_path"], "my_app::components::card");
        assert_eq!(card["file"], dir.join("src/components/card.rs").to_string_lossy().as_ref());
        assert_eq!(card["line"], 3);
        assert_eq!(card["params"][0]["type_short_name"], "CardProps");
        assert_eq!(card["params"][0]["schema"][0]["constraint"], "max 80 chars");
        assert_eq!(card["params"][0]["schema"][0]["type"], "String", "no #[props]: no wrapping");

        let types = json["types"].as_array().unwrap();
        let mode = types.iter().find(|t| t["short_name"] == "Mode").unwrap();
        assert_eq!(mode["shape"]["variants"][0]["docs"], "Plain.");
        assert_eq!(mode["shape"]["variants"][1]["payload"].as_array().unwrap().len(), 1);

        let values = json["values"].as_array().unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["spelled"], "my_app::Hype");
        assert_eq!(values[0]["import"], "my_app");
        assert_eq!(values[0]["value_of"], "ToneRef");

        let _ = fs::remove_dir_all(&dir);
    }

    /// The point of a source scan: a file mid-edit must not take the
    /// others down with it.
    #[test]
    fn a_file_that_does_not_parse_is_skipped_and_the_rest_still_scan() {
        let (dir, plan) = fake_crate(
            "broken",
            &[
                ("src/lib.rs", "mod ok; mod bad;\n"),
                ("src/ok.rs", "#[component]\nfn Fine() -> Element { todo!() }\n"),
                ("src/bad.rs", "#[component]\nfn Broken( -> Element {\n"),
            ],
        );
        let json = crate_json(&plan, &dir).unwrap();
        let names: Vec<&str> = json["components"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Fine"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A crate the scan cannot read in full still lists what it read
    /// before the refusal: completion wants something, and the compiled
    /// catalog will supply the rest.
    #[test]
    fn a_crate_the_scan_refuses_still_lists_what_it_read() {
        let (dir, plan) = fake_crate(
            "refused",
            &[(
                "src/lib.rs",
                "#[component]\nfn Before() -> Element { todo!() }\ninventory::submit! { ::runtime_core::__mcp::IconSetEntry { name: \"x\", icons: ICONS } }\n",
            )],
        );
        let json = crate_json(&plan, &dir).unwrap();
        assert_eq!(json["components"][0]["name"], "Before");
        let _ = fs::remove_dir_all(&dir);
    }
}
