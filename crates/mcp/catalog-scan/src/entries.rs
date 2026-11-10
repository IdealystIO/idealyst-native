//! `inventory::submit! { … }` literal → catalog entry.
//!
//! The catalog macros register each entry as a struct literal of string
//! and integer literals, nested slices of the same, and three macros
//! (`module_path!()`, `file!()`, `line!()`). That is the whole grammar a
//! compiled extractor evaluates, so the scanner evaluates it the same
//! way: literals as written, the three macros from the invocation site
//! (see [`Site`]). Anything else in a catalog literal (a `const`, a
//! function call) cannot be evaluated from source and is an error.
//!
//! Strings are leaked to `&'static str`, as
//! [`mcp_catalog::ResolvedCatalog::build_from_json`] does for a catalog
//! read from JSON: the entry types are the linker-section types, and the
//! scanner runs in a short-lived process.

use std::collections::HashMap;

use mcp_catalog::{
    AnimationEntry, CatalogParts, ComponentEntry, EdgeRef, MethodEntry, ParamSpec, PropFieldSpec, PropsSchemaEntry,
    RecipeEntry, ScopeEntry, ToolEntry, TypeEntry, TypeShape, ValueEntry, VariantSpec,
};

/// Where an entry's `module_path!()` / `file!()` / `line!()` point: the
/// outermost macro invocation that produced it, as rustc reports them.
#[derive(Debug, Clone)]
pub struct Site {
    pub module_path: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
enum Val {
    Str(String),
    Int(u64),
    Arr(Vec<Val>),
    /// Last path segment(s) of the struct (`ComponentEntry`,
    /// `TypeShape::Struct`) and its fields.
    Struct(String, HashMap<String, Val>),
}

/// The entry types the catalog macros register (and `ExternalEntry`,
/// which `#[component(external)]` registers for `idealyst export`).
const SCANNED: &[&str] = &[
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
];

/// Whether a submitted value's type path names a catalog type — the
/// `__mcp` re-export every catalog macro spells, or the catalog crate
/// itself. `inventory` carries other registries too (remote imports, …),
/// which the scanner ignores.
///
/// A path through neither that still ends in a catalog type's name
/// (`ScopeEntry { … }` after a `use`) is a catalog registration too: it is
/// taken as one and refused like any hand registration, rather than
/// skipped as someone else's.
pub fn is_catalog_submission(expr: &syn::Expr) -> bool {
    let syn::Expr::Struct(s) = expr else { return false };
    s.path.segments.iter().any(|seg| seg.ident == "__mcp" || seg.ident == "mcp_catalog")
        || s.path.segments.last().is_some_and(|seg| crate::CATALOG_TYPES.contains(&seg.ident.to_string().as_str()))
}

/// Evaluate one submitted catalog literal into `parts`.
pub fn add_submission(expr: &syn::Expr, site: &Site, parts: &mut CatalogParts) -> Result<(), String> {
    // The type first: a hand-registered catalog type is refused by name,
    // whatever its fields hold.
    if let syn::Expr::Struct(s) = expr {
        let kind = s.path.segments.last().map(|seg| seg.ident.to_string()).unwrap_or_default();
        if !SCANNED.contains(&kind.as_str()) {
            return Err(format!(
                "a `{kind}` registered by hand; the scanner reads only what the catalog macros emit"
            ));
        }
    }
    let Val::Struct(kind, f) = eval(expr, site)? else {
        return Err("a catalog submission is not a struct literal".into());
    };
    let mut f = Fields(f, &kind);
    match kind.as_str() {
        "ComponentEntry" => parts.components.push(leak(ComponentEntry {
            name: f.str("name")?,
            module_path: f.str("module_path")?,
            file: f.str("file")?,
            line: f.int("line")?,
            docs: f.str("docs")?,
            composes: f.list("composes", |mut e| Ok(EdgeRef { name: e.str("name")?, line: e.int("line")? }))?,
            params: f.list("params", param)?,
        })),
        "MethodEntry" => parts.methods.push(leak(MethodEntry {
            parent_module_path: f.str("parent_module_path")?,
            parent_name: f.str("parent_name")?,
            name: f.str("name")?,
            docs: f.str("docs")?,
            params: f.list("params", param)?,
            return_type: f.str("return_type")?,
        })),
        "AnimationEntry" => parts.animations.push(leak(AnimationEntry {
            parent_module_path: f.str("parent_module_path")?,
            parent_name: f.str("parent_name")?,
            binding: f.str("binding")?,
            initial: f.str("initial")?,
            line: f.int("line")?,
        })),
        "PropsSchemaEntry" => parts.props_schemas.push(leak(PropsSchemaEntry {
            short_name: f.str("short_name")?,
            module_path: f.str("module_path")?,
            fields: f.list("fields", field)?,
        })),
        "TypeEntry" => {
            let shape = match f.take("shape")? {
                Val::Struct(k, s) if k == "TypeShape::Struct" => {
                    TypeShape::Struct { fields: Fields(s, &k).list("fields", field)? }
                }
                Val::Struct(k, s) if k == "TypeShape::Enum" => TypeShape::Enum {
                    variants: Fields(s, &k).list("variants", |mut v| {
                        Ok(VariantSpec { name: v.str("name")?, docs: v.str("docs")?, payload: v.list("payload", field)? })
                    })?,
                },
                other => return Err(format!("unrecognised TypeEntry shape {other:?}")),
            };
            parts.types.push(leak(TypeEntry {
                short_name: f.str("short_name")?,
                module_path: f.str("module_path")?,
                docs: f.str("docs")?,
                shape,
            }))
        }
        "ValueEntry" => parts.values.push(leak(ValueEntry {
            short_name: f.str("short_name")?,
            module_path: f.str("module_path")?,
            docs: f.str("docs")?,
            value_of: f.str("value_of")?,
            via: f.str("via")?,
        })),
        "ToolEntry" => parts.tools.push(leak(ToolEntry {
            name: f.str("name")?,
            module_path: f.str("module_path")?,
            file: f.str("file")?,
            line: f.int("line")?,
            docs: f.str("docs")?,
            params: f.list("params", param)?,
            return_type: f.str("return_type")?,
        })),
        "RecipeEntry" => parts.recipes.push(leak(RecipeEntry {
            name: f.str("name")?,
            target: f.str("target")?,
            module_path: f.str("module_path")?,
            file: f.str("file")?,
            line: f.int("line")?,
            docs: f.str("docs")?,
            source: f.str("source")?,
            uses: f.str_list("uses")?,
        })),
        "ScopeEntry" => parts.scopes.push(leak(ScopeEntry {
            slug: f.str("slug")?,
            title: f.str("title")?,
            docs: f.str("docs")?,
            module_path: f.str("module_path")?,
            order: f.int("order")?,
        })),
        // `idealyst export`'s manifest, not part of the catalog document.
        "ExternalEntry" => {}
        other => unreachable!("`{other}` is not in SCANNED"),
    }
    Ok(())
}

fn param(mut p: Fields) -> Result<ParamSpec, String> {
    Ok(ParamSpec { name: p.str("name")?, type_str: p.str("type_str")?, type_short_name: p.str("type_short_name")? })
}

fn field(mut p: Fields) -> Result<PropFieldSpec, String> {
    Ok(PropFieldSpec {
        name: p.str("name")?,
        type_str: p.str("type_str")?,
        doc: p.str("doc")?,
        constraint: p.str("constraint")?,
    })
}

fn leak<T>(v: T) -> &'static T {
    Box::leak(Box::new(v))
}

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The fields of one struct literal, consumed by name.
struct Fields<'k>(HashMap<String, Val>, &'k str);

impl Fields<'_> {
    fn take(&mut self, name: &str) -> Result<Val, String> {
        self.0.remove(name).ok_or_else(|| format!("`{}` literal has no `{name}` field", self.1))
    }

    fn str(&mut self, name: &str) -> Result<&'static str, String> {
        match self.take(name)? {
            Val::Str(s) => Ok(leak_str(s)),
            other => Err(format!("`{}.{name}` is not a string: {other:?}", self.1)),
        }
    }

    fn int(&mut self, name: &str) -> Result<u32, String> {
        match self.take(name)? {
            Val::Int(n) => u32::try_from(n).map_err(|_| format!("`{}.{name}` overflows u32", self.1)),
            other => Err(format!("`{}.{name}` is not an integer: {other:?}", self.1)),
        }
    }

    fn list<T>(&mut self, name: &str, each: impl Fn(Fields) -> Result<T, String>) -> Result<&'static [T], String> {
        let Val::Arr(items) = self.take(name)? else {
            return Err(format!("`{}.{name}` is not a slice", self.1));
        };
        let out = items
            .into_iter()
            .map(|v| match v {
                Val::Struct(k, f) => each(Fields(f, leak_str(k))),
                other => Err(format!("`{}.{name}` holds a non-struct {other:?}", self.1)),
            })
            .collect::<Result<Vec<T>, String>>()?;
        Ok(Box::leak(out.into_boxed_slice()))
    }

    fn str_list(&mut self, name: &str) -> Result<&'static [&'static str], String> {
        let Val::Arr(items) = self.take(name)? else {
            return Err(format!("`{}.{name}` is not a slice", self.1));
        };
        let out = items
            .into_iter()
            .map(|v| match v {
                Val::Str(s) => Ok(leak_str(s)),
                other => Err(format!("`{}.{name}` holds a non-string {other:?}", self.1)),
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Box::leak(out.into_boxed_slice()))
    }
}

fn eval(expr: &syn::Expr, site: &Site) -> Result<Val, String> {
    use syn::Expr;
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Str(s) => Ok(Val::Str(s.value())),
            syn::Lit::Int(i) => i.base10_parse::<u64>().map(Val::Int).map_err(|e| e.to_string()),
            other => Err(format!("unsupported literal `{}`", quote::quote!(#other))),
        },
        Expr::Macro(m) => {
            let name = m.mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
            match name.as_str() {
                "module_path" => Ok(Val::Str(site.module_path.clone())),
                "file" => Ok(Val::Str(site.file.clone())),
                "line" => Ok(Val::Int(site.line.into())),
                other => Err(format!("unsupported macro `{other}!` in a catalog literal")),
            }
        }
        Expr::Reference(r) => eval(&r.expr, site),
        Expr::Group(g) => eval(&g.expr, site),
        Expr::Paren(p) => eval(&p.expr, site),
        Expr::Array(a) => a.elems.iter().map(|e| eval(e, site)).collect::<Result<_, _>>().map(Val::Arr),
        Expr::Struct(s) => {
            let segs: Vec<String> = s.path.segments.iter().map(|seg| seg.ident.to_string()).collect();
            // `TypeShape::Struct { … }` is an enum variant: keep the enum's
            // name too, so the two shapes stay apart.
            let name = match segs.as_slice() {
                [.., a, b] if a == "TypeShape" => format!("{a}::{b}"),
                [.., last] => last.clone(),
                [] => return Err("a struct literal with no path".into()),
            };
            let mut fields = HashMap::new();
            for fv in &s.fields {
                let syn::Member::Named(id) = &fv.member else {
                    return Err(format!("`{name}` literal has a positional field"));
                };
                fields.insert(id.to_string(), eval(&fv.expr, site)?);
            }
            Ok(Val::Struct(name, fields))
        }
        other => Err(format!("cannot evaluate `{}` from source", quote::quote!(#other))),
    }
}
