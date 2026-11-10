//! Build-time extraction of each component's source, for the docs page's
//! "View source", and of each component crate's public names, for the
//! Imports block (so `idea_ui::components::alert::Alert` is written as the
//! `idea_ui::Alert` a caller imports) and the component list (a library's
//! private components get no page). The public names come from walking
//! the crate's module tree from `src/lib.rs`: which modules are `pub mod`,
//! and what each module's `pub use` items re-export, followed through
//! chains (`table/mod.rs` re-exports `group::TableGroup`, `lib.rs`
//! re-exports `components::table::TableGroup`).
//!
//! Shared by `build.rs` (which runs it, via `#[path]`) and this crate's
//! unit tests. The catalog records each component's file (absolute, from
//! `file!()`); this parses that file and returns the component fn's text
//! as written — doc comments and `#[component]` included, body comments
//! kept — by slicing the original lines the item spans. Parsing with `syn`
//! rather than scanning for braces is what makes a `'{'` or a format
//! string inside the body harmless.

use std::collections::BTreeMap;

/// The source of fn `name` in `text`: from its first attribute (or `fn`
/// line) through its closing brace, whole lines. Searches inline modules
/// too. `None` when the file doesn't parse or has no such fn.
pub fn fn_source(text: &str, name: &str) -> Option<String> {
    let file = syn::parse_file(text).ok()?;
    let (first, last) = find(&file.items, name)?;
    let lines: Vec<&str> = text.lines().collect();
    let body = lines.get(first - 1..last)?.join("\n");
    Some(dedent(&body))
}

/// `(first line, last line)`, 1-based, of the fn named `name`.
fn find(items: &[syn::Item], name: &str) -> Option<(usize, usize)> {
    for item in items {
        match item {
            syn::Item::Fn(f) if f.sig.ident == name => {
                let start = f
                    .attrs
                    .first()
                    .map(|a| a.pound_token.span.start().line)
                    .unwrap_or_else(|| f.sig.fn_token.span.start().line);
                let end = f.block.brace_token.span.close().end().line;
                return Some((start, end));
            }
            syn::Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    if let Some(found) = find(items, name) {
                        return Some(found);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Strip the indentation every non-blank line shares (a fn inside a
/// module is indented).
fn dedent(block: &str) -> String {
    let indent = block
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    block
        .lines()
        .map(|l| if l.len() >= indent { &l[indent..] } else { l.trim_start() })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A catalog `file`, as a readable path. An extracted project's catalog
/// records absolute paths; the framework's own records `file!()`'s
/// workspace-relative ones (`crates/ui/idea-ui/src/…`), found by trying
/// each directory above `base` (the build's manifest dir).
pub fn resolve_file(file: &str, base: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(file);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    base.ancestors().map(|dir| dir.join(path)).find(|p| p.is_file())
}

/// `module_path::Name` → source, for every catalog component whose file
/// can be read and parsed (paths resolved against `base`, see
/// [`resolve_file`]). Each file is read once.
pub fn component_sources(cat: &serde_json::Value, base: &std::path::Path) -> BTreeMap<String, String> {
    let mut files: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for c in cat["components"].as_array().into_iter().flatten() {
        let (Some(name), Some(module), Some(file)) = (c["name"].as_str(), c["module_path"].as_str(), c["file"].as_str())
        else {
            continue;
        };
        let text = files
            .entry(file.to_string())
            .or_insert_with(|| resolve_file(file, base).and_then(|p| std::fs::read_to_string(p).ok()));
        if let Some(src) = text.as_deref().and_then(|t| fn_source(t, name)) {
            out.insert(format!("{module}::{name}"), src);
        }
    }
    out
}

/// A crate's public names, for the docs' Imports block and component
/// list: each public item's defining path (`idea_ui::components::table::
/// group::TableGroup`) maps to the shortest path a caller can `use` it by
/// (`idea_ui::TableGroup`). An item missing from `public` can't be named
/// from outside the crate.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CrateExports {
    pub public: BTreeMap<String, String>,
}

/// One module of the crate being resolved.
#[derive(Default)]
struct Module {
    /// Every ancestor (and itself) is `pub mod`: its `pub` names are
    /// reachable from outside.
    public: bool,
    /// Items defined here: name → declared `pub`.
    items: BTreeMap<String, bool>,
    /// `use` names: alias → every (target, declared `pub`) it names — one
    /// name can import a module and a macro at once (`tone`, `tone!`).
    uses: BTreeMap<String, Vec<(String, bool)>>,
    /// `use target::*`: (absolute module, declared `pub`).
    globs: Vec<(String, bool)>,
}

/// The module tree of the crate rooted at `root` (its `src/lib.rs`),
/// walked through `mod` declarations. Returns every file read, for
/// `rerun-if-changed`.
fn read_modules(krate: &str, root: &std::path::Path) -> (BTreeMap<String, Module>, Vec<std::path::PathBuf>) {
    let mut modules = BTreeMap::new();
    let mut files = Vec::new();
    read_file_module(krate.to_string(), true, root, true, &mut modules, &mut files);
    (modules, files)
}

fn read_file_module(
    path: String,
    public: bool,
    file: &std::path::Path,
    owns_dir: bool,
    modules: &mut BTreeMap<String, Module>,
    files: &mut Vec<std::path::PathBuf>,
) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    files.push(file.to_path_buf());
    let Ok(parsed) = syn::parse_file(&text) else { return };
    // `mod x;` in `lib.rs` / `mod.rs` lives beside it; in `a.rs`, under `a/`.
    let dir = if owns_dir {
        file.parent().map(|d| d.to_path_buf())
    } else {
        file.parent().zip(file.file_stem()).map(|(d, stem)| d.join(stem))
    };
    read_items(path, public, &parsed.items, dir.as_deref(), modules, files);
}

fn read_items(
    path: String,
    public: bool,
    items: &[syn::Item],
    dir: Option<&std::path::Path>,
    modules: &mut BTreeMap<String, Module>,
    files: &mut Vec<std::path::PathBuf>,
) {
    let mut module = Module { public, ..Module::default() };
    let mut children = Vec::new();
    for item in items {
        let (name, vis) = match item {
            syn::Item::Fn(i) => (i.sig.ident.to_string(), &i.vis),
            syn::Item::Struct(i) => (i.ident.to_string(), &i.vis),
            syn::Item::Enum(i) => (i.ident.to_string(), &i.vis),
            syn::Item::Type(i) => (i.ident.to_string(), &i.vis),
            syn::Item::Const(i) => (i.ident.to_string(), &i.vis),
            syn::Item::Static(i) => (i.ident.to_string(), &i.vis),
            syn::Item::Trait(i) => (i.ident.to_string(), &i.vis),
            syn::Item::Union(i) => (i.ident.to_string(), &i.vis),
            syn::Item::Macro(i) => {
                // `#[macro_export]` macros live at the crate root.
                if let Some(ident) = &i.ident {
                    module.items.insert(ident.to_string(), false);
                }
                continue;
            }
            syn::Item::Mod(m) => {
                let name = m.ident.to_string();
                let is_pub = matches!(m.vis, syn::Visibility::Public(_));
                module.items.insert(name.clone(), is_pub);
                children.push((m, name, is_pub));
                continue;
            }
            syn::Item::Use(u) => {
                let is_pub = matches!(u.vis, syn::Visibility::Public(_));
                collect_use(&u.tree, Vec::new(), is_pub, &mut module);
                continue;
            }
            _ => continue,
        };
        module.items.insert(name, matches!(vis, syn::Visibility::Public(_)));
    }
    for (m, name, is_pub) in children {
        let child = format!("{path}::{name}");
        let child_public = public && is_pub;
        match (&m.content, dir) {
            (Some((_, inner)), _) => {
                read_items(child, child_public, inner, dir.map(|d| d.join(&name)).as_deref(), modules, files)
            }
            (None, Some(dir)) => {
                let flat = dir.join(format!("{name}.rs"));
                if flat.is_file() {
                    read_file_module(child, child_public, &flat, false, modules, files);
                } else {
                    read_file_module(child, child_public, &dir.join(&name).join("mod.rs"), true, modules, files);
                }
            }
            (None, None) => {}
        }
    }
    modules.insert(path, module);
}

/// Record a `use` tree's names, with targets still relative (resolved
/// once every module is known, by [`absolute`]).
fn collect_use(tree: &syn::UseTree, prefix: Vec<String>, is_pub: bool, module: &mut Module) {
    match tree {
        syn::UseTree::Path(p) => {
            let mut next = prefix;
            next.push(p.ident.to_string());
            collect_use(&p.tree, next, is_pub, module);
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last().cloned() {
                    module.uses.entry(last).or_default().push((prefix.join("::"), is_pub));
                }
            } else {
                let mut target = prefix;
                target.push(name.clone());
                module.uses.entry(name).or_default().push((target.join("::"), is_pub));
            }
        }
        syn::UseTree::Rename(r) => {
            let mut target = prefix;
            target.push(r.ident.to_string());
            module.uses.entry(r.rename.to_string()).or_default().push((target.join("::"), is_pub));
        }
        syn::UseTree::Glob(_) => module.globs.push((prefix.join("::"), is_pub)),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                collect_use(t, prefix.clone(), is_pub, module);
            }
        }
    }
}

/// A `use` path written in module `at`, made absolute: `crate::`,
/// `self::` and `super::` as Rust reads them, a first segment naming a
/// child module or a name `at` defines or imports is relative to `at`,
/// and anything else names another crate.
fn absolute(target: &str, at: &str, krate: &str, modules: &BTreeMap<String, Module>) -> String {
    let segs: Vec<&str> = target.split("::").collect();
    let mut base: Vec<String> = at.split("::").map(str::to_string).collect();
    let mut rest = &segs[..];
    match rest.first().copied() {
        Some("crate") => {
            base = vec![krate.to_string()];
            rest = &rest[1..];
        }
        Some("self") => rest = &rest[1..],
        Some("super") => {
            while rest.first() == Some(&"super") {
                base.pop();
                rest = &rest[1..];
            }
        }
        Some(first) => {
            let local = modules.get(at).is_some_and(|m| m.items.contains_key(first) || m.uses.contains_key(first));
            if !local {
                return target.to_string();
            }
        }
        None => return target.to_string(),
    }
    base.extend(rest.iter().map(|s| s.to_string()));
    base.join("::")
}

/// Where `path` (an absolute path into this crate or another) is defined:
/// followed through `use` aliases and glob imports until it reaches an
/// item, or leaves the crate.
fn definition(path: &str, krate: &str, modules: &BTreeMap<String, Module>, depth: usize) -> String {
    let Some((parent, name)) = path.rsplit_once("::") else { return path.to_string() };
    let Some(module) = modules.get(parent) else { return path.to_string() };
    if depth > 16 || module.items.contains_key(name) {
        return path.to_string();
    }
    if let Some((target, _)) = module.uses.get(name).and_then(|t| t.first()) {
        return definition(&absolute(target, parent, krate, modules), krate, modules, depth + 1);
    }
    for (glob, _) in &module.globs {
        let from = absolute(glob, parent, krate, modules);
        let candidate = format!("{from}::{name}");
        if modules.get(&from).is_some_and(|m| m.items.contains_key(name) || m.uses.contains_key(name)) {
            return definition(&candidate, krate, modules, depth + 1);
        }
    }
    path.to_string()
}

/// The public names of crate `krate`, whose root source file is `root`
/// (see [`CrateExports`]). Returns the files read too.
pub fn crate_public_names(krate: &str, root: &std::path::Path) -> (CrateExports, Vec<std::path::PathBuf>) {
    let (modules, files) = read_modules(krate, root);
    let mut public: BTreeMap<String, String> = BTreeMap::new();
    let mut offer = |defined: String, at: String| {
        let better = public.get(&defined).is_none_or(|have| {
            (at.matches("::").count(), &at) < (have.matches("::").count(), have)
        });
        if better {
            public.insert(defined, at);
        }
    };
    for (path, module) in modules.iter().filter(|(_, m)| m.public) {
        for (name, &is_pub) in &module.items {
            if is_pub {
                offer(format!("{path}::{name}"), format!("{path}::{name}"));
            }
        }
        for (name, targets) in &module.uses {
            for (target, is_pub) in targets {
                if *is_pub {
                    let defined = definition(&absolute(target, path, krate, &modules), krate, &modules, 0);
                    offer(defined, format!("{path}::{name}"));
                }
            }
        }
        for (glob, is_pub) in &module.globs {
            if !*is_pub {
                continue;
            }
            let from = absolute(glob, path, krate, &modules);
            let Some(source) = modules.get(&from) else { continue };
            let names = source
                .items
                .iter()
                .filter(|(_, p)| **p)
                .map(|(n, _)| n)
                .chain(source.uses.iter().filter(|(_, t)| t.iter().any(|(_, p)| *p)).map(|(n, _)| n));
            for name in names {
                offer(definition(&format!("{from}::{name}"), krate, &modules, 0), format!("{path}::{name}"));
            }
        }
    }
    (CrateExports { public }, files)
}

/// The public names of every crate a catalog component comes from, keyed
/// by crate name, and the files read. A crate's root is `src/lib.rs`
/// beside the nearest `Cargo.toml` above a component's file (resolved
/// against `base`, see [`resolve_file`]); a crate without one (an app's
/// binary) is left out — its imports keep their defining paths.
pub fn crate_exports(
    cat: &serde_json::Value,
    base: &std::path::Path,
) -> (BTreeMap<String, CrateExports>, Vec<std::path::PathBuf>) {
    let mut out = BTreeMap::new();
    let mut files = Vec::new();
    for c in cat["components"].as_array().into_iter().flatten() {
        let (Some(module), Some(file)) = (c["module_path"].as_str(), c["file"].as_str()) else { continue };
        let krate = module.split("::").next().unwrap_or(module).to_string();
        if out.contains_key(&krate) {
            continue;
        }
        // From the resolved, absolute file: a relative path's last
        // ancestor is the empty path, which would "find" whatever
        // `Cargo.toml` sits in the current directory.
        let Some(file) = resolve_file(file, base) else { continue };
        let Some(root) = file.ancestors().skip(1).find(|d| d.join("Cargo.toml").is_file()).map(|d| d.join("src/lib.rs"))
        else {
            continue;
        };
        if root.is_file() {
            let (exports, read) = crate_public_names(&krate, &root);
            files.extend(read);
            out.insert(krate, exports);
        }
    }
    (out, files)
}

/// Drop the components a library keeps private (a helper like a table's
/// row toggle) from `cat`: a reader of its docs can't use them. Only for a
/// crate that exports at least one component — an app's components are
/// all its own, and stay.
pub fn drop_private_components(cat: &mut serde_json::Value, exports: &BTreeMap<String, CrateExports>) {
    let is_public = |c: &serde_json::Value| -> Option<bool> {
        let (module, name) = (c["module_path"].as_str()?, c["name"].as_str()?);
        let krate = module.split("::").next()?;
        Some(exports.get(krate)?.public.contains_key(&format!("{module}::{name}")))
    };
    let Some(components) = cat["components"].as_array() else { return };
    let library_crates: std::collections::BTreeSet<String> = components
        .iter()
        .filter(|c| is_public(c) == Some(true))
        .filter_map(|c| c["module_path"].as_str()?.split("::").next().map(str::to_string))
        .collect();
    let kept: Vec<serde_json::Value> = components
        .iter()
        .filter(|c| {
            let krate = c["module_path"].as_str().and_then(|m| m.split("::").next()).unwrap_or("");
            !library_crates.contains(krate) || is_public(c) == Some(true)
        })
        .cloned()
        .collect();
    cat["components"] = serde_json::Value::Array(kept);
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"use x::y;

/// A badge.
/// Second line.
#[component]
pub fn Badge(props: &BadgeProps) -> Element {
    // A comment the reader should see.
    let s = format!("{}", '{');
    ui! { view() }
}

mod inner {
    /// Nested.
    #[component]
    fn Deep() -> Element {
        ui! { view() }
    }
}
"#;

    #[test]
    fn takes_the_fn_with_its_docs_and_comments() {
        let src = fn_source(FILE, "Badge").unwrap();
        assert!(src.starts_with("/// A badge.\n/// Second line.\n#[component]"), "{src}");
        assert!(src.contains("// A comment the reader should see."), "{src}");
        assert!(src.ends_with("ui! { view() }\n}"), "a brace in a char literal doesn't end it early: {src}");
    }

    #[test]
    fn finds_fns_in_inline_modules_and_dedents_them() {
        let src = fn_source(FILE, "Deep").unwrap();
        assert!(src.starts_with("/// Nested.\n#[component]\nfn Deep()"), "{src}");
    }

    #[test]
    fn component_sources_reads_each_catalog_file_and_skips_unreadable_ones() {
        let dir = std::env::temp_dir().join(format!("docs-app-sources-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("badge.rs");
        std::fs::write(&file, FILE).unwrap();
        let cat = serde_json::json!({ "components": [
            { "name": "Badge", "module_path": "lib::badge", "file": file.to_str().unwrap() },
            { "name": "Gone", "module_path": "lib::gone", "file": "/does/not/exist.rs" },
        ]});
        let sources = component_sources(&cat, &dir);
        assert!(sources["lib::badge::Badge"].contains("pub fn Badge"));
        assert!(!sources.contains_key("lib::gone::Gone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A small crate on disk: `lib.rs` re-exports through a private
    /// module's own re-export, a glob, a rename and another crate.
    fn write_crate(files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("docs-app-exports-{}-{}", std::process::id(), files.len()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn public_names_follow_re_export_chains_globs_and_renames() {
        let dir = write_crate(&[
            (
                "src/lib.rs",
                "pub mod components;\nmod private;\npub use components::table::{Table, TableGroup};\npub use idea_theme::extensible::{tone, ToneRef as Tone};\npub use crate::components::card::*;\nuse components::hidden::Hidden;\n",
            ),
            ("src/components/mod.rs", "pub mod table;\npub mod card;\nmod hidden;\n"),
            ("src/components/table/mod.rs", "mod group;\npub use group::TableGroup;\npub fn Table() {}\n"),
            ("src/components/table/group.rs", "pub fn TableGroup() {}\nfn GroupToggle() {}\n"),
            ("src/components/card.rs", "pub fn Card() {}\npub struct CardPadding;\nfn helper() {}\n"),
            ("src/components/hidden.rs", "pub fn Hidden() {}\n"),
            ("src/private.rs", "pub fn Secret() {}\n"),
        ]);
        let (ex, files) = crate_public_names("idea_ui", &dir.join("src/lib.rs"));
        let public = |p: &str| ex.public.get(p).map(String::as_str);
        assert_eq!(public("idea_ui::components::table::group::TableGroup"), Some("idea_ui::TableGroup"), "two hops");
        assert_eq!(public("idea_ui::components::table::Table"), Some("idea_ui::Table"), "the root beats the module path");
        assert_eq!(public("idea_ui::components::card::Card"), Some("idea_ui::Card"), "a glob");
        assert_eq!(public("idea_theme::extensible::tone"), Some("idea_ui::tone"), "another crate's item");
        assert_eq!(public("idea_theme::extensible::ToneRef"), Some("idea_ui::Tone"), "a rename");
        for private in [
            "idea_ui::components::table::group::GroupToggle",
            "idea_ui::components::card::helper",
            "idea_ui::components::hidden::Hidden",
            "idea_ui::private::Secret",
        ] {
            assert_eq!(public(private), None, "{private}");
        }
        assert_eq!(files.len(), 7, "{files:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a library's private components are dropped: a crate that
    /// exports no component (an app) keeps all of its own.
    #[test]
    fn private_components_of_a_library_are_dropped() {
        let mut exports = BTreeMap::new();
        exports.insert(
            "lib".to_string(),
            CrateExports { public: [("lib::a::Shown".to_string(), "lib::Shown".to_string())].into() },
        );
        exports.insert("app".to_string(), CrateExports::default());
        let mut cat = serde_json::json!({ "components": [
            { "name": "Shown", "module_path": "lib::a" },
            { "name": "Helper", "module_path": "lib::a" },
            { "name": "Screen", "module_path": "app::screens" },
            { "name": "Other", "module_path": "elsewhere" },
        ]});
        drop_private_components(&mut cat, &exports);
        let names: Vec<_> = cat["components"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Shown", "Screen", "Other"]);
    }

    /// On idea-ui's real crate: components resolve to their root names,
    /// through the table module's own re-exports too, and the table's
    /// private helpers aren't public.
    #[test]
    fn idea_ui_components_resolve_to_their_root_names() {
        let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/idea-ui/src/lib.rs");
        let (ex, _) = crate_public_names("idea_ui", &lib);
        let public = |p: &str| ex.public.get(p).map(String::as_str);
        assert_eq!(public("idea_ui::components::alert::Alert"), Some("idea_ui::Alert"));
        assert_eq!(public("idea_ui::components::table::group::TableGroup"), Some("idea_ui::TableGroup"));
        assert_eq!(public("idea_ui::components::table::pager::TablePager"), Some("idea_ui::TablePager"));
        assert_eq!(public("idea_theme::extensible::tone"), Some("idea_ui::tone"));
        assert_eq!(public("idea_ui::components::table::group::GroupToggle"), None);
        assert_eq!(public("idea_ui::components::table::columns::ColumnResizeHandle"), None);
    }

    /// Regression: the framework's own catalog records workspace-relative
    /// files. Read from the build's manifest dir they didn't exist (no
    /// source), and a relative path's empty last ancestor matched the
    /// build's OWN `Cargo.toml`, so docs-app's `lib.rs` was read as every
    /// component crate's root and no import was ever shortened.
    #[test]
    fn regression_workspace_relative_files_resolve_from_the_manifest_dir() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let rel = "crates/ui/idea-ui/src/components/alert.rs";
        assert!(resolve_file(rel, manifest).is_some_and(|p| p.ends_with(rel)));
        let cat = serde_json::json!({ "components": [
            { "name": "Alert", "module_path": "idea_ui::components::alert", "file": rel },
        ]});
        assert!(component_sources(&cat, manifest)["idea_ui::components::alert::Alert"].contains("pub fn Alert"));
        let (exports, _) = crate_exports(&cat, manifest);
        assert_eq!(exports["idea_ui"].public["idea_ui::components::alert::Alert"], "idea_ui::Alert");
    }

    #[test]
    fn a_missing_fn_or_unparseable_file_has_no_source() {
        assert!(fn_source(FILE, "Nope").is_none());
        assert!(fn_source("fn (", "Badge").is_none());
    }
}
