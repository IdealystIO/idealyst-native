//! The parts of a file only the SERVER build compiles.
//!
//! `#[server]` (and its siblings `#[channel]`, `#[subscription]`,
//! `#[sse]`) splits a function in two: the author's body under
//! `cfg(feature = "server")`, and a stub generated from the SIGNATURE
//! under `cfg(not(feature = "server"))`. A client build — the web bundle,
//! a native app — never sets `server`: the stub is what makes a call
//! reach the server, so a client build with the feature on would carry
//! the database layer instead. So the body of such a function, and any
//! item gated on `feature = "server"`, are not part of the client program
//! at all.
//!
//! The dev loop decides patch-or-rebuild for the CLIENT program. Without
//! this, every `#[server]` body edit in a library crate rebuilt the
//! bundle and reloaded the page (an `async fn` body is one a dependent
//! compiles from metadata, see [`crate::downstream`]), and in the app
//! crate it built a hot patch — both for code the client does not
//! contain. The server's own watcher rebuilds and restarts the server
//! either way.
//!
//! Recognition is syntactic, and deliberately narrow, because a false
//! positive is a client edit the page never receives:
//!
//! - the attribute is `#[server::server]` (or `::server::…`), or a bare
//!   `#[server]` whose name this module, or a parent it glob-imports with
//!   `use super::*`, imports from the `server` crate (`use server::server`,
//!   `use server::{server, …}`, `use server::*`, `as` renames). A bare
//!   `#[channel]` from some other crate is never mistaken for one;
//! - a `cfg` is server-only when it is `feature = "server"`, or an
//!   `all(…)` containing one. `any(…)` and `not(…)` are not.

use std::collections::BTreeSet;
use std::ops::Range;

use syn::spanned::Spanned;

/// The attribute macros that cfg-split a function on `feature = "server"`.
const SPLIT_MACROS: [&str; 4] = ["server", "channel", "subscription", "sse"];

/// Which split macros a module can name bare.
#[derive(Clone, Default)]
pub(crate) struct Scope {
    names: BTreeSet<String>,
}

impl Scope {
    /// The scope of a module whose items are `items`, nested in `parent`
    /// (`None` at the file root).
    pub(crate) fn of(items: &[syn::Item], parent: Option<&Scope>) -> Scope {
        let mut names = BTreeSet::new();
        for item in items {
            let syn::Item::Use(u) = item else { continue };
            match &u.tree {
                syn::UseTree::Path(p) if p.ident == "server" => imports_from_server(&p.tree, &mut names),
                syn::UseTree::Path(p) if p.ident == "super" => {
                    if matches!(&*p.tree, syn::UseTree::Glob(_)) {
                        if let Some(parent) = parent {
                            names.extend(parent.names.iter().cloned());
                        }
                    }
                }
                _ => {}
            }
        }
        Scope { names }
    }

    /// Whether `attrs` hold one of the split macros.
    pub(crate) fn is_split_fn(&self, attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            let segs: Vec<String> = a.path().segments.iter().map(|s| s.ident.to_string()).collect();
            match segs.as_slice() {
                [name] => self.names.contains(name),
                [krate, name] => krate == "server" && SPLIT_MACROS.contains(&name.as_str()),
                _ => false,
            }
        })
    }
}

/// The names a `use server::<tree>` brings in that are split macros.
fn imports_from_server(tree: &syn::UseTree, out: &mut BTreeSet<String>) {
    match tree {
        syn::UseTree::Name(n) if SPLIT_MACROS.contains(&n.ident.to_string().as_str()) => {
            out.insert(n.ident.to_string());
        }
        syn::UseTree::Rename(r) if SPLIT_MACROS.contains(&r.ident.to_string().as_str()) => {
            out.insert(r.rename.to_string());
        }
        syn::UseTree::Glob(_) => out.extend(SPLIT_MACROS.iter().map(|s| s.to_string())),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                imports_from_server(t, out);
            }
        }
        // `server::foo::bar` is not one of the macros.
        _ => {}
    }
}

/// Whether `attrs` hold a `cfg` no client build satisfies.
pub(crate) fn server_cfg(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg") && a.parse_args::<syn::Meta>().is_ok_and(|m| server_only_predicate(&m)))
}

fn server_only_predicate(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::NameValue(nv) if nv.path.is_ident("feature") => matches!(
            &nv.value,
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) if s.value() == "server"
        ),
        syn::Meta::List(list) if list.path.is_ident("all") => list
            .parse_args_with(syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
            .is_ok_and(|parts| parts.iter().any(server_only_predicate)),
        _ => false,
    }
}

/// `text` as a client build compiles it: every split macro's body and
/// every server-cfg'd item cut out. `None` when the file does not parse.
///
/// Byte-exact everywhere else — whitespace and comments included — so
/// two files with the same view differ ONLY in what the client does not
/// compile. A token rendering would also equate a comment edit in client
/// code, which moves the line numbers the client binary carries.
pub fn client_view(text: &str) -> Option<String> {
    let file: syn::File = syn::parse_file(text).ok()?;
    let mut cuts: Vec<Range<usize>> = Vec::new();
    collect(&file.items, &Scope::of(&file.items, None), &mut cuts);
    cuts.sort_by_key(|r| r.start);

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for cut in cuts {
        // A cut inside one already taken (a `#[server]` fn inside a
        // `cfg(feature = "server")` module).
        if cut.start < cursor {
            continue;
        }
        out.push_str(text.get(cursor..cut.start)?);
        out.push_str("\u{0}server\u{0}");
        cursor = cut.end.min(text.len());
    }
    out.push_str(text.get(cursor..)?);
    Some(out)
}

fn collect(items: &[syn::Item], scope: &Scope, cuts: &mut Vec<Range<usize>>) {
    for item in items {
        if server_cfg(item_attrs(item)) {
            cuts.push(item.span().byte_range());
            continue;
        }
        match item {
            syn::Item::Fn(f) if scope.is_split_fn(&f.attrs) => {
                cuts.push(f.block.brace_token.span.join().byte_range());
            }
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    collect(inner, &Scope::of(inner, Some(scope)), cuts);
                }
            }
            syn::Item::Impl(imp) => {
                for it in &imp.items {
                    let attrs = match it {
                        syn::ImplItem::Fn(f) => &f.attrs,
                        syn::ImplItem::Const(c) => &c.attrs,
                        syn::ImplItem::Type(t) => &t.attrs,
                        syn::ImplItem::Macro(m) => &m.attrs,
                        _ => continue,
                    };
                    if server_cfg(attrs) {
                        cuts.push(it.span().byte_range());
                    }
                }
            }
            syn::Item::Trait(t) => {
                for it in &t.items {
                    let attrs = match it {
                        syn::TraitItem::Fn(f) => &f.attrs,
                        syn::TraitItem::Const(c) => &c.attrs,
                        syn::TraitItem::Type(t) => &t.attrs,
                        syn::TraitItem::Macro(m) => &m.attrs,
                        _ => continue,
                    };
                    if server_cfg(attrs) {
                        cuts.push(it.span().byte_range());
                    }
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &str, b: &str) -> bool {
        client_view(a).expect("parses") == client_view(b).expect("parses")
    }

    const PING: &str = r#"use server::server;
use server::ServerError;

#[server]
pub async fn ping() -> Result<String, ServerError> { Ok("a".into()) }

pub fn label() -> &'static str { "client" }
"#;

    #[test]
    fn a_server_fn_body_is_not_client_code() {
        assert!(same(PING, &PING.replace(r#""a""#, r#""b""#)));
        // A new line inside the body too.
        assert!(same(PING, &PING.replace(r#"{ Ok("a".into()) }"#, "{\n    Ok(\"a\".into())\n}")));
    }

    #[test]
    fn the_signature_and_everything_else_are() {
        // The client stub is generated from the signature.
        assert!(!same(PING, &PING.replace("ping()", "ping(n: u32)")));
        assert!(!same(PING, &PING.replace(r#""client""#, r#""other""#)));
        // A comment moves the client binary's line numbers.
        assert!(!same(PING, &PING.replace("pub fn label", "// note\npub fn label")));
        // Losing the attribute puts the body in the client.
        assert!(!same(PING, &PING.replace("#[server]\n", "")));
    }

    #[test]
    fn a_bare_attribute_counts_only_when_imported_from_server() {
        let other = PING.replace("use server::server;", "use other::server;");
        assert!(!same(&other, &other.replace(r#""a""#, r#""b""#)));
        let none = PING.replace("use server::server;\n", "");
        assert!(!same(&none, &none.replace(r#""a""#, r#""b""#)));

        for import in ["use server::{server, ServerError};", "use server::*;", "use ::server::server;"] {
            let src = PING.replace("use server::server;", import);
            assert!(same(&src, &src.replace(r#""a""#, r#""b""#)), "{import}");
        }
        let renamed = PING.replace("use server::server;", "use server::server as rpc;").replace("#[server]", "#[rpc]");
        assert!(same(&renamed, &renamed.replace(r#""a""#, r#""b""#)));

        let qualified = PING.replace("use server::server;\n", "").replace("#[server]", "#[server::server]");
        assert!(same(&qualified, &qualified.replace(r#""a""#, r#""b""#)));
    }

    #[test]
    fn the_sibling_macros_split_the_same_way() {
        let src = r#"use server::{channel, sse, subscription};
#[channel] pub async fn chat(s: Socket<In, Out>) { let x = 1; }
#[subscription] pub async fn feed() -> impl Stream<Item = u8> { a() }
#[sse] pub async fn ticks() -> impl Stream<Item = u8> { a() }
"#;
        assert!(same(src, &src.replace("let x = 1;", "let x = 2;").replace("a()", "b()")));
    }

    #[test]
    fn a_nested_module_sees_its_own_imports_and_a_super_glob() {
        let src = r#"use server::server;
mod own { use server::server; #[server] async fn a() -> R { 1 } }
mod glob { use super::*; #[server] async fn b() -> R { 1 } }
mod none { #[server] async fn c() -> R { 1 } }
"#;
        assert!(same(src, &src.replace("a() -> R { 1 }", "a() -> R { 2 }")));
        assert!(same(src, &src.replace("b() -> R { 1 }", "b() -> R { 2 }")));
        assert!(!same(src, &src.replace("c() -> R { 1 }", "c() -> R { 2 }")));
    }

    #[test]
    fn a_server_cfg_item_is_not_client_code() {
        let src = r#"#[cfg(feature = "server")]
fn helper() -> u8 { 1 }
#[cfg(all(test, feature = "server"))]
mod t { fn x() {} }
#[cfg(not(feature = "server"))]
fn client() -> u8 { 1 }
#[cfg(any(feature = "server", test))]
fn either() -> u8 { 1 }
struct S;
impl S {
    #[cfg(feature = "server")]
    fn m(&self) -> u8 { 1 }
}
"#;
        assert!(same(src, &src.replace("fn helper() -> u8 { 1 }", "fn helper() -> u16 { 2 }")));
        assert!(same(src, &src.replace("fn x() {}", "fn x() { y() }")));
        assert!(same(src, &src.replace("fn m(&self) -> u8 { 1 }", "fn m(&self) -> u8 { 2 }")));
        assert!(!same(src, &src.replace("fn client() -> u8 { 1 }", "fn client() -> u8 { 2 }")));
        assert!(!same(src, &src.replace("fn either() -> u8 { 1 }", "fn either() -> u8 { 2 }")));
        // Dropping the gate puts the item in the client.
        assert!(!same(src, &src.replacen("#[cfg(feature = \"server\")]\nfn helper", "fn helper", 1)));
    }

    #[test]
    fn an_unparseable_file_has_no_view() {
        assert!(client_view("fn (").is_none());
    }
}
