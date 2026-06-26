//! Svelte `<script>` walker.
//!
//! Svelte's reactivity rules:
//!
//! - `export let foo = default;` — declares a prop. Lowers to a
//!   prop field with the given default.
//! - `let foo = expr;` — declares a top-level binding. Reactive
//!   iff *anything* reassigns it later (in functions, in `$:`,
//!   in event handlers). For our scope we assume any non-prop
//!   top-level let is reactive (`signal!`) — Svelte's compiler
//!   does a usage analysis but our fixtures don't have const
//!   lets, and over-promoting to a signal is harmless.
//! - `$: expr;` — reactive statement. If `expr` is an assignment
//!   (`x = y`), lowers to a derived signal; otherwise lowers to
//!   `effect!({ … })`.
//! - `function name() { … }` — handler. Recorded for the markup
//!   walker to substitute when an `on:click={name}` references it.

use port_core::ir::*;
use port_tsx::ast;
use std::collections::HashMap;

pub struct ScriptWalkResult {
    pub props: PropsType,
    pub preamble: Vec<Reactive>,
    pub handler_fns: HashMap<String, String>,
    pub state_names: Vec<String>,
    /// Every TS interface in the `<script>` block. Threaded into
    /// `Module.local_interfaces` for the project-level cross-file
    /// registry. Svelte's idiomatic source uses `export let`
    /// rather than typed interfaces, so this is usually empty —
    /// but interfaces *are* allowed and we capture them when present.
    pub local_interfaces: HashMap<String, PropsType>,
}

pub fn walk(module: &ast::Module, report: &mut PortReport) -> ScriptWalkResult {
    let mut props = PropsType::default();
    let mut preamble = Vec::new();
    let mut handler_fns = HashMap::new();
    let mut state_names = Vec::new();
    let mut local_interfaces: HashMap<String, PropsType> = HashMap::new();

    // Pass 1: collect interfaces.
    let mut iface_decls: HashMap<String, &ast::TsInterfaceDecl> = HashMap::new();
    for item in &module.body {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::TsInterface(i))) = item {
            iface_decls.insert(i.id.sym.to_string(), i);
        }
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(e)) = item {
            if let ast::Decl::TsInterface(i) = &e.decl {
                iface_decls.insert(i.id.sym.to_string(), i);
            }
        }
    }
    for (name, decl) in &iface_decls {
        local_interfaces.insert(name.clone(), lift_interface_to_props(decl));
    }
    let _ = report;

    for item in &module.body {
        // Svelte's `export let` parses as `ExportDecl(Var(...))`.
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(e)) = item {
            if let ast::Decl::Var(v) = &e.decl {
                for d in &v.decls {
                    lift_prop(d, &mut props);
                }
            }
            continue;
        }
        let stmt = match item {
            ast::ModuleItem::Stmt(s) => s,
            _ => continue,
        };
        match stmt {
            ast::Stmt::Decl(ast::Decl::Var(decl)) => {
                for d in &decl.decls {
                    lift_let(d, &mut preamble, &mut state_names);
                }
            }
            ast::Stmt::Decl(ast::Decl::Fn(f)) => {
                let name = f.ident.sym.to_string();
                let body = render_fn_body(&f.function, &state_names);
                handler_fns.insert(name, body);
            }
            ast::Stmt::Labeled(lbl) if lbl.label.sym.as_ref() == "$" => {
                // `$: stmt;` is a labeled statement with label `$`.
                lift_reactive_statement(&lbl.body, &mut preamble, &state_names, report);
            }
            _ => {}
        }
    }

    ScriptWalkResult { props, preamble, handler_fns, state_names, local_interfaces }
}

/// Lower a TS interface declaration to a `PropsType`. Mirrors
/// `port_tsx`'s `lift_interface` but with a narrower type table
/// (Svelte scripts rarely use function-typed fields).
fn lift_interface_to_props(iface: &ast::TsInterfaceDecl) -> PropsType {
    let mut props = PropsType::default();
    for member in &iface.body.body {
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

fn ts_type_to_rust(t: &ast::TsType) -> Option<String> {
    use ast::TsKeywordTypeKind as K;
    match t {
        ast::TsType::TsKeywordType(k) => match k.kind {
            K::TsNumberKeyword => Some("i32".into()),
            K::TsBooleanKeyword => Some("bool".into()),
            K::TsStringKeyword => Some("String".into()),
            _ => None,
        },
        _ => None,
    }
}

fn lift_prop(d: &ast::VarDeclarator, props: &mut PropsType) {
    let name = match &d.name {
        ast::Pat::Ident(b) => b.id.sym.to_string(),
        _ => return,
    };
    let default = d.init.as_ref().map(|e| render_simple_expr(e));
    let ty = infer_ty(d.init.as_deref());
    props.fields.push(PropField {
        name,
        ty,
        optional: default.is_some(),
        default,
    });
}

fn lift_let(
    d: &ast::VarDeclarator,
    preamble: &mut Vec<Reactive>,
    state_names: &mut Vec<String>,
) {
    let name = match &d.name {
        ast::Pat::Ident(b) => b.id.sym.to_string(),
        _ => return,
    };
    let init = d
        .init
        .as_ref()
        .map(|e| {
            // If init references a prop, rewrite to `props.X`.
            // For the fixture: `let count = initial;` → `props.initial`.
            render_init_expr(e, state_names)
        })
        .unwrap_or_else(|| "Default::default()".into());
    state_names.push(name.clone());
    preamble.push(Reactive::State {
        name,
        setter: "= ".into(),
        init,
    });
}

fn lift_reactive_statement(
    body: &ast::Stmt,
    preamble: &mut Vec<Reactive>,
    state_names: &[String],
    report: &mut PortReport,
) {
    match body {
        // `$: name = expr;` → derived signal (MVP: emit as Let).
        ast::Stmt::Expr(es) => {
            // Could be `$: x = expr` (assignment) or `$: someCall()` (side effect).
            if let ast::Expr::Assign(a) = &*es.expr {
                let lhs = match &a.left {
                    ast::AssignTarget::Simple(ast::SimpleAssignTarget::Ident(i)) => {
                        i.id.sym.to_string()
                    }
                    _ => "?".into(),
                };
                let rhs = render_simple_expr(&a.right);
                preamble.push(Reactive::Let {
                    name: lhs,
                    expr: format!("move || {}", rhs),
                });
                return;
            }
            // Side effect — emit Effect, body as hole.
            let body_text = render_call_summary(&es.expr);
            let hole = Hole {
                kind: HoleKind::HandlerBody,
                reason: "$: reactive side-effect body — JS imperative code, AI pass needed".into(),
                original: SourceSnippet::new(body_text),
            };
            report.record(hole.clone());
            preamble.push(Reactive::Effect {
                body: SourceSnippet::new(port_core::render_inline_hole(&hole)),
                deps: None,
            });
        }
        _ => {
            report.record(Hole {
                kind: HoleKind::Unsupported,
                reason: "$: with non-expression body".into(),
                original: SourceSnippet::new("…$:…"),
            });
        }
    }
    let _ = state_names; // reserved for future ident rewriting in $: bodies
}

fn render_simple_expr(e: &ast::Expr) -> String {
    match e {
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("\"{}\"", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Bool(b)) => b.value.to_string(),
        ast::Expr::Ident(i) => i.sym.to_string(),
        ast::Expr::Bin(b) => {
            let l = render_simple_expr(&b.left);
            let r = render_simple_expr(&b.right);
            format!("{} {} {}", l, binop_str(b.op), r)
        }
        _ => "/* expr */".into(),
    }
}

fn render_init_expr(e: &ast::Expr, _state_names: &[String]) -> String {
    // Svelte: `let count = initial;` — `initial` is a prop, render
    // as `props.initial`. We assume bare idents in init position
    // are prop refs (the only other thing would be a literal).
    match e {
        ast::Expr::Ident(i) => format!("props.{}", i.sym),
        _ => render_simple_expr(e),
    }
}

fn render_fn_body(func: &ast::Function, state_names: &[String]) -> String {
    let Some(body) = &func.body else { return "/* empty */".into() };
    if body.stmts.len() != 1 {
        return "/* multi-stmt fn body */".into();
    }
    match &body.stmts[0] {
        ast::Stmt::Expr(es) => render_handler_expr(&es.expr, state_names),
        _ => "/* non-expr stmt */".into(),
    }
}

/// Render an expression that appears inside a handler / function
/// body, applying Svelte's signal rewriting:
///   - Bare ref to a state name → `.get()`
///   - Assignment `x = expr` → `x.set(expr_rewritten)` (where
///     `expr_rewritten` recursively rewrites refs)
fn render_handler_expr(e: &ast::Expr, state_names: &[String]) -> String {
    match e {
        ast::Expr::Assign(a) => {
            let lhs = match &a.left {
                ast::AssignTarget::Simple(ast::SimpleAssignTarget::Ident(i)) => {
                    i.id.sym.to_string()
                }
                _ => return "/* complex assign */".into(),
            };
            let rhs = render_handler_expr(&a.right, state_names);
            if state_names.iter().any(|n| n == &lhs) {
                format!("{}.set({})", lhs, rhs)
            } else {
                format!("{} = {}", lhs, rhs)
            }
        }
        ast::Expr::Ident(i) => {
            let n = i.sym.to_string();
            if state_names.iter().any(|s| s == &n) {
                format!("{}.get()", n)
            } else {
                n
            }
        }
        ast::Expr::Bin(b) => {
            let l = render_handler_expr(&b.left, state_names);
            let r = render_handler_expr(&b.right, state_names);
            format!("{} {} {}", l, binop_str(b.op), r)
        }
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("\"{}\"", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Bool(b)) => b.value.to_string(),
        ast::Expr::Update(u) => {
            let arg = match &*u.arg {
                ast::Expr::Ident(i) => i.sym.to_string(),
                _ => return "/* complex update */".into(),
            };
            let op = if u.op == ast::UpdateOp::PlusPlus { "+ 1" } else { "- 1" };
            if state_names.iter().any(|s| s == &arg) {
                format!("{}.set({}.get() {})", arg, arg, op)
            } else {
                format!("{} {} {}", arg, op, "")
            }
        }
        _ => "/* unsupported handler expr */".into(),
    }
}

fn infer_ty(init: Option<&ast::Expr>) -> String {
    match init {
        Some(ast::Expr::Lit(ast::Lit::Num(_))) => "i32".into(),
        Some(ast::Expr::Lit(ast::Lit::Str(_))) => "String".into(),
        Some(ast::Expr::Lit(ast::Lit::Bool(_))) => "bool".into(),
        _ => "i32".into(),
    }
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
        _ => "/*op*/",
    }
}

fn render_call_summary(e: &ast::Expr) -> String {
    match e {
        ast::Expr::Call(c) => {
            let callee = match &c.callee {
                ast::Callee::Expr(e) => render_callee(e),
                _ => "?".into(),
            };
            let args: Vec<String> = c.args.iter().map(|a| render_arg(&a.expr)).collect();
            format!("{}({});", callee, args.join(", "))
        }
        _ => render_simple_expr(e),
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
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("'{}'", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Ident(i) => i.sym.to_string(),
        _ => "…".into(),
    }
}
