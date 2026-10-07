//! The rule registry and the single-pass AST visitor that runs them.
//!
//! Each rule has a stable kebab-case id (the key used in
//! `idealyst-lint.toml` and in `// idealyst-lint-disable …` directives), a
//! default level, and a one-line summary surfaced by `idealyst lint
//! --rules`. Detection logic lives in the per-rule submodules; the
//! [`Linter`] visitor below walks the parsed file once and dispatches the
//! relevant nodes to each rule.
//!
//! Why one shared visitor instead of one `Visit` per rule: the three
//! patterns key off disjoint node kinds (fn items, call exprs, struct
//! exprs, macro invocations), so a single walk covers them with no
//! redundant traversal (the one extra pass, `prefer_ui::FileContext`,
//! is a file-level import/shadow scan the call-site rule consults, not a
//! second rule walk) — and crucially, `syn`'s default visitor does
//! **not** descend into macro token streams, so anything inside
//! `ui! { … }` is invisible here. That's exactly right: these rules ask
//! "what did the author write *outside* the macro," which only the
//! un-expanded surface can answer. (Macro invocation *nodes* — name +
//! bang — are still visible; that's how the removed `signal!` is caught.)

use syn::visit::Visit;

use crate::config::Level;
use crate::diagnostic::RawDiag;

mod component_case;
mod keyed_list;
mod prefer_component;
mod prefer_macros;
mod prefer_ui;
mod premint_crawl;
mod signal_across_await;
mod snapshot_condition;
mod snapshot_loop;

/// Static metadata for one lint rule.
pub struct RuleInfo {
    /// Stable kebab-case id used in config + inline directives.
    pub id: &'static str,
    /// Level applied when no config overrides it.
    pub default_level: Level,
    /// One-line description for `--rules` / docs.
    pub summary: &'static str,
}

/// Every rule the engine knows about, in display order. The `Config`
/// defaults and the `--rules` listing both derive from this slice, so a
/// new rule is wired up by adding its id here and a branch in [`Linter`].
pub fn all_rules() -> &'static [RuleInfo] {
    &[
        RuleInfo {
            id: prefer_macros::SIGNAL_RULE,
            default_level: Level::Warn,
            summary: "use the `signal(…)` function — flags redundant `Signal::new(…)` and the removed `signal!` macro",
        },
        RuleInfo {
            id: prefer_macros::EFFECT_RULE,
            default_level: Level::Warn,
            summary: "use `effect!{ … }` instead of calling `Effect::new(…)` directly",
        },
        RuleInfo {
            id: prefer_macros::MEMO_RULE,
            default_level: Level::Warn,
            summary: "use the `memo(move || …)` function — flags the removed `memo!` macro",
        },
        RuleInfo {
            id: prefer_macros::TEXT_FSTRING_RULE,
            default_level: Level::Warn,
            summary: "interpolate in the text literal (`text { \"count: {count}\" }`) — flags the removed `text_fmt!` / `bind!` macros",
        },
        RuleInfo {
            id: prefer_ui::RULE,
            default_level: Level::Warn,
            summary: "build elements with the `ui!` / `jsx!` macro, not by hand",
        },
        RuleInfo {
            id: prefer_ui::CONTROL_FLOW_RULE,
            default_level: Level::Warn,
            summary: "a hand-called `when(…)` / `switch(…)` — write `if` / `match` inside `ui!`; suppress with a `-- reason` where the macro can't express the shape",
        },
        RuleInfo {
            id: component_case::RULE,
            default_level: Level::Error,
            summary: "`#[component]` functions must be PascalCase",
        },
        RuleInfo {
            id: prefer_component::RULE,
            default_level: Level::Warn,
            summary: "a free fn returning `Element` without `#[component]` — likely a component outside the paradigm; annotate it so call sites use `ui!` dispatch",
        },
        RuleInfo {
            id: snapshot_condition::RULE,
            default_level: Level::Warn,
            summary: "a hoisted `.get()` snapshot used as a `ui!` condition — the branch silently never updates",
        },
        RuleInfo {
            id: keyed_list::RULE,
            default_level: Level::Warn,
            summary: "a child list built by hand (`.push(ui!{…})` / `.map(|x| ui!{…})`) outside the macro — keys are erased; use `for … , key = …`",
        },
        RuleInfo {
            id: snapshot_loop::RULE,
            default_level: Level::Warn,
            summary: "`for … in <expr>.get()` inside `ui!`/`jsx!` — a build-time snapshot; iterate the Signal itself with `key = …`",
        },
        RuleInfo {
            id: signal_across_await::RULE,
            default_level: Level::Warn,
            summary: "a scope-owned signal (local, prop, struct field, or through a closure / callback) touched inside a detached `spawn_async` — after an `.await`, or at all on web where the body starts after the event's flush — aborts with `stale-signal-handle` once the scope is torn down",
        },
        RuleInfo {
            id: signal_across_await::ANCHOR_RULE,
            default_level: Level::Warn,
            summary: "a handler that writes a component signal and then calls a bare `spawn_then` — the task is anchored to the control, so if the write rebuilds it the result is silently dropped; use `spawn_then_in(&alive, …)`",
        },
        RuleInfo {
            id: premint_crawl::STATE_KEYED_RULE,
            default_level: Level::Warn,
            summary: "sheet identity/cache key selected by a runtime conditional — the arm not taken at mount gets no premint CSS (UNCRAWLED panic under --premint-only); make the state a variant axis",
        },
        RuleInfo {
            id: premint_crawl::COMPUTED_RULE,
            default_level: Level::Warn,
            summary: "`with_computed` layer — a premint disqualifier with one slot; use a variant axis, the inline layer, or `stylesheet!`",
        },
    ]
}

/// Walk a parsed file and collect every rule finding (pre-severity,
/// pre-suppression). The caller resolves severity and suppression.
pub(crate) fn collect(file: &syn::File) -> Vec<RawDiag> {
    // One cheap pre-pass gathers the file-level facts a node-local rule
    // can't see from its own node — which framework constructors the
    // file imports, which idents it shadows — so `prefer-ui-macro` can
    // judge a bare `view(…)` with evidence instead of guessing.
    let file_cx = prefer_ui::FileContext::scan(file);
    // Same-file struct declarations, so a props struct's signal and
    // callback fields are visible to `signal-across-await`.
    let signal_cx = signal_across_await::FileContext::scan(file);
    let mut linter = Linter { diags: Vec::new(), file_cx, signal_cx };
    linter.visit_file(file);
    // Whole-file rule: needs call counts / value uses / imports across
    // the file before it can judge any one fn, so it runs its own walk.
    prefer_component::check_file(file, &mut linter.diags);
    linter.diags
}

struct Linter {
    diags: Vec<RawDiag>,
    file_cx: prefer_ui::FileContext,
    signal_cx: signal_across_await::FileContext,
}

impl<'ast> Visit<'ast> for Linter {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        component_case::check_fn(node, &mut self.diags);
        snapshot_condition::check_fn(node, &mut self.diags);
        signal_across_await::check_fn(
            &node.attrs,
            &node.sig,
            &node.block,
            &self.signal_cx,
            &mut self.diags,
        );
        syn::visit::visit_item_fn(self, node);
    }

    // Methods spawn too — a `fn start(&self, on_err: Rc<dyn Fn(String)>)`
    // that calls its callback from a task is the same bug as a free fn.
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        signal_across_await::check_fn(
            &node.attrs,
            &node.sig,
            &node.block,
            &self.signal_cx,
            &mut self.diags,
        );
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        prefer_macros::check_call(node, &mut self.diags);
        prefer_ui::check_call(node, &self.file_cx, &mut self.diags);
        premint_crawl::check_call(node, &mut self.diags);
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        prefer_ui::check_struct(node, &mut self.diags);
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        keyed_list::check_method_call(node, &mut self.diags);
        premint_crawl::check_method_call(node, &mut self.diags);
        syn::visit::visit_expr_method_call(self, node);
    }

    // Macro INVOCATIONS are visible AST nodes (their bodies aren't) —
    // reached from every position (expr, stmt, item) via `visit_macro`.
    // This is how a leftover `signal!(…)` call is flagged, and where the
    // rules that TOKENIZE `ui!`/`jsx!` bodies (snapshot-loop) hook in.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        prefer_macros::check_macro(node, &mut self.diags);
        snapshot_loop::check_ui_macro(node, &mut self.diags);
        visit_vec_elements(self, node);
        syn::visit::visit_macro(self, node);
    }
}

/// `vec![a, b, c]` is ordinary Rust expressions behind a macro, and it is
/// exactly where a hand-built child list lives (`view(vec![switch(…)])`) —
/// but `syn` never descends into macro tokens, so everything inside it was
/// invisible to every rule. Re-parse the comma form and walk it like any
/// other code. (The `vec![x; n]` repeat form fails the parse and is left
/// alone; other macros keep their opaque bodies.)
fn visit_vec_elements(linter: &mut Linter, node: &syn::Macro) {
    if !node.path.is_ident("vec") {
        return;
    }
    let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
    if let Ok(elems) = syn::parse::Parser::parse2(parser, node.tokens.clone()) {
        for e in &elems {
            linter.visit_expr(e);
        }
    }
}

// ---------------------------------------------------------------------------
// Shared path helpers used by the rule modules.
// ---------------------------------------------------------------------------

/// The last path segment's ident, as a `String`.
pub(crate) fn last_segment(path: &syn::Path) -> Option<String> {
    path.segments.last().map(|s| s.ident.to_string())
}

/// The path segment `n` positions from the end (`0` == last). Returns the
/// segment ident as a `String`, or `None` if the path is too short.
pub(crate) fn nth_from_end(path: &syn::Path, n: usize) -> Option<String> {
    let len = path.segments.len();
    if n >= len {
        return None;
    }
    Some(path.segments[len - 1 - n].ident.to_string())
}

/// True when a MODULE segment of the path — any segment but the last — has
/// the given ident (e.g. detecting a `glue::` qualifier in
/// `runtime_vocabulary::glue::view`). The last segment is the called item
/// itself, never a qualifier: counting it is how `Pool::builder()` once
/// read as "a path through the `builder` module".
pub(crate) fn has_module_segment(path: &syn::Path, ident: &str) -> bool {
    let n = path.segments.len();
    path.segments.iter().take(n.saturating_sub(1)).any(|s| s.ident == ident)
}
