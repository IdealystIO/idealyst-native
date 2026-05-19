//! Vue `<script setup>` → porter `Component` body (preamble + props).
//!
//! Walks the script AST looking for the canonical Composition-API
//! patterns:
//!
//! - `interface CounterProps { … }` — collected as prop shape.
//! - `defineProps<CounterProps>()` / `withDefaults(defineProps<…>(), { … })`
//!   — locates the prop interface + harvests defaults.
//! - `const count = ref(props.initial)` — emits `Reactive::State`.
//! - `watchEffect(() => …)` — emits `Reactive::Effect`.
//! - `function increment() { count.value++ }` — recorded for the
//!   template walker to inline when its name appears in an
//!   `@click` handler.

use port_core::ir::*;
use port_tsx::ast;
use std::collections::HashMap;

pub struct ScriptWalkResult {
    pub component_name: String,
    pub props: PropsType,
    pub preamble: Vec<Reactive>,
    /// Function decls keyed by name → rendered Rust body. The
    /// template walker substitutes these when an event attribute
    /// names a function (`@click="increment"`).
    pub handler_fns: HashMap<String, String>,
    /// State signal names produced by `ref()` so the template
    /// walker can rewrite reads. Currently informational; the
    /// template walker assumes single-ident reads are reactive.
    pub state_names: Vec<String>,
    /// Every TS interface in the script, lowered to a `PropsType`.
    /// Threaded into `Module.local_interfaces` so the project-level
    /// driver can resolve cross-file context-type references.
    pub local_interfaces: HashMap<String, PropsType>,
}

pub fn walk(
    module: &ast::Module,
    component_name: &str,
    report: &mut PortReport,
) -> ScriptWalkResult {
    let mut interfaces: HashMap<String, &ast::TsInterfaceDecl> = HashMap::new();
    for item in &module.body {
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::TsInterface(i))) = item {
            interfaces.insert(i.id.sym.to_string(), i);
        }
    }

    let mut props = PropsType::default();
    let mut preamble = Vec::new();
    let mut handler_fns = HashMap::new();
    let mut state_names = Vec::new();
    let mut prop_defaults: HashMap<String, String> = HashMap::new();

    for item in &module.body {
        let stmt = match item {
            ast::ModuleItem::Stmt(s) => s,
            _ => continue,
        };
        match stmt {
            ast::Stmt::Decl(ast::Decl::Var(decl)) => {
                for d in &decl.decls {
                    walk_var(d, &interfaces, &mut props, &mut preamble, &mut state_names, &mut prop_defaults, report);
                }
            }
            ast::Stmt::Expr(es) => walk_top_expr(&es.expr, &mut preamble, report),
            ast::Stmt::Decl(ast::Decl::Fn(f)) => {
                let name = f.ident.sym.to_string();
                let body = render_fn_body(&f.function);
                handler_fns.insert(name, body);
            }
            _ => {}
        }
    }

    // Apply harvested defaults.
    for (k, v) in &prop_defaults {
        if let Some(field) = props.fields.iter_mut().find(|f| &f.name == k) {
            field.default = Some(v.clone());
        }
    }

    let _ = component_name; // reserved for future diagnostics

    // Lower every interface to a `PropsType` for the cross-file
    // registry. The same lifter (`lift_interface`) that produces
    // component-prop structs is used here so field shapes stay
    // consistent.
    let mut local_interfaces = HashMap::new();
    for (name, decl) in &interfaces {
        local_interfaces.insert(name.clone(), lift_interface(decl, name, report));
    }

    ScriptWalkResult {
        component_name: component_name.to_string(),
        props,
        preamble,
        handler_fns,
        state_names,
        local_interfaces,
    }
}

fn walk_var(
    d: &ast::VarDeclarator,
    interfaces: &HashMap<String, &ast::TsInterfaceDecl>,
    props: &mut PropsType,
    preamble: &mut Vec<Reactive>,
    state_names: &mut Vec<String>,
    prop_defaults: &mut HashMap<String, String>,
    report: &mut PortReport,
) {
    let Some(init) = &d.init else { return };
    let call = match &**init {
        ast::Expr::Call(c) => c,
        _ => return,
    };
    let callee_name = match &call.callee {
        ast::Callee::Expr(e) => match &**e {
            ast::Expr::Ident(i) => i.sym.to_string(),
            _ => return,
        },
        _ => return,
    };
    let binding_name = match &d.name {
        ast::Pat::Ident(b) => Some(b.id.sym.to_string()),
        _ => None,
    };

    match callee_name.as_str() {
        "ref" => {
            let Some(name) = binding_name else { return };
            let init_expr = call
                .args
                .first()
                .map(|a| render_expr(&a.expr))
                .unwrap_or_else(|| "()".into());
            state_names.push(name.clone());
            preamble.push(Reactive::State {
                name,
                setter: "set_".into(), // Vue has no setter ident
                init: init_expr,
            });
        }
        "withDefaults" => {
            // withDefaults(defineProps<T>(), { foo: 1, bar: "x" })
            // First arg: defineProps<T>()  — extract T from generics.
            if let Some(inner) = call.args.first() {
                if let ast::Expr::Call(define_call) = &*inner.expr {
                    if let Some(type_args) = &define_call.type_args {
                        if let Some(first_arg) = type_args.params.first() {
                            if let ast::TsType::TsTypeRef(r) = &**first_arg {
                                if let ast::TsEntityName::Ident(id) = &r.type_name {
                                    let iface_name = id.sym.to_string();
                                    if let Some(iface) = interfaces.get(&iface_name) {
                                        *props = lift_interface(iface, &iface_name, report);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Second arg: { foo: 1 } object literal of defaults.
            if let Some(defaults_arg) = call.args.get(1) {
                if let ast::Expr::Object(obj) = &*defaults_arg.expr {
                    for p in &obj.props {
                        if let ast::PropOrSpread::Prop(b) = p {
                            if let ast::Prop::KeyValue(kv) = &**b {
                                let key = match &kv.key {
                                    ast::PropName::Ident(i) => i.sym.to_string(),
                                    _ => continue,
                                };
                                let value = render_expr(&kv.value);
                                prop_defaults.insert(key, value);
                            }
                        }
                    }
                }
            }
        }
        "defineProps" => {
            // Bare defineProps without withDefaults — just extract
            // the type.
            if let Some(type_args) = &call.type_args {
                if let Some(first_arg) = type_args.params.first() {
                    if let ast::TsType::TsTypeRef(r) = &**first_arg {
                        if let ast::TsEntityName::Ident(id) = &r.type_name {
                            let iface_name = id.sym.to_string();
                            if let Some(iface) = interfaces.get(&iface_name) {
                                *props = lift_interface(iface, &iface_name, report);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn walk_top_expr(expr: &ast::Expr, preamble: &mut Vec<Reactive>, report: &mut PortReport) {
    let call = match expr {
        ast::Expr::Call(c) => c,
        _ => return,
    };
    let callee_name = match &call.callee {
        ast::Callee::Expr(e) => match &**e {
            ast::Expr::Ident(i) => i.sym.to_string(),
            _ => return,
        },
        _ => return,
    };
    if !matches!(callee_name.as_str(), "watchEffect" | "watch" | "onMounted" | "onUnmounted") {
        return;
    }
    let Some(first) = call.args.first() else { return };
    let body_summary = arrow_body_summary(&first.expr);
    let hole = Hole {
        kind: HoleKind::HandlerBody,
        reason: format!("{} body — JS imperative code, AI pass needed", callee_name),
        original: SourceSnippet::new(body_summary),
    };
    report.record(hole.clone());
    preamble.push(Reactive::Effect {
        body: SourceSnippet::new(port_core::render_inline_hole(&hole)),
        deps: None,
    });
}

fn lift_interface(
    iface: &ast::TsInterfaceDecl,
    name: &str,
    report: &mut PortReport,
) -> PropsType {
    let mut props = PropsType::default();
    for member in &iface.body.body {
        if let ast::TsTypeElement::TsPropertySignature(sig) = member {
            let prop_name = match &*sig.key {
                ast::Expr::Ident(i) => i.sym.to_string(),
                _ => continue,
            };
            let ty = sig
                .type_ann
                .as_ref()
                .and_then(|t| ts_type_to_rust(&t.type_ann))
                .unwrap_or_else(|| {
                    report.record(Hole {
                        kind: HoleKind::PropType,
                        reason: format!("prop `{}.{}` has unmappable TS type", name, prop_name),
                        original: SourceSnippet::new(format!("{}.{}", name, prop_name)),
                    });
                    "()".into()
                });
            props.fields.push(PropField {
                name: prop_name,
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

fn render_fn_body(func: &ast::Function) -> String {
    // For simple `function name() { count.value++ }` bodies we
    // render the inner statement as a Rust expression. Anything
    // else returns a hole-style placeholder which the template
    // walker leaves as a `todo!` when the handler is referenced.
    let Some(body) = &func.body else {
        return "/* empty */".into();
    };
    if body.stmts.len() != 1 {
        return "/* multi-stmt fn body */".into();
    }
    let stmt = &body.stmts[0];
    match stmt {
        ast::Stmt::Expr(es) => render_expr(&es.expr),
        _ => "/* non-expr stmt */".into(),
    }
}

/// Render a script-side expression to a Rust string. This is a
/// reduced version of the TSX renderer because the Vue script
/// idiom is narrower (refs via `.value`, no JSX).
fn render_expr(e: &ast::Expr) -> String {
    match e {
        ast::Expr::Ident(i) => i.sym.to_string(),
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("\"{}\"", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Bool(b)) => b.value.to_string(),
        ast::Expr::Member(m) => {
            let obj = match &*m.obj {
                ast::Expr::Ident(i) => i.sym.to_string(),
                _ => return "/* complex member obj */".into(),
            };
            let prop = match &m.prop {
                ast::MemberProp::Ident(i) => i.sym.to_string(),
                _ => return "/* complex member prop */".into(),
            };
            // `props.initial` stays as-is.
            // `count.value` is a Vue ref read — translate to
            // `count.get()`.
            if prop == "value" {
                format!("{}.get()", obj)
            } else {
                format!("{}.{}", obj, prop)
            }
        }
        ast::Expr::Update(u) => {
            // `count.value++` shape — translate to a setter call.
            let arg = match &*u.arg {
                ast::Expr::Member(m) => match (&*m.obj, &m.prop) {
                    (ast::Expr::Ident(name), ast::MemberProp::Ident(p)) if p.sym.as_ref() == "value" => {
                        let n = name.sym.to_string();
                        let op = if u.op == ast::UpdateOp::PlusPlus { "+ 1" } else { "- 1" };
                        return format!("{}.set({}.get() {})", n, n, op);
                    }
                    _ => "/*ref?*/".into(),
                },
                _ => "/*update?*/".into(),
            };
            arg
        }
        _ => "/* unsupported expr */".into(),
    }
}

fn arrow_body_summary(e: &ast::Expr) -> String {
    if let ast::Expr::Arrow(a) = e {
        match &*a.body {
            ast::BlockStmtOrExpr::BlockStmt(b) if b.stmts.len() == 1 => {
                if let ast::Stmt::Expr(es) = &b.stmts[0] {
                    return render_call_summary(&es.expr);
                }
            }
            ast::BlockStmtOrExpr::Expr(e) => return render_call_summary(e),
            _ => {}
        }
    }
    "…effect body…".into()
}

fn render_call_summary(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::Call(c) => {
            let callee = match &c.callee {
                ast::Callee::Expr(e) => render_callee(e),
                _ => "?".into(),
            };
            let args: Vec<String> = c.args.iter().map(|a| render_arg_summary(&a.expr)).collect();
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

fn render_arg_summary(e: &ast::Expr) -> String {
    match e {
        ast::Expr::Lit(ast::Lit::Str(s)) => format!("'{}'", s.value.to_atom_lossy()),
        ast::Expr::Lit(ast::Lit::Num(n)) => n.value.to_string(),
        ast::Expr::Ident(i) => i.sym.to_string(),
        ast::Expr::Member(m) => {
            let obj = match &*m.obj {
                ast::Expr::Ident(i) => i.sym.to_string(),
                _ => "?".into(),
            };
            let prop = match &m.prop {
                ast::MemberProp::Ident(i) => i.sym.to_string(),
                _ => "?".into(),
            };
            format!("{}.{}", obj, prop)
        }
        _ => "…".into(),
    }
}
