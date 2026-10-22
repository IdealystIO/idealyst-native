//! ADDITIVE shape edits: a save whose only change outside function
//! bodies is new items.
//!
//! [`crate::shape_of`] draws the hot-patch tier's safety boundary: a
//! file whose shape moved rebuilds, because a patch is spliced into a
//! process where every other crate still holds the old layouts. Adding
//! an item is the one shape change that boundary is wider than it needs
//! to be for. Within the save, no other crate's source changed, so
//! nothing outside the edited crate can name the new item yet — the
//! dependents' compiled code, and the base metadata the patch replays
//! them against, are still exact. The new item is new code, which a
//! patch already carries (a body edit that instantiates a new generic
//! puts functions in the patch the base never had).
//!
//! What this module decides, from source, is whether a save is ONLY
//! that: every item the file had is still there, unchanged and in
//! order, and each new one is of a kind that cannot change the meaning
//! of code that already existed. [`outline`] records a file's items as
//! digests (what an archive keeps); [`additions`] compares a new text
//! against one and returns the new items, or why the save is not
//! additive.
//!
//! # What "cannot change existing code" rules out
//!
//! A new item can alter code that did not change in three ways, and
//! each is refused or left to a check the caller makes:
//!
//! - **Through a name.** A new `struct Bar` beside `use other::*` makes
//!   every `Bar` in the module mean the new type: an existing struct
//!   embedding `Bar` changes layout with no change to its own tokens. A
//!   new `fn drop` shadows the prelude's. So every name an addition
//!   introduces is returned ([`AddedName`]), and the caller requires it
//!   to be FRESH: no file the name could reach — the crate's, and for a
//!   `pub` name every dependent's — may already mention it. Code that
//!   never mentions a name cannot resolve differently because of it.
//! - **Through a trait.** A new `impl Trait for Existing` can change
//!   which impl an existing call picks (autoref specialization, a
//!   blanket impl's bound now holding) without any name changing. A
//!   trait impl is accepted only for a type the same save adds.
//! - **Through storage or opaque expansion.** A new `static` has no
//!   storage a later patch could keep (each patch would give it a fresh
//!   copy); a `const _`, an unknown item macro or an unknown attribute
//!   macro can expand to anything, a trait impl on an existing type
//!   included. All refused. `stylesheet!`, `#[component]` and
//!   `#[props]` are the framework's own and expand only to items named
//!   after their input, which the freshness check covers through
//!   [`AddedName::prefix`].
//!
//! What source cannot see — whether the compiled crate's EXISTING
//! symbols kept their names once the new items were expanded beside
//! them — is checked after the patch's replay, against the base module
//! (`build_web::hotpatch_additive`).

use std::collections::{BTreeMap, BTreeSet};

use quote::ToTokens;
use syn::visit_mut::VisitMut;

use crate::shape::BlankBodies;

/// One item of a file's outline, as digests.
///
/// The digest function is the caller's ([`outline`] takes it): the
/// archive that stores an outline and the decider that compares one
/// must agree on it, and they live in a crate that already owns one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    /// The inline-module path the item sits in: `""` at the file root,
    /// `"a::b"` inside `mod a { mod b { … } }`.
    pub scope: String,
    /// The digest of the item's shape ([`crate::shape_of`]'s blanking).
    /// For an inherent `impl` block, of its header alone — attributes,
    /// generics, self type, where clause — with `members` holding the
    /// rest. For an inline `mod`, of the `mod` line; its items are
    /// entries of their own under the module's scope.
    pub head: String,
    /// For an inherent `impl` block, one digest per member, in order.
    pub members: Vec<String>,
    /// Whether this is an inherent `impl` block, whose members may grow.
    pub open: bool,
    /// Whether this is one leaf of a `use` (`use a::{b, c}` is two). The
    /// order of imports means nothing, so a scope's imports are compared
    /// as a set: each one the file had must still be there, and the rest
    /// are additions.
    pub import: bool,
}

/// How far a new name can reach: the code that has to be checked for
/// an existing mention of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// Private: its module and that module's descendants — the file it
    /// is in, and the files of its submodules.
    Module,
    /// `pub(crate)`, `pub(super)`, `pub(in …)`: anywhere in the crate.
    Crate,
    /// `pub`: the crate and every crate that depends on it.
    Public,
}

fn reach_of(vis: &syn::Visibility) -> Reach {
    match vis {
        syn::Visibility::Public(_) => Reach::Public,
        syn::Visibility::Restricted(_) => Reach::Crate,
        syn::Visibility::Inherited => Reach::Module,
    }
}

/// A name a new item introduces, and how far it can reach.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AddedName {
    pub name: String,
    /// Which existing code could resolve to it, and so must not mention
    /// it already.
    pub reach: Reach,
    /// Whether a macro derives further names from it (`#[component]`
    /// emits `<Name>Props` and `__<Name>_hot_impl`, `stylesheet!` emits
    /// `<name>_style` and `<Name><Axis>`): then no existing identifier
    /// may START with it, or with its `snake_case` form.
    pub prefix: bool,
}

impl AddedName {
    /// Whether `ident`, an identifier some existing source mentions,
    /// could resolve to this name or to one derived from it.
    pub fn collides_with(&self, ident: &str) -> bool {
        if ident == self.name {
            return true;
        }
        if !self.prefix {
            return false;
        }
        let snake = snake_case(&self.name);
        ident.starts_with(&self.name)
            || ident.starts_with(&snake)
            || ident.strip_prefix("__").is_some_and(|rest| rest.starts_with(&self.name))
            || ident.ends_with(&format!("_{}", self.name))
    }
}

/// One new item of a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addition {
    /// What it is, for a log line: `fn helper`, `struct Row`,
    /// `impl Grid { fn total }`.
    pub label: String,
    /// The names it introduces, each of which must be fresh.
    pub names: Vec<AddedName>,
}

/// The file's items, as digests. `None` when it does not parse.
pub fn outline(text: &str, digest: &dyn Fn(&str) -> String) -> Option<Vec<OutlineEntry>> {
    let file = syn::parse_file(text).ok()?;
    let mut out = Vec::new();
    for e in entries(&file.items, "") {
        out.push(e.outline(digest));
    }
    Some(out)
}

/// Every identifier the file mentions, macro bodies and attributes
/// included — what the freshness check compares a new name against.
/// `None` when the file does not lex.
pub fn idents(text: &str) -> Option<BTreeSet<String>> {
    let tokens: proc_macro2::TokenStream = text.parse().ok()?;
    let mut out = BTreeSet::new();
    collect_idents(tokens, &mut out);
    Some(out)
}

fn collect_idents(tokens: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
    for tt in tokens {
        match tt {
            proc_macro2::TokenTree::Ident(i) => {
                let s = i.to_string();
                out.insert(s.strip_prefix("r#").map(str::to_string).unwrap_or(s));
            }
            proc_macro2::TokenTree::Group(g) => collect_idents(g.stream(), out),
            _ => {}
        }
    }
}

/// The new items of `new_text` against `old`, its outline before the
/// save — or why the save is not purely additive. See the module docs.
///
/// `Ok(vec![])` means the outlines agree: the shape did not move at all.
pub fn additions(
    old: &[OutlineEntry],
    new_text: &str,
    digest: &dyn Fn(&str) -> String,
) -> Result<Vec<Addition>, String> {
    let file = syn::parse_file(new_text).map_err(|_| "the file does not parse".to_string())?;
    let new = entries(&file.items, "");

    let mut old_by_scope: BTreeMap<&str, Vec<&OutlineEntry>> = BTreeMap::new();
    for e in old {
        old_by_scope.entry(e.scope.as_str()).or_default().push(e);
    }
    let mut new_by_scope: BTreeMap<String, Vec<(&Entry<'_>, OutlineEntry)>> = BTreeMap::new();
    for e in &new {
        new_by_scope.entry(e.scope.clone()).or_default().push((e, e.outline(digest)));
    }
    for scope in old_by_scope.keys() {
        if !new_by_scope.contains_key(*scope) {
            return Err(format!("module `{scope}` was removed or changed"));
        }
    }

    // Types the save adds, anywhere in the file: a trait impl for one of
    // these cannot change what existing code resolves to, since no
    // existing code has a value of it.
    let mut matched_new: Vec<(&Entry<'_>, Vec<&syn::ImplItem>)> = Vec::new();
    let mut added: Vec<&Entry<'_>> = Vec::new();
    for (scope, all_news) in &new_by_scope {
        let all_olds = old_by_scope.get(scope.as_str()).cloned().unwrap_or_default();
        // Imports, as a set: each the file had must still be there.
        let mut new_imports: Vec<&(&Entry<'_>, OutlineEntry)> = all_news.iter().filter(|(_, o)| o.import).collect();
        for old in all_olds.iter().filter(|o| o.import) {
            let Some(at) = new_imports.iter().position(|(_, now)| now.head == old.head) else {
                return Err(format!("an import in `{}` was removed or changed", if scope.is_empty() { "the file" } else { scope }));
            };
            new_imports.remove(at);
        }
        added.extend(new_imports.into_iter().map(|(e, _)| *e));

        let olds: Vec<&OutlineEntry> = all_olds.into_iter().filter(|o| !o.import).collect();
        let news: Vec<&(&Entry<'_>, OutlineEntry)> = all_news.iter().filter(|(_, o)| !o.import).collect();
        let mut at = 0;
        for (entry, now) in news.iter().map(|p| (&p.0, &p.1)) {
            let Some(before) = olds.get(at) else {
                added.push(*entry);
                continue;
            };
            if before.head == now.head && before.members == now.members && before.open == now.open {
                at += 1;
                continue;
            }
            if before.open && now.open && before.head == now.head {
                let grown = grown_members(&before.members, &now.members).ok_or_else(|| {
                    format!("an existing member of `{}` changed or was removed", entry.label())
                })?;
                let syn::Item::Impl(imp) = entry.item else { unreachable!("open entries are impls") };
                matched_new.push((*entry, grown.into_iter().map(|i| &imp.items[i]).collect()));
                at += 1;
                continue;
            }
            added.push(*entry);
        }
        if at != olds.len() {
            let what = news.get(at).map(|p| p.0.label()).unwrap_or_else(|| "the end of the file".into());
            return Err(format!("an existing item changed, moved or was removed (near {what})"));
        }
    }

    let new_types: BTreeSet<String> = added
        .iter()
        .filter_map(|e| match e.item {
            syn::Item::Struct(s) => Some(s.ident.to_string()),
            syn::Item::Enum(s) => Some(s.ident.to_string()),
            syn::Item::Union(s) => Some(s.ident.to_string()),
            syn::Item::Type(s) => Some(s.ident.to_string()),
            _ => None,
        })
        .collect();
    let old_scopes: BTreeSet<&str> = old_by_scope.keys().copied().collect();

    let mut out = Vec::new();
    for entry in added {
        // Inside a module the save adds, nothing that existed can see a
        // name: only the module's own name has to be fresh, and it is
        // the module entry's.
        let in_new_module = !old_scopes.contains(entry.scope.as_str());
        let mut names = match &entry.import {
            Some(leaf) => validate_import(entry.item, leaf, in_new_module)?,
            None => validate(entry.item, &new_types)?,
        };
        if in_new_module {
            names.clear();
        }
        out.push(Addition { label: entry.label(), names });
    }
    for (entry, members) in matched_new {
        let syn::Item::Impl(imp) = entry.item else { unreachable!() };
        let mut names = Vec::new();
        let mut labels = Vec::new();
        for m in members {
            let (name, reach) = inherent_member(m)?;
            labels.push(name.clone());
            names.push(AddedName { name, reach, prefix: false });
        }
        out.push(Addition {
            label: format!("impl {} {{ {} }}", self_type_name(&imp.self_ty).unwrap_or_default(), labels.join(", ")),
            names,
        });
    }
    Ok(out)
}

/// The indices of `now`'s members that `before` does not account for,
/// when `before` is a subsequence of `now`; `None` otherwise.
fn grown_members(before: &[String], now: &[String]) -> Option<Vec<usize>> {
    let mut at = 0;
    let mut grown = Vec::new();
    for (i, m) in now.iter().enumerate() {
        if before.get(at) == Some(m) {
            at += 1;
        } else {
            grown.push(i);
        }
    }
    (at == before.len()).then_some(grown)
}

/// One item, where it sits — or, for a `use`, one of its leaves.
struct Entry<'a> {
    scope: String,
    item: &'a syn::Item,
    import: Option<UseLeaf>,
}

/// One imported name: `a::b::c as d` of a `use` tree.
#[derive(Clone)]
struct UseLeaf {
    /// The leaf as one import, `a :: b :: c as d`, for its digest.
    text: String,
    /// The name it binds: `None` for a glob, or for `as _`.
    name: Option<String>,
    glob: bool,
}

fn use_leaves(tree: &syn::UseTree, path: &mut Vec<String>, out: &mut Vec<UseLeaf>) {
    match tree {
        syn::UseTree::Path(p) => {
            path.push(p.ident.to_string());
            use_leaves(&p.tree, path, out);
            path.pop();
        }
        syn::UseTree::Name(n) => {
            let full = path.iter().cloned().chain(std::iter::once(n.ident.to_string())).collect::<Vec<_>>().join("::");
            let name = if n.ident == "self" { path.last().cloned() } else { Some(n.ident.to_string()) };
            out.push(UseLeaf { text: full, name, glob: false });
        }
        syn::UseTree::Rename(r) => {
            let full = path.iter().cloned().chain(std::iter::once(r.ident.to_string())).collect::<Vec<_>>().join("::");
            let name = (r.rename != "_").then(|| r.rename.to_string());
            out.push(UseLeaf { text: format!("{full} as {}", r.rename), name, glob: false });
        }
        syn::UseTree::Glob(_) => {
            out.push(UseLeaf { text: format!("{}::*", path.join("::")), name: None, glob: true });
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                use_leaves(t, path, out);
            }
        }
    }
}

impl Entry<'_> {
    fn outline(&self, digest: &dyn Fn(&str) -> String) -> OutlineEntry {
        if let (Some(leaf), syn::Item::Use(u)) = (&self.import, self.item) {
            // Attributes and visibility are part of each leaf: `pub use`
            // and `use` of one path are different imports.
            let (attrs, vis) = (&u.attrs, &u.vis);
            let text = format!("{} {}", quote::quote!(#(#attrs)* #vis use), leaf.text);
            return OutlineEntry {
                scope: self.scope.clone(),
                head: digest(&text),
                members: Vec::new(),
                open: false,
                import: true,
            };
        }
        match self.item {
            syn::Item::Impl(imp) if imp.trait_.is_none() => {
                let mut head = imp.clone();
                head.items.clear();
                let members = imp
                    .items
                    .iter()
                    .map(|m| {
                        let mut m = m.clone();
                        BlankBodies.visit_impl_item_mut(&mut m);
                        digest(&m.to_token_stream().to_string())
                    })
                    .collect();
                OutlineEntry {
                    scope: self.scope.clone(),
                    head: digest(&head.to_token_stream().to_string()),
                    members,
                    open: true,
                    import: false,
                }
            }
            syn::Item::Mod(m) if m.content.is_some() => {
                let mut head = m.clone();
                if let Some((_, items)) = &mut head.content {
                    items.clear();
                }
                OutlineEntry {
                    scope: self.scope.clone(),
                    head: digest(&head.to_token_stream().to_string()),
                    members: Vec::new(),
                    open: false,
                    import: false,
                }
            }
            item => {
                let mut item = item.clone();
                BlankBodies.visit_item_mut(&mut item);
                OutlineEntry {
                    scope: self.scope.clone(),
                    head: digest(&item.to_token_stream().to_string()),
                    members: Vec::new(),
                    open: false,
                    import: false,
                }
            }
        }
    }

    fn label(&self) -> String {
        let at = if self.scope.is_empty() { String::new() } else { format!("{}::", self.scope) };
        match &self.import {
            Some(leaf) => format!("{at}use {}", leaf.text),
            None => format!("{at}{}", item_label(self.item)),
        }
    }
}

/// Every item, inline modules flattened into scoped entries after the
/// module's own.
fn entries<'a>(items: &'a [syn::Item], scope: &str) -> Vec<Entry<'a>> {
    let mut out = Vec::new();
    for item in items {
        if let syn::Item::Use(u) = item {
            let mut leaves = Vec::new();
            use_leaves(&u.tree, &mut Vec::new(), &mut leaves);
            for leaf in leaves {
                out.push(Entry { scope: scope.to_string(), item, import: Some(leaf) });
            }
            continue;
        }
        out.push(Entry { scope: scope.to_string(), item, import: None });
        if let syn::Item::Mod(m) = item {
            if let Some((_, inner)) = &m.content {
                let inner_scope =
                    if scope.is_empty() { m.ident.to_string() } else { format!("{scope}::{}", m.ident) };
                out.extend(entries(inner, &inner_scope));
            }
        }
    }
    out
}

fn item_label(item: &syn::Item) -> String {
    match item {
        syn::Item::Fn(f) => format!("fn {}", f.sig.ident),
        syn::Item::Struct(s) => format!("struct {}", s.ident),
        syn::Item::Enum(s) => format!("enum {}", s.ident),
        syn::Item::Union(s) => format!("union {}", s.ident),
        syn::Item::Type(s) => format!("type {}", s.ident),
        syn::Item::Const(s) => format!("const {}", s.ident),
        syn::Item::Static(s) => format!("static {}", s.ident),
        syn::Item::Trait(s) => format!("trait {}", s.ident),
        syn::Item::Mod(s) => format!("mod {}", s.ident),
        syn::Item::Use(_) => "use".to_string(),
        syn::Item::Impl(imp) => {
            let ty = self_type_name(&imp.self_ty).unwrap_or_else(|| "?".into());
            match &imp.trait_ {
                Some((_, path, _)) => format!(
                    "impl {} for {ty}",
                    path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
                ),
                None => format!("impl {ty}"),
            }
        }
        syn::Item::Macro(m) => match &m.ident {
            Some(name) => format!("macro_rules! {name}"),
            None => format!("{}!", macro_name(&m.mac)),
        },
        _ => "item".to_string(),
    }
}

fn macro_name(mac: &syn::Macro) -> String {
    mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
}

/// The last segment of a path type: `Grid` for `crate::Grid<T>`.
fn self_type_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) if p.qself.is_none() => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}


/// Attributes an added item may carry. Anything else may be an
/// attribute macro, which can expand to anything.
const KNOWN_ATTRS: &[&str] = &[
    "doc",
    "allow",
    "warn",
    "deny",
    "forbid",
    "expect",
    "cfg",
    "derive",
    "inline",
    "cold",
    "must_use",
    "track_caller",
    "deprecated",
    "non_exhaustive",
    "repr",
    "test",
    "automatically_derived",
    // Helper attributes of derives this accepts.
    "serde",
    // The framework's own item macros: they expand to items named after
    // their input (see `AddedName::prefix`).
    "component",
    "props",
    "method",
];

/// Whether the attributes allow the item, and whether one of them is a
/// framework macro that derives names from the item's.
fn check_attrs(attrs: &[syn::Attribute]) -> Result<bool, String> {
    let mut framework = false;
    for a in attrs {
        let path = a.path();
        let first = path.segments.first().map(|s| s.ident.to_string()).unwrap_or_default();
        if path.segments.len() > 1 && matches!(first.as_str(), "rustfmt" | "clippy") {
            continue;
        }
        let last = path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        if !KNOWN_ATTRS.contains(&last.as_str()) {
            return Err(format!("it carries `#[{}]`, an attribute this check does not know", path.to_token_stream()));
        }
        if matches!(last.as_str(), "component" | "props") {
            // `#[component(lazy)]` moves the component into a split chunk
            // the base has no place for.
            if a.to_token_stream().to_string().contains("lazy") {
                return Err("it is a `#[component(lazy)]`".into());
            }
            framework = true;
        }
    }
    Ok(framework)
}

/// The names an added item introduces, or why it is not additive.
fn validate(item: &syn::Item, new_types: &BTreeSet<String>) -> Result<Vec<AddedName>, String> {
    let label = item_label(item);
    let refuse = |why: &str| Err(format!("`{label}` {why}"));
    let named = |ident: &syn::Ident, vis: &syn::Visibility, prefix: bool| AddedName {
        name: ident.to_string(),
        reach: reach_of(vis),
        prefix,
    };
    let framework = |attrs: &[syn::Attribute]| check_attrs(attrs).map_err(|why| format!("`{label}`: {why}"));
    match item {
        syn::Item::Fn(f) => {
            let prefix = framework(&f.attrs)?;
            Ok(vec![named(&f.sig.ident, &f.vis, prefix)])
        }
        syn::Item::Struct(s) => {
            let prefix = framework(&s.attrs)?;
            Ok(vec![named(&s.ident, &s.vis, prefix)])
        }
        syn::Item::Enum(s) => {
            framework(&s.attrs)?;
            Ok(vec![named(&s.ident, &s.vis, false)])
        }
        syn::Item::Union(s) => {
            framework(&s.attrs)?;
            Ok(vec![named(&s.ident, &s.vis, false)])
        }
        syn::Item::Type(s) => {
            framework(&s.attrs)?;
            Ok(vec![named(&s.ident, &s.vis, false)])
        }
        syn::Item::Const(c) => {
            if c.ident == "_" {
                return refuse("can expand to anything, a trait impl for an existing type included");
            }
            framework(&c.attrs)?;
            Ok(vec![named(&c.ident, &c.vis, false)])
        }
        syn::Item::Static(_) => refuse("is new storage: every later patch would give it a fresh copy"),
        syn::Item::Trait(t) => {
            framework(&t.attrs)?;
            if t.unsafety.is_some() || t.auto_token.is_some() {
                return refuse("is an unsafe or auto trait");
            }
            // A trait brought into scope by a glob gives every type that
            // implements it these methods: they must be fresh too.
            let mut names = vec![named(&t.ident, &t.vis, false)];
            for m in &t.items {
                let ident = match m {
                    syn::TraitItem::Fn(f) => &f.sig.ident,
                    syn::TraitItem::Const(c) => &c.ident,
                    syn::TraitItem::Type(t) => &t.ident,
                    _ => return refuse("has a member this check does not know"),
                };
                names.push(AddedName { name: ident.to_string(), reach: reach_of(&t.vis), prefix: false });
            }
            Ok(names)
        }
        syn::Item::Impl(imp) => {
            framework(&imp.attrs)?;
            if imp.unsafety.is_some() || imp.trait_.as_ref().is_some_and(|(bang, _, _)| bang.is_some()) {
                return refuse("is an unsafe or negative impl");
            }
            let ty = self_type_name(&imp.self_ty);
            let of_new_type = ty.as_ref().is_some_and(|t| new_types.contains(t));
            if imp.trait_.is_some() {
                if !of_new_type {
                    return refuse(
                        "implements a trait for a type the save did not add, which can change which \
                         impl existing code calls",
                    );
                }
                return Ok(Vec::new());
            }
            // An inherent impl. On a new type, nothing existing has a
            // value to call its methods on; on an existing one, each
            // method name must be fresh, or it could shadow a trait
            // method an existing call resolves to.
            let mut names = Vec::new();
            for m in &imp.items {
                let (name, reach) = inherent_member(m).map_err(|why| format!("`{label}`: {why}"))?;
                if !of_new_type {
                    names.push(AddedName { name, reach, prefix: false });
                }
            }
            Ok(names)
        }
        syn::Item::Mod(m) => {
            framework(&m.attrs)?;
            if m.content.is_none() {
                return refuse("is a new file, which this build never scanned");
            }
            Ok(vec![named(&m.ident, &m.vis, false)])
        }
        // Every `use` reaches the decision as its leaves (`validate_import`).
        syn::Item::Use(_) => refuse("is an import outside the import check"),
        syn::Item::Macro(m) => {
            framework(&m.attrs)?;
            if let Some(name) = &m.ident {
                if m.attrs.iter().any(|a| a.path().is_ident("macro_export")) {
                    return refuse("is exported to other crates");
                }
                // A macro is in textual scope after its definition, in this
                // file and the modules declared after it.
                return Ok(vec![AddedName { name: name.to_string(), reach: Reach::Module, prefix: false }]);
            }
            if crate::shape::is_stylesheet(&m.mac) {
                let name = m
                    .mac
                    .tokens
                    .clone()
                    .into_iter()
                    .find_map(|tt| match tt {
                        proc_macro2::TokenTree::Ident(i) if i != "pub" && i != "crate" => Some(i.to_string()),
                        _ => None,
                    })
                    .ok_or_else(|| format!("`{label}` has no name"))?;
                return Ok(vec![AddedName { name, reach: Reach::Public, prefix: true }]);
            }
            refuse("can expand to anything")
        }
        _ => refuse("is a kind of item this check does not accept"),
    }
}

/// The names a new import binds, or why it is not additive.
/// `in_new_module`: the import sits in a module the save adds, where no
/// existing code can see what it binds, so even a glob is harmless.
fn validate_import(item: &syn::Item, leaf: &UseLeaf, in_new_module: bool) -> Result<Vec<AddedName>, String> {
    let syn::Item::Use(u) = item else { unreachable!("an import entry is a `use`") };
    check_attrs(&u.attrs).map_err(|why| format!("`use {}`: {why}", leaf.text))?;
    if in_new_module {
        return Ok(Vec::new());
    }
    if leaf.glob {
        return Err(format!("`use {}` is a glob, which binds names this check cannot list", leaf.text));
    }
    Ok(leaf.name.iter().map(|name| AddedName { name: name.clone(), reach: reach_of(&u.vis), prefix: false }).collect())
}

/// An inherent impl member's name and reach. Methods and consts only:
/// an associated type or a macro there is not something a type's
/// existing users could be unaffected by.
fn inherent_member(m: &syn::ImplItem) -> Result<(String, Reach), String> {
    match m {
        syn::ImplItem::Fn(f) => {
            check_attrs(&f.attrs)?;
            Ok((f.sig.ident.to_string(), reach_of(&f.vis)))
        }
        syn::ImplItem::Const(c) => {
            check_attrs(&c.attrs)?;
            Ok((c.ident.to_string(), reach_of(&c.vis)))
        }
        _ => Err("a new impl member that is not a method or a const".into()),
    }
}

fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i != 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> String {
        // Any injective-enough digest will do for the tests: the text.
        s.to_string()
    }

    fn added(before: &str, after: &str) -> Result<Vec<Addition>, String> {
        additions(&outline(before, &d).expect("parses"), after, &d)
    }

    fn names(before: &str, after: &str) -> Vec<String> {
        added(before, after)
            .unwrap_or_else(|e| panic!("not additive: {e}"))
            .into_iter()
            .flat_map(|a| a.names.into_iter().map(|n| n.name))
            .collect()
    }

    fn refused(before: &str, after: &str) -> String {
        match added(before, after) {
            Err(why) => why,
            Ok(a) => panic!("additive, but should not be: {a:?}"),
        }
    }

    const BASE: &str = r#"
        use other::*;

        #[component]
        pub fn Badge(label: String) -> Element {
            ui! { text { "{label}" } }
        }

        pub struct Row { pub a: u32 }

        impl Row {
            pub fn total(&self) -> u32 { self.a }
        }

        impl Clone for Row {
            fn clone(&self) -> Self { Row { a: self.a } }
        }

        mod inner {
            pub fn helper() -> u32 { 1 }
        }
    "#;

    #[test]
    fn an_unchanged_shape_adds_nothing() {
        assert_eq!(added(BASE, &BASE.replace("self.a }", "self.a + 1 }")).unwrap(), vec![]);
    }

    #[test]
    fn a_fn_at_the_end_is_an_addition() {
        let after = format!("{BASE}\npub fn fresh_helper(n: u32) -> u32 {{ n * 2 }}\n");
        let a = added(BASE, &after).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].label, "fn fresh_helper");
        assert_eq!(a[0].names, vec![AddedName { name: "fresh_helper".into(), reach: Reach::Public, prefix: false }]);
    }

    /// Placement does not matter at the source level: whether the
    /// compiled symbols of the items after it kept their names is the
    /// post-replay check's question.
    #[test]
    fn an_item_between_existing_ones_is_an_addition() {
        let after = BASE.replace("pub struct Row", "fn between() {}\n        pub struct Row");
        assert_eq!(names(BASE, &after), vec!["between"]);
    }

    #[test]
    fn a_new_component_derives_names_from_its_own() {
        let after = format!("{BASE}\n#[component]\nfn Gauge(v: f32) -> Element {{ ui! {{ view {{}} }} }}\n");
        let a = added(BASE, &after).unwrap();
        assert_eq!(a[0].names, vec![AddedName { name: "Gauge".into(), reach: Reach::Module, prefix: true }]);
        let n = &a[0].names[0];
        assert!(n.collides_with("GaugeProps"));
        assert!(n.collides_with("__Gauge_hot_impl"));
        assert!(!n.collides_with("Gau"));
    }

    #[test]
    fn a_stylesheet_derives_snake_case_names() {
        let after = format!("{BASE}\nstylesheet! {{ CardSheet<()> {{ base(_t) {{ padding: 4.0 }} }} }}\n");
        let a = added(BASE, &after).unwrap();
        let n = &a[0].names[0];
        assert_eq!(n.name, "CardSheet");
        assert!(n.prefix);
        assert!(n.collides_with("card_sheet_style"));
        assert!(n.collides_with("CardSheetSize"));
    }

    #[test]
    fn a_new_type_with_its_impls_is_an_addition() {
        let after = format!(
            "{BASE}\n#[derive(Debug, Clone)]\npub struct Fresh {{ v: u8 }}\n\
             impl Fresh {{ pub fn new() -> Self {{ Fresh {{ v: 0 }} }} }}\n\
             impl Default for Fresh {{ fn default() -> Self {{ Fresh::new() }} }}\n"
        );
        // The impls' members need not be fresh: nothing existing has a
        // `Fresh` to call `new` on.
        assert_eq!(names(BASE, &after), vec!["Fresh"]);
    }

    #[test]
    fn a_method_on_an_existing_type_must_be_fresh() {
        let after = BASE.replace(
            "pub fn total(&self) -> u32 { self.a }",
            "pub fn total(&self) -> u32 { self.a }\n            fn doubled(&self) -> u32 { self.a * 2 }",
        );
        let a = added(BASE, &after).unwrap();
        assert_eq!(a[0].names, vec![AddedName { name: "doubled".into(), reach: Reach::Module, prefix: false }]);
    }

    #[test]
    fn a_new_inherent_impl_on_an_existing_type_names_its_methods() {
        let after = format!("{BASE}\nimpl Row {{ pub fn halved(&self) -> u32 {{ self.a / 2 }} }}\n");
        assert_eq!(names(BASE, &after), vec!["halved"]);
    }

    #[test]
    fn items_in_a_new_module_need_no_fresh_names() {
        let after = format!("{BASE}\nmod extra {{ use super::*; pub fn total() -> u32 {{ 0 }} }}\n");
        assert_eq!(names(BASE, &after), vec!["extra"]);
    }

    #[test]
    fn an_addition_inside_an_existing_module_is_scoped() {
        let after = BASE.replace(
            "pub fn helper() -> u32 { 1 }",
            "pub fn helper() -> u32 { 1 }\n            pub fn helper_two() -> u32 { 2 }",
        );
        let a = added(BASE, &after).unwrap();
        assert_eq!(a[0].label, "inner::fn helper_two");
    }

    /// The everyday companion of an additive edit: one more name in an
    /// existing import list. Imports compare as a set.
    #[test]
    fn a_name_added_to_an_import_list_is_an_addition() {
        let before = "use runtime_core::{ui, Element};\nfn a() {}\n";
        let after = "use runtime_core::{component, ui, Element};\nfn a() {}\n";
        assert_eq!(names(before, after), vec!["component"]);
        // Reordered or regrouped imports are the same imports.
        assert_eq!(names(before, "use runtime_core::Element;\nuse runtime_core::ui;\nfn a() {}\n"), Vec::<String>::new());
        // A removed one is not additive.
        assert!(refused(before, "use runtime_core::ui;\nfn a() {}\n").contains("import"));
        // `pub use` of the same path is a different import.
        let a = added("use x::Y;\n", "use x::Y;\npub use x::Z;\n").unwrap();
        assert_eq!(a[0].names, vec![AddedName { name: "Z".into(), reach: Reach::Public, prefix: false }]);
    }

    #[test]
    fn visibility_decides_reach() {
        let a = added("fn a() {}\n", "fn a() {}\npub(crate) fn b() {}\nfn c() {}\n").unwrap();
        assert_eq!(a[0].names[0].reach, Reach::Crate);
        assert_eq!(a[1].names[0].reach, Reach::Module);
    }

    #[test]
    fn a_named_use_introduces_its_names_and_a_glob_is_refused() {
        let after = format!("{BASE}\nuse std::collections::{{BTreeMap, HashMap as Map}};\n");
        assert_eq!(names(BASE, &after), vec!["BTreeMap", "Map"]);
        assert!(refused(BASE, &format!("{BASE}\nuse std::collections::*;\n")).contains("glob"));
    }

    // ── Not additive ────────────────────────────────────────────────

    #[test]
    fn a_changed_signature_is_not_additive() {
        assert!(refused(BASE, &BASE.replace("label: String", "label: String, n: u8")).contains("existing item"));
    }

    #[test]
    fn a_removed_item_is_not_additive() {
        let after = BASE.replace("pub struct Row { pub a: u32 }", "");
        refused(BASE, &after);
    }

    #[test]
    fn reordering_is_not_additive() {
        let a = "fn one() {}\nfn two() {}\n";
        let b = "fn two() {}\nfn one() {}\n";
        refused(a, b);
    }

    #[test]
    fn a_new_field_is_not_additive() {
        refused(BASE, &BASE.replace("pub a: u32", "pub a: u32, pub b: u32"));
    }

    #[test]
    fn a_trait_impl_for_an_existing_type_is_refused() {
        let after = format!("{BASE}\nimpl Default for Row {{ fn default() -> Self {{ Row {{ a: 0 }} }} }}\n");
        assert!(refused(BASE, &after).contains("did not add"));
    }

    #[test]
    fn a_member_added_to_a_trait_impl_is_refused() {
        let after = BASE.replace(
            "fn clone(&self) -> Self { Row { a: self.a } }",
            "fn clone(&self) -> Self { Row { a: self.a } }\n            fn clone_from(&mut self, o: &Self) { self.a = o.a; }",
        );
        refused(BASE, &after);
    }

    #[test]
    fn a_static_is_refused() {
        assert!(refused(BASE, &format!("{BASE}\nstatic COUNT: u32 = 0;\n")).contains("storage"));
    }

    #[test]
    fn a_const_underscore_is_refused() {
        refused(BASE, &format!("{BASE}\nconst _: () = ();\n"));
    }

    #[test]
    fn an_unknown_attribute_macro_is_refused() {
        assert!(refused(BASE, &format!("{BASE}\n#[tracing::instrument]\nfn traced() {{}}\n")).contains("attribute"));
    }

    #[test]
    fn a_server_fn_is_refused() {
        refused(BASE, &format!("{BASE}\n#[server]\npub async fn save() -> Result<(), ServerError> {{ Ok(()) }}\n"));
    }

    #[test]
    fn a_lazy_component_is_refused() {
        refused(BASE, &format!("{BASE}\n#[component(lazy)]\nfn Heavy() -> Element {{ ui! {{ view {{}} }} }}\n"));
    }

    #[test]
    fn an_unknown_item_macro_is_refused() {
        refused(BASE, &format!("{BASE}\nlazy_static! {{ static ref X: u8 = 0; }}\n"));
    }

    #[test]
    fn an_exported_macro_is_refused_and_a_local_one_is_a_name() {
        refused(BASE, &format!("{BASE}\n#[macro_export]\nmacro_rules! m {{ () => {{}} }}\n"));
        assert_eq!(names(BASE, &format!("{BASE}\nmacro_rules! local_m {{ () => {{}} }}\n")), vec!["local_m"]);
    }

    #[test]
    fn a_new_file_module_is_refused() {
        refused(BASE, &format!("{BASE}\nmod sibling;\n"));
    }

    #[test]
    fn a_trait_names_its_methods() {
        let after = format!("{BASE}\npub trait Scored {{ fn score(&self) -> u32; }}\n");
        assert_eq!(names(BASE, &after), vec!["Scored", "score"]);
    }

    #[test]
    fn idents_include_macro_bodies() {
        let ids = idents("fn a() { ui! { view { Badge(label = x) } } }").unwrap();
        assert!(ids.contains("Badge") && ids.contains("label") && ids.contains("x"));
        assert!(idents("fn r#type() {}").unwrap().contains("type"));
    }
}
