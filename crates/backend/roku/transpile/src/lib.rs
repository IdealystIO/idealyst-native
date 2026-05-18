//! Rust → BrightScript transpiler (library form).
//!
//! Exposes [`transpile_fn`], which walks a `syn::ItemFn` and
//! produces equivalent BrightScript source. Two callers:
//!
//! - `backend-roku-macros`' `#[method]` attribute, for compile-time
//!   validation and emission of a `*_BRS` sibling constant.
//! - The `idealyst brs` CLI subcommand, which scans a user's project
//!   for `#[method]`-tagged functions and concatenates their
//!   transpilations into a single `.brs` file shipped in the Roku
//!   .pkg.
//!
//! Anything outside the supported subset returns a `syn::Error`
//! pointing at the offending span. Callers decide what to do with
//! it — emit a compile diagnostic (macro) or print + abort (CLI).
//!
//! ## Supported subset (v0)
//!
//! - **Types**: `i8`..`u32`, `usize`, `isize` → `integer`; `i64`/`u64`
//!   → `longinteger`; `f32` → `float`; `f64` → `double`; `bool` →
//!   `boolean`; `&str` / `String` → `string`. Returning `()` makes
//!   the function emit as a `sub`; everything else as a `function`.
//! - **Statements**: `let name = expr;` (mut allowed but ignored —
//!   BrightScript has no const), expression statements, `return expr;`.
//! - **Expressions**: integer / float / bool / string literals,
//!   identifiers, parenthesized exprs, arithmetic (`+ - * / %`),
//!   comparison (`== != < > <= >=`), logical (`&& ||`), unary `-` /
//!   `!`, function calls (single-segment paths only).
//! - **Control flow**: `if`/`else if`/`else` (as statement OR in
//!   tail position of a function body), `while cond { ... }`,
//!   `for i in start..end` and `..=end`.
//!
//! ## Explicitly not supported (yet)
//!
//! Closures, traits, impl blocks, generics, lifetimes, references
//! (except `&str`), `match`, custom structs/enums, `let mut x;`
//! (no initializer), `let-else`, pattern destructuring, method
//! calls (`x.foo()`), associated paths (`Module::name`), `loop`,
//! `break` / `continue`, ranges with `step_by`, async, await,
//! macros, attributes on inner items. Every one of these emits a
//! pinpointed compile error.

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{
    BinOp, Block, Expr, ExprForLoop, ExprIf, ExprMatch, ExprWhile, FnArg, ItemFn, Lit,
    Local, Pat, Path, ReturnType, Stmt, Type, UnOp,
};

pub fn transpile_fn(item: &ItemFn) -> syn::Result<String> {
    if !item.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.sig.generics,
            "generics aren't supported in #[method]",
        ));
    }
    if item.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &item.sig,
            "async functions aren't supported in #[method]",
        ));
    }
    if item.sig.unsafety.is_some() {
        return Err(syn::Error::new_spanned(
            &item.sig,
            "unsafe functions aren't supported in #[method]",
        ));
    }
    if item.sig.constness.is_some() {
        return Err(syn::Error::new_spanned(
            &item.sig,
            "const functions aren't supported in #[method]",
        ));
    }
    let mut e = Emitter::new();
    e.function(item)?;
    Ok(e.out)
}

struct Emitter {
    out: String,
    indent: usize,
}

impl Emitter {
    fn new() -> Self {
        Self {
            out: String::new(),
            indent: 0,
        }
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
    }

    fn line(&mut self, s: &str) {
        self.write_indent();
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn nl(&mut self) {
        self.out.push('\n');
    }

    fn function(&mut self, item: &ItemFn) -> syn::Result<()> {
        let name = item.sig.ident.to_string();

        let mut args: Vec<(String, String)> = Vec::new();
        for arg in &item.sig.inputs {
            match arg {
                FnArg::Receiver(r) => {
                    return Err(syn::Error::new_spanned(
                        r,
                        "self receivers aren't supported in #[method]; \
                         #[method] only annotates free functions",
                    ));
                }
                FnArg::Typed(p) => {
                    let arg_name = match &*p.pat {
                        Pat::Ident(pi) if pi.by_ref.is_none() && pi.subpat.is_none() => {
                            pi.ident.to_string()
                        }
                        _ => {
                            return Err(syn::Error::new_spanned(
                                &p.pat,
                                "argument patterns must be simple identifiers",
                            ));
                        }
                    };
                    let arg_ty = map_type(&p.ty)?;
                    args.push((arg_name, arg_ty));
                }
            }
        }

        let return_ty = match &item.sig.output {
            ReturnType::Default => None,
            ReturnType::Type(_, ty) => {
                // `-> ()` returns nothing in BrightScript terms.
                if is_unit(ty) {
                    None
                } else {
                    Some(map_type(ty)?)
                }
            }
        };

        let args_str: Vec<String> =
            args.iter().map(|(n, t)| format!("{} as {}", n, t)).collect();
        let args_joined = args_str.join(", ");
        let sig_line = match &return_ty {
            Some(ret) => format!("function {}({}) as {}", name, args_joined, ret),
            None => format!("sub {}({})", name, args_joined),
        };
        self.line(&sig_line);

        self.indent += 1;
        self.block(&item.block, return_ty.is_some())?;
        self.indent -= 1;

        self.line(if return_ty.is_some() {
            "end function"
        } else {
            "end sub"
        });
        Ok(())
    }

    /// Emit statements. `tail` is true when the block is in
    /// tail-return position (the function body, or a branch of an
    /// `if` that itself is in tail position) — the last expression
    /// without semicolon becomes `return <expr>`.
    fn block(&mut self, block: &Block, tail: bool) -> syn::Result<()> {
        let n = block.stmts.len();
        for (i, stmt) in block.stmts.iter().enumerate() {
            let is_last = i + 1 == n;
            self.stmt(stmt, tail && is_last)?;
        }
        Ok(())
    }

    fn stmt(&mut self, stmt: &Stmt, tail: bool) -> syn::Result<()> {
        match stmt {
            Stmt::Local(l) => self.local(l),
            Stmt::Expr(e, semi) => {
                if semi.is_none() && tail {
                    self.tail_expr(e)
                } else {
                    self.expr_stmt(e)
                }
            }
            Stmt::Item(item) => Err(syn::Error::new_spanned(
                item,
                "nested items aren't supported in #[method]",
            )),
            Stmt::Macro(m) => Err(syn::Error::new_spanned(
                m,
                "macro calls aren't supported in #[method]",
            )),
        }
    }

    fn local(&mut self, l: &Local) -> syn::Result<()> {
        let name = match &l.pat {
            Pat::Ident(pi) if pi.by_ref.is_none() && pi.subpat.is_none() => {
                pi.ident.to_string()
            }
            Pat::Type(pt) => match &*pt.pat {
                Pat::Ident(pi) if pi.by_ref.is_none() && pi.subpat.is_none() => {
                    pi.ident.to_string()
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        &l.pat,
                        "let patterns must be simple identifiers",
                    ));
                }
            },
            _ => {
                return Err(syn::Error::new_spanned(
                    &l.pat,
                    "let patterns must be simple identifiers",
                ));
            }
        };
        let init = l.init.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(l, "let without initializer isn't supported")
        })?;
        if init.diverge.is_some() {
            return Err(syn::Error::new_spanned(l, "let-else isn't supported"));
        }
        self.write_indent();
        self.push(&name);
        self.push(" = ");
        self.expr(&init.expr)?;
        self.nl();
        Ok(())
    }

    /// Emit an expression as a statement (semicolon present or
    /// non-tail position). `if`/`while`/`for` get their block form;
    /// everything else writes one indented line.
    fn expr_stmt(&mut self, e: &Expr) -> syn::Result<()> {
        match e {
            Expr::If(i) => self.emit_if(i, false),
            Expr::While(w) => self.emit_while(w),
            Expr::ForLoop(f) => self.emit_for(f),
            Expr::Block(b) => self.block(&b.block, false),
            Expr::Assign(a) => {
                self.write_indent();
                self.expr(&a.left)?;
                self.push(" = ");
                self.expr(&a.right)?;
                self.nl();
                Ok(())
            }
            Expr::Return(r) => {
                self.write_indent();
                self.push("return");
                if let Some(val) = &r.expr {
                    self.push(" ");
                    self.expr(val)?;
                }
                self.nl();
                Ok(())
            }
            Expr::Call(_) | Expr::Path(_) => {
                self.write_indent();
                self.expr(e)?;
                self.nl();
                Ok(())
            }
            _ => Err(syn::Error::new_spanned(
                e,
                "this expression isn't valid as a statement in #[method]",
            )),
        }
    }

    /// Emit an expression in tail position: each control-flow branch
    /// must produce its own `return`, so we recurse into `if` and
    /// blocks rather than wrapping the whole thing in `return`.
    fn tail_expr(&mut self, e: &Expr) -> syn::Result<()> {
        match e {
            Expr::If(i) => self.emit_if(i, true),
            Expr::Match(m) => self.emit_match(m, true),
            Expr::Block(b) => self.block(&b.block, true),
            Expr::Return(r) => {
                self.write_indent();
                self.push("return");
                if let Some(val) = &r.expr {
                    self.push(" ");
                    self.expr(val)?;
                }
                self.nl();
                Ok(())
            }
            _ => {
                self.write_indent();
                self.push("return ");
                self.expr(e)?;
                self.nl();
                Ok(())
            }
        }
    }

    /// Lower a `match` expression in tail position into a chain of
    /// `if`/`else if`/`else` over the scrutinee. v0 grammar:
    ///
    /// - Arms must use literal patterns (`Lit::Int` / `Lit::Str` /
    ///   `Lit::Bool`) or the wildcard `_`. No guards. No bindings,
    ///   no struct/enum patterns.
    /// - `_` must be the final arm — it becomes the bare `else`.
    /// - The match's value must reach a `return` in every arm; we
    ///   require tail position because matching mid-block needs
    ///   value-lifting (let-temp-via-statements) which the
    ///   transpiler doesn't do yet.
    fn emit_match(&mut self, m: &ExprMatch, tail: bool) -> syn::Result<()> {
        if !tail {
            return Err(syn::Error::new_spanned(
                m,
                "`match` is only supported in tail position; assign \
                 via separate `if` statements, or extract into a
                 helper #[method].",
            ));
        }
        if m.arms.is_empty() {
            return Err(syn::Error::new_spanned(
                m,
                "match with zero arms isn't supported",
            ));
        }

        for (i, arm) in m.arms.iter().enumerate() {
            if arm.guard.is_some() {
                return Err(syn::Error::new_spanned(
                    arm,
                    "match guards (`if cond`) aren't supported in #[method]",
                ));
            }
            let is_first = i == 0;
            let is_last = i + 1 == m.arms.len();

            match &arm.pat {
                Pat::Wild(_) => {
                    if !is_last {
                        return Err(syn::Error::new_spanned(
                            &arm.pat,
                            "`_` arm must come last",
                        ));
                    }
                    if is_first {
                        return Err(syn::Error::new_spanned(
                            &arm.pat,
                            "match with only a `_` arm — drop the match \
                             and use the body directly",
                        ));
                    }
                    self.line("else");
                    self.indent += 1;
                    self.tail_expr(&arm.body)?;
                    self.indent -= 1;
                }
                Pat::Lit(lit_expr) => {
                    self.write_indent();
                    self.push(if is_first { "if " } else { "else if " });
                    self.expr(&m.expr)?;
                    self.push(" = ");
                    self.lit(&lit_expr.lit)?;
                    self.push(" then\n");
                    self.indent += 1;
                    self.tail_expr(&arm.body)?;
                    self.indent -= 1;
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "match patterns must be a literal (int/str/bool) \
                         or `_`. v0 doesn't support bindings, ranges, or \
                         struct/enum patterns.",
                    ));
                }
            }
        }
        self.line("end if");
        Ok(())
    }

    fn emit_if(&mut self, i: &ExprIf, tail: bool) -> syn::Result<()> {
        self.write_indent();
        self.push("if ");
        self.expr(&i.cond)?;
        self.push(" then\n");
        self.indent += 1;
        self.block(&i.then_branch, tail)?;
        self.indent -= 1;
        if let Some((_, else_branch)) = &i.else_branch {
            match &**else_branch {
                Expr::If(elif) => {
                    self.write_indent();
                    self.push("else ");
                    // emit_if writes its own "if cond then" prefix, but we
                    // need it to share a line with our "else " marker.
                    // Easiest: drop the leading indent and emit inline.
                    self.push("if ");
                    self.expr(&elif.cond)?;
                    self.push(" then\n");
                    self.indent += 1;
                    self.block(&elif.then_branch, tail)?;
                    self.indent -= 1;
                    if let Some((_, inner_else)) = &elif.else_branch {
                        self.emit_else(inner_else, tail)?;
                    }
                }
                Expr::Block(b) => {
                    self.line("else");
                    self.indent += 1;
                    self.block(&b.block, tail)?;
                    self.indent -= 1;
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        else_branch,
                        "else branch must be a block or another `if`",
                    ));
                }
            }
        }
        self.line("end if");
        Ok(())
    }

    /// Recursive emit for nested `else if`/`else` after the first
    /// `else if` (so the chain stays flat: `else if .. else if .. else`).
    fn emit_else(&mut self, else_branch: &Expr, tail: bool) -> syn::Result<()> {
        match else_branch {
            Expr::If(elif) => {
                self.write_indent();
                self.push("else if ");
                self.expr(&elif.cond)?;
                self.push(" then\n");
                self.indent += 1;
                self.block(&elif.then_branch, tail)?;
                self.indent -= 1;
                if let Some((_, inner)) = &elif.else_branch {
                    self.emit_else(inner, tail)?;
                }
                Ok(())
            }
            Expr::Block(b) => {
                self.line("else");
                self.indent += 1;
                self.block(&b.block, tail)?;
                self.indent -= 1;
                Ok(())
            }
            _ => Err(syn::Error::new_spanned(
                else_branch,
                "else branch must be a block or another `if`",
            )),
        }
    }

    fn emit_while(&mut self, w: &ExprWhile) -> syn::Result<()> {
        self.write_indent();
        self.push("while ");
        self.expr(&w.cond)?;
        self.nl();
        self.indent += 1;
        self.block(&w.body, false)?;
        self.indent -= 1;
        self.line("end while");
        Ok(())
    }

    fn emit_for(&mut self, f: &ExprForLoop) -> syn::Result<()> {
        let var = match &*f.pat {
            Pat::Ident(pi) if pi.by_ref.is_none() && pi.subpat.is_none() => {
                pi.ident.to_string()
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    &f.pat,
                    "for-loop pattern must be a simple identifier",
                ));
            }
        };
        let range = match &*f.expr {
            Expr::Range(r) => r,
            _ => {
                return Err(syn::Error::new_spanned(
                    &f.expr,
                    "for-loop iterator must be a range literal (`a..b` or `a..=b`)",
                ));
            }
        };
        let start = range.start.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(range, "range must have an explicit start")
        })?;
        let end = range.end.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(range, "range must have an explicit end")
        })?;
        let inclusive = matches!(range.limits, syn::RangeLimits::Closed(_));

        self.write_indent();
        self.push(&format!("for {} = ", var));
        self.expr(start)?;
        self.push(" to ");
        if inclusive {
            self.expr(end)?;
        } else {
            // Exclusive range: BrightScript's `to` is inclusive, so
            // emit `(end) - 1`. Parens guard against precedence
            // surprises with non-literal ends.
            self.push("(");
            self.expr(end)?;
            self.push(") - 1");
        }
        self.nl();
        self.indent += 1;
        self.block(&f.body, false)?;
        self.indent -= 1;
        self.line("end for");
        Ok(())
    }

    /// Emit an expression inline (no newline, no leading indent).
    fn expr(&mut self, e: &Expr) -> syn::Result<()> {
        match e {
            Expr::Lit(l) => self.lit(&l.lit),
            Expr::Path(p) => {
                let ident = single_ident(&p.path)?;
                self.push(&ident);
                Ok(())
            }
            Expr::Binary(b) => {
                self.expr(&b.left)?;
                self.push(" ");
                self.push(map_binop(&b.op, b.op.span())?);
                self.push(" ");
                self.expr(&b.right)?;
                Ok(())
            }
            Expr::Unary(u) => {
                match u.op {
                    UnOp::Neg(_) => self.push("-"),
                    UnOp::Not(_) => self.push("not "),
                    UnOp::Deref(_) => {
                        return Err(syn::Error::new_spanned(u, "deref isn't supported"));
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            u,
                            "unsupported unary operator",
                        ));
                    }
                }
                self.expr(&u.expr)?;
                Ok(())
            }
            Expr::Paren(p) => {
                self.push("(");
                self.expr(&p.expr)?;
                self.push(")");
                Ok(())
            }
            Expr::Call(c) => {
                let func_ident = match &*c.func {
                    Expr::Path(p) => single_ident(&p.path)?,
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &c.func,
                            "function calls must reference a named function",
                        ));
                    }
                };
                self.push(&func_ident);
                self.push("(");
                for (i, arg) in c.args.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.expr(arg)?;
                }
                self.push(")");
                Ok(())
            }
            Expr::If(_) => Err(syn::Error::new_spanned(
                e,
                "`if` as a value expression is only supported in tail position; \
                 lift it into a statement or use a separate function",
            )),
            Expr::Block(_) => Err(syn::Error::new_spanned(
                e,
                "block expressions as values aren't supported in #[method]",
            )),
            _ => Err(syn::Error::new_spanned(
                e,
                "unsupported expression kind for #[method]",
            )),
        }
    }

    fn lit(&mut self, l: &Lit) -> syn::Result<()> {
        match l {
            Lit::Int(i) => {
                // base10_digits drops any suffix (`42i32` → "42").
                self.push(i.base10_digits());
            }
            Lit::Float(f) => {
                self.push(f.base10_digits());
            }
            Lit::Bool(b) => {
                self.push(if b.value { "true" } else { "false" });
            }
            Lit::Str(s) => {
                let v = s.value();
                self.push("\"");
                // BrightScript escapes a literal `"` as two quotes.
                for ch in v.chars() {
                    if ch == '"' {
                        self.push("\"\"");
                    } else {
                        self.out.push(ch);
                    }
                }
                self.push("\"");
            }
            _ => {
                return Err(syn::Error::new_spanned(l, "unsupported literal kind"));
            }
        }
        Ok(())
    }
}

fn map_type(ty: &Type) -> syn::Result<String> {
    if let Type::Path(p) = ty {
        let name = single_ident(&p.path)?;
        let result = match name.as_str() {
            "i8" | "i16" | "i32" | "u8" | "u16" | "u32" | "isize" | "usize" => "integer",
            "i64" | "u64" => "longinteger",
            "f32" => "float",
            "f64" => "double",
            "bool" => "boolean",
            "String" => "string",
            _ => {
                return Err(syn::Error::new_spanned(
                    ty,
                    format!("unsupported type `{}`", name),
                ));
            }
        };
        return Ok(result.to_string());
    }
    if let Type::Reference(r) = ty {
        if let Type::Path(p) = &*r.elem {
            let name = single_ident(&p.path)?;
            if name == "str" {
                return Ok("string".to_string());
            }
        }
        return Err(syn::Error::new_spanned(
            ty,
            "only `&str` is supported among reference types",
        ));
    }
    Err(syn::Error::new_spanned(ty, "unsupported type"))
}

fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

fn single_ident(path: &Path) -> syn::Result<String> {
    if path.segments.len() != 1 {
        return Err(syn::Error::new_spanned(
            path,
            "only single-segment paths are supported (no `module::name`)",
        ));
    }
    if !path.segments[0].arguments.is_empty() {
        return Err(syn::Error::new_spanned(
            path,
            "type arguments aren't supported",
        ));
    }
    Ok(path.segments[0].ident.to_string())
}

fn map_binop(op: &BinOp, span: Span) -> syn::Result<&'static str> {
    Ok(match op {
        BinOp::Add(_) => "+",
        BinOp::Sub(_) => "-",
        BinOp::Mul(_) => "*",
        BinOp::Div(_) => "/",
        BinOp::Rem(_) => "mod",
        BinOp::Eq(_) => "=",
        BinOp::Ne(_) => "<>",
        BinOp::Lt(_) => "<",
        BinOp::Gt(_) => ">",
        BinOp::Le(_) => "<=",
        BinOp::Ge(_) => ">=",
        BinOp::And(_) => "and",
        BinOp::Or(_) => "or",
        _ => {
            return Err(syn::Error::new(
                span,
                "unsupported binary operator (bitwise/shift/compound-assign not supported)",
            ));
        }
    })
}
