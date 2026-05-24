//! Walk an `swc_ecma_ast::Module` and produce a `port_core::ir::Module`.
//!
//! Scope is intentionally narrow — we recognize:
//!
//! - exported function components: `export function Counter(...)`
//!   and `export function Counter(props)` with a TS type annotation
//!   on the first param,
//! - prop interfaces: `interface CounterProps { foo?: T }`,
//! - destructured-with-defaults param: `({ initial = 0 }: CounterProps)`,
//! - hook calls in the function body, classified via the
//!   [`Lifter`] trait,
//! - JSX in the return expression.
//!
//! Anything we don't recognize either becomes a hole (for in-body
//! statements / JSX attribute values) or is dropped from the IR
//! with a `PortReport` entry (top-level items).
//!
//! The TSX surface is huge; this scope is "enough to handle the
//! bundled Counter fixtures while emitting clean holes for the
//! rest." Each shape recognized here is a concrete commitment;
//! the porter's *coverage promise* lives in this file.

use port_core::ir::*;
use port_core::ParseError;
use std::collections::{HashMap, HashSet};
use swc_common::{sync::Lrc, SourceMap, Span, Spanned};
use swc_ecma_ast as ast;

/// Frontend-specific reactive-primitive recognition. Implemented by
/// `port-react` (`useState`/`useEffect`/...) and `port-solid`
/// (`createSignal`/`createEffect`/...).
pub trait Lifter {
    /// Classify a `<callee>(<args>)` call in a component body. The
    /// lifter inspects the callee identifier (e.g. `"useState"`)
    /// and the surrounding `let`-binding context (provided as
    /// `binding`) to decide which IR primitive this maps to.
    /// Returning `None` means "I don't recognize this call; treat
    /// it as a hole or pass-through."
    fn classify_call(&self, callee: &str, ctx: &CallContext) -> Option<LiftedCall>;

    /// Read-access style for state signals declared by this
    /// framework. React's `useState` returns a value (bare ident);
    /// Solid's `createSignal` returns a callable (parens). This
    /// influences how the lifter rewrites identifier reads inside
    /// JSX/handlers to `.get()` calls.
    fn signal_read_style(&self) -> ReadStyle;
}

/// How a frontend's signal reads look in the source language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStyle {
    /// React: bare identifier is a value. `<Text>{count}</Text>` /
    /// `count + 1`. Identifier rewriter inserts `.get()`.
    BareIdent,
    /// Solid: callable. `<Text>{count()}</Text>` / `count() + 1`.
    /// Identifier rewriter unwraps the call and inserts `.get()`.
    CallExpression,
}

/// Context for a `<callee>(<args>)` call site. The lifter consults
/// the surrounding binding (if any) to know whether the call is a
/// `let [a, b] = useState(0)` destructure (needs name + setter) or
/// a bare expression like `useEffect(() => {})`.
pub struct CallContext<'a> {
    pub args: &'a [ast::ExprOrSpread],
    pub binding: Option<BindingPattern<'a>>,
}

/// The lvalue surrounding a recognized call.
pub enum BindingPattern<'a> {
    /// `let [name, setter] = call(...)` — two-ident array.
    Pair { name: &'a str, setter: &'a str },
    /// `let name = call(...)` — single ident.
    Single { name: &'a str },
}

/// What the lifter wants the lifter pipeline to emit for one call.
pub enum LiftedCall {
    /// Emit `Reactive::State { name, setter, init }`. The init
    /// expression is the call's first argument, rendered to a Rust
    /// expression string by the shared renderer.
    State,
    /// Emit `Reactive::Effect { body, deps }`. Body comes from the
    /// first argument (an arrow function); deps comes from the
    /// second (`Some([…])` for React-style, `None` for
    /// auto-tracking frameworks).
    Effect { has_deps: bool },
    /// `useContext(Ctx)` / Solid `useContext(Ctx)`. Emits
    /// `let name = inject::<Ctx>();` — idealyst's context API is
    /// type-keyed; the first argument's identifier is used as
    /// the type name. The user is expected to define a Rust type
    /// matching that name in their project (often by porting
    /// `createContext<T>(default)` into a `pub struct Ctx { … }`).
    Inject,
    /// Drop the call silently; the lifter recognized it but it has
    /// no IR equivalent worth modeling (e.g., type-only calls).
    Drop,
}

/// The kind of reactive declaration the lifter emitted — used by
/// the body walker to track scope-local state idents so JSX /
/// handler walks can rewrite reads/writes.
#[derive(Debug, Clone)]
pub enum ReactiveKind {
    State { name: String, setter: String },
}

// =============================================================================
// Public entry
// =============================================================================

pub fn lift_module(
    ast: &ast::Module,
    lifter: &dyn Lifter,
    cm: &Lrc<SourceMap>,
) -> Result<(Module, PortReport), ParseError> {
    let mut report = PortReport::default();
    let mut module = Module {
        source_tool: String::new(), // filled by caller
        imports: default_imports(),
        components: Vec::new(),
        passthroughs: Vec::new(),
        local_interfaces: HashMap::new(),
        unresolved_context_aliases: Vec::new(),
    };

    // First pass: collect every TS *type definition* keyed by
    // name. We accept two shapes that lower to the same
    // `PropsType` IR:
    //
    //   interface X { … }                  — TsInterfaceDecl
    //   type X = { … }                     — TsTypeAliasDecl with TsTypeLit body
    //
    // Both produce a struct's worth of fields. Anything more
    // exotic (`type X = A | B`, mapped types, generics) is
    // skipped — the porter doesn't model TS's full type system.
    let mut types: HashMap<String, PropsType> = HashMap::new();
    for item in &ast.body {
        if let Some((name, props)) = extract_type_decl(item) {
            types.insert(name, props);
        }
    }
    module.local_interfaces = types.clone();

    // Second pass: detect `const X = createContext<T>(default)`
    // (with or without `export`) and emit a Rust struct named `X`
    // mirroring `T`'s shape, as a passthrough at module top. This
    // makes `inject::<X>()` and `provide(X { … })` references
    // resolve without the user having to hand-write the type.
    for item in &ast.body {
        if let Some((alias, type_name)) = context_alias_from_item(item) {
            if let Some(props) = types.get(&type_name) {
                module
                    .passthroughs
                    .push(SourceSnippet::new(port_core::emit::render_struct(&alias, props)));
            } else {
                // Type lives somewhere else (imported from
                // another file). Record the alias for the
                // project-level driver to resolve against its
                // cross-file registry; if no driver runs (single-
                // file CLI use), the alias surfaces as a sentinel
                // empty struct at emit time.
                module.unresolved_context_aliases.push((alias, type_name));
            }
        }
    }

    // Third pass: find exported components. We accept four
    // distinct export shapes, in descending order of how often
    // real React/Solid/Vue/Svelte code uses them today:
    //
    //   export function Foo(…) { … }
    //   export default function Foo(…) { … }
    //   export const Foo = (…) => { … }
    //   export default (…) => { … }
    //
    // Each normalizes to a `ComponentInput` (name + first param +
    // body) which `lift_component_from_input` then walks. The
    // shape-specific code lives only in `collect_components`.
    for item in &ast.body {
        for input in collect_components(item) {
            if let Some(c) = lift_component_from_input(input, lifter, &types, cm, &mut report) {
                module.components.push(c);
            }
        }
    }

    // Empty modules are *not* an error — many real source files
    // (app entry points, type-only exports, re-exports) legitimately
    // contain zero components. The project porter classifies these
    // as Skipped via the `component_count` field.
    report.component_count = module.components.len();
    Ok((module, report))
}

fn default_imports() -> Vec<String> {
    vec![
        "component".into(),
        "jsx".into(),
        "signal".into(),
        "effect".into(),
        "on_cleanup".into(),
        "provide".into(),
        "inject".into(),
        "Primitive".into(),
        "Signal".into(),
    ]
}

// =============================================================================
// Export-shape normalization
// =============================================================================

/// One component, normalized across the four supported export
/// shapes so the body walker doesn't care whether the source was
/// `function`, `const … = () =>`, or default-exported.
struct ComponentInput<'a> {
    name: String,
    first_param: Option<&'a ast::Pat>,
    body: CompBody<'a>,
}

/// Bodies come in two flavors: a `{ stmt; stmt; return … }` block
/// (function declarations + arrow with block body), or a single
/// expression (arrow with expression body — `() => <Jsx/>`). The
/// latter is treated as if it were `{ return <expr>; }`.
enum CompBody<'a> {
    Block(&'a [ast::Stmt]),
    Expr(&'a ast::Expr),
}

/// Extract zero or more component inputs from one module-level
/// item. Returns a Vec because `export const A = …, B = …;`
/// could declare multiple components on one line (we still walk
/// each).
/// Detect `const X = createContext<T>(default)` (with or without
/// `export`). Returns `(X, T)` — the alias's binding name and
/// the referenced interface name. Anything else (no type arg,
/// non-ident type arg) returns `None`.
fn context_alias_from_item(item: &ast::ModuleItem) -> Option<(String, String)> {
    let var_decl: &ast::VarDecl = match item {
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(e)) => match &e.decl {
            ast::Decl::Var(v) => v,
            _ => return None,
        },
        ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Var(v))) => v,
        _ => return None,
    };
    for d in &var_decl.decls {
        let alias = match &d.name {
            ast::Pat::Ident(b) => b.id.sym.to_string(),
            _ => continue,
        };
        let Some(init) = &d.init else { continue };
        let call = match &**init {
            ast::Expr::Call(c) => c,
            _ => continue,
        };
        let callee_name = match &call.callee {
            ast::Callee::Expr(e) => match &**e {
                ast::Expr::Ident(id) => id.sym.to_string(),
                _ => continue,
            },
            _ => continue,
        };
        if callee_name != "createContext" {
            continue;
        }
        let Some(type_args) = &call.type_args else { continue };
        let Some(first) = type_args.params.first() else { continue };
        if let Some(type_name) = extract_context_type_name(first) {
            return Some((alias, type_name));
        }
    }
    None
}

/// Peel a TS type argument down to a single named TypeRef.
/// Handles:
///
/// - Direct refs: `T` → `Some("T")`.
/// - Nullable unions: `T | null`, `T | undefined`, `null | T` →
///   `Some("T")` — the dominant "context might be missing" idiom
///   in React codebases.
/// - Intersections with one non-keyword arm: `T & SomeMix` —
///   similar peeling, though rarer.
///
/// Returns `None` for ambiguous unions (two or more non-null
/// TypeRefs), inline object types, generic parameters, etc. The
/// caller treats `None` as "user must hand-write the type."
fn extract_context_type_name(t: &ast::TsType) -> Option<String> {
    match t {
        ast::TsType::TsTypeRef(r) => match &r.type_name {
            ast::TsEntityName::Ident(id) => Some(id.sym.to_string()),
            ast::TsEntityName::TsQualifiedName(_) => None,
        },
        ast::TsType::TsUnionOrIntersectionType(uoi) => {
            let types = match uoi {
                ast::TsUnionOrIntersectionType::TsUnionType(u) => &u.types,
                ast::TsUnionOrIntersectionType::TsIntersectionType(i) => &i.types,
            };
            let mut found: Option<String> = None;
            for ty in types {
                if is_null_or_undefined(ty) {
                    continue;
                }
                if let Some(name) = extract_context_type_name(ty) {
                    if found.is_some() {
                        return None; // ambiguous: two named arms
                    }
                    found = Some(name);
                }
            }
            found
        }
        _ => None,
    }
}

fn is_null_or_undefined(t: &ast::TsType) -> bool {
    matches!(
        t,
        ast::TsType::TsKeywordType(k)
            if matches!(
                k.kind,
                ast::TsKeywordTypeKind::TsNullKeyword | ast::TsKeywordTypeKind::TsUndefinedKeyword
            )
    )
}

fn collect_components<'a>(item: &'a ast::ModuleItem) -> Vec<ComponentInput<'a>> {
    let mut out = Vec::new();
    match item {
        // `export function Foo() {}` — the classic shape.
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(e)) => match &e.decl {
            ast::Decl::Fn(f) => {
                if let Some(input) = from_fn_decl(f) {
                    out.push(input);
                }
            }
            ast::Decl::Var(decl) => {
                for d in &decl.decls {
                    if let Some(input) = from_const_arrow(d) {
                        out.push(input);
                    }
                }
            }
            _ => {}
        },
        // `export default function Foo() {}` (or anonymous).
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultDecl(e)) => {
            if let ast::DefaultDecl::Fn(fn_expr) = &e.decl {
                if let Some(input) = from_fn_expr(fn_expr) {
                    out.push(input);
                }
            }
        }
        // `export default () => …`
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(e)) => {
            if let ast::Expr::Arrow(arrow) = &*e.expr {
                out.push(from_arrow("Default".into(), arrow));
            }
        }
        _ => {}
    }
    out
}

fn from_fn_decl(f: &ast::FnDecl) -> Option<ComponentInput<'_>> {
    let body = f.function.body.as_ref()?;
    let name = f.ident.sym.to_string();
    // Same convention filter as `from_const_arrow`: components are
    // PascalCase. `useFoo`, `helper`, etc. are custom hooks or
    // utilities, not components — skip them so they don't render
    // as `#[component] fn`.
    if !name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        return None;
    }
    let first_param = f.function.params.first().map(|p| &p.pat);
    Some(ComponentInput {
        name,
        first_param,
        body: CompBody::Block(&body.stmts),
    })
}

fn from_fn_expr(f: &ast::FnExpr) -> Option<ComponentInput<'_>> {
    let body = f.function.body.as_ref()?;
    let first_param = f.function.params.first().map(|p| &p.pat);
    let name = f
        .ident
        .as_ref()
        .map(|i| i.sym.to_string())
        .unwrap_or_else(|| "Default".into());
    Some(ComponentInput {
        name,
        first_param,
        body: CompBody::Block(&body.stmts),
    })
}

fn from_const_arrow(d: &ast::VarDeclarator) -> Option<ComponentInput<'_>> {
    // We're after `const Foo = (…) => …`. The binding pattern
    // must be a simple ident (anything else isn't a component).
    let name = match &d.name {
        ast::Pat::Ident(b) => b.id.sym.to_string(),
        _ => return None,
    };
    let arrow = match d.init.as_deref()? {
        ast::Expr::Arrow(a) => a,
        _ => return None,
    };
    // Skip non-component identifiers by convention: components
    // start with an uppercase letter. This filters out
    // `const useThing = () => …` (hooks) and `const helper = …`
    // (regular utilities) so we don't try to render them as
    // `#[component] fn …`.
    if !name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        return None;
    }
    Some(from_arrow(name, arrow))
}

fn from_arrow<'a>(name: String, arrow: &'a ast::ArrowExpr) -> ComponentInput<'a> {
    let first_param = arrow.params.first();
    let body = match &*arrow.body {
        ast::BlockStmtOrExpr::BlockStmt(b) => CompBody::Block(&b.stmts),
        ast::BlockStmtOrExpr::Expr(e) => CompBody::Expr(e),
    };
    ComponentInput { name, first_param, body }
}

// =============================================================================
// Component lifting
// =============================================================================

fn lift_component_from_input(
    input: ComponentInput<'_>,
    lifter: &dyn Lifter,
    types: &HashMap<String, PropsType>,
    cm: &Lrc<SourceMap>,
    report: &mut PortReport,
) -> Option<Component> {
    let name = input.name;

    // Lift props. Three recognized shapes:
    //   ({ a = 0, b }: PropsType)      — destructured with TS type
    //   ({ a = 0, b })                 — destructured, no type
    //   (props: PropsType)             — single named param
    let (props, prop_defaults_from_destructure) =
        lift_props(input.first_param, types, &name, report);

    let mut state_idents: HashMap<String, String> = HashMap::new(); // setter -> name
    let mut preamble: Vec<Reactive> = Vec::new();
    let mut ret_jsx: Option<JsxNode> = None;

    // Build the prop-fields set so expression rendering can rewrite
    // bare `initial` → `props.initial` for destructured props.
    let mut prop_fields: HashSet<String> = props.fields.iter().map(|f| f.name.clone()).collect();
    // If the param was a destructure, the destructure keys are also
    // prop fields (and were merged into props above).
    for (k, _) in &prop_defaults_from_destructure {
        prop_fields.insert(k.clone());
    }

    // Expression-body arrows (`() => <Jsx/>`) are treated as if
    // they returned that expression. Block bodies walk statements
    // normally.
    let stmts: &[ast::Stmt] = match input.body {
        CompBody::Block(stmts) => stmts,
        CompBody::Expr(e) => {
            ret_jsx = Some(lift_return_jsx(e, lifter, &prop_fields, &state_idents, report));
            &[]
        }
    };

    for stmt in stmts {
        match stmt {
            ast::Stmt::Decl(ast::Decl::Var(decl)) => {
                lift_var_decl(decl, lifter, &prop_fields, &mut state_idents, &mut preamble, cm, report);
            }
            ast::Stmt::Expr(es) => {
                lift_expr_stmt(&es.expr, lifter, &prop_fields, &state_idents, &mut preamble, cm, report);
            }
            ast::Stmt::Return(rs) => {
                if let Some(arg) = &rs.arg {
                    ret_jsx = Some(lift_return_jsx(arg, lifter, &prop_fields, &state_idents, report));
                }
            }
            _ => {
                // Unrecognized top-level statement — record as
                // an Unsupported hole and continue.
                report.record(Hole {
                    kind: HoleKind::Unsupported,
                    reason: "top-level statement in component body".into(),
                    original: SourceSnippet::new("…statement…"),
                });
            }
        }
    }

    let returns = match ret_jsx {
        Some(j) => ReturnExpr::Jsx(j),
        None => ReturnExpr::Hole(Hole {
            kind: HoleKind::Unsupported,
            reason: "component has no JSX return".into(),
            original: SourceSnippet::new(format!("fn {}", name)),
        }),
    };

    // Merge defaults from destructure pattern back into props.
    let mut props = props;
    for (k, v) in prop_defaults_from_destructure {
        if let Some(field) = props.fields.iter_mut().find(|f| f.name == k) {
            field.default = Some(v);
        }
    }

    Some(Component {
        name,
        props,
        body: ComponentBody { preamble, returns },
    })
}

// =============================================================================
// Props
// =============================================================================

fn lift_props(
    param: Option<&ast::Pat>,
    types: &HashMap<String, PropsType>,
    component_name: &str,
    report: &mut PortReport,
) -> (PropsType, HashMap<String, String>) {
    let Some(pat) = param else {
        return (PropsType::default(), HashMap::new());
    };
    let type_ann = match pat {
        ast::Pat::Ident(b) => b.type_ann.as_ref(),
        ast::Pat::Object(o) => o.type_ann.as_ref(),
        _ => None,
    };

    let mut props = PropsType::default();
    if let Some(t) = type_ann {
        if let ast::TsType::TsTypeRef(r) = &*t.type_ann {
            if let ast::TsEntityName::Ident(id) = &r.type_name {
                let name = id.sym.to_string();
                if let Some(found) = types.get(&name) {
                    props = found.clone();
                }
            }
        }
    }

    // Walk the destructure pattern (if any) for both defaults and
    // — when there's no TS interface giving us field types —
    // bare field names. The harvested names are what lets the
    // body renderer rewrite `<input value={value} />` →
    // `<input value={props.value} />` even when the source had no
    // typed interface.
    let mut destructure_defaults: HashMap<String, String> = HashMap::new();
    if let ast::Pat::Object(obj) = pat {
        for p in &obj.props {
            match p {
                ast::ObjectPatProp::Assign(a) => {
                    let name = a.key.sym.to_string();
                    if let Some(default) = &a.value {
                        destructure_defaults.insert(name.clone(), render_default_expr(default));
                    }
                    if props.fields.iter().all(|f| f.name != name) {
                        // Add an untyped placeholder. The Rust
                        // type is `()` — clearly wrong-yet-compiling
                        // for non-unit props, by design: the porter
                        // also records a PropType hole below, and
                        // a TODO comment is emitted next to the
                        // field. The win is that JSX/expressions
                        // referencing `name` now route to
                        // `props.name`.
                        let ty = guess_untyped_field_ty(&name);
                        props.fields.push(PropField {
                            name,
                            ty,
                            optional: a.value.is_some(),
                            default: None,
                        });
                    }
                }
                ast::ObjectPatProp::KeyValue(kv) => {
                    // `{ foo: localName }` — rename in the
                    // destructure. We register the *source* key
                    // as the prop field; the `localName` is what
                    // the body refers to, but the lifter doesn't
                    // currently rewrite that. Record as a hole
                    // so the AI pass sees it.
                    if let ast::PropName::Ident(i) = &kv.key {
                        let name = i.sym.to_string();
                        if props.fields.iter().all(|f| f.name != name) {
                            let ty = guess_untyped_field_ty(&name);
                            props.fields.push(PropField {
                                name: name.clone(),
                                ty,
                                optional: true,
                                default: None,
                            });
                        }
                        report.record(Hole {
                            kind: HoleKind::PropType,
                            reason: format!("renamed destructure `{}` — body refs may not be rewritten", name),
                            original: SourceSnippet::new(format!("{{ {}: … }}", name)),
                        });
                    }
                }
                ast::ObjectPatProp::Rest(_) => {
                    report.record(Hole {
                        kind: HoleKind::Unsupported,
                        reason: "rest destructure `...rest` in props".into(),
                        original: SourceSnippet::new("...rest"),
                    });
                }
            }
        }
    }

    // If we found no prop interface, record a hole so the user
    // knows the porter dropped a typed param shape.
    if props.fields.is_empty() && type_ann.is_some() {
        report.record(Hole {
            kind: HoleKind::PropType,
            reason: format!("could not resolve prop type for component `{}`", component_name),
            original: SourceSnippet::new("…prop type…"),
        });
    }

    (props, destructure_defaults)
}

/// Extract a `(name, PropsType)` pair from a module item if it's
/// a type definition the porter can model.
///
/// Recognized shapes:
///
/// - `interface X { … }` — direct.
/// - `type X = { … }` — type alias with an inline object literal
///   on the RHS. Anything more exotic (`type X = A | B`, mapped
///   types, generics with default fallbacks) is skipped.
pub(crate) fn extract_type_decl(item: &ast::ModuleItem) -> Option<(String, PropsType)> {
    // Unwrap an optional `export` wrapper.
    let decl: &ast::Decl = match item {
        ast::ModuleItem::Stmt(ast::Stmt::Decl(d)) => d,
        ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(e)) => &e.decl,
        _ => return None,
    };
    match decl {
        ast::Decl::TsInterface(iface) => {
            Some((iface.id.sym.to_string(), lift_members(&iface.body.body)))
        }
        ast::Decl::TsTypeAlias(alias) => {
            if let ast::TsType::TsTypeLit(lit) = &*alias.type_ann {
                Some((alias.id.sym.to_string(), lift_members(&lit.members)))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Lift a list of TS type-element members (interface body or
/// type-literal members) to a `PropsType`. Unmappable field
/// types fall back to `()`; the caller doesn't record holes
/// here because cross-file resolution may rehome these later.
fn lift_members(members: &[ast::TsTypeElement]) -> PropsType {
    let mut props = PropsType::default();
    for member in members {
        if let ast::TsTypeElement::TsPropertySignature(sig) = member {
            let name = match &*sig.key {
                ast::Expr::Ident(i) => i.sym.to_string(),
                _ => continue,
            };
            let ty = sig
                .type_ann
                .as_ref()
                .and_then(|t| ts_type_to_rust(&t.type_ann))
                .unwrap_or_else(|| "()".into());
            props.fields.push(PropField {
                name,
                ty,
                optional: sig.optional,
                default: None,
            });
        }
    }
    props
}

/// Map a TS type to a Rust type. Narrow but real — recognized
/// shapes get real types; unknown types fall back to `None` so
/// the caller can record a `PropType` hole.
///
/// Recognized shapes:
///
/// - primitive keywords: `number` → `i32`, `boolean` → `bool`,
///   `string` → `String`.
/// - function types: `(a: T, b: U) => void` →
///   `Option<Box<dyn Fn(T, U) + Send + Sync>>`. Wrapped in
///   `Option` so the props struct still derives `Default` (None);
///   `Send + Sync` so the closure can cross thread boundaries
///   under reasonable framework backends.
fn ts_type_to_rust(t: &ast::TsType) -> Option<String> {
    match t {
        ast::TsType::TsKeywordType(k) => match k.kind {
            ast::TsKeywordTypeKind::TsNumberKeyword => Some("i32".into()),
            ast::TsKeywordTypeKind::TsBooleanKeyword => Some("bool".into()),
            ast::TsKeywordTypeKind::TsStringKeyword => Some("String".into()),
            _ => None,
        },
        ast::TsType::TsFnOrConstructorType(fc) => {
            if let ast::TsFnOrConstructorType::TsFnType(fn_ty) = fc {
                Some(ts_fn_to_rust(fn_ty))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Heuristic type for a destructured prop with no TS annotation.
/// `onClick`, `onChange`, etc. → callback type. Everything else →
/// `()` placeholder. The convention is well-established enough
/// (camelCase `on*` for event props) that the false-positive rate
/// is negligible.
fn guess_untyped_field_ty(name: &str) -> String {
    let is_event_like = name.starts_with("on")
        && name.len() > 2
        && name.chars().nth(2).map(|c| c.is_ascii_uppercase()).unwrap_or(false);
    if is_event_like {
        "Option<Box<dyn Fn() + Send + Sync>>".into()
    } else {
        "()".into()
    }
}

/// Render a TS function type as `Option<Box<dyn Fn(args) + Send + Sync>>`.
/// Args whose types are unmappable fall back to `()`; the whole
/// signature stays well-formed Rust.
fn ts_fn_to_rust(fn_ty: &ast::TsFnType) -> String {
    let args: Vec<String> = fn_ty
        .params
        .iter()
        .map(|p| {
            let ty_ann = match p {
                ast::TsFnParam::Ident(b) => b.type_ann.as_ref(),
                ast::TsFnParam::Array(a) => a.type_ann.as_ref(),
                ast::TsFnParam::Object(o) => o.type_ann.as_ref(),
                ast::TsFnParam::Rest(r) => r.type_ann.as_ref(),
            };
            ty_ann
                .and_then(|t| ts_type_to_rust(&t.type_ann))
                .unwrap_or_else(|| "()".into())
        })
        .collect();
    format!("Option<Box<dyn Fn({}) + Send + Sync>>", args.join(", "))
}

// =============================================================================
// Body statement lifting
// =============================================================================

fn lift_var_decl(
    decl: &ast::VarDecl,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &mut HashMap<String, String>,
    preamble: &mut Vec<Reactive>,
    cm: &Lrc<SourceMap>,
    report: &mut PortReport,
) {
    for declarator in &decl.decls {
        let Some(init) = &declarator.init else { continue };
        let call = match &**init {
            ast::Expr::Call(c) => c,
            _ => continue,
        };
        let callee_name = match &call.callee {
            ast::Callee::Expr(e) => match &**e {
                ast::Expr::Ident(id) => id.sym.to_string(),
                _ => continue,
            },
            _ => continue,
        };

        let binding = pattern_to_binding(&declarator.name);
        let ctx = CallContext { args: &call.args, binding };
        let Some(lift_kind) = lifter.classify_call(&callee_name, &ctx) else {
            // Unknown call in let position — port as a plain
            // function call. Custom hooks aren't a framework
            // concept; they're just functions. If a Rust
            // `fn callee_name` exists (user-written or
            // hand-ported from the source hook body), the
            // generated code links against it; otherwise the
            // build fails with a clear "function not found"
            // error pointing at the spot to address.
            let args: Vec<String> = call.args.iter()
                .map(|a| render_expr(&a.expr, prop_fields, state_idents, lifter))
                .collect();
            let expr = format!("{}({})", callee_name, args.join(", "));
            let name = match pattern_to_binding(&declarator.name) {
                Some(BindingPattern::Single { name }) => name.to_string(),
                Some(BindingPattern::Pair { name, setter }) => {
                    // Rust tuple destructure for paired returns.
                    format!("({}, {})", name, setter)
                }
                None => {
                    // The binding pattern is exotic (object
                    // destructure, etc.). Preserve the call as a
                    // statement and record an Unsupported hole so
                    // the AI pass can untangle.
                    preamble.push(Reactive::Stmt(expr));
                    report.record(Hole {
                        kind: HoleKind::Unsupported,
                        reason: format!("exotic binding pattern around `{}`", callee_name),
                        original: SourceSnippet::new(callee_name.clone()),
                    });
                    continue;
                }
            };
            preamble.push(Reactive::Let { name, expr });
            continue;
        };

        match lift_kind {
            LiftedCall::State => {
                let (name, setter) = match pattern_to_binding(&declarator.name) {
                    Some(BindingPattern::Pair { name, setter }) => {
                        (name.to_string(), setter.to_string())
                    }
                    Some(BindingPattern::Single { name }) => {
                        // Solid-style: `const count = createSignal(...)`
                        // is unusual (idiom is the pair destructure),
                        // but support it by synthesizing a setter
                        // name the lifter never actually emits.
                        (name.to_string(), format!("set{}", capitalize(name)))
                    }
                    None => {
                        report.record(Hole {
                            kind: HoleKind::Unsupported,
                            reason: format!("could not resolve binding for `{}`", callee_name),
                            original: SourceSnippet::new(callee_name),
                        });
                        continue;
                    }
                };
                let init_expr = call
                    .args
                    .first()
                    .map(|a| render_expr(&a.expr, prop_fields, state_idents, lifter))
                    .unwrap_or_else(|| "()".into());
                state_idents.insert(setter.clone(), name.clone());
                preamble.push(Reactive::State {
                    name,
                    setter,
                    init: init_expr,
                });
            }
            LiftedCall::Effect { has_deps } => {
                lift_effect_call(&call.args, has_deps, lifter, state_idents, preamble, cm, report);
            }
            LiftedCall::Inject => {
                let name = match pattern_to_binding(&declarator.name) {
                    Some(BindingPattern::Single { name }) => name.to_string(),
                    _ => {
                        report.record(Hole {
                            kind: HoleKind::Unsupported,
                            reason: format!(
                                "could not resolve binding for `{}` (context consumer)",
                                callee_name
                            ),
                            original: SourceSnippet::new(callee_name.clone()),
                        });
                        continue;
                    }
                };
                // First argument's identifier is the React context
                // value, which we adopt verbatim as the Rust type
                // name. `useContext(ThemeContext)` →
                // `inject::<ThemeContext>()`. The user defines a
                // matching Rust type (typically by porting the
                // `createContext<T>(default)` declaration into a
                // `pub struct ThemeContext { … }`).
                let type_name = call.args.first().and_then(|a| match &*a.expr {
                    ast::Expr::Ident(i) => Some(i.sym.to_string()),
                    _ => None,
                });
                let expr = match type_name {
                    Some(t) => format!("inject::<{}>()", t),
                    None => {
                        report.record(Hole {
                            kind: HoleKind::PropType,
                            reason: format!(
                                "non-identifier context argument to `{}`",
                                callee_name
                            ),
                            original: SourceSnippet::new(callee_name.clone()),
                        });
                        "inject::<()>()".into()
                    }
                };
                preamble.push(Reactive::Let { name, expr });
            }
            LiftedCall::Drop => {}
        }
    }
}

fn lift_expr_stmt(
    expr: &ast::Expr,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    preamble: &mut Vec<Reactive>,
    cm: &Lrc<SourceMap>,
    report: &mut PortReport,
) {
    let call = match expr {
        ast::Expr::Call(c) => c,
        _ => return,
    };
    let callee_name = match &call.callee {
        ast::Callee::Expr(e) => match &**e {
            ast::Expr::Ident(id) => id.sym.to_string(),
            _ => return,
        },
        _ => return,
    };
    let ctx = CallContext { args: &call.args, binding: None };
    let Some(lift_kind) = lifter.classify_call(&callee_name, &ctx) else {
        // Unknown call at statement position — port as a plain
        // expression statement. `useImperativeHandle(ref, …)` and
        // similar end up here.
        let args: Vec<String> = call.args.iter()
            .map(|a| render_expr(&a.expr, prop_fields, state_idents, lifter))
            .collect();
        preamble.push(Reactive::Stmt(format!("{}({})", callee_name, args.join(", "))));
        return;
    };
    match lift_kind {
        LiftedCall::Effect { has_deps } => {
            let mut deps_owned = state_idents.clone();
            lift_effect_call(&call.args, has_deps, lifter, &mut deps_owned, preamble, cm, report);
        }
        LiftedCall::Drop => {}
        LiftedCall::State | LiftedCall::Inject => {
            // useState / useContext at statement position is
            // meaningless (return value would be discarded) — skip.
        }
    }
}

fn lift_effect_call(
    args: &[ast::ExprOrSpread],
    has_deps: bool,
    _lifter: &dyn Lifter,
    state_idents: &mut HashMap<String, String>,
    preamble: &mut Vec<Reactive>,
    cm: &Lrc<SourceMap>,
    report: &mut PortReport,
) {
    let Some(first) = args.first() else { return };

    // Split the effect arrow body into setup + (optional) cleanup
    // so React's `return () => …`, Solid's `onCleanup(…)`, and
    // Vue's `watchEffect((onCleanup) => … onCleanup(…))` all
    // surface their cleanup as `on_cleanup(move || …);` inside
    // the emitted `effect!({ … })`.
    let shape = extract_effect_shape(&first.expr);
    let line = span_line(first.expr.span(), cm);

    let setup_hole = Hole {
        kind: HoleKind::HandlerBody,
        reason: "effect setup body — JS imperative code, AI pass needed".into(),
        original: SourceSnippet::at(shape.setup_summary, line),
    };
    report.record(setup_hole.clone());

    let mut body_text = port_core::render_inline_hole(&setup_hole);

    if let Some(cleanup_summary) = shape.cleanup_summary {
        let cleanup_hole = Hole {
            kind: HoleKind::HandlerBody,
            reason: "effect cleanup body — JS imperative code, AI pass needed".into(),
            original: SourceSnippet::at(cleanup_summary, line),
        };
        report.record(cleanup_hole.clone());
        // The framework's on_cleanup helper registers a callback
        // that fires before the effect's next re-run and on
        // disposal. Pair the setup hole with a cleanup line so
        // both halves are visible to the AI / human reviewer.
        body_text.push_str("\n");
        body_text.push_str(&format!(
            "on_cleanup(move || {{ {} }});",
            port_core::render_inline_hole(&cleanup_hole)
        ));
    }

    let deps = if has_deps {
        args.get(1).and_then(|a| match &*a.expr {
            ast::Expr::Array(arr) => Some(
                arr.elems
                    .iter()
                    .filter_map(|el| el.as_ref().and_then(|e| match &*e.expr {
                        ast::Expr::Ident(i) => Some(i.sym.to_string()),
                        _ => None,
                    }))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
    } else {
        None
    };

    let _ = &state_idents;

    preamble.push(Reactive::Effect {
        body: SourceSnippet::new(body_text),
        deps,
    });
}

/// Split an effect arrow body into a setup summary + optional
/// cleanup summary.
///
/// Recognized cleanup shapes:
///
/// - **React**: `return () => …;` (or `return function() { … }`)
///   anywhere in the body. The setup is everything *before* the
///   return statement.
/// - **Solid / Vue**: an `onCleanup(arrow)` call statement.
///   The setup is the remaining statements; the cleanup is
///   the arrow body passed to `onCleanup`.
struct EffectShape {
    setup_summary: String,
    cleanup_summary: Option<String>,
}

fn extract_effect_shape(expr: &ast::Expr) -> EffectShape {
    let ast::Expr::Arrow(arrow) = expr else {
        return EffectShape {
            setup_summary: "…non-arrow effect body…".into(),
            cleanup_summary: None,
        };
    };
    match &*arrow.body {
        ast::BlockStmtOrExpr::Expr(e) => EffectShape {
            setup_summary: render_call_summary(e),
            cleanup_summary: None,
        },
        ast::BlockStmtOrExpr::BlockStmt(b) => {
            let mut setup_stmts: Vec<&ast::Stmt> = Vec::new();
            let mut cleanup: Option<String> = None;
            for stmt in &b.stmts {
                // React: `return <arrow>`
                if let ast::Stmt::Return(r) = stmt {
                    if let Some(arg) = &r.arg {
                        if let Some(body_summary) = arrow_or_fn_body_summary(arg) {
                            cleanup = Some(body_summary);
                        }
                    }
                    break;
                }
                // Solid / Vue: `onCleanup(arrow);` statement.
                if let ast::Stmt::Expr(es) = stmt {
                    if let Some(body_summary) = on_cleanup_call_arg_summary(&es.expr) {
                        cleanup = Some(body_summary);
                        continue;
                    }
                }
                setup_stmts.push(stmt);
            }
            let setup_summary = summarize_stmts(&setup_stmts);
            EffectShape { setup_summary, cleanup_summary: cleanup }
        }
    }
}

/// Returns a body summary if `expr` is an arrow or function
/// expression (i.e. a cleanup callback).
fn arrow_or_fn_body_summary(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Arrow(a) => Some(match &*a.body {
            ast::BlockStmtOrExpr::Expr(e) => render_call_summary(e),
            ast::BlockStmtOrExpr::BlockStmt(b) => summarize_stmts(&b.stmts.iter().collect::<Vec<_>>()),
        }),
        ast::Expr::Fn(f) => Some(match f.function.body.as_ref() {
            Some(b) => summarize_stmts(&b.stmts.iter().collect::<Vec<_>>()),
            None => "…empty cleanup fn…".into(),
        }),
        _ => None,
    }
}

/// Returns a cleanup body summary if `expr` is a call to a
/// cleanup-registration function (Solid's `onCleanup(arrow)` or
/// Vue's `onCleanup(arrow)` via the watchEffect param).
fn on_cleanup_call_arg_summary(expr: &ast::Expr) -> Option<String> {
    let ast::Expr::Call(c) = expr else { return None };
    let ast::Callee::Expr(callee) = &c.callee else { return None };
    let ast::Expr::Ident(id) = &**callee else { return None };
    // The bare name `onCleanup` is the canonical spelling in both
    // Solid (imported from `solid-js`) and Vue (the parameter
    // name on `watchEffect((onCleanup) => …)`). We accept any
    // call site with that ident as a cleanup registration; if a
    // codebase aliases it, those calls fall through to the
    // generic setup hole (still correct, just less specific).
    if id.sym.as_ref() == "onCleanup" {
        let arg = c.args.first()?;
        arrow_or_fn_body_summary(&arg.expr)
    } else {
        None
    }
}

fn summarize_stmts(stmts: &[&ast::Stmt]) -> String {
    if stmts.is_empty() {
        return "…empty…".into();
    }
    if stmts.len() == 1 {
        if let ast::Stmt::Expr(es) = stmts[0] {
            return render_call_summary(&es.expr);
        }
    }
    "…multi-statement body…".into()
}

fn render_call_summary(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::Call(c) => {
            let callee = match &c.callee {
                ast::Callee::Expr(e) => render_callee(e),
                _ => "?".into(),
            };
            let args: Vec<String> = c.args.iter().map(|a| render_arg(&a.expr)).collect();
            format!("{}({});", callee, args.join(", "))
        }
        _ => "…expression…".into(),
    }
}

fn render_callee(e: &ast::Expr) -> String {
    match e {
        ast::Expr::Ident(i) => i.sym.to_string(),
        ast::Expr::Member(m) => {
            let obj = render_callee(&m.obj);
            let prop = match &m.prop {
                ast::MemberProp::Ident(i) => i.sym.to_string(),
                _ => "?".into(),
            };
            format!("{}.{}", obj, prop)
        }
        _ => "?".into(),
    }
}

fn render_arg(e: &ast::Expr) -> String {
    match e {
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("\"{}\"", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Ident(i) => i.sym.to_string(),
        ast::Expr::Call(c) => {
            // Solid's `count()` reads. Render as a call so the hole
            // message keeps the original syntax.
            let callee = match &c.callee {
                ast::Callee::Expr(e) => render_callee(e),
                _ => "?".into(),
            };
            format!("{}()", callee)
        }
        ast::Expr::Member(m) => render_callee(&ast::Expr::Member(m.clone())),
        _ => "…".into(),
    }
}

// =============================================================================
// JSX lifting
// =============================================================================

fn lift_return_jsx(
    expr: &ast::Expr,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    report: &mut PortReport,
) -> JsxNode {
    // Allow `(<JSX>)` parenthesization.
    let inner = match expr {
        ast::Expr::Paren(p) => &*p.expr,
        other => other,
    };
    match inner {
        ast::Expr::JSXElement(el) => {
            jsx_element_to_node(el, lifter, prop_fields, state_idents, report)
        }
        ast::Expr::JSXFragment(f) => {
            // Fragments lower to a Vec of children — we don't have
            // a proper fragment IR node; wrap in a View for now.
            let mut kids: Vec<JsxNode> = f
                .children
                .iter()
                .map(|c| jsx_child_to_node(c, lifter, prop_fields, state_idents, report))
                .collect();
            kids = coalesce_children(kids);
            JsxNode::Element { name: "View".into(), attrs: vec![], children: Some(kids) }
        }
        _ => JsxNode::Hole(Hole {
            kind: HoleKind::Unsupported,
            reason: "return statement is not JSX".into(),
            original: SourceSnippet::new("…return…"),
        }),
    }
}

fn jsx_element_to_node(
    el: &ast::JSXElement,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    report: &mut PortReport,
) -> JsxNode {
    let name = jsx_name(&el.opening.name);

    // Context providers: `<Ctx.Provider value={v}>children</Ctx.Provider>`.
    // Lower to a Rust block `{ provide(v); jsx!{ children… } }` so
    // the provided value lives in the consuming subtree's scope.
    // The type-keyed `provide`/`inject` model means the
    // `Ctx.Provider` wrapper itself collapses entirely — there's
    // no wrapper element in the output tree, just the scope-local
    // provision.
    if let Some(ctx_name) = is_provider_element(&el.opening.name) {
        return lower_provider(el, &ctx_name, lifter, prop_fields, state_idents, report);
    }

    let attrs = el
        .opening
        .attrs
        .iter()
        .filter_map(|a| match a {
            ast::JSXAttrOrSpread::JSXAttr(attr) => {
                Some(jsx_attr_to_ir(attr, lifter, prop_fields, state_idents, report))
            }
            ast::JSXAttrOrSpread::SpreadElement(s) => {
                // idealyst's `jsx!` has no spread equivalent — but
                // silently dropping the spread means the AI pass
                // never sees it. Emit a placeholder attribute with
                // a `todo!()` value, naming it `_spread` so it's
                // searchable in the output. The `jsx!` macro will
                // reject the unknown attr name, surfacing the TODO
                // at compile time rather than at runtime.
                let original = render_expr(&s.expr, prop_fields, state_idents, lifter);
                let hole = Hole {
                    kind: HoleKind::AttributeValue,
                    reason: "JSX spread `{...x}` has no idealyst equivalent".into(),
                    original: SourceSnippet::new(format!("{{...{}}}", original)),
                };
                // Record in the report too — the embedded
                // JsxAttrValue::Hole alone wouldn't show up in
                // the project-level hole histogram.
                report.record(hole.clone());
                Some(JsxAttr {
                    name: "_spread".into(),
                    value: JsxAttrValue::Hole(hole),
                })
            }
        })
        .collect::<Vec<_>>();
    let children = if el.opening.self_closing {
        None
    } else {
        let raw: Vec<JsxNode> = el
            .children
            .iter()
            .map(|c| jsx_child_to_node(c, lifter, prop_fields, state_idents, report))
            .collect();
        Some(coalesce_children(raw))
    };
    JsxNode::Element { name, attrs, children }
}

/// Collapse adjacent JSX text + expression children into a single
/// `format!(...)` expression. JSX source like
/// `<Text>Count: {count}</Text>` parses as three children
/// (text "Count: ", expr {count}, text "") which the porter would
/// otherwise emit as separate sibling nodes — wrong for the
/// `Text` semantics. We coalesce to one `format!("Count: {}", …)`.
///
/// Also drops empty / whitespace-only Text nodes that JSX emits
/// between sibling elements.
fn coalesce_children(children: Vec<JsxNode>) -> Vec<JsxNode> {
    // Drop empty Text nodes (JSX whitespace).
    let cleaned: Vec<JsxNode> = children
        .into_iter()
        .filter(|c| !matches!(c, JsxNode::Text(s) if s.is_empty()))
        .collect();

    // If the cleaned children are all Text/Expr (no elements), and
    // there's at least one Expr, coalesce into one format!().
    let textlike = cleaned.iter().all(|c| matches!(c, JsxNode::Text(_) | JsxNode::Expr(_)));
    let any_expr = cleaned.iter().any(|c| matches!(c, JsxNode::Expr(_)));
    if textlike && any_expr && !cleaned.is_empty() {
        let mut fmt = String::new();
        let mut args: Vec<String> = Vec::new();
        for c in &cleaned {
            match c {
                JsxNode::Text(s) => fmt.push_str(s),
                JsxNode::Expr(e) => {
                    fmt.push_str("{}");
                    args.push(e.clone());
                }
                _ => unreachable!(),
            }
        }
        let combined = if args.is_empty() {
            // All-text case shouldn't hit because any_expr was true.
            format!("\"{}\"", fmt)
        } else {
            format!("format!(\"{}\", {})", fmt, args.join(", "))
        };
        return vec![JsxNode::Expr(combined)];
    }
    cleaned
}

/// Render an object literal as a Rust struct construction
/// `StructName { field: value, … }`. Used by [`lower_provider`]
/// to turn `<Ctx.Provider value={{ a, b: c }}>` into
/// `provide(Ctx { a: props.a, b: c });`.
///
/// Shorthand (`{ accent }`) prop entries become `accent` when the
/// rewritten value matches the key (Rust struct shorthand), or
/// `accent: props.accent` when the renderer routed the ident
/// through prop rewriting.
fn render_object_as_struct(
    obj: &ast::ObjectLit,
    struct_name: &str,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    lifter: &dyn Lifter,
) -> String {
    let mut fields: Vec<String> = Vec::new();
    for p in &obj.props {
        let prop = match p {
            ast::PropOrSpread::Prop(b) => b,
            ast::PropOrSpread::Spread(_) => {
                fields.push("/* spread props */".into());
                continue;
            }
        };
        match &**prop {
            ast::Prop::Shorthand(i) => {
                let key = i.sym.to_string();
                let ident_expr = ast::Expr::Ident(i.clone());
                let value = render_expr(&ident_expr, prop_fields, state_idents, lifter);
                if value == key {
                    fields.push(key);
                } else {
                    fields.push(format!("{}: {}", key, value));
                }
            }
            ast::Prop::KeyValue(kv) => {
                let key = match &kv.key {
                    ast::PropName::Ident(i) => i.sym.to_string(),
                    ast::PropName::Str(s) => s.value.to_atom_lossy().to_string(),
                    _ => "?".into(),
                };
                let value = render_expr(&kv.value, prop_fields, state_idents, lifter);
                fields.push(format!("{}: {}", key, value));
            }
            _ => fields.push("/* unsupported prop */".into()),
        }
    }
    if fields.is_empty() {
        format!("{} {{}}", struct_name)
    } else {
        format!("{} {{ {} }}", struct_name, fields.join(", "))
    }
}

/// True iff this element name is a context provider — i.e. a
/// member expression whose final property is `Provider`. Returns
/// the root object name (the React context value's ident), which
/// idealyst treats as the *type* under `provide`/`inject`.
fn is_provider_element(n: &ast::JSXElementName) -> Option<String> {
    let ast::JSXElementName::JSXMemberExpr(m) = n else { return None };
    if m.prop.sym.as_ref() != "Provider" { return None }
    // The object is either a bare ident (`Ctx.Provider`) or a
    // deeper chain (`Foo.Ctx.Provider`). We use whatever the
    // immediate `.Provider`-bearing object is named; that matches
    // the React idiom where `Ctx` is the context value.
    Some(match &m.obj {
        ast::JSXObject::Ident(i) => i.sym.to_string(),
        ast::JSXObject::JSXMemberExpr(inner) => inner.prop.sym.to_string(),
    })
}

/// Lower `<Ctx.Provider value={v}>…</Ctx.Provider>` to a Rust
/// block `{ provide(v); jsx!{children…} }`. The block is wrapped
/// in `JsxNode::Expr` so the outer `jsx!` accepts it as a
/// `{expr}` child evaluating to a `Primitive`.
fn lower_provider(
    el: &ast::JSXElement,
    ctx_name: &str,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    report: &mut PortReport,
) -> JsxNode {
    // Find the `value={…}` attribute. Other attributes on a
    // Provider are unusual but possible; if present they're
    // ignored — record a hole each so the AI pass sees them.
    let mut value_expr: Option<String> = None;
    for a in &el.opening.attrs {
        let ast::JSXAttrOrSpread::JSXAttr(attr) = a else {
            report.record(Hole {
                kind: HoleKind::AttributeValue,
                reason: format!("spread on `{}.Provider` — dropped", ctx_name),
                original: SourceSnippet::new("{...spread}"),
            });
            continue;
        };
        let attr_name = match &attr.name {
            ast::JSXAttrName::Ident(i) => i.sym.to_string(),
            _ => continue,
        };
        if attr_name == "value" {
            value_expr = match &attr.value {
                Some(ast::JSXAttrValue::JSXExprContainer(c)) => match &c.expr {
                    ast::JSXExpr::Expr(e) => {
                        // Special case: an object literal as the
                        // Provider's value renders as a struct
                        // construction `Ctx { field: val, … }`.
                        // We know the type (it's the provider's
                        // identifier) so we can produce a
                        // compilable expression instead of falling
                        // through to "unsupported expr."
                        if let ast::Expr::Object(obj) = &**e {
                            Some(render_object_as_struct(
                                obj, ctx_name, prop_fields, state_idents, lifter,
                            ))
                        } else {
                            Some(render_expr(e, prop_fields, state_idents, lifter))
                        }
                    }
                    ast::JSXExpr::JSXEmptyExpr(_) => None,
                },
                Some(ast::JSXAttrValue::Str(s)) => Some(format!("\"{}\"", s.value.to_atom_lossy())),
                _ => None,
            };
        } else {
            report.record(Hole {
                kind: HoleKind::AttributeValue,
                reason: format!("non-`value` attr `{}` on `{}.Provider` — dropped", attr_name, ctx_name),
                original: SourceSnippet::new(attr_name),
            });
        }
    }
    let value_expr = value_expr.unwrap_or_else(|| {
        report.record(Hole {
            kind: HoleKind::AttributeValue,
            reason: format!("`{}.Provider` had no `value` attribute", ctx_name),
            original: SourceSnippet::new(format!("<{}.Provider>", ctx_name)),
        });
        format!("todo!(\"port: missing `value` on `{}.Provider`\")", ctx_name)
    });

    // Walk children and render each via inline JSX so the
    // resulting block has a single Primitive return.
    let kids: Vec<JsxNode> = el
        .children
        .iter()
        .map(|c| jsx_child_to_node(c, lifter, prop_fields, state_idents, report))
        .collect();
    let kids = coalesce_children(kids);

    let children_block = if kids.len() == 1 {
        port_core::emit::render_node_inline(&kids[0])
    } else {
        // Multiple children: wrap in `jsx! { <>…</> }` so the
        // block still yields a single value the framework can
        // accept.
        let frag = JsxNode::Element {
            name: "View".into(),
            attrs: vec![],
            children: Some(kids),
        };
        port_core::emit::render_node_inline(&frag)
    };

    JsxNode::Expr(format!(
        "{{ provide({value}); {children} }}",
        value = value_expr,
        children = children_block,
    ))
}

fn jsx_name(n: &ast::JSXElementName) -> String {
    match n {
        ast::JSXElementName::Ident(i) => i.sym.to_string(),
        ast::JSXElementName::JSXMemberExpr(m) => {
            let prop = m.prop.sym.to_string();
            let obj = jsx_object(&m.obj);
            format!("{}.{}", obj, prop)
        }
        ast::JSXElementName::JSXNamespacedName(n) => {
            format!("{}:{}", n.ns.sym, n.name.sym)
        }
    }
}

fn jsx_object(o: &ast::JSXObject) -> String {
    match o {
        ast::JSXObject::Ident(i) => i.sym.to_string(),
        ast::JSXObject::JSXMemberExpr(m) => {
            let prop = m.prop.sym.to_string();
            let obj = jsx_object(&m.obj);
            format!("{}.{}", obj, prop)
        }
    }
}

fn jsx_attr_to_ir(
    attr: &ast::JSXAttr,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    _report: &mut PortReport,
) -> JsxAttr {
    let name = match &attr.name {
        ast::JSXAttrName::Ident(i) => i.sym.to_string(),
        ast::JSXAttrName::JSXNamespacedName(n) => format!("{}:{}", n.ns.sym, n.name.sym),
    };
    // React/Solid use camelCase for events (`onClick`); idealyst
    // uses snake_case (`on_click`). Mechanical rename.
    let name = rename_event_attr(&name);
    let value = match &attr.value {
        None => JsxAttrValue::StringLit(String::new()),
        Some(ast::JSXAttrValue::Str(s)) => {
            JsxAttrValue::StringLit(s.value.to_atom_lossy().to_string())
        }
        Some(ast::JSXAttrValue::JSXExprContainer(c)) => match &c.expr {
            ast::JSXExpr::Expr(e) => {
                JsxAttrValue::Expr(render_expr(e, prop_fields, state_idents, lifter))
            }
            ast::JSXExpr::JSXEmptyExpr(_) => JsxAttrValue::StringLit(String::new()),
        },
        Some(_) => JsxAttrValue::Hole(Hole {
            kind: HoleKind::AttributeValue,
            reason: "unsupported attribute value shape".into(),
            original: SourceSnippet::new("…"),
        }),
    };
    JsxAttr { name, value }
}

fn rename_event_attr(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("on") {
        if rest.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            // onClick → on_click
            let mut out = String::from("on");
            for ch in rest.chars() {
                if ch.is_uppercase() {
                    out.push('_');
                    out.push(ch.to_ascii_lowercase());
                } else {
                    out.push(ch);
                }
            }
            return out;
        }
    }
    name.to_string()
}

fn jsx_child_to_node(
    c: &ast::JSXElementChild,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    report: &mut PortReport,
) -> JsxNode {
    match c {
        ast::JSXElementChild::JSXText(t) => {
            // Standard JSX whitespace rules: drop the chunk entirely
            // if it's all whitespace; otherwise collapse leading /
            // trailing newline-adjacent whitespace but preserve
            // internal spaces. Trailing spaces before `{expr}` and
            // leading spaces after are content, not formatting.
            let raw = t.value.as_ref();
            if raw.trim().is_empty() {
                JsxNode::Text(String::new())
            } else {
                JsxNode::Text(normalize_jsx_text(raw))
            }
        }
        ast::JSXElementChild::JSXElement(e) => {
            jsx_element_to_node(e, lifter, prop_fields, state_idents, report)
        }
        ast::JSXElementChild::JSXExprContainer(c) => match &c.expr {
            ast::JSXExpr::Expr(e) => {
                // Special case: ternary with JSX branches in child
                // position (`{isEditing ? <A/> : <B/>}` — the
                // dominant ternary shape in real React). Emit as
                // a Rust `if`-expression whose branches are
                // recursive `jsx! {}` invocations producing
                // Primitive values.
                if let ast::Expr::Cond(cond) = &**e {
                    if branch_contains_jsx(&cond.cons) || branch_contains_jsx(&cond.alt) {
                        return lower_jsx_cond_child(cond, lifter, prop_fields, state_idents, report);
                    }
                }
                JsxNode::Expr(render_expr(e, prop_fields, state_idents, lifter))
            }
            ast::JSXExpr::JSXEmptyExpr(_) => JsxNode::Text(String::new()),
        },
        ast::JSXElementChild::JSXFragment(_) => JsxNode::Hole(Hole {
            kind: HoleKind::Unsupported,
            reason: "JSX fragment in child position".into(),
            original: SourceSnippet::new("<>…</>"),
        }),
        _ => JsxNode::Hole(Hole {
            kind: HoleKind::Unsupported,
            reason: "unrecognized JSX child".into(),
            original: SourceSnippet::new("…"),
        }),
    }
}

/// True if `e` is a JSX element/fragment directly or through one
/// level of parens. Used to decide whether to take the
/// JSX-branch-aware Cond path or the plain expression path.
fn branch_contains_jsx(e: &ast::Expr) -> bool {
    match e {
        ast::Expr::JSXElement(_) | ast::Expr::JSXFragment(_) => true,
        ast::Expr::Paren(p) => branch_contains_jsx(&p.expr),
        _ => false,
    }
}

/// Lower a JSX-branch ternary to a single `JsxNode::Expr` whose
/// text is `if cond { jsx!{…} } else { jsx!{…} }`. Each branch is
/// recursively lifted to a `JsxNode` and then rendered via
/// `port_core::emit::render_node_inline`, producing a complete
/// `jsx!` invocation that the outer macro will accept as a
/// `{expr}` child.
fn lower_jsx_cond_child(
    cond: &ast::CondExpr,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    report: &mut PortReport,
) -> JsxNode {
    let test = render_expr(&cond.test, prop_fields, state_idents, lifter);
    let cons = render_branch(&cond.cons, lifter, prop_fields, state_idents, report);
    let alt = render_branch(&cond.alt, lifter, prop_fields, state_idents, report);
    JsxNode::Expr(format!("if {} {{ {} }} else {{ {} }}", test, cons, alt))
}

/// Render a single ternary branch. JSX → recursive
/// `port_core::emit::render_node_inline`. Non-JSX → the regular
/// expression renderer (which handles literals, idents, calls).
fn render_branch(
    e: &ast::Expr,
    lifter: &dyn Lifter,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    report: &mut PortReport,
) -> String {
    match e {
        ast::Expr::Paren(p) => render_branch(&p.expr, lifter, prop_fields, state_idents, report),
        ast::Expr::JSXElement(el) => {
            let node = jsx_element_to_node(el, lifter, prop_fields, state_idents, report);
            port_core::emit::render_node_inline(&node)
        }
        ast::Expr::JSXFragment(_) => {
            // Fragments in ternary branches: emit a placeholder
            // hole that's still syntactically valid in Rust expr
            // position.
            "todo!(\"port: JSX fragment in ternary branch\")".into()
        }
        _ => render_expr(e, prop_fields, state_idents, lifter),
    }
}

// =============================================================================
// Expression rendering
// =============================================================================

/// Render an swc `Expr` to a Rust expression string, applying:
///
/// - prop rewriting (bare ident referring to a destructured prop
///   field → `props.X`),
/// - state signal-read rewriting (bare ident → `.get()` for React,
///   `name()` → `name.get()` for Solid),
/// - setter call rewriting (`setCount(expr)` → `count.set(expr)`).
fn render_expr(
    e: &ast::Expr,
    prop_fields: &HashSet<String>,
    state_idents: &HashMap<String, String>,
    lifter: &dyn Lifter,
) -> String {
    let state_names: HashSet<&str> = state_idents.values().map(|s| s.as_str()).collect();
    render_expr_inner(e, prop_fields, &state_names, state_idents, lifter)
}

fn render_expr_inner(
    e: &ast::Expr,
    prop_fields: &HashSet<String>,
    state_names: &HashSet<&str>,
    state_idents: &HashMap<String, String>,
    lifter: &dyn Lifter,
) -> String {
    match e {
        ast::Expr::Ident(i) => {
            let name = i.sym.to_string();
            if state_names.contains(name.as_str()) {
                format!("{}.get()", name)
            } else if prop_fields.contains(&name) {
                // Destructured prop or bare prop-field reference.
                format!("props.{}", name)
            } else {
                name
            }
        }
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("\"{}\"", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Bool(b)) => b.value.to_string(),
        ast::Expr::Bin(b) => {
            let l = render_expr_inner(&b.left, prop_fields, state_names, state_idents, lifter);
            let r = render_expr_inner(&b.right, prop_fields, state_names, state_idents, lifter);
            // `a ?? b` — nullish coalescing. No clean Rust analog
            // without Option modeling. Most ports of `props.x ?? D`
            // are already covered by the prop's `#[component(default)]`
            // so dropping the fallback is usually correct; emit
            // the LHS with the original RHS in a comment so the
            // AI pass can decide.
            if matches!(b.op, ast::BinaryOp::NullishCoalescing) {
                return format!("{} /* ?? {} (verify default covers) */", l, r);
            }
            format!("{} {} {}", l, binop_str(b.op), r)
        }
        ast::Expr::Call(c) => {
            if let ast::Callee::Expr(callee) = &c.callee {
                if let ast::Expr::Ident(id) = &**callee {
                    let n = id.sym.to_string();
                    // Solid signal read: `count()` for state idents.
                    if state_names.contains(n.as_str()) && c.args.is_empty()
                        && lifter.signal_read_style() == ReadStyle::CallExpression
                    {
                        return format!("{}.get()", n);
                    }
                    // Setter call: `setCount(expr)` → `count.set(expr)`.
                    if let Some(state_name) = state_idents.get(&n) {
                        let arg = c
                            .args
                            .first()
                            .map(|a| {
                                render_expr_inner(&a.expr, prop_fields, state_names, state_idents, lifter)
                            })
                            .unwrap_or_default();
                        return format!("{}.set({})", state_name, arg);
                    }
                }
            }
            // Generic call: render callee + args through the
            // expression renderer so prop rewriting applies to
            // both (`onToggle(id)` → `props.onToggle(props.id)`)
            // and arguments are preserved (don't drop them as a
            // literal ellipsis).
            let callee = match &c.callee {
                ast::Callee::Expr(e) => render_expr_inner(e, prop_fields, state_names, state_idents, lifter),
                _ => "?".into(),
            };
            let args: Vec<String> = c
                .args
                .iter()
                .map(|a| render_expr_inner(&a.expr, prop_fields, state_names, state_idents, lifter))
                .collect();
            format!("{}({})", callee, args.join(", "))
        }
        ast::Expr::Arrow(arrow) => render_arrow(arrow, prop_fields, state_names, state_idents, lifter),
        ast::Expr::Member(m) => {
            // Recurse so nested member chains (`e.target.value`,
            // `props.foo.bar`) render correctly instead of bottoming
            // out at "?". The recursion goes through `render_expr_inner`
            // so prop rewriting still applies to the innermost ident.
            let obj = render_expr_inner(&m.obj, prop_fields, state_names, state_idents, lifter);
            let prop = match &m.prop {
                ast::MemberProp::Ident(i) => i.sym.to_string(),
                _ => "?".into(),
            };
            format!("{}.{}", obj, prop)
        }
        ast::Expr::Paren(p) => {
            render_expr_inner(&p.expr, prop_fields, state_names, state_idents, lifter)
        }
        // Ternary `cond ? cons : alt` → Rust if-expression.
        // Works directly for attribute-value positions where all
        // three subexpressions are simple. JSX branches degrade
        // to `/* unsupported expr */` here; the child-position
        // walker has a separate path that handles JSX branches
        // by emitting recursive `jsx!` invocations.
        ast::Expr::Cond(c) => {
            let test = render_expr_inner(&c.test, prop_fields, state_names, state_idents, lifter);
            let cons = render_expr_inner(&c.cons, prop_fields, state_names, state_idents, lifter);
            let alt = render_expr_inner(&c.alt, prop_fields, state_names, state_idents, lifter);
            format!("if {} {{ {} }} else {{ {} }}", test, cons, alt)
        }
        // Unary `!`, `-`, etc. — frequent in `if (!isEditing)` style.
        ast::Expr::Unary(u) => {
            let inner = render_expr_inner(&u.arg, prop_fields, state_names, state_idents, lifter);
            let op = match u.op {
                ast::UnaryOp::Bang => "!",
                ast::UnaryOp::Minus => "-",
                ast::UnaryOp::Plus => "",
                _ => "/*unary*/",
            };
            format!("{}{}", op, inner)
        }
        _ => "/* unsupported expr */".into(),
    }
}

fn render_arrow(
    arrow: &ast::ArrowExpr,
    prop_fields: &HashSet<String>,
    state_names: &HashSet<&str>,
    state_idents: &HashMap<String, String>,
    lifter: &dyn Lifter,
) -> String {
    let body = match &*arrow.body {
        ast::BlockStmtOrExpr::Expr(e) => {
            render_expr_inner(e, prop_fields, state_names, state_idents, lifter)
        }
        ast::BlockStmtOrExpr::BlockStmt(b) => {
            if b.stmts.len() == 1 {
                if let ast::Stmt::Expr(es) = &b.stmts[0] {
                    render_expr_inner(&es.expr, prop_fields, state_names, state_idents, lifter)
                } else {
                    "/* multi-stmt arrow */".into()
                }
            } else {
                "/* multi-stmt arrow */".into()
            }
        }
    };
    format!("move || {}", body)
}

/// Convert an swc `Span` to a 1-based line number using the
/// `SourceMap`. Used to populate `Hole::original.line`.
fn span_line(span: Span, cm: &Lrc<SourceMap>) -> u32 {
    cm.lookup_char_pos(span.lo).line as u32
}

/// Normalize JSX text: a chunk like "\n    Count: " (leading newline
/// + indent before content) is just "Count: " semantically. We trim
/// only leading/trailing *runs* containing a newline, preserving
/// non-newline trailing spaces (which are meaningful before `{expr}`).
fn normalize_jsx_text(raw: &str) -> String {
    let trimmed_start = match raw.find('\n') {
        Some(_) => {
            // Find the first non-whitespace char after the last
            // newline in the leading whitespace.
            let mut last_nl = 0;
            for (i, c) in raw.char_indices() {
                if c == '\n' {
                    last_nl = i + 1;
                } else if !c.is_whitespace() {
                    break;
                }
            }
            &raw[last_nl..]
        }
        None => raw,
    };
    let trimmed = match trimmed_start.rfind('\n') {
        Some(idx) => {
            // Anything after the last newline that is all whitespace
            // is stripped; otherwise keep up to and including that
            // content.
            let tail = &trimmed_start[idx + 1..];
            if tail.chars().all(|c| c.is_whitespace()) {
                &trimmed_start[..idx]
            } else {
                trimmed_start
            }
        }
        None => trimmed_start,
    };
    trimmed.to_string()
}

fn binop_str(op: ast::BinaryOp) -> &'static str {
    match op {
        ast::BinaryOp::Add => "+",
        ast::BinaryOp::Sub => "-",
        ast::BinaryOp::Mul => "*",
        ast::BinaryOp::Div => "/",
        ast::BinaryOp::Mod => "%",
        ast::BinaryOp::EqEq | ast::BinaryOp::EqEqEq => "==",
        ast::BinaryOp::NotEq | ast::BinaryOp::NotEqEq => "!=",
        ast::BinaryOp::Lt => "<",
        ast::BinaryOp::LtEq => "<=",
        ast::BinaryOp::Gt => ">",
        ast::BinaryOp::GtEq => ">=",
        ast::BinaryOp::LogicalAnd => "&&",
        ast::BinaryOp::LogicalOr => "||",
        _ => "/*op*/",
    }
}

fn render_default_expr(e: &ast::Expr) -> String {
    match e {
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("\"{}\"", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Bool(b)) => b.value.to_string(),
        _ => "Default::default()".into(),
    }
}

// =============================================================================
// Small helpers
// =============================================================================

fn pattern_to_binding(p: &ast::Pat) -> Option<BindingPattern<'_>> {
    match p {
        ast::Pat::Array(arr) => {
            if arr.elems.len() == 2 {
                let first = arr.elems[0].as_ref()?;
                let second = arr.elems[1].as_ref()?;
                let name = match first {
                    ast::Pat::Ident(b) => b.id.sym.as_str(),
                    _ => return None,
                };
                let setter = match second {
                    ast::Pat::Ident(b) => b.id.sym.as_str(),
                    _ => return None,
                };
                Some(BindingPattern::Pair { name, setter })
            } else {
                None
            }
        }
        ast::Pat::Ident(b) => Some(BindingPattern::Single { name: b.id.sym.as_str() }),
        _ => None,
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
