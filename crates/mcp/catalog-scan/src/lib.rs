//! An app's MCP catalog entries, read from source.
//!
//! The compiled route to an app's catalog is a generated binary that
//! links every project crate with `runtime-core/catalog` on and prints
//! its `inventory` registrations — a full host build of the app, which
//! on a large workspace costs minutes of several cores and which
//! `idealyst mcp --watch` used to redo on every save. None of that build
//! is needed to know the entries: every one is a struct literal of string
//! and integer literals plus `module_path!()`, `file!()` and `line!()`.
//!
//! So [`scan`] reads them from source:
//!
//! 1. **Crawl** each crate's module tree from its root file, following
//!    `mod` declarations (`#[path]` included) the way rustc does, and
//!    skipping everything `#[cfg]`'d off for a host dev build ([`Cfg`]).
//! 2. **Expand** every item a catalog macro is attached to —
//!    `#[component]`, `#[lazy_component]`, `#[props]`,
//!    `#[derive(IdealystSchema)]`, `#[idealyst_tool]`, `recipe!`,
//!    `doc_scope!` — by calling the macro's own expansion in
//!    `runtime-macros-expand`, then process the output again, just as
//!    rustc would (a `#[props]` struct's output still carries its
//!    `#[derive(IdealystSchema)]`). `macro_rules!` that expand to catalog
//!    macros are run through [`macro_rules`]'s interpreter first.
//! 3. **Read** each emitted `inventory::submit! { … }` literal into a
//!    catalog entry, with `module_path!()` / `file!()` / `line!()` taken
//!    from the outermost invocation, as rustc reports them ([`entries`]).
//!
//! Running the real expansion is what keeps this honest: the rules that
//! shape an entry (inline-props wrapping, the injected `bind_to` prop,
//! `#[method]` lifting, the remote split) exist exactly once.
//!
//! A crate holding anything the scan cannot reproduce — an invocation no
//! rule of its `macro_rules!` matches, a catalog literal holding a
//! `const`, a catalog type registered by hand (`register_style_token!`
//! computes its default at run time) — is refused with a [`ScanError`],
//! and the caller compiles that crate instead. The scan never guesses,
//! and one refused crate costs no other crate its scan.
//!
//! What the scan does not cover, by construction: catalog macros used
//! inside function bodies (the walk reads module-level items), entries
//! gated on a custom `--cfg` (see [`Cfg`]), and catalog macros reached
//! through a proc macro other than the framework's.

mod cfg;
mod entries;
mod items;
pub mod macro_rules;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use mcp_catalog::CatalogParts;
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::Attribute;

pub use cfg::Cfg;
use entries::Site;
use items::{Kind, TokItem};
use macro_rules::MacroDef;

/// A crate whose catalog entries are read from source.
#[derive(Debug, Clone)]
pub struct ScanCrate {
    /// The crate's name as `module_path!()` begins with it (the lib
    /// target's name, `-` already `_`).
    pub name: String,
    /// Its root source file (`src/lib.rs`), absolute — `file!()` reports
    /// paths built from it.
    pub root: PathBuf,
    /// The configuration its `#[cfg]`s are evaluated against: the host
    /// set plus this crate's enabled features.
    pub cfg: Cfg,
}

/// A dependency whose `#[macro_export]` `macro_rules!` a scanned crate
/// may invoke. Only the definitions are read; nothing in it is scanned
/// for entries (its entries come from the compiled dependency catalog).
#[derive(Debug, Clone)]
pub struct MacroCrate {
    /// Crate name, for `$crate` and for `name::mac!` paths.
    pub name: String,
    /// Its source directory; every `.rs` file under it is read.
    pub src_dir: PathBuf,
}

/// Why a workspace cannot be scanned. The caller falls back to the
/// compiled extractor and says why.
#[derive(Debug)]
pub struct ScanError {
    pub file: PathBuf,
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file.display(), self.line, self.message)
    }
}

impl std::error::Error for ScanError {}

/// The attribute macros the scan expands, by the last segment of the
/// path they are written with (`#[component]`, `#[runtime_core::props]`).
const ATTR_MACROS: &[&str] = &["component", "lazy_component", "props", "idealyst_tool"];

/// Every catalog entry type. A macro body naming one registers catalog
/// entries by hand (`register_style_token!`, a recipe table), so it is
/// expanded — and what it registers is then either read or refused,
/// never silently dropped.
pub(crate) const CATALOG_TYPES: &[&str] = &[
    "ComponentEntry",
    "MethodEntry",
    "AnimationEntry",
    "PropsSchemaEntry",
    "TypeEntry",
    "ValueEntry",
    "ToolEntry",
    "RecipeEntry",
    "ScopeEntry",
    "ExternalEntry",
    "PrimitiveEntry",
    "UtilityEntry",
    "MacroEntry",
    "StateEntry",
    "StyleTokenEntry",
    "GuideEntry",
    "SdkEntry",
    "IconSetEntry",
];

/// How deep expansions may nest before the scan gives up (a macro that
/// re-invokes itself without consuming input).
const MAX_EXPANSION_DEPTH: usize = 64;

/// What [`scan`] read.
#[derive(Debug)]
pub struct Scanned {
    /// The entries of every crate that was read in full.
    pub parts: CatalogParts,
    /// The crates that could not be. None of a refused crate's entries
    /// are in [`parts`](Self::parts): the caller compiles those crates
    /// instead.
    pub refused: Vec<Refused>,
    /// Files that do not parse (an edit in progress) or could not be
    /// read. Their own entries are missing; the `mod` declarations in
    /// them are still followed, so their child modules are not.
    pub skipped: Vec<ScanError>,
}

/// A crate [`scan`] could not read in full.
#[derive(Debug)]
pub struct Refused {
    /// Its index in the `crates` passed to [`scan`].
    pub krate: usize,
    /// The first thing in it the scan could not reproduce.
    pub error: ScanError,
    /// What was read before that. Not a catalog — the crate's other
    /// entries are missing — but better than nothing where something
    /// approximate is wanted (editor completion while the compiled
    /// catalog builds).
    pub partial: CatalogParts,
}

/// Read the catalog entries of every crate in `crates`. See the crate
/// docs.
///
/// A source file that does not even tokenize (an unclosed delimiter
/// mid-edit) is skipped and reported in [`Scanned::skipped`]; an item
/// that does not parse costs only itself. A compiled extractor could not
/// build either. A crate whose source is fine but whose registrations the
/// scan cannot reproduce is [refused](Scanned::refused) as a whole — one
/// unscannable library does not cost the rest of the workspace its scan.
pub fn scan(crates: &[ScanCrate], macro_deps: &[MacroCrate]) -> Scanned {
    let mut modules: Vec<Module> = Vec::new();
    let mut skipped = Vec::new();
    for (idx, krate) in crates.iter().enumerate() {
        crawl_crate(idx, krate, &mut modules, &mut skipped);
    }
    let table = MacroTable::build(crates, &modules, macro_deps);
    let mut out = Scanned { parts: CatalogParts::default(), refused: Vec::new(), skipped };
    let mut by_crate: Vec<Vec<Module>> = (0..crates.len()).map(|_| Vec::new()).collect();
    for m in modules {
        by_crate[m.krate].push(m);
    }
    let mut scanner = Scanner { crates, table, parts: CatalogParts::default() };
    for (idx, mods) in by_crate.into_iter().enumerate() {
        let result = mods.into_iter().try_for_each(|module| {
            let ctx = Ctx { krate: idx, module_path: module.module_path, file: module.file, line: None, depth: 0 };
            scanner.items(module.items, &ctx)
        });
        let read = std::mem::take(&mut scanner.parts);
        match result {
            Ok(()) => out.parts.extend(read),
            Err(error) => out.refused.push(Refused { krate: idx, error, partial: read }),
        }
    }
    out
}

// ---------------------------------------------------------------------
// Crawl
// ---------------------------------------------------------------------

/// One source file's module.
struct Module {
    krate: usize,
    module_path: String,
    file: PathBuf,
    items: Vec<TokItem>,
}

fn crawl_crate(idx: usize, krate: &ScanCrate, out: &mut Vec<Module>, skipped: &mut Vec<ScanError>) {
    let dir = krate.root.parent().map(Path::to_path_buf).unwrap_or_default();
    crawl_file(idx, krate, &krate.root, krate.name.clone(), dir, out, skipped)
}

/// Load `file` as module `module_path` whose child modules live in `dir`.
fn crawl_file(
    idx: usize,
    krate: &ScanCrate,
    file: &Path,
    module_path: String,
    dir: PathBuf,
    out: &mut Vec<Module>,
    skipped: &mut Vec<ScanError>,
) {
    use std::str::FromStr;
    let err = |line: usize, message: String| ScanError { file: file.to_path_buf(), line, message };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => return skipped.push(err(0, format!("read: {e}"))),
    };
    let path_dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    match TokenStream::from_str(&text) {
        Ok(tokens) => {
            let items = items::split(tokens);
            crawl_mods(idx, krate, file, &items, &module_path, &dir, &path_dir, out, skipped);
            out.push(Module { krate: idx, module_path, file: file.to_path_buf(), items });
        }
        Err(e) => {
            skipped.push(err(e.span().start().line, format!("does not tokenize, skipped: {e}")));
            // Keep the subtree: the file's own `mod x;` lines still say
            // where its children are.
            let decls: Vec<TokItem> =
                mod_decls_by_line(&text).iter().flat_map(|m| items::split(m.to_token_stream())).collect();
            crawl_mods(idx, krate, file, &decls, &module_path, &dir, &path_dir, out, skipped);
        }
    }
}

/// The `mod name;` declarations of a file that does not tokenize: each
/// line (with the `#[…]` attribute lines directly above it) that parses
/// on its own as out-of-line `mod` items. Multi-line attributes are lost
/// here; it is the last resort for a file mid-edit.
fn mod_decls_by_line(text: &str) -> Vec<syn::Item> {
    let mut items = Vec::new();
    let mut attrs = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with("#[") {
            attrs.push_str(line);
            attrs.push('\n');
            continue;
        }
        if line.contains("mod ") && line.ends_with(';') {
            if let Ok(file) = syn::parse_str::<syn::File>(&format!("{attrs}{line}")) {
                items.extend(file.items.into_iter().filter(|i| matches!(i, syn::Item::Mod(m) if m.content.is_none())));
            }
        }
        if !line.starts_with("//") {
            attrs.clear();
        }
    }
    items
}

/// Find the out-of-line `mod` declarations among `items` (descending into
/// inline modules) and crawl their files.
///
/// `dir` is where an undecorated child `mod x;` lives (`dir/x.rs` or
/// `dir/x/mod.rs`); `path_dir` is what a `#[path]` on it is relative to.
/// They differ only at a non-`mod.rs` file's top level, where rustc
/// resolves `#[path]` against the file's own directory but plain children
/// against `<dir>/<file stem>/`.
#[allow(clippy::too_many_arguments)]
fn crawl_mods(
    idx: usize,
    krate: &ScanCrate,
    file: &Path,
    items: &[TokItem],
    module_path: &str,
    dir: &Path,
    path_dir: &Path,
    out: &mut Vec<Module>,
    skipped: &mut Vec<ScanError>,
) {
    for item in items {
        let Kind::Mod { name, body } = item.kind() else { continue };
        let mut attrs = item.attrs.clone();
        krate.cfg.expand_cfg_attr(&mut attrs);
        if !krate.cfg.enabled(&attrs) {
            continue;
        }
        let child_path = format!("{module_path}::{name}");
        let path_attr = attrs.iter().find(|a| a.path().is_ident("path")).and_then(|a| match &a.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value: syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }),
                ..
            }) => Some(s.value()),
            _ => None,
        });
        match body {
            Some(group) => {
                let child_dir = path_attr.as_ref().map(|p| path_dir.join(p)).unwrap_or_else(|| dir.join(&name));
                // Inside an inline module, `#[path]` resolves against the
                // module directory, inline components included.
                let inner = items::split(group.stream());
                crawl_mods(idx, krate, file, &inner, &child_path, &child_dir, &child_dir, out, skipped);
            }
            None => {
                let (child_file, child_dir) = match path_attr {
                    // A `#[path]` file owns its directory like a `mod.rs`.
                    Some(p) => {
                        let f = path_dir.join(p);
                        let d = f.parent().map(Path::to_path_buf).unwrap_or_default();
                        (f, d)
                    }
                    None => {
                        let flat = dir.join(format!("{name}.rs"));
                        if flat.is_file() {
                            (flat, dir.join(&name))
                        } else {
                            (dir.join(&name).join("mod.rs"), dir.join(&name))
                        }
                    }
                };
                crawl_file(idx, krate, &child_file, child_path, child_dir, out, skipped);
            }
        }
    }
}

// ---------------------------------------------------------------------
// Macro table
// ---------------------------------------------------------------------

/// Every `macro_rules!` a scanned crate can invoke, and which of them
/// matter: a macro is a *trigger* when its body mentions a catalog macro,
/// directly or through another trigger it invokes. Only triggers are
/// expanded.
struct MacroTable {
    defs: Vec<MacroDef>,
    trigger: Vec<bool>,
    /// Per scanned crate: its own macros by name (any visibility).
    local: Vec<HashMap<String, Vec<usize>>>,
    /// `#[macro_export]`ed macros by crate name, then macro name.
    exported: HashMap<String, HashMap<String, Vec<usize>>>,
}

impl MacroTable {
    fn build(crates: &[ScanCrate], modules: &[Module], macro_deps: &[MacroCrate]) -> Self {
        let mut t = MacroTable { defs: Vec::new(), trigger: Vec::new(), local: vec![HashMap::new(); crates.len()], exported: HashMap::new() };
        for module in modules {
            let krate = &crates[module.krate];
            let mut found = Vec::new();
            collect_macro_defs(&module.items, Some(&krate.cfg), &mut found);
            for (name, body, exported) in found {
                let id = t.push(MacroDef::parse_or_broken(&name, body.clone(), "crate"));
                t.local[module.krate].entry(name.clone()).or_default().push(id);
                if exported {
                    let def = MacroDef::parse_or_broken(&name, body, &krate.name);
                    let id = t.push(def);
                    t.exported.entry(krate.name.clone()).or_default().entry(name).or_default().push(id);
                }
            }
        }
        for dep in macro_deps {
            for file in rust_files(&dep.src_dir) {
                let Ok(text) = std::fs::read_to_string(&file) else { continue };
                if !text.contains("macro_export") {
                    continue;
                }
                let Ok(tokens) = <TokenStream as std::str::FromStr>::from_str(&text) else { continue };
                let mut found = Vec::new();
                collect_macro_defs(&items::split(tokens), None, &mut found);
                for (name, body, exported) in found {
                    if !exported {
                        continue;
                    }
                    let id = t.push(MacroDef::parse_or_broken(&name, body, &dep.name));
                    t.exported.entry(dep.name.clone()).or_default().entry(name).or_default().push(id);
                }
            }
        }
        t.compute_triggers();
        t
    }

    fn push(&mut self, def: MacroDef) -> usize {
        self.defs.push(def);
        self.trigger.push(false);
        self.defs.len() - 1
    }

    fn compute_triggers(&mut self) {
        let mut names: HashSet<String> = HashSet::new();
        loop {
            let mut changed = false;
            for (i, def) in self.defs.iter().enumerate() {
                if !self.trigger[i] && mentions_catalog(&def.body, &names) {
                    self.trigger[i] = true;
                    names.insert(def.name.clone());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// The trigger macro `path!` resolves to from crate `krate`, if any.
    /// `Err` when several different trigger definitions could be meant.
    fn resolve(&self, krate: usize, crates: &[ScanCrate], path: &syn::Path) -> Result<Option<&MacroDef>, String> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let Some(name) = segs.last() else { return Ok(None) };
        let mut ids: Vec<usize> = Vec::new();
        let qualifier = (segs.len() >= 2).then(|| segs[0].as_str()).filter(|q| !matches!(*q, "crate" | "self" | "super" | "$crate"));
        match qualifier.and_then(|q| self.exported.get(q)) {
            Some(m) => ids.extend(m.get(name).into_iter().flatten()),
            None if qualifier.is_none() => ids.extend(self.local[krate].get(name).into_iter().flatten()),
            // A crate name the table does not know: a renamed dependency
            // (`theme = { package = "idea-theme" }`). Resolve by name.
            None => {}
        }
        if ids.is_empty() {
            // Not defined here: an imported `#[macro_export]` macro of
            // another scanned crate or a dependency.
            for (krate_name, m) in &self.exported {
                if krate_name == &crates[krate].name {
                    continue;
                }
                ids.extend(m.get(name).into_iter().flatten());
            }
        }
        let triggers: Vec<&MacroDef> = ids.iter().filter(|&&i| self.trigger[i]).map(|&i| &self.defs[i]).collect();
        let Some(first) = triggers.first() else { return Ok(None) };
        let first_body = first.body.to_string();
        if triggers.iter().any(|d| d.body.to_string() != first_body) {
            return Err(format!("`{name}!` could be any of several macros that emit catalog entries"));
        }
        Ok(Some(first))
    }
}

/// `macro_rules!` definitions among `items` and inline modules: (name,
/// rules, `#[macro_export]`ed). With a `cfg`, definitions it switches off
/// are skipped.
fn collect_macro_defs(items: &[TokItem], cfg: Option<&Cfg>, out: &mut Vec<(String, TokenStream, bool)>) {
    for item in items {
        let mut attrs = item.attrs.clone();
        if let Some(cfg) = cfg {
            cfg.expand_cfg_attr(&mut attrs);
            if !cfg.enabled(&attrs) {
                continue;
            }
        }
        match item.kind() {
            Kind::MacroRules { name, body } => {
                let exported = attrs.iter().any(|a| a.path().is_ident("macro_export"));
                out.push((name, body.stream(), exported));
            }
            Kind::Mod { body: Some(g), .. } => collect_macro_defs(&items::split(g.stream()), cfg, out),
            _ => {}
        }
    }
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Whether `tokens` (a macro body) use a catalog macro: one of the
/// [`ATTR_MACROS`] as an attribute, `IdealystSchema`, `recipe!`,
/// `doc_scope!`, a catalog type or the catalog crate (a registration by
/// hand), or a macro in `triggers`.
fn mentions_catalog(tokens: &TokenStream, triggers: &HashSet<String>) -> bool {
    let tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
    for (i, tt) in tts.iter().enumerate() {
        match tt {
            TokenTree::Punct(p) if p.as_char() == '#' => {
                if let Some(TokenTree::Group(g)) = tts.get(i + 1) {
                    if attr_path_last(&g.stream()).is_some_and(|n| ATTR_MACROS.contains(&n.as_str())) {
                        return true;
                    }
                }
            }
            TokenTree::Ident(id) => {
                let s = id.to_string();
                if s == "IdealystSchema" || s == "__mcp" || s == "mcp_catalog" || CATALOG_TYPES.contains(&s.as_str()) {
                    return true;
                }
                let bang = matches!(tts.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!');
                if bang && (s == "recipe" || s == "doc_scope" || triggers.contains(&s)) {
                    return true;
                }
            }
            TokenTree::Group(g) => {
                if mentions_catalog(&g.stream(), triggers) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// The last identifier of the path an attribute's tokens start with
/// (`runtime_core::props` → `props`, `$crate::component(remote)` →
/// `component`).
fn attr_path_last(tokens: &TokenStream) -> Option<String> {
    let mut last = None;
    for tt in tokens.clone() {
        match tt {
            TokenTree::Ident(id) => last = Some(id.to_string()),
            TokenTree::Punct(p) if matches!(p.as_char(), ':' | '$') => {}
            _ => break,
        }
    }
    last
}

// ---------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------

struct Scanner<'a> {
    crates: &'a [ScanCrate],
    table: MacroTable,
    parts: CatalogParts,
}

#[derive(Clone)]
struct Ctx {
    krate: usize,
    module_path: String,
    file: PathBuf,
    /// The line of the outermost macro invocation being expanded, once
    /// inside one: `line!()` anywhere in its output reports it.
    line: Option<u32>,
    depth: usize,
}

impl Ctx {
    fn inside(&self, line: u32) -> Ctx {
        Ctx { line: Some(self.line.unwrap_or(line)), depth: self.depth + 1, ..self.clone() }
    }

    fn err(&self, line: usize, message: impl Into<String>) -> ScanError {
        ScanError { file: self.file.clone(), line: self.line.map_or(line, |l| l as usize), message: message.into() }
    }
}

/// What to do with one item, decided while its kind borrows it.
enum Action {
    Skip,
    Module(String, TokenStream),
    Call(syn::Path, TokenStream, u32),
    Decorated,
}

impl Scanner<'_> {
    fn items(&mut self, items: Vec<TokItem>, ctx: &Ctx) -> Result<(), ScanError> {
        if ctx.depth > MAX_EXPANSION_DEPTH {
            return Err(ctx.err(0, "macro expansion nests too deep"));
        }
        for item in items {
            self.item(item, ctx)?;
        }
        Ok(())
    }

    /// Process an expansion's output, item by item.
    fn expansion(&mut self, out: TokenStream, ctx: &Ctx) -> Result<(), ScanError> {
        self.items(items::split(out), ctx)
    }

    fn item(&mut self, mut item: TokItem, ctx: &Ctx) -> Result<(), ScanError> {
        let cfg = &self.crates[ctx.krate].cfg;
        cfg.expand_cfg_attr(&mut item.attrs);
        if !cfg.enabled(&item.attrs) {
            return Ok(());
        }
        let action = match item.kind() {
            // An out-of-line module is its own crawled `Module`.
            Kind::Mod { name, body: Some(g) } => Action::Module(name, g.stream()),
            Kind::MacroCall { path, args, line } => Action::Call(path, args.stream(), line),
            Kind::Decorable => Action::Decorated,
            Kind::Mod { body: None, .. } | Kind::MacroRules { .. } | Kind::Other => Action::Skip,
        };
        match action {
            Action::Skip => Ok(()),
            Action::Module(name, body) => {
                let ctx = Ctx { module_path: format!("{}::{name}", ctx.module_path), ..ctx.clone() };
                self.items(items::split(body), &ctx)
            }
            Action::Call(path, tokens, line) => self.item_macro(&path, tokens, line, ctx),
            Action::Decorated => self.decorated(item, ctx),
        }
    }

    /// An item that may carry a catalog attribute macro or derive.
    fn decorated(&mut self, mut item: TokItem, ctx: &Ctx) -> Result<(), ScanError> {
        // rustc expands the first attribute macro and hands it the item
        // with every other attribute; whatever it emits is expanded next.
        if let Some(pos) = item.attrs.iter().position(|a| last_ident(a.path()).is_some_and(|n| ATTR_MACROS.contains(&n.as_str()))) {
            let attr = item.attrs.remove(pos);
            let name = last_ident(attr.path()).expect("matched above");
            let args = match &attr.meta {
                syn::Meta::List(l) => l.tokens.clone(),
                _ => TokenStream::new(),
            };
            let line = attr.pound_token.span.start().line as u32;
            let input = item.tokens();
            let out = match name.as_str() {
                "component" => runtime_macros_expand::component(args, input),
                "lazy_component" => runtime_macros_expand::lazy_component(args, input),
                "props" => runtime_macros_expand::props(args, input),
                "idealyst_tool" => runtime_macros_expand::idealyst_tool(args, input),
                _ => unreachable!("ATTR_MACROS"),
            };
            return self.expansion(out, &ctx.inside(line));
        }
        if derives_schema(&item.attrs) {
            let out = runtime_macros_expand::derive_idealyst_schema(item.tokens());
            return self.expansion(out, &ctx.inside(item.line));
        }
        Ok(())
    }

    fn item_macro(&mut self, path: &syn::Path, tokens: TokenStream, line: u32, ctx: &Ctx) -> Result<(), ScanError> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let name = segs.last().cloned().unwrap_or_default();
        if name == "submit" && segs.iter().any(|s| s == "inventory") {
            let expr: syn::Expr = match syn::parse2(tokens) {
                Ok(e) => e,
                // Not an expression: not something a catalog macro emits.
                Err(_) => return Ok(()),
            };
            if !entries::is_catalog_submission(&expr) {
                return Ok(());
            }
            let site = Site {
                module_path: ctx.module_path.clone(),
                file: ctx.file.to_string_lossy().into_owned(),
                line: ctx.line.unwrap_or(line),
            };
            return entries::add_submission(&expr, &site, &mut self.parts).map_err(|e| ctx.err(line as usize, e));
        }
        let inner = ctx.inside(line);
        match name.as_str() {
            "recipe" => self.expansion(runtime_macros_expand::recipe(tokens), &inner),
            "doc_scope" => self.expansion(runtime_macros_expand::doc_scope(tokens), &inner),
            _ => {
                let def = self.table.resolve(ctx.krate, self.crates, path).map_err(|e| ctx.err(line as usize, e))?;
                let Some(def) = def else { return Ok(()) };
                let out = def.expand(tokens).map_err(|e| ctx.err(line as usize, e))?;
                self.expansion(out, &inner)
            }
        }
    }
}

fn last_ident(path: &syn::Path) -> Option<String> {
    path.segments.last().map(|s| s.ident.to_string())
}

/// Whether `#[derive(…)]` among `attrs` names `IdealystSchema`.
fn derives_schema(attrs: &[Attribute]) -> bool {
    attrs.iter().filter(|a| a.path().is_ident("derive")).any(|a| {
        a.parse_args_with(syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
            .is_ok_and(|paths| paths.iter().any(|p| last_ident(p).as_deref() == Some("IdealystSchema")))
    })
}
