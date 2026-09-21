//! `idealyst catalog-scan DIR` — the project's own catalog entries from
//! its SOURCE, without compiling anything.
//!
//! The compiled catalog (`catalog-json`) is authoritative: it links the
//! real crates and reads the `inventory` registrations the macros emit.
//! It is also slow to refresh (a cargo build of the app) and brittle
//! while the author is mid-edit (one file that doesn't compile takes
//! the whole catalog down). This command is the fast, forgiving half:
//! it parses each `.rs` file under the crate's `src/` with `syn` and
//! lifts the same facts the macros would register —
//!
//! - `#[component] fn Name(…)` → a component with its params, or its
//!   props struct's fields when the signature is `props: &NameProps`;
//! - `#[derive(IdealystSchema)] enum` → a type with its variants;
//! - `#[schema(value_of = …, via = …)] struct` → a value, spelled
//!   through the catalog's own `value_route` so it matches the
//!   compiled entry byte for byte —
//!
//! in the same JSON shape `catalog-json` emits for those slices, plus
//! `scanned_crate` (the crate's name as a module path root) so a
//! consumer can replace that crate's compiled entries with the fresh
//! ones. A file that doesn't parse is reported on stderr and skipped;
//! every other file still contributes. Sub-second on a large crate.
//!
//! What it deliberately does NOT do: dependencies (idea-ui's components
//! come from the compiled catalog), `composes` edges, primitives,
//! macros, utilities, tokens — none of which change when the author
//! adds a component.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use quote::ToTokens;
use serde_json::{json, Value};
use syn::{Attribute, Expr, Fields, Item, Lit, Meta, Type};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Crate directory (the one holding `Cargo.toml` + `src/`).
    #[arg(default_value = ".")]
    pub dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve crate dir {}", args.dir.display()))?;
    let json = scan_crate(&dir)?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

/// Scan one crate directory into the catalog-shaped JSON described in
/// the module docs.
pub fn scan_crate(dir: &Path) -> Result<Value> {
    let crate_name = crate_name(dir)?;
    let src = dir.join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    files.sort();

    let mut scan = Scan::default();
    // Two passes: props structs first, so a component in an earlier
    // file can resolve a props struct declared in a later one.
    let mut parsed: Vec<(PathBuf, String, syn::File)> = Vec::new();
    for file in &files {
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[catalog-scan] skipping {}: {e}", file.display());
                continue;
            }
        };
        match syn::parse_file(&text) {
            Ok(ast) => parsed.push((file.clone(), module_path_for(&crate_name, &src, file), ast)),
            Err(e) => eprintln!(
                "[catalog-scan] skipping {} (does not parse: {e}); other files still scanned",
                file.display()
            ),
        }
    }
    for (_, module, ast) in &parsed {
        scan.collect_structs(module, &ast.items);
    }
    for (file, module, ast) in &parsed {
        scan.collect_entries(file, module, &ast.items);
    }

    Ok(json!({
        "catalog_version": 2,
        "scanned_crate": crate_name,
        "components": scan.components,
        "types": scan.types,
        "values": scan.values,
    }))
}

/// `[package] name` of the crate, spelled as a module path root.
fn crate_name(dir: &Path) -> Result<String> {
    let manifest_path = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parse {}", manifest_path.display()))?;
    let name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .with_context(|| format!("{} has no [package] name", manifest_path.display()))?;
    Ok(name.replace('-', "_"))
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `module_path!()` as rustc would report it for a file, from its place
/// under `src/`: `lib.rs` / `main.rs` → the crate, `a/mod.rs` → `a`,
/// `a/b.rs` → `a::b`. Inline `mod x { … }` blocks extend it during the
/// walk.
fn module_path_for(crate_name: &str, src: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(src).unwrap_or(file);
    let mut segs: Vec<String> = rel
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    if let Some(last) = segs.pop() {
        let stem = last.trim_end_matches(".rs");
        if stem != "lib" && stem != "main" && stem != "mod" {
            segs.push(stem.to_string());
        }
    }
    std::iter::once(crate_name.to_string())
        .chain(segs)
        .collect::<Vec<_>>()
        .join("::")
}

#[derive(Default)]
struct Scan {
    /// Named structs by `module::Name` and by bare `Name` — the latter
    /// for the common case of a props struct declared next to its fn.
    structs: std::collections::HashMap<String, PropsStruct>,
    components: Vec<Value>,
    types: Vec<Value>,
    values: Vec<Value>,
}

#[derive(Clone)]
struct PropsStruct {
    /// `#[props]` rewrites data fields to `Reactive<T>`; mirror that so
    /// the scanned type reads like the compiled one.
    props_attr: bool,
    fields: Vec<(String, Type, Vec<Attribute>)>,
}

impl Scan {
    fn collect_structs(&mut self, module: &str, items: &[Item]) {
        for item in items {
            match item {
                Item::Struct(s) => {
                    if let Fields::Named(named) = &s.fields {
                        let ps = PropsStruct {
                            props_attr: s.attrs.iter().any(|a| attr_is(a, "props")),
                            fields: named
                                .named
                                .iter()
                                .filter_map(|f| {
                                    Some((f.ident.as_ref()?.to_string(), f.ty.clone(), f.attrs.clone()))
                                })
                                .collect(),
                        };
                        self.structs.insert(format!("{module}::{}", s.ident), ps.clone());
                        self.structs.entry(s.ident.to_string()).or_insert(ps);
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        self.collect_structs(&format!("{module}::{}", m.ident), inner);
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_entries(&mut self, file: &Path, module: &str, items: &[Item]) {
        for item in items {
            match item {
                Item::Fn(f) if f.attrs.iter().any(|a| attr_is(a, "component")) => {
                    let params = self.params_for(module, f);
                    self.components.push(json!({
                        "name": f.sig.ident.to_string(),
                        "module_path": module,
                        "file": file.display().to_string(),
                        "line": 0,
                        "docs": docs_of(&f.attrs),
                        "params": params,
                        "composes": [],
                    }));
                }
                Item::Enum(e) if derives_schema(&e.attrs) => {
                    let variants: Vec<Value> = e
                        .variants
                        .iter()
                        .map(|v| {
                            let payload: Vec<Value> = match &v.fields {
                                Fields::Unit => Vec::new(),
                                Fields::Named(n) => n
                                    .named
                                    .iter()
                                    .map(|f| {
                                        json!({
                                            "name": f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                                            "type": type_str(&f.ty),
                                            "doc": docs_of(&f.attrs),
                                            "constraint": "",
                                        })
                                    })
                                    .collect(),
                                Fields::Unnamed(u) => u
                                    .unnamed
                                    .iter()
                                    .map(|f| json!({ "name": "", "type": type_str(&f.ty), "doc": docs_of(&f.attrs), "constraint": "" }))
                                    .collect(),
                            };
                            json!({ "name": v.ident.to_string(), "docs": docs_of(&v.attrs), "payload": payload })
                        })
                        .collect();
                    self.types.push(json!({
                        "short_name": e.ident.to_string(),
                        "module_path": module,
                        "fqn": format!("{module}::{}", e.ident),
                        "docs": docs_of(&e.attrs),
                        "shape": { "kind": "enum", "variants": variants },
                    }));
                }
                Item::Struct(s) => {
                    if let Some((value_of, via)) = schema_value_of(&s.attrs) {
                        let (import, prefix) = mcp_catalog::value_route(module, &via);
                        let name = s.ident.to_string();
                        let spelled = if prefix.is_empty() { name.clone() } else { format!("{prefix}::{name}") };
                        self.values.push(json!({
                            "short_name": name,
                            "module_path": module,
                            "docs": docs_of(&s.attrs),
                            "value_of": value_of,
                            "via": via,
                            "spelled": spelled,
                            "import": import,
                        }));
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        self.collect_entries(file, &format!("{module}::{}", m.ident), inner);
                    }
                }
                _ => {}
            }
        }
    }

    /// A component's params as `ParamSpec`s; a lone `props: &XProps`
    /// carries `schema` = the struct's fields, as the compiled catalog's
    /// prop-field inliner would produce.
    fn params_for(&self, module: &str, f: &syn::ItemFn) -> Vec<Value> {
        let typed: Vec<(String, &Type)> = f
            .sig
            .inputs
            .iter()
            .filter_map(|arg| match arg {
                syn::FnArg::Typed(pt) => {
                    let name = match &*pt.pat {
                        syn::Pat::Ident(p) => p.ident.to_string(),
                        other => other.to_token_stream().to_string(),
                    };
                    Some((name, &*pt.ty))
                }
                syn::FnArg::Receiver(_) => None,
            })
            .collect();
        typed
            .iter()
            .map(|(name, ty)| {
                let short = type_short_name(ty);
                let mut spec = json!({
                    "name": name,
                    "type": type_str(ty),
                    "type_short_name": short,
                });
                if typed.len() == 1 {
                    let props = self
                        .structs
                        .get(&format!("{module}::{short}"))
                        .or_else(|| self.structs.get(&short));
                    if let Some(ps) = props {
                        spec["schema"] = Value::Array(
                            ps.fields
                                .iter()
                                .map(|(fname, fty, attrs)| {
                                    let ty = if ps.props_attr && !prop_forced_static(attrs) && should_wrap(fty) {
                                        format!("Reactive<{}>", type_str(fty))
                                    } else {
                                        type_str(fty)
                                    };
                                    json!({
                                        "name": fname,
                                        "type": ty,
                                        "doc": docs_of(attrs),
                                        "constraint": schema_constraint(attrs),
                                    })
                                })
                                .collect(),
                        );
                    }
                }
                spec
            })
            .collect()
    }
}

fn attr_is(attr: &Attribute, name: &str) -> bool {
    attr.path().segments.last().is_some_and(|s| s.ident == name)
}

fn derives_schema(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        attr_is(a, "derive") && a.to_token_stream().to_string().contains("IdealystSchema")
    })
}

/// `///` lines, joined — what the macros capture as `docs`.
fn docs_of(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr_is(attr, "doc") {
            continue;
        }
        if let Meta::NameValue(nv) = &attr.meta {
            if let Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. }) = &nv.value {
                let raw = s.value();
                lines.push(raw.strip_prefix(' ').unwrap_or(&raw).to_string());
            }
        }
    }
    lines.join("\n")
}

/// `#[schema(value_of = "…", via = "…")]` on a type.
fn schema_value_of(attrs: &[Attribute]) -> Option<(String, String)> {
    let mut target = None;
    let mut via = String::new();
    for attr in attrs.iter().filter(|a| attr_is(a, "schema")) {
        let _ = attr.parse_nested_meta(|m| {
            if m.path.is_ident("value_of") {
                let s: syn::LitStr = m.value()?.parse()?;
                target = Some(s.value());
            } else if m.path.is_ident("via") {
                let s: syn::LitStr = m.value()?.parse()?;
                via = s.value();
            }
            Ok(())
        });
    }
    target.map(|t| (t, via))
}

/// `#[schema(constraint = "…")]` on a field.
fn schema_constraint(attrs: &[Attribute]) -> String {
    let mut found = String::new();
    for attr in attrs.iter().filter(|a| attr_is(a, "schema")) {
        let _ = attr.parse_nested_meta(|m| {
            if m.path.is_ident("constraint") {
                let s: syn::LitStr = m.value()?.parse()?;
                found = s.value();
            }
            Ok(())
        });
    }
    found
}

/// `#[prop(static)]` keeps a `#[props]` field bare.
fn prop_forced_static(attrs: &[Attribute]) -> bool {
    attrs.iter().filter(|a| attr_is(a, "prop")).any(|a| {
        let mut is_static = false;
        let _ = a.parse_nested_meta(|m| {
            if m.path.is_ident("static") {
                is_static = true;
            }
            Ok(())
        });
        is_static
    })
}

/// Mirror of `runtime_macros::props_attr::should_wrap` — the data-vs-not
/// rule `#[props]` applies. Kept in sync by the test below against the
/// same shapes that module's tests pin.
fn should_wrap(ty: &Type) -> bool {
    const SKIP: &[&str] = &[
        "Rc", "Arc", "Box", "Signal", "ReadSignal", "WriteSignal", "Reactive", "Rx", "Ref",
        "Bound", "Bindable", "RefFill", "Action", "Element", "ChildList", "Vec", "HashMap",
        "BTreeMap", "HashSet", "PhantomData",
    ];
    match ty {
        Type::Path(tp) => {
            let Some(seg) = tp.path.segments.last() else {
                return true;
            };
            let name = seg.ident.to_string();
            if SKIP.contains(&name.as_str()) {
                return false;
            }
            if name == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(t) = arg {
                            return should_wrap(t);
                        }
                    }
                }
                return true;
            }
            true
        }
        _ => false,
    }
}

fn type_str(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}

/// Last path segment, through references: `&FooProps` → `FooProps`.
fn type_short_name(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => type_short_name(&r.elem),
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_crate(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("idealyst-scan-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/components")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n").unwrap();
        dir
    }

    #[test]
    fn scans_components_props_enums_and_values_with_module_paths() {
        let dir = fake_crate("basic");
        fs::write(
            dir.join("src/lib.rs"),
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
        )
        .unwrap();
        fs::write(
            dir.join("src/components/mod.rs"),
            r#"
pub mod card;
mod inner {
    /// Nested.
    #[component]
    fn Deep(props: &DeepProps) -> Element { todo!() }
    #[props]
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
        )
        .unwrap();
        fs::write(
            dir.join("src/components/card.rs"),
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
        )
        .unwrap();

        let json = scan_crate(&dir).unwrap();
        assert_eq!(json["scanned_crate"], "my_app");
        let comps = json["components"].as_array().unwrap();
        let by_name = |n: &str| comps.iter().find(|c| c["name"] == n).unwrap_or_else(|| panic!("{n} scanned"));

        // Inline props: the params are the props; attrs don't leak into types.
        let counter = by_name("Counter");
        assert_eq!(counter["module_path"], "my_app");
        assert_eq!(counter["docs"], "Counts clicks.");
        assert_eq!(counter["params"][1]["name"], "label");
        assert_eq!(counter["params"][1]["type"], "String");
        assert!(counter["params"][0].get("schema").is_none());

        // Explicit props in a nested inline mod, with #[props] wrapping mirrored.
        let deep = by_name("Deep");
        assert_eq!(deep["module_path"], "my_app::components::inner");
        let schema = deep["params"][0]["schema"].as_array().unwrap();
        let field = |n: &str| schema.iter().find(|f| f["name"] == n).unwrap();
        assert_eq!(field("title")["type"], "Reactive<String>");
        assert_eq!(field("title")["doc"], "Shown.");
        assert_eq!(field("fixed")["type"], "u32", "#[prop(static)] stays bare");
        assert_eq!(field("on_click")["type"], "Rc < dyn Fn () >", "handlers never wrap");
        assert_eq!(field("tone")["type"], "Reactive<Option < ToneRef >>", "Option<data> wraps");

        // Path-qualified attribute, file-module path, constraint hint.
        let card = by_name("Card");
        assert_eq!(card["module_path"], "my_app::components::card");
        assert_eq!(card["params"][0]["type_short_name"], "CardProps");
        assert_eq!(card["params"][0]["schema"][0]["constraint"], "max 80 chars");
        assert_eq!(card["params"][0]["schema"][0]["type"], "String", "no #[props]: no wrapping");

        let types = json["types"].as_array().unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0]["short_name"], "Mode");
        assert_eq!(types[0]["shape"]["variants"][0]["docs"], "Plain.");
        assert_eq!(types[0]["shape"]["variants"][1]["payload"].as_array().unwrap().len(), 1);

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
        let dir = fake_crate("broken");
        fs::write(dir.join("src/lib.rs"), "mod ok; mod bad;\n").unwrap();
        fs::write(dir.join("src/ok.rs"), "#[component]\nfn Fine() -> Element { todo!() }\n").unwrap();
        fs::write(dir.join("src/bad.rs"), "#[component]\nfn Broken( -> Element {\n").unwrap();
        let json = scan_crate(&dir).unwrap();
        let names: Vec<&str> = json["components"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Fine"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn module_paths_follow_file_layout() {
        let src = Path::new("/c/src");
        let mp = |f: &str| module_path_for("my_app", src, &src.join(f));
        assert_eq!(mp("lib.rs"), "my_app");
        assert_eq!(mp("main.rs"), "my_app");
        assert_eq!(mp("a/mod.rs"), "my_app::a");
        assert_eq!(mp("a/b.rs"), "my_app::a::b");
        assert_eq!(mp("a.rs"), "my_app::a");
    }
}
