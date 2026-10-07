//! `signal-across-await` — a signal owned by a mortal scope, touched from a
//! detached task, which aborts the app if that scope is torn down while the
//! task is in flight.
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
//! # Why the task body is the dangerous place
//!
//! `spawn_async` is fully detached — it has no relationship with the scope
//! that spawned it — and two things put a teardown between the spawn and
//! the line that touches the signal:
//!
//! - **every `.await` is a flush boundary.** The host's post-dispatch hook
//!   flushes after each future poll, so the world commits, structural
//!   drivers run, and scopes are torn down *between two adjacent lines of
//!   the same async block*. Navigation, a host rebuild, or a `switch`
//!   re-key all unmount subtrees there.
//! - **on web, the body does not start in this turn.** The web executor
//!   (`web_glue::spawn_local`) queues the task, and its drain loop runs
//!   every queued *microtask* — which is where the dispatch's flush is
//!   queued — before it polls any task. So even a write *before* the first
//!   `.await`, or in a body with no await at all, runs after the flush of
//!   the event that spawned it. (Apple and Android poll the first segment
//!   synchronously inside `spawn`, so there the prelude does run in the
//!   caller's turn; the lint reports the web behaviour, which is the one
//!   that aborts.)
//!
//! A `Signal<T>` is `Copy` and carries no ownership, so it slides into an
//! `async move` with the same gesture that puts it in a `ui!` tree — and
//! the compiler has no lifetime to object to. When the task runs, the
//! handle names a slot its scope already freed, and `runtime_world`
//! raises `idealyst[stale-signal-handle]`.
//!
//! # What counts as a mortal signal
//!
//! - in a `#[component]` fn: a `let` whose initializer calls `signal(…)` /
//!   `memo(…)` (so `signal(0).split()` and `Form { v: signal(0) }` count),
//!   or a `let` annotated with a signal type;
//! - in ANY fn: a parameter typed `Signal` / `ReadSignal` / `WriteSignal`
//!   / `Memo` — a prop is owned by the parent, which unmounts just as
//!   easily — and the signal-typed fields of a parameter whose struct is
//!   declared in the same file (`props: &RowProps` → `props.n`);
//! - a field reached through a struct binding (`form.v.set(…)`).
//!
//! A locally created signal in a non-component fn is NOT a candidate: in
//! `app()` it is root-owned and outlives every task.
//!
//! # Indirect access
//!
//! The write does not have to be spelled inside the task:
//!
//! - **a local closure** (`let bump = move || n.set(…)`, or an
//!   `Rc<dyn Fn>` / `Box<dyn Fn>` built from one, or a `.clone()` of
//!   either) that touches a mortal signal, called from the task;
//! - **a callback parameter** (`on_err: Rc<dyn Fn(String)>`, `impl Fn`,
//!   a generic `F: Fn`) or a callback field of a same-file props struct,
//!   called from the task. The callee cannot see what the caller's
//!   closure writes; a caller's closure that writes its component's
//!   signals is the common case, so the call is reported where it is
//!   made.
//!
//! # Detection
//!
//! Only inside an `async` block passed to `spawn_async(…)`, or to the
//! FUTURE half of `spawn_then(…)` / `spawn_then_in(…)` (the callback is
//! where signal work belongs and is never scanned); a block handed to any
//! other spawner never matches.
//!
//! Every hit in the block is reported. A hit after the block's first
//! `.await` (in source order — a write inside a `match fetch().await { … }`
//! arm counts) gets the flush-boundary message; a hit before it gets the
//! deferred-start message, whose fix is different (do it before
//! spawning).
//!
//! Reads are flagged as well as writes: `valid.set(scoped.get())` dies on
//! the read half, and unlike a write a stale read can never be made
//! benign — there is no value to return.
//!
//! An `is_alive()` guard — either `if s.is_alive() { … }` or the bail-out
//! `if !s.is_alive() { return; }` — suppresses the hits it covers. That
//! is the declared-intent escape, the role `.peek()` plays for
//! `snapshot-condition`. It stops at the next `.await`, because a probe
//! only proves liveness until the task suspends again.
//!
//! # Known false positives
//!
//! A **root** component (`#[component] fn app()`) never unmounts, so its
//! signals do outlive every task — but nothing in the source distinguishes
//! a root from a screen. A callback parameter whose every caller passes a
//! root-owned closure is the same. Suppress per-line or per-file:
//! `// idealyst-lint-disable-file signal-across-await`.
//!
//! # What it deliberately misses
//!
//! Signals that reach the task through `inject`, or through a struct
//! declared in another file; a `spawn_async` written inline inside a `ui!`
//! prop (that body is a DSL and does not re-parse as Rust); and the
//! cross-scope case where a *surviving* effect reads a signal owned by a
//! dying scope. All need ownership information this rule does not have.
//!
//! # `spawn-then-handler-anchor`
//!
//! The second rule this module reports ([`ANCHOR_RULE`]) covers the trap
//! the first rule's own advice leads into: `spawn_then` called from an
//! event handler anchors to the node that mounted the handler, so when the
//! handler's own write rebuilds that control (`busy` driving the button's
//! `loading`) the result is silently dropped. It fires on a closure that
//! writes a mortal signal and calls a bare `spawn_then`, when that signal
//! shapes the same fn's `ui!` tree (a `loading` / `disabled` prop on a
//! non-`Button` tag, or an `if` / `match` condition), and the closure is
//! not wrapped by `ScopeAlive::wrap*` / `run_within`. idea-ui's `Button`
//! re-anchors its own `on_click`, which is why its `loading` prop is not
//! evidence. The fix is
//! `spawn_then_in(&alive, …)` with `alive` taken in the component body —
//! see `runtime_vocabulary::scoped_spawn`.

use std::collections::{HashMap, HashSet};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::diagnostic::RawDiag;

pub(crate) const RULE: &str = "signal-across-await";

/// A handler that writes a signal and then spawns with the ambient anchor.
pub(crate) const ANCHOR_RULE: &str = "spawn-then-handler-anchor";

/// Spawners whose async body this rule inspects, and which argument
/// holds it.
///
/// - `spawn_async(fut)` — fully detached; every async-block argument is
///   in scope for the rule.
/// - `spawn_then(fut, then)` / `spawn_then_in(&alive, fut, then)` — the
///   scope-safe forms. Their **callback** is exactly where signal work
///   belongs and is never scanned; their **future** is still plain
///   detached IO, so a signal touched there is the same bug.
///
/// A block handed to any other spawner never matches, which is the
/// forward-compatible escape hatch.
const INSPECTED_SPAWNERS: &[(&str, Option<usize>)] =
    &[("spawn_async", None), ("spawn_then", Some(0)), ("spawn_then_in", Some(1))];

/// Signal operations that route through the arena and therefore abort on a
/// stale handle. Split by class so the message can say which half died.
const WRITE_OPS: &[&str] =
    &["set", "set_always", "set_untracked", "update", "update_untracked", "touch"];
const READ_OPS: &[&str] =
    &["get", "get_untracked", "peek", "with", "with_untracked", "read", "read_untracked"];

/// Handle types whose slot lives in a scope's arena.
const SIGNAL_TYPES: &[&str] = &["Signal", "ReadSignal", "WriteSignal", "Memo"];

/// Constructors that allocate a scope-owned slot.
const SIGNAL_CTORS: &[&str] = &["signal", "memo"];

/// `ScopeAlive` methods that publish an explicit token around a handler,
/// re-anchoring every `spawn_then` reached from inside it.
const ANCHORING_WRAPPERS: &[&str] =
    &["wrap0", "wrap1", "wrap2", "wrap0_opt", "wrap1_opt", "wrap2_opt", "run_within"];

// ---------------------------------------------------------------------------
// File-level facts
// ---------------------------------------------------------------------------

/// Same-file struct declarations, so `props: &RowProps` can resolve
/// `props.n` to a signal and `props.on_done` to a callback.
#[derive(Default)]
pub(crate) struct FileContext {
    structs: HashMap<String, StructFields>,
}

#[derive(Default, Clone)]
struct StructFields {
    signals: HashSet<String>,
    callbacks: HashSet<String>,
}

impl FileContext {
    pub(crate) fn scan(file: &syn::File) -> Self {
        let mut cx = FileContext::default();
        cx.visit_file(file);
        cx
    }

    fn fields_of(&self, ty: &syn::Type) -> Option<&StructFields> {
        type_name(ty).and_then(|n| self.structs.get(&n))
    }
}

impl<'ast> Visit<'ast> for FileContext {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let generics = fn_generic_params(&node.generics);
        let mut fields = StructFields::default();
        for f in &node.fields {
            let Some(name) = &f.ident else { continue };
            if is_signal_type(&f.ty) {
                fields.signals.insert(name.to_string());
            } else if is_callback_type(&f.ty, &generics) {
                fields.callbacks.insert(name.to_string());
            }
        }
        if !fields.signals.is_empty() || !fields.callbacks.is_empty() {
            self.structs.insert(node.ident.to_string(), fields);
        }
        visit::visit_item_struct(self, node);
    }
}

// ---------------------------------------------------------------------------
// Per-fn ownership environment
// ---------------------------------------------------------------------------

/// Who owns a mortal signal — decides the wording, not the verdict.
#[derive(Clone, Copy, PartialEq)]
enum Owner {
    /// Created in this component's body.
    Component,
    /// Arrived as a parameter / prop: the caller's scope owns it.
    Caller,
}

/// Which accesses through a binding reach a mortal signal.
#[derive(Clone)]
enum Access {
    /// The binding itself, or any field chain under it.
    Any,
    /// Only these first-level fields (a known struct).
    Fields(HashSet<String>),
}

#[derive(Clone)]
struct Root {
    access: Access,
    owner: Owner,
}

/// How a callable reached the task.
#[derive(Clone)]
enum Callable {
    /// A closure defined in this fn that touches `signal`.
    Local { signal: String, owner: Owner },
    /// A callback parameter / prop: what it writes is invisible here.
    Param,
}

pub(crate) struct Env {
    roots: HashMap<String, Root>,
    callables: HashMap<String, Callable>,
    /// Param bindings whose same-file struct has callback fields.
    callback_fields: HashMap<String, HashSet<String>>,
}

impl Env {
    /// The mortal signal an access chain names, rendered for the message.
    fn signal_at(&self, chain: &(String, Vec<String>)) -> Option<(String, Owner)> {
        let (root, fields) = chain;
        let r = self.roots.get(root)?;
        let ok = match &r.access {
            Access::Any => true,
            Access::Fields(set) => fields.first().is_some_and(|f| set.contains(f)),
        };
        ok.then(|| (render_chain(chain), r.owner))
    }

    /// The callable a call expression's callee names.
    fn callable_at(&self, func: &syn::Expr) -> Option<(String, Callable)> {
        let chain = access_chain(func)?;
        if chain.1.is_empty() {
            return self.callables.get(&chain.0).map(|c| (chain.0.clone(), c.clone()));
        }
        let set = self.callback_fields.get(&chain.0)?;
        (chain.1.len() == 1 && set.contains(&chain.1[0]))
            .then(|| (render_chain(&chain), Callable::Param))
    }

    fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.callables.is_empty() && self.callback_fields.is_empty()
    }
}

fn build_env(
    attrs: &[syn::Attribute],
    sig: &syn::Signature,
    block: &syn::Block,
    cx: &FileContext,
) -> Env {
    let is_component = attrs.iter().any(|a| a.path().is_ident("component"));
    let generics = fn_generic_params(&sig.generics);
    let mut env = Env {
        roots: HashMap::new(),
        callables: HashMap::new(),
        callback_fields: HashMap::new(),
    };

    // Parameters: owned by the caller, in any fn.
    for input in &sig.inputs {
        let syn::FnArg::Typed(pt) = input else { continue };
        let syn::Pat::Ident(pi) = &*pt.pat else { continue };
        let name = pi.ident.to_string();
        if is_signal_type(&pt.ty) {
            env.roots.insert(name, Root { access: Access::Any, owner: Owner::Caller });
        } else if is_callback_type(&pt.ty, &generics) {
            env.callables.insert(name, Callable::Param);
        } else if let Some(fields) = cx.fields_of(&pt.ty) {
            if !fields.signals.is_empty() {
                env.roots.insert(
                    name.clone(),
                    Root { access: Access::Fields(fields.signals.clone()), owner: Owner::Caller },
                );
            }
            if !fields.callbacks.is_empty() {
                env.callback_fields.insert(name, fields.callbacks.clone());
            }
        }
    }

    let mut collector = LetCollector { lets: Vec::new() };
    collector.visit_block(block);
    let lets = collector.lets;

    // Locally created signals: component bodies only (`app()` state is
    // root-owned and outlives every task).
    if is_component {
        for (pat, ty, init) in &lets {
            let mut names = Vec::new();
            collect_pat_idents(pat, &mut names);
            let access = if let Some(fields) = ty.as_ref().and_then(|t| cx.fields_of(t)) {
                Some(Access::Fields(fields.signals.clone()))
            } else if ty.as_ref().is_some_and(is_signal_type) {
                Some(Access::Any)
            } else if let Some(init) = init {
                match init {
                    syn::Expr::Struct(es) if cx.structs.contains_key(&last_ident(&es.path)) => {
                        Some(Access::Fields(cx.structs[&last_ident(&es.path)].signals.clone()))
                    }
                    _ if contains_signal_ctor(init) => Some(Access::Any),
                    _ => None,
                }
            } else {
                None
            };
            let Some(access) = access else { continue };
            if matches!(&access, Access::Fields(f) if f.is_empty()) {
                continue;
            }
            for n in names {
                env.roots.insert(n, Root { access: access.clone(), owner: Owner::Component });
            }
        }
    }

    // Local closures that touch a mortal signal — or call one that does —
    // and their aliases. Fixed point: `let a = …; let b = move || a();`.
    loop {
        let mut changed = false;
        for (pat, _, init) in &lets {
            let syn::Pat::Ident(pi) = strip_pat_type(pat) else { continue };
            let name = pi.ident.to_string();
            if env.callables.contains_key(&name) || env.roots.contains_key(&name) {
                continue;
            }
            let Some(init) = init else { continue };
            let found = if let Some(alias) = alias_of(init) {
                env.callables.get(&alias).cloned()
            } else if contains_closure(init) {
                first_touch(init, &env)
            } else {
                None
            };
            if let Some(c) = found {
                env.callables.insert(name, c);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    env
}

/// What a closure-bearing initializer touches first, as a [`Callable`].
fn first_touch(init: &syn::Expr, env: &Env) -> Option<Callable> {
    let mut f = HitFinder { env, hits: Vec::new() };
    f.visit_expr(init);
    f.hits.into_iter().next().map(|h| match h.kind {
        HitKind::Signal { name, owner, .. } => Callable::Local { signal: name, owner },
        HitKind::Call { callable: Callable::Local { signal, owner }, .. } => {
            Callable::Local { signal, owner }
        }
        // Calling a callback param from a closure makes the closure as
        // opaque as the param.
        HitKind::Call { callable: Callable::Param, .. } => Callable::Param,
    })
}

/// Every `let` in the fn body (outside async blocks, which bind task
/// locals), as `(pattern, annotated type, initializer)`.
struct LetCollector {
    lets: Vec<(syn::Pat, Option<syn::Type>, Option<syn::Expr>)>,
}

impl<'ast> Visit<'ast> for LetCollector {
    fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}
    fn visit_local(&mut self, node: &'ast syn::Local) {
        let ty = match &node.pat {
            syn::Pat::Type(pt) => Some((*pt.ty).clone()),
            _ => None,
        };
        let init = node.init.as_ref().map(|i| (*i.expr).clone());
        self.lets.push((node.pat.clone(), ty, init));
        visit::visit_local(self, node);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub(crate) fn check_fn(
    attrs: &[syn::Attribute],
    sig: &syn::Signature,
    block: &syn::Block,
    cx: &FileContext,
    out: &mut Vec<RawDiag>,
) {
    let env = build_env(attrs, sig, block, cx);
    if env.is_empty() {
        return;
    }

    // Async blocks handed to the detached spawners, anywhere in the body
    // (usually nested inside an event-handler closure).
    let mut finder = DetachedSpawnFinder { blocks: Vec::new() };
    finder.visit_block(block);
    for b in finder.blocks {
        check_async_block(&b, &env, out);
    }

    check_handler_anchors(block, &env, out);
}

/// Collects the `async { … }` blocks passed to an inspected spawner.
struct DetachedSpawnFinder {
    blocks: Vec<syn::Block>,
}

impl<'ast> Visit<'ast> for DetachedSpawnFinder {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Some((_, only_arg)) = spawner_of(node) {
            for (i, arg) in node.args.iter().enumerate() {
                if only_arg.is_some_and(|want| want != i) {
                    continue; // e.g. `spawn_then`'s callback — signals belong there
                }
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
        if let Some(block) = effect_body(node) {
            self.visit_block(&block);
        }
        visit::visit_macro(self, node);
    }
}

fn spawner_of(node: &syn::ExprCall) -> Option<(&'static str, Option<usize>)> {
    let syn::Expr::Path(p) = &*node.func else { return None };
    let last = p.path.segments.last()?;
    INSPECTED_SPAWNERS.iter().find(|(name, _)| last.ident == *name).copied()
}

fn effect_body(node: &syn::Macro) -> Option<syn::Block> {
    let is_effect = node.path.segments.last().is_some_and(|s| s.ident == "effect");
    if !is_effect {
        return None;
    }
    syn::parse2::<syn::Block>(node.tokens.clone()).ok()
}

/// Report every mortal-signal touch and every risky call in the block,
/// split by whether it runs after the block's first `.await`.
///
/// Source position rather than statement index: the writes that matter are
/// often nested inside the awaiting statement itself —
/// `let x = match fetch().await { Err(e) => { err.set(…) } }` suspends in
/// the scrutinee and runs the arm afterwards.
///
/// Known imprecision: `busy.set(fetch().await)`, where the call's span
/// starts before the await it contains, is reported with the deferred
/// wording rather than the flush-boundary one. Both are real.
fn check_async_block(block: &syn::Block, env: &Env, out: &mut Vec<RawDiag>) {
    let first_await = first_await_pos(block);
    let mut guards = GuardCollector { ranges: Vec::new() };
    guards.visit_block(block);
    let mut finder = HitFinder { env, hits: Vec::new() };
    finder.visit_block(block);
    for hit in finder.hits {
        let at = pos(hit.span);
        if guards.ranges.iter().any(|(lo, hi)| *lo <= at && at <= *hi) {
            continue; // author declared intent with an `is_alive()` guard
        }
        let after_await = first_await.is_some_and(|fa| at > fa);
        out.push(diag_for(&hit, after_await));
    }
}

fn diag_for(hit: &Hit, after_await: bool) -> RawDiag {
    let when = if after_await {
        "after an `.await` in a detached task — if that scope is torn down while the task \
         is in flight"
    } else {
        "inside `spawn_async`, which does not run its body in this turn (on web the first \
         poll is queued behind this event's flush) — if that flush tears the scope down"
    };
    let (message, is_write) = match &hit.kind {
        HitKind::Signal { name, owner, is_write } => {
            let owned = owner_phrase(*owner);
            let verb = if *is_write { "written" } else { "read" };
            let half = if *is_write { "write" } else { "read" };
            (
                format!(
                    "`{name}` is owned by {owned} but is {verb} {when}, the {half} lands on a \
                     freed slot and the app aborts with `stale-signal-handle`"
                ),
                *is_write,
            )
        }
        HitKind::Call { name, callable: Callable::Local { signal, owner } } => (
            format!(
                "`{name}` touches `{signal}`, which is owned by {}, and is called {when}, \
                 that access lands on a freed slot and the app aborts with \
                 `stale-signal-handle`",
                owner_phrase(*owner)
            ),
            true,
        ),
        HitKind::Call { name, callable: Callable::Param } => (
            format!(
                "`{name}` is a callback supplied by the caller and is called {when} — a \
                 caller's closure that writes its component's signals lands on a freed slot \
                 and the app aborts with `stale-signal-handle`"
            ),
            true,
        ),
    };
    RawDiag::new(RULE, message, hit.span).with_help(help_for(after_await, is_write, &hit.kind))
}

fn owner_phrase(owner: Owner) -> &'static str {
    match owner {
        Owner::Component => "this component's scope",
        Owner::Caller => "the caller's scope (it arrived as a parameter / prop)",
    }
}

fn help_for(after_await: bool, is_write: bool, kind: &HitKind) -> String {
    let mut help = if after_await {
        String::from(
            "every `.await` is a flush boundary, so the scope can be torn down between two \
             lines of the same async block. Move the signal work into `spawn_then(future, \
             |result| { … })` — its callback runs inside a turn or not at all, so the update \
             is atomic. `resource(deps, fetcher)` and `mutation(handler)` carry the same \
             guard for fetch-and-store / submit-and-settle. If the state must outlive the \
             component, create the signal outside the component body so it is root-owned.",
        )
    } else {
        String::from(
            "do the synchronous part BEFORE calling `spawn_async` (and drop the spawn \
             entirely if the body never awaits); put work that needs the result in \
             `spawn_then(future, |result| { … })`'s callback, which runs inside a turn or \
             not at all.",
        )
    };
    if matches!(kind, HitKind::Call { callable: Callable::Param, .. }) {
        help.push_str(
            " For a callback, return the result through `spawn_then(future, move |r| \
             on_done(r))`: the callback then runs only while the scope that called this fn \
             is alive.",
        );
    }
    if !is_write {
        help.push_str(
            " A stale READ can never be made safe — there is no value to return — so this one \
             has to be restructured, not guarded.",
        );
    }
    help.push_str(
        " From an event handler whose own write rebuilds its control (a `busy` flag driving \
         `loading`), plain `spawn_then` binds to that control and drops the result: take \
         `let alive = ScopeAlive::current();` in the component body and use \
         `spawn_then_in(&alive, future, |result| { … })`.",
    );
    help
}

// ---------------------------------------------------------------------------
// Hit finding
// ---------------------------------------------------------------------------

enum HitKind {
    Signal { name: String, owner: Owner, is_write: bool },
    Call { name: String, callable: Callable },
}

struct Hit {
    kind: HitKind,
    span: proc_macro2::Span,
}

/// Finds mortal-signal ops (`n.set(…)`, `form.v.get()`, `props.n.set(…)`)
/// and calls to risky callables (`bump()`, `on_done(v)`,
/// `(props.on_done)(v)`).
struct HitFinder<'a> {
    env: &'a Env,
    hits: Vec<Hit>,
}

impl<'ast> Visit<'ast> for HitFinder<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        let is_write = WRITE_OPS.contains(&method.as_str());
        if is_write || READ_OPS.contains(&method.as_str()) {
            if let Some((name, owner)) =
                access_chain(&node.receiver).and_then(|c| self.env.signal_at(&c))
            {
                self.hits.push(Hit {
                    kind: HitKind::Signal { name, owner, is_write },
                    span: node.span(),
                });
            }
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Some((name, callable)) = self.env.callable_at(&node.func) {
            self.hits.push(Hit { kind: HitKind::Call { name, callable }, span: node.span() });
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if let Some(block) = effect_body(node) {
            self.visit_block(&block);
        }
        visit::visit_macro(self, node);
    }
}

// ---------------------------------------------------------------------------
// spawn-then-handler-anchor
// ---------------------------------------------------------------------------

/// Flag a closure that writes a mortal signal and calls a bare
/// `spawn_then` with no explicit anchor in effect.
///
/// The ambient token at handler time is the node that mounted the
/// handler. If the handler's own write rebuilds that node, the token
/// flips on the next flush and the callback never runs. The lint cannot
/// see which writes rebuild the control, so any mortal write in the same
/// handler arms it; the fix — `spawn_then_in` with the component's token
/// — is correct whether or not this particular write rebuilds anything.
fn check_handler_anchors(block: &syn::Block, env: &Env, out: &mut Vec<RawDiag>) {
    let mut shaping = ShapingUses { names: HashSet::new() };
    shaping.visit_block(block);
    if shaping.names.is_empty() {
        return; // no signal shapes this fn's tree: no write can rebuild a control
    }
    let mut wrapped = WrappedNames { names: HashSet::new() };
    wrapped.visit_block(block);
    let mut v =
        HandlerWalker { env, shaping: shaping.names, wrapped: wrapped.names, anchored: 0, out };
    v.visit_block(block);
}

/// Idents that SHAPE the fn's `ui!` / `jsx!` tree — the evidence that a
/// write to them can rebuild a control:
///
/// - the value of a `loading = …` / `disabled = …` prop (a live
///   structural prop rebuilds the pressable in place), except on a
///   `Button` tag — idea-ui's `Button` publishes its own body-scope
///   token around `on_click`, so a handler passed to it is already
///   anchored past its own rebuilds (`idea-ui/tests/loading_button_spawn.rs`);
/// - an `if` / `match` condition (the arm, and every control in it, is
///   torn down when it flips).
///
/// Without one of these the handler's writes cannot rebuild the control
/// it is mounted on, and the rule stays quiet — a `status` text or an
/// input's `value` is the common, harmless case.
struct ShapingUses {
    names: HashSet<String>,
}

impl<'ast> Visit<'ast> for ShapingUses {
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let last = node.path.segments.last().map(|s| s.ident.to_string());
        if matches!(last.as_deref(), Some("ui" | "jsx")) {
            scan_shaping(node.tokens.clone(), None, &mut self.names);
        }
        visit::visit_macro(self, node);
    }
}

fn scan_shaping(
    stream: proc_macro2::TokenStream,
    tag: Option<&str>,
    out: &mut HashSet<String>,
) {
    use proc_macro2::{Delimiter, TokenTree};
    let toks: Vec<TokenTree> = stream.into_iter().collect();
    let idents_until = |from: usize, stop: &dyn Fn(&TokenTree) -> bool, out: &mut HashSet<String>| {
        let mut j = from;
        while j < toks.len() && !stop(&toks[j]) {
            collect_idents(&toks[j], out);
            j += 1;
        }
    };
    let mut prev_ident: Option<String> = None;
    for (i, t) in toks.iter().enumerate() {
        match t {
            TokenTree::Ident(id) => {
                let name = id.to_string();
                let is_assign = matches!(toks.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '=')
                    && !matches!(toks.get(i + 2), Some(TokenTree::Punct(p)) if p.as_char() == '=');
                if (name == "loading" || name == "disabled") && is_assign && tag != Some("Button") {
                    idents_until(
                        i + 2,
                        &|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ','),
                        out,
                    );
                } else if name == "if" || name == "match" {
                    idents_until(
                        i + 1,
                        &|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace),
                        out,
                    );
                }
                prev_ident = Some(name);
            }
            TokenTree::Group(g) => {
                let inner_tag =
                    if g.delimiter() == Delimiter::Parenthesis { prev_ident.as_deref() } else { tag };
                scan_shaping(g.stream(), inner_tag, out);
                prev_ident = None;
            }
            _ => prev_ident = None,
        }
    }
}

fn collect_idents(t: &proc_macro2::TokenTree, out: &mut HashSet<String>) {
    match t {
        proc_macro2::TokenTree::Ident(i) => {
            out.insert(i.to_string());
        }
        proc_macro2::TokenTree::Group(g) => {
            for t in g.stream() {
                collect_idents(&t, out);
            }
        }
        _ => {}
    }
}

/// Bindings handed to a wrapper later — `let h = Rc::new(…);
/// let h = alive.wrap0(h);` — so the closure that initialized them is
/// anchored even though it is not lexically inside the wrapper call.
struct WrappedNames {
    names: HashSet<String>,
}

impl<'ast> Visit<'ast> for WrappedNames {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if ANCHORING_WRAPPERS.contains(&node.method.to_string().as_str()) {
            for a in &node.args {
                if let Some(n) = alias_of(a) {
                    self.names.insert(n);
                }
            }
        }
        visit::visit_expr_method_call(self, node);
    }
}

struct HandlerWalker<'a, 'o> {
    env: &'a Env,
    /// Idents that shape the tree (see [`ShapingUses`]).
    shaping: HashSet<String>,
    wrapped: HashSet<String>,
    /// >0 while inside the args of a `ScopeAlive` wrapper / `run_within`,
    /// or a `spawn_then` callback (which runs under `run_within` already).
    anchored: usize,
    out: &'o mut Vec<RawDiag>,
}

impl<'ast> Visit<'ast> for HandlerWalker<'_, '_> {
    fn visit_local(&mut self, node: &'ast syn::Local) {
        let is_wrapped = matches!(strip_pat_type(&node.pat), syn::Pat::Ident(pi)
            if self.wrapped.contains(&pi.ident.to_string()));
        if is_wrapped {
            self.anchored += 1;
            visit::visit_local(self, node);
            self.anchored -= 1;
        } else {
            visit::visit_local(self, node);
        }
    }

    // Task bodies are `signal-across-await`'s business; a `spawn_then`
    // reached from one has no ambient scope at all, which that rule's
    // call/op findings already cover.
    fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if ANCHORING_WRAPPERS.contains(&node.method.to_string().as_str()) {
            self.visit_expr(&node.receiver);
            self.anchored += 1;
            for a in &node.args {
                self.visit_expr(a);
            }
            self.anchored -= 1;
            return;
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if is_named_call(node, "spawn_then") || is_named_call(node, "spawn_then_in") {
            // The callback runs under `run_within(alive)`; a nested
            // `spawn_then` there inherits the parent's token.
            let cb = if is_named_call(node, "spawn_then") { 1 } else { 2 };
            for (i, a) in node.args.iter().enumerate() {
                if i == cb {
                    self.anchored += 1;
                    self.visit_expr(a);
                    self.anchored -= 1;
                } else {
                    self.visit_expr(a);
                }
            }
            return;
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        if self.anchored == 0 {
            let mut scan = HandlerBodyScan {
                env: self.env,
                shaping: &self.shaping,
                write: None,
                spawns: Vec::new(),
            };
            scan.visit_expr(&node.body);
            if let Some(write) = scan.write {
                for span in scan.spawns {
                    self.out.push(
                        RawDiag::new(
                            ANCHOR_RULE,
                            format!(
                                "`spawn_then` in a handler that also writes `{write}`, which \
                                 shapes this component's tree — the task is anchored to the \
                                 control this handler is mounted on, so if that write rebuilds \
                                 the control the callback is silently dropped"
                            ),
                            span,
                        )
                        .with_help(
                            "anchor the task to the component instead: take `let alive = \
                             ScopeAlive::current();` in the component body and call \
                             `spawn_then_in(&alive, future, |result| { … })` — or wrap the \
                             whole handler with `alive.wrap0(…)`. idea-ui's `Button` already \
                             re-anchors its `on_click`; a control built from primitives (or \
                             any component that does not) needs it here.",
                        ),
                    );
                }
            }
        }
        visit::visit_expr_closure(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if let Some(block) = effect_body(node) {
            self.visit_block(&block);
        }
        visit::visit_macro(self, node);
    }
}

/// One handler body, not descending into nested closures (each is its
/// own handler, walked separately) or async blocks.
struct HandlerBodyScan<'a> {
    env: &'a Env,
    shaping: &'a HashSet<String>,
    /// The first write to a signal that shapes the tree.
    write: Option<String>,
    spawns: Vec<proc_macro2::Span>,
}

impl<'ast> Visit<'ast> for HandlerBodyScan<'_> {
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if ANCHORING_WRAPPERS.contains(&node.method.to_string().as_str()) {
            return; // `alive.run_within(|| spawn_then(…))` is anchored
        }
        if self.write.is_none() && WRITE_OPS.contains(&node.method.to_string().as_str()) {
            if let Some(chain) = access_chain(&node.receiver) {
                let shapes = self.shaping.contains(&chain.0)
                    || chain.1.last().is_some_and(|f| self.shaping.contains(f));
                if shapes {
                    if let Some((name, _)) = self.env.signal_at(&chain) {
                        self.write = Some(name);
                    }
                }
            }
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if is_named_call(node, "spawn_then") {
            self.spawns.push(node.func.span());
        }
        if self.write.is_none() {
            if let Some((name, Callable::Local { signal, .. })) = self.env.callable_at(&node.func)
            {
                let root = signal.split('.').next().unwrap_or_default();
                if self.shaping.contains(root) {
                    self.write = Some(format!("{signal}` (through `{name}`"));
                }
            }
        }
        visit::visit_expr_call(self, node);
    }
}

fn is_named_call(node: &syn::ExprCall, name: &str) -> bool {
    matches!(&*node.func, syn::Expr::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == name))
}

// ---------------------------------------------------------------------------
// Syntax helpers
// ---------------------------------------------------------------------------

/// `root.a.b` → `("root", ["a", "b"])`, through parens and `&`.
fn access_chain(expr: &syn::Expr) -> Option<(String, Vec<String>)> {
    match expr {
        syn::Expr::Paren(p) => access_chain(&p.expr),
        syn::Expr::Group(g) => access_chain(&g.expr),
        syn::Expr::Reference(r) => access_chain(&r.expr),
        syn::Expr::Path(p) => p.path.get_ident().map(|i| (i.to_string(), Vec::new())),
        syn::Expr::Field(f) => {
            let (root, mut fields) = access_chain(&f.base)?;
            fields.push(match &f.member {
                syn::Member::Named(i) => i.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            });
            Some((root, fields))
        }
        _ => None,
    }
}

fn render_chain((root, fields): &(String, Vec<String>)) -> String {
    std::iter::once(root.as_str()).chain(fields.iter().map(String::as_str)).collect::<Vec<_>>().join(".")
}

/// `a`, `a.clone()`, `Rc::clone(&a)` → `a`.
fn alias_of(init: &syn::Expr) -> Option<String> {
    match init {
        syn::Expr::Path(p) => p.path.get_ident().map(|i| i.to_string()),
        syn::Expr::MethodCall(m) if m.method == "clone" && m.args.is_empty() => {
            alias_of(&m.receiver)
        }
        syn::Expr::Call(c) if c.args.len() == 1 => {
            let syn::Expr::Path(p) = &*c.func else { return None };
            let is_clone = p.path.segments.last().is_some_and(|s| s.ident == "clone");
            if is_clone { alias_of(&c.args[0]) } else { None }
        }
        syn::Expr::Reference(r) => alias_of(&r.expr),
        syn::Expr::Paren(p) => alias_of(&p.expr),
        _ => None,
    }
}

fn contains_closure(expr: &syn::Expr) -> bool {
    struct F {
        found: bool,
    }
    impl<'ast> Visit<'ast> for F {
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {
            self.found = true;
        }
    }
    let mut f = F { found: false };
    f.visit_expr(expr);
    f.found
}

/// True when the expression contains a call to `signal(…)` / `memo(…)`.
fn contains_signal_ctor(expr: &syn::Expr) -> bool {
    struct F {
        found: bool,
    }
    impl<'ast> Visit<'ast> for F {
        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let syn::Expr::Path(p) = &*node.func {
                if p.path.segments.last().is_some_and(|s| SIGNAL_CTORS.iter().any(|c| s.ident == c)) {
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

fn strip_pat_type(pat: &syn::Pat) -> &syn::Pat {
    match pat {
        syn::Pat::Type(pt) => strip_pat_type(&pt.pat),
        p => p,
    }
}

fn last_ident(path: &syn::Path) -> String {
    path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
}

/// The named type behind `T`, `&T`, `&mut T` — the last path segment.
fn type_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => type_name(&r.elem),
        syn::Type::Paren(p) => type_name(&p.elem),
        syn::Type::Group(g) => type_name(&g.elem),
        syn::Type::Path(p) => Some(last_ident(&p.path)),
        _ => None,
    }
}

fn is_signal_type(ty: &syn::Type) -> bool {
    type_name(ty).is_some_and(|n| SIGNAL_TYPES.contains(&n.as_str()))
}

/// `Rc<dyn Fn(..)>`, `Box<dyn FnMut>`, `impl Fn`, `&dyn Fn`, or a
/// generic parameter bounded by an `Fn` trait.
fn is_callback_type(ty: &syn::Type, fn_generics: &HashSet<String>) -> bool {
    struct F<'g> {
        generics: &'g HashSet<String>,
        found: bool,
    }
    impl<'ast> Visit<'ast> for F<'_> {
        fn visit_trait_bound(&mut self, node: &'ast syn::TraitBound) {
            if is_fn_trait(&node.path) {
                self.found = true;
            }
            visit::visit_trait_bound(self, node);
        }
        fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
            if node.qself.is_none() {
                if let Some(i) = node.path.get_ident() {
                    if self.generics.contains(&i.to_string()) {
                        self.found = true;
                    }
                }
            }
            visit::visit_type_path(self, node);
        }
    }
    let mut f = F { generics: fn_generics, found: false };
    f.visit_type(ty);
    f.found
}

fn is_fn_trait(path: &syn::Path) -> bool {
    path.segments.last().is_some_and(|s| s.ident == "Fn" || s.ident == "FnMut" || s.ident == "FnOnce")
}

/// Generic type params bounded by an `Fn` trait, inline or in `where`.
fn fn_generic_params(generics: &syn::Generics) -> HashSet<String> {
    let bounded = |bounds: &syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>| {
        bounds.iter().any(|b| matches!(b, syn::TypeParamBound::Trait(t) if is_fn_trait(&t.path)))
    };
    let mut out = HashSet::new();
    for p in generics.type_params() {
        if bounded(&p.bounds) {
            out.insert(p.ident.to_string());
        }
    }
    if let Some(w) = &generics.where_clause {
        for pred in &w.predicates {
            let syn::WherePredicate::Type(pt) = pred else { continue };
            if let syn::Type::Path(tp) = &pt.bounded_ty {
                if let Some(i) = tp.path.get_ident() {
                    if bounded(&pt.bounds) {
                        out.insert(i.to_string());
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Positions and `is_alive()` guards
// ---------------------------------------------------------------------------

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
/// `.peek()` plays for `snapshot-condition`. It is deliberately
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse from real source text, not `quote!`: this rule compares
    /// source POSITIONS, and `quote!` stamps every token with the same
    /// call-site span, which would collapse all ordering to equal.
    fn diags(src: &str) -> Vec<RawDiag> {
        rule_diags(src, RULE)
    }

    fn anchor_diags(src: &str) -> Vec<RawDiag> {
        rule_diags(src, ANCHOR_RULE)
    }

    /// A whole file, through the shared visitor — same-file structs feed
    /// the props / field cases.
    fn rule_diags(src: &str, rule: &str) -> Vec<RawDiag> {
        let file = syn::parse_file(src).expect("test source must parse");
        crate::rules::collect(&file).into_iter().filter(|d| d.rule == rule).collect()
    }

    fn is_deferred(d: &RawDiag) -> bool {
        d.message.contains("does not run its body in this turn")
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

    /// Regression (case 6 of the report): the prelude was exempt on the
    /// belief that it runs before the task can be suspended. On web it
    /// does not run in this turn at all — web-glue drains queued
    /// microtasks (where the dispatch's flush sits) before polling any
    /// task — so it is reported, with the deferred-start wording.
    #[test]
    fn regression_a_write_before_the_await_is_flagged_as_deferred() {
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
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(is_deferred(&out[0]), "{out:?}");
        assert!(
            out[0].help.as_deref().unwrap_or_default().contains("BEFORE calling `spawn_async`"),
            "{out:?}"
        );
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

    /// Regression (case 6): no await at all is still a deferred start.
    #[test]
    fn regression_a_task_with_no_await_is_flagged_as_deferred() {
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
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(is_deferred(&out[0]), "{out:?}");
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
        // never awaits, so the write gets the deferred wording, not the
        // flush-boundary one.
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
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(is_deferred(&out[0]), "{out:?}");
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

    #[test]
    fn spawn_then_callback_is_never_scanned() {
        // The callback is exactly where signal work belongs — it runs
        // inside a turn or not at all.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_then(async move { save().await }, move |_saved| {
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
    fn spawn_then_future_half_is_still_scanned() {
        // Signals do not belong in the IO half; after an await there it
        // is the same bug as a raw `spawn_async`.
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let go = move || {
        spawn_then(
            async move {
                save().await;
                busy.set(false);
            },
            move |_| {},
        );
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("busy"), "{out:?}");
    }

    // -----------------------------------------------------------------
    // The reported misses (CLI 1.5.2). Each is the same runtime abort.
    // -----------------------------------------------------------------

    /// Case 1: the write is inside a local closure the task calls.
    #[test]
    fn regression_write_through_a_local_closure_called_after_the_await() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let n = signal(0u32);
    let bump = move || n.set(n.get() + 1);
    spawn_async(async move { let _ = fetch().await; bump(); });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`bump` touches `n`"), "{out:?}");
        assert!(!is_deferred(&out[0]), "{out:?}");
    }

    /// Case 2: an `Rc<dyn Fn>` built from a closure, cloned before use.
    #[test]
    fn regression_write_through_an_rc_dyn_fn_callback() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let n = signal(0u32);
    let on_done: Rc<dyn Fn(u32)> = Rc::new(move |v| n.set(v));
    let cb = on_done.clone();
    spawn_async(async move { let v = fetch().await; cb(v); });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`cb` touches `n`"), "{out:?}");
    }

    /// Case 3: a `Signal<T>` prop is owned by the parent — just as mortal.
    #[test]
    fn regression_signal_prop_is_a_candidate() {
        let out = diags(
            r#"
#[component]
pub fn Row(n: Signal<u32>) -> Element {
    spawn_async(async move { let v = fetch().await; n.set(v); });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("the caller's scope"), "{out:?}");
    }

    /// Case 3, explicit-props form: a same-file `#[props]` struct's
    /// signal field, reached through `props.n`.
    #[test]
    fn regression_signal_field_of_a_props_struct_is_a_candidate() {
        let out = diags(
            r#"
#[props]
pub struct RowProps { pub n: Signal<u32>, pub label: String }

#[component]
pub fn Row(props: &RowProps) -> Element {
    let n = props.n;
    let label = props.label.clone();
    spawn_async(async move { let v = fetch().await; props.n.set(v); });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`props.n`"), "{out:?}");
    }

    /// Case 4: a signal held in a struct field.
    #[test]
    fn regression_signal_inside_a_struct_field() {
        let out = diags(
            r#"
#[derive(Clone, Copy)]
struct Form { v: Signal<u32>, count: u32 }

#[component]
fn A() -> Element {
    let form = Form { v: signal(0), count: 0 };
    spawn_async(async move { let v = fetch().await; form.v.set(v); let c = form.count; });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`form.v`"), "{out:?}");
    }

    /// Case 4 without the struct declaration in view: the initializer
    /// still allocates a signal, so any field access counts.
    #[test]
    fn signal_struct_from_another_file_matches_any_field() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let form = Form { v: signal(0) };
    spawn_async(async move { let v = fetch().await; form.v.set(v); });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
    }

    /// Case 5: a helper (not a component) takes a callback and calls it
    /// from its task. The caller's closure is invisible; the call is not.
    #[test]
    fn regression_callback_parameter_called_from_a_task() {
        let out = diags(
            r#"
pub fn start(on_err: Rc<dyn Fn(String)>) {
    spawn_async(async move { let _ = fetch().await; on_err("x".into()); });
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("callback supplied by the caller"), "{out:?}");
        assert!(out[0].help.as_deref().unwrap_or_default().contains("on_done(r)"), "{out:?}");
    }

    #[test]
    fn callback_parameter_spellings() {
        for sig in [
            "fn start(cb: impl Fn() + 'static)",
            "fn start<F: Fn() + 'static>(cb: F)",
            "fn start<F>(cb: F) where F: FnOnce() + 'static",
            "fn start(cb: Box<dyn FnMut()>)",
        ] {
            let src = format!(
                "{sig} {{ spawn_async(async move {{ fetch().await; cb(); }}); }}"
            );
            let out = diags(&src);
            assert_eq!(out.len(), 1, "{sig} → {out:?}");
        }
    }

    /// A method is checked like a free fn.
    #[test]
    fn callback_parameter_in_an_impl_method() {
        let out = diags(
            r#"
impl Uploader {
    pub fn start(&self, on_done: Rc<dyn Fn()>) {
        spawn_async(async move { upload().await; on_done(); });
    }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
    }

    /// A callback field of a same-file props struct, called as
    /// `(props.on_done)()`.
    #[test]
    fn callback_field_of_a_props_struct() {
        let out = diags(
            r#"
pub struct SaveProps { pub on_done: Rc<dyn Fn()> }

#[component]
pub fn Save(props: &SaveProps) -> Element {
    spawn_async(async move { save().await; (props.on_done)(); });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`props.on_done`"), "{out:?}");
    }

    /// Passing a callback THROUGH `spawn_then` is the fix — clean.
    #[test]
    fn callback_routed_through_spawn_then_is_clean() {
        let out = diags(
            r#"
pub fn start(on_err: Rc<dyn Fn(String)>) {
    spawn_then(async move { fetch().await }, move |r| on_err(r));
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// A closure that touches nothing mortal is not a candidate.
    #[test]
    fn a_closure_over_plain_values_is_clean() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let n = signal(0u32);
    let log = move |s: &str| println!("{s}");
    spawn_async(async move { fetch().await; log("done"); });
    ui! { view() {} }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// Indirection chains resolve: `b` calls `a`, which writes.
    #[test]
    fn a_closure_calling_a_writing_closure_is_a_candidate() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let n = signal(0u32);
    let a = move || n.set(1);
    let b = move || a();
    spawn_async(async move { fetch().await; b(); });
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`b` touches `n`"), "{out:?}");
    }

    /// `spawn_then_in`'s future half is scanned; its callback is not.
    #[test]
    fn spawn_then_in_halves() {
        let out = diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let alive = ScopeAlive::current();
    let go = move || {
        spawn_then_in(&alive, async move { save().await; busy.set(false); }, move |_| busy.set(false));
    };
    ui! { view() {} }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(!is_deferred(&out[0]), "{out:?}");
    }

    // -----------------------------------------------------------------
    // spawn-then-handler-anchor
    // -----------------------------------------------------------------

    /// The trap the rule's own advice leads into: the handler's first
    /// write rebuilds the button, and the bare `spawn_then` was anchored
    /// to it.
    #[test]
    fn regression_handler_write_then_bare_spawn_then_is_flagged() {
        let out = anchor_diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let save: Rc<dyn Fn()> = Rc::new(move || {
        busy.set(true);
        spawn_then(fetch(), move |_| busy.set(false));
    });
    ui! { button(label = "Save", on_click = save, disabled = busy) }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("`busy`"), "{out:?}");
        assert!(
            out[0].help.as_deref().unwrap_or_default().contains("spawn_then_in(&alive"),
            "{out:?}"
        );
    }

    #[test]
    fn explicitly_anchored_handlers_are_clean() {
        let out = anchor_diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let alive = ScopeAlive::current();
    let a: Rc<dyn Fn()> = Rc::new(move || {
        busy.set(true);
        spawn_then_in(&alive, fetch(), move |_| busy.set(false));
    });
    let b = alive.wrap0(Rc::new(move || {
        busy.set(true);
        spawn_then(fetch(), move |_| busy.set(false));
    }));
    let c: Rc<dyn Fn()> = Rc::new(move || {
        busy.set(true);
        spawn_then(fetch(), move |_| busy.set(false));
    });
    let c = alive.wrap0(c);
    let d = move || {
        busy.set(true);
        alive.run_within(|| spawn_then(fetch(), move |_| busy.set(false)));
    };
    ui! { if busy.get() { activity_indicator() } else { view() {} } }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// No mortal write in the handler → nothing to rebuild the control.
    /// And a chained `spawn_then` inside a callback inherits the parent's
    /// token via `run_within`.
    #[test]
    fn handler_without_a_write_and_chained_spawns_are_clean() {
        let out = anchor_diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let a = move || {
        spawn_then(fetch(), move |_| {
            busy.set(false);
            spawn_then(refresh(), move |_| busy.set(true));
        });
    };
    ui! { if busy.get() { activity_indicator() } else { view() {} } }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// The write can come through a local closure too.
    #[test]
    fn handler_write_through_a_local_closure_arms_the_rule() {
        let out = anchor_diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let start = move || busy.set(true);
    let a = move || {
        start();
        spawn_then(fetch(), move |_| {});
    };
    ui! { match busy.get() { true => activity_indicator(), false => view() {} } }
}
"#,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("through `start`"), "{out:?}");
    }

    /// idea-ui's `Button` re-anchors its own `on_click`, so its
    /// `loading` prop is not evidence — the reported example, written
    /// against idea-ui, already works.
    #[test]
    fn idea_ui_button_loading_is_not_evidence() {
        let out = anchor_diags(
            r#"
#[component]
fn A() -> Element {
    let busy = signal(false);
    let save: Rc<dyn Fn()> = Rc::new(move || {
        busy.set(true);
        spawn_then(fetch(), move |_| busy.set(false));
    });
    ui! { Button(label = "Save", loading = busy, on_click = save.clone()) }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// A write that only feeds text cannot rebuild the control — the
    /// todo-sync demo's `status` line is the shape that must stay clean.
    #[test]
    fn a_write_that_does_not_shape_the_tree_is_clean() {
        let out = anchor_diags(
            r#"
#[component]
fn A() -> Element {
    let status = signal(String::new());
    let draft = signal(String::new());
    let sync: Rc<dyn Fn()> = Rc::new(move || {
        status.set("syncing".into());
        spawn_then(sync_now(), move |_| status.set("synced".into()));
    });
    ui! {
        view() {
            text { "{status}" }
            text_input(value = draft)
            button(label = "Sync", on_click = sync.clone(), disabled = false)
        }
    }
}
"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }
}
