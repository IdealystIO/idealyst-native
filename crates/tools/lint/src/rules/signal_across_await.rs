//! `signal-across-await` — a component-scoped signal touched after an
//! `.await`, which aborts the app if the component unmounts mid-flight.
//!
//! ```ignore
//! #[component]
//! fn EditReport(id: ReportId) -> Element {
//!     let busy = signal(false);          // owned by THIS component's scope
//!     let on_save = move || {
//!         spawn_async(async move {
//!             save_report(id).await;     // ← scope can die here
//!             busy.set(false);           // ← writes a freed slot: abort
//!         });
//!     };
//!     // …
//! }
//! ```
//!
//! # Why an await is the dangerous line
//!
//! `spawn_async` is fully detached — it has no relationship with the scope
//! that spawned it. And every `.await` is a **flush boundary**: the host's
//! post-dispatch hook flushes after each future poll, so the world commits,
//! structural drivers run, and scopes are torn down *between two adjacent
//! lines of the same async block*. Navigation, a host rebuild, or a
//! `switch` re-key all unmount subtrees there.
//!
//! A `Signal<T>` is `Copy` and carries no ownership, so it slides into an
//! `async move` with the same gesture that puts it in a `ui!` tree — and
//! the compiler has no lifetime to object to. When the continuation
//! resumes, the handle names a slot its scope already freed, and
//! `runtime_world` raises `idealyst[stale-signal-handle]`.
//!
//! The same code is perfectly safe when the signal is root-owned (created
//! in `app()`, outside any component), which is why this rule only fires
//! inside `#[component]` fns: that is where `signal(…)` produces a handle
//! with a *shorter* life than a detached task.
//!
//! # Detection (high precision over recall)
//!
//! - only fns annotated `#[component]`;
//! - only bindings from a top-level `let NAME = signal(…)` in that body
//!   (a signal arriving via a prop or `inject` is owned elsewhere and
//!   invisible here);
//! - only inside an `async` block passed to `spawn_async(…)` — a block
//!   handed to a scope-anchored spawner is safe by construction and never
//!   matches;
//! - only uses that appear, in source order, after the block's first
//!   `.await` (so the safe prelude `busy.set(true); fetch().await;` is
//!   clean, while a write inside a `match fetch().await { … }` arm is
//!   not).
//!
//! Reads are flagged as well as writes: `valid.set(scoped.get())` dies on
//! the read half, and unlike a write a stale read can never be made
//! benign — there is no value to return.
//!
//! An `is_alive()` guard — either `if s.is_alive() { … }` or the bail-out
//! `if !s.is_alive() { return; }` — suppresses the writes it covers. That
//! is the declared-intent escape, the role `.get_untracked()` plays for
//! `snapshot-condition`. It stops at the next `.await`, because a probe
//! only proves liveness until the task suspends again.
//!
//! # Known false positive
//!
//! A **root** component (`#[component] fn app()`) never unmounts, so its
//! signals do outlive every task — but nothing in the source distinguishes
//! a root from a screen. Suppress per-file:
//! `// idealyst-lint-disable-file signal-across-await`.
//!
//! # What it deliberately misses
//!
//! Signals that reach the task through a prop, `inject`, or a captured
//! struct field; a `spawn_async` written inline inside a `ui!` prop (that
//! body is a DSL and does not re-parse as Rust); and the cross-scope case
//! where a *surviving* effect reads a signal owned by a dying scope. All
//! need ownership information this rule does not have.

use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::diagnostic::RawDiag;

pub(crate) const RULE: &str = "signal-across-await";

/// The spawner whose tasks are detached from the spawning scope. A block
/// handed to any other spawner is not this rule's business — that is the
/// forward-compatible escape hatch (a scope-anchored spawner, or an
/// explicitly-named detached one, simply never matches here).
const DETACHED_SPAWNER: &str = "spawn_async";

/// Signal operations that route through the arena and therefore abort on a
/// stale handle. Split by class so the message can say which half died.
const WRITE_OPS: &[&str] =
    &["set", "set_always", "set_untracked", "update", "update_untracked", "touch"];
const READ_OPS: &[&str] =
    &["get", "get_untracked", "peek", "with", "with_untracked", "read", "read_untracked"];

pub(crate) fn check_fn(item: &syn::ItemFn, out: &mut Vec<RawDiag>) {
    if !item.attrs.iter().any(|a| a.path().is_ident("component")) {
        return;
    }

    // Pass 1: component-scoped signal bindings.
    let candidates = scoped_signal_bindings(&item.block);
    if candidates.is_empty() {
        return;
    }

    // Pass 2: async blocks handed to the detached spawner, anywhere in the
    // body (usually nested inside an event-handler closure).
    let mut finder = DetachedSpawnFinder { blocks: Vec::new() };
    finder.visit_block(&item.block);
    for block in finder.blocks {
        check_async_block(&block, &candidates, out);
    }
}

/// Top-level `let NAME = …signal(…)…` bindings. The initializer only has
/// to *contain* a `signal(…)` call, so `signal(0).split()` and
/// `signal(v).read_only()` are covered; tuple patterns from `.split()`
/// contribute both halves.
fn scoped_signal_bindings(block: &syn::Block) -> Vec<String> {
    let mut names = Vec::new();
    for stmt in &block.stmts {
        let syn::Stmt::Local(local) = stmt else { continue };
        let Some(init) = &local.init else { continue };
        if !contains_signal_ctor(&init.expr) {
            continue;
        }
        collect_pat_idents(&local.pat, &mut names);
    }
    names
}

/// True when the expression contains a call to a function named `signal`.
fn contains_signal_ctor(expr: &syn::Expr) -> bool {
    struct F {
        found: bool,
    }
    impl<'ast> Visit<'ast> for F {
        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let syn::Expr::Path(p) = &*node.func {
                if p.path.segments.last().is_some_and(|s| s.ident == "signal") {
                    self.found = true;
                }
            }
            visit::visit_expr_call(self, node);
        }
    }
    let mut f = F { found: false };
    f.visit_expr(expr);
    f.found
}

/// Idents bound by a pattern — `x`, `(a, b)`, `mut x`, `x: T`.
fn collect_pat_idents(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(pi) => out.push(pi.ident.to_string()),
        syn::Pat::Type(pt) => collect_pat_idents(&pt.pat, out),
        syn::Pat::Tuple(pt) => {
            for elem in &pt.elems {
                collect_pat_idents(elem, out);
            }
        }
        _ => {}
    }
}

/// Collects the `async { … }` blocks passed as an argument to
/// `spawn_async(…)`.
struct DetachedSpawnFinder {
    blocks: Vec<syn::Block>,
}

impl<'ast> Visit<'ast> for DetachedSpawnFinder {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        let is_detached = match &*node.func {
            syn::Expr::Path(p) => {
                p.path.segments.last().is_some_and(|s| s.ident == DETACHED_SPAWNER)
            }
            _ => false,
        };
        if is_detached {
            for arg in &node.args {
                if let syn::Expr::Async(a) = arg {
                    self.blocks.push(a.block.clone());
                }
            }
        }
        visit::visit_expr_call(self, node);
    }

    /// `syn`'s visitor does not descend into macro token streams, and
    /// `effect!{ … }` is exactly where mount-time loads live — the
    /// "fetch on open" idiom is an `effect!` wrapping a `spawn_async`.
    /// Re-parse the body as a block so the walk continues inside it.
    /// Spans survive `parse2`, so reported positions stay correct.
    ///
    /// Only `effect!` is re-entered: its body is ordinary Rust. `ui!` /
    /// `jsx!` bodies are a DSL that would not parse, so a `spawn_async`
    /// written inline in an `on_click` prop inside `ui!` is a known miss.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let is_effect =
            node.path.segments.last().is_some_and(|s| s.ident == "effect");
        if is_effect {
            if let Ok(block) = syn::parse2::<syn::Block>(node.tokens.clone()) {
                self.visit_block(&block);
            }
        }
        visit::visit_macro(self, node);
    }
}

/// Flag every candidate signal op that appears, **in source order**, after
/// the block's first `.await`.
///
/// Source position rather than statement index: the writes that matter are
/// often nested inside the awaiting statement itself —
/// `let x = match fetch().await { Err(e) => { err.set(…) } }` suspends in
/// the scrutinee and runs the arm afterwards. Statement granularity misses
/// that entire shape. Comparing `(line, column)` still keeps the safe
/// prelude — `busy.set(true); fetch().await;` — clean, because the write
/// textually precedes the await.
///
/// Known miss: `busy.set(fetch().await)`, where the call's span starts
/// before the await it contains. Rare, and the containing statement is
/// usually flagged by another op anyway.
fn check_async_block(block: &syn::Block, candidates: &[String], out: &mut Vec<RawDiag>) {
    let Some(first_await) = first_await_pos(block) else {
        return; // no suspension point: the whole body runs in one turn
    };
    let mut guards = GuardCollector { ranges: Vec::new() };
    guards.visit_block(block);
    let mut finder = SignalOpFinder { candidates, hits: Vec::new() };
    finder.visit_block(block);
    for (name, method, span) in finder.hits {
        if pos(span) <= first_await {
            continue; // runs before the task can be suspended
        }
        if guards.ranges.iter().any(|(lo, hi)| *lo <= pos(span) && pos(span) <= *hi) {
            continue; // author declared intent with an `is_alive()` guard
        }
        let is_write = WRITE_OPS.contains(&method.as_str());
        let verb = if is_write { "written" } else { "read" };
        let consequence = if is_write {
            "the write lands on a freed slot"
        } else {
            "the read lands on a freed slot"
        };
        out.push(
            RawDiag::new(
                RULE,
                format!(
                    "`{name}` is owned by this component's scope but is {verb} after an \
                     `.await` — if the component unmounts while the task is in flight, \
                     {consequence} and the app aborts with `stale-signal-handle`"
                ),
                span,
            )
            .with_help(help_for(is_write)),
        );
    }
}

/// `(line, column)` of a span's start, as a comparable source position.
fn pos(span: proc_macro2::Span) -> (usize, usize) {
    let lc = span.start();
    (lc.line, lc.column)
}

/// `(line, column)` of a span's end.
fn pos_end(span: proc_macro2::Span) -> (usize, usize) {
    let lc = span.end();
    (lc.line, lc.column)
}

/// Source ranges the author has explicitly guarded with `is_alive()`.
///
/// Two shapes, both in the wild:
///
/// ```ignore
/// if busy.is_alive() { busy.set(false); }   // positive: the arm is guarded
///
/// if !busy.is_alive() { return; }           // bail-out: the REST of the
/// busy.set(false);                          // enclosing block is guarded
/// ```
///
/// This is the rule's declared-intent escape, the same role
/// `.get_untracked()` plays for `snapshot-condition`. It is deliberately
/// narrow: a probe only proves liveness until the *next* await, so a guard
/// followed by another `.await` leaves the writes after it flagged, which
/// is exactly the residual bug worth reporting.
struct GuardCollector {
    ranges: Vec<((usize, usize), (usize, usize))>,
}

impl<'ast> Visit<'ast> for GuardCollector {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let block_end = pos_end(block.brace_token.span.close());
        for stmt in &block.stmts {
            let syn::Stmt::Expr(syn::Expr::If(if_expr), _) = stmt else { continue };
            if !contains_is_alive(&if_expr.cond) {
                continue;
            }
            if matches!(&*if_expr.cond, syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Not(_)))
            {
                // Bail-out form: guarded only if the arm actually leaves.
                if block_diverges(&if_expr.then_branch) {
                    self.ranges.push((pos_end(stmt.span()), block_end));
                }
            } else {
                let b = &if_expr.then_branch;
                self.ranges.push((
                    pos(b.brace_token.span.open()),
                    pos_end(b.brace_token.span.close()),
                ));
            }
        }
        visit::visit_block(self, block);
    }
}

/// True when the expression contains an `.is_alive()` call.
fn contains_is_alive(expr: &syn::Expr) -> bool {
    struct F {
        found: bool,
    }
    impl<'ast> Visit<'ast> for F {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == "is_alive" {
                self.found = true;
            }
            visit::visit_expr_method_call(self, node);
        }
    }
    let mut f = F { found: false };
    f.visit_expr(expr);
    f.found
}

/// True when the block leaves its enclosing function/task via `return`,
/// not counting returns inside a nested closure or async block.
fn block_diverges(block: &syn::Block) -> bool {
    struct F {
        found: bool,
    }
    impl<'ast> Visit<'ast> for F {
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
        fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}
        fn visit_expr_return(&mut self, node: &'ast syn::ExprReturn) {
            self.found = true;
            visit::visit_expr_return(self, node);
        }
    }
    let mut f = F { found: false };
    f.visit_block(block);
    f.found
}

/// Source position of the block's earliest `.await`, ignoring awaits that
/// belong to a nested `async` block (those suspend that block, not this).
fn first_await_pos(block: &syn::Block) -> Option<(usize, usize)> {
    struct F {
        earliest: Option<(usize, usize)>,
    }
    impl<'ast> Visit<'ast> for F {
        fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}
        fn visit_expr_await(&mut self, node: &'ast syn::ExprAwait) {
            let p = pos(node.await_token.span);
            self.earliest = Some(match self.earliest {
                Some(cur) if cur <= p => cur,
                _ => p,
            });
            visit::visit_expr_await(self, node);
        }
    }
    let mut f = F { earliest: None };
    f.visit_block(block);
    f.earliest
}

fn help_for(is_write: bool) -> String {
    let mut help = String::from(
        "every `.await` is a flush boundary, so the component can be torn down between two \
         lines of the same async block. Prefer `resource(deps, fetcher)` for fetch-and-store \
         or `mutation(handler)` for submit-and-settle — both keep a liveness guard across the \
         await. If the state must outlive the component, create the signal outside the \
         component body so it is root-owned.",
    );
    if !is_write {
        help.push_str(
            " A stale READ can never be made safe — there is no value to return — so this one \
             has to be restructured, not guarded.",
        );
    }
    help
}

/// Finds `NAME.op(…)` where `NAME` is a candidate binding and `op` is an
/// arena-routed signal operation.
struct SignalOpFinder<'a> {
    candidates: &'a [String],
    hits: Vec<(String, String, proc_macro2::Span)>,
}

impl<'ast> Visit<'ast> for SignalOpFinder<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if let syn::Expr::Path(p) = &*node.receiver {
            if let Some(ident) = p.path.get_ident() {
                let name = ident.to_string();
                let method = node.method.to_string();
                let is_signal_op = WRITE_OPS.contains(&method.as_str())
                    || READ_OPS.contains(&method.as_str());
                if is_signal_op && self.candidates.iter().any(|c| *c == name) {
                    self.hits.push((name, method, node.span()));
                }
            }
        }
        visit::visit_expr_method_call(self, node);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Parse from real source text, not `quote!`: this rule compares
    /// source POSITIONS, and `quote!` stamps every token with the same
    /// call-site span, which would collapse all ordering to equal.
    fn diags(src: &str) -> Vec<RawDiag> {
        let item: syn::ItemFn = syn::parse_str(src).expect("test source must parse");
        let mut out = Vec::new();
        check_fn(&item, &mut out);
        out
    }

    #[test]
    fn flags_the_canonical_save_then_navigate() {
        let out = diags(
            r#"
#[component]
fn EditReport() -> Element {
    let busy = signal(false);
    let on_save = move || {
        spawn_async(async move {
            save_report().await;
            busy.set(false);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("busy"), "{out:?}");
        assert!(out[0].message.contains("written"), "{out:?}");
    }

    #[test]
    fn a_write_before_the_await_is_clean() {
        // The safe prelude: it runs before any suspension point.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_async(async move {
            busy.set(true);
            fetch().await;
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn flags_a_write_inside_the_awaiting_statement_s_match_arm() {
        // The shape statement-granularity missed: the block suspends in
        // the scrutinee, then runs the arm.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let load_error = signal(None);
    let go = move || {
        spawn_async(async move {
            let sheet = match timesheet().await {
                Ok(s) => s,
                Err(e) => {
                    load_error.set(Some(e));
                    return;
                }
            };
            use_it(sheet);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("load_error"), "{out:?}");
    }

    #[test]
    fn flags_a_read_after_the_await_with_a_read_specific_help() {
        // `valid.set(scoped.get())` — the half that can never be benign.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let scoped = signal(0);
    let go = move || {
        spawn_async(async move {
            thing().await;
            global.set(scoped.get());
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("read"), "{out:?}");
        assert!(
            out[0].help.as_deref().unwrap_or_default().contains("never be made safe"),
            "{out:?}"
        );
    }

    #[test]
    fn flags_every_offending_line_not_just_the_first() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let dirty = signal(false);
    let go = move || {
        spawn_async(async move {
            save().await;
            busy.set(false);
            dirty.set(false);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 2, "each line needs its own decision: {out:?}");
    }

    #[test]
    fn flags_a_write_after_a_second_await() {
        // The "probe expires after the next await" shape.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let rows = signal(0);
    let go = move || {
        spawn_async(async move {
            save().await;
            refresh().await;
            rows.set(1);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn split_halves_are_both_candidates() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let (count, set_count) = signal(0).split();
    let go = move || {
        spawn_async(async move {
            load().await;
            set_count.set(1);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn a_non_signal_binding_is_clean() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let store = make_store();
    let go = move || {
        spawn_async(async move {
            load().await;
            store.set(1);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_scope_anchored_spawner_never_matches() {
        // The forward-compatible escape: only the detached spawner is
        // this rule's business.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_scoped(async move {
            save().await;
            busy.set(false);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_task_with_no_await_is_clean() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_async(async move {
            busy.set(false);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_non_component_fn_is_ignored() {
        // App-level state is root-owned and outlives every task.
        let out = diags(
            r#"
fn app() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_async(async move {
            save().await;
            busy.set(false);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_nested_async_block_s_await_does_not_arm_the_outer_block() {
        // The inner block's suspension point is its own; the outer body
        // still runs in one turn, so the write is safe.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_async(async move {
            let inner = async { fetch().await };
            busy.set(true);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_positive_is_alive_guard_suppresses_the_arm() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_async(async move {
            save().await;
            if busy.is_alive() {
                busy.set(false);
            }
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_bail_out_is_alive_guard_suppresses_the_rest_of_the_block() {
        // CrewForge's prevailing shape.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let write_error = signal(None);
    let go = move || {
        spawn_async(async move {
            let wrote = save().await;
            if !busy.is_alive() {
                return;
            }
            match wrote {
                Ok(_) => {}
                Err(e) => write_error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_bail_out_guard_that_does_not_return_is_not_a_guard() {
        // Without the `return` the block falls through and the writes
        // still execute — flag them.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_async(async move {
            save().await;
            if !busy.is_alive() {
                log("gone");
            }
            busy.set(false);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn a_guard_does_not_cover_writes_after_a_later_await() {
        // A probe only proves liveness until the NEXT suspension point.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let rows = signal(0);
    let go = move || {
        spawn_async(async move {
            save().await;
            if busy.is_alive() {
                busy.set(false);
            }
            refresh().await;
            rows.set(1);
        });
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("rows"), "{out:?}");
    }

    #[test]
    fn descends_into_an_effect_macro_body() {
        // The mount-time load idiom: `effect!` wrapping a `spawn_async`.
        // `syn` does not walk macro bodies, so this needs the re-parse.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let loading = signal(false);
    effect!({
        loading.set(true);
        spawn_async(async move {
            fetch().await;
            loading.set(false);
        });
    });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("loading"), "{out:?}");
    }
}
