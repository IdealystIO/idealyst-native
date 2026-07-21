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
//! redundant traversal — and crucially, `syn`'s default visitor does
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
mod prefer_macros;
mod prefer_ui;
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
            id: component_case::RULE,
            default_level: Level::Error,
            summary: "`#[component]` functions must be PascalCase",
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
    ]
}

/// Walk a parsed file and collect every rule finding (pre-severity,
/// pre-suppression). The caller resolves severity and suppression.
pub(crate) fn collect(file: &syn::File) -> Vec<RawDiag> {
    let mut linter = Linter { diags: Vec::new() };
    linter.visit_file(file);
    linter.diags
}

struct Linter {
    diags: Vec<RawDiag>,
}

impl<'ast> Visit<'ast> for Linter {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        component_case::check_fn(node, &mut self.diags);
        snapshot_condition::check_fn(node, &mut self.diags);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        prefer_macros::check_call(node, &mut self.diags);
        prefer_ui::check_call(node, &mut self.diags);
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        prefer_ui::check_struct(node, &mut self.diags);
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        keyed_list::check_method_call(node, &mut self.diags);
        syn::visit::visit_expr_method_call(self, node);
    }

    // Macro INVOCATIONS are visible AST nodes (their bodies aren't) —
    // reached from every position (expr, stmt, item) via `visit_macro`.
    // This is how a leftover `signal!(…)` call is flagged, and where the
    // rules that TOKENIZE `ui!`/`jsx!` bodies (snapshot-loop) hook in.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        prefer_macros::check_macro(node, &mut self.diags);
        snapshot_loop::check_ui_macro(node, &mut self.diags);
        syn::visit::visit_macro(self, node);
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

/// True when any segment of the path has the given ident (e.g. detecting a
/// `builder::` qualifier anywhere in `runtime_core::builder::view`).
pub(crate) fn has_segment(path: &syn::Path, ident: &str) -> bool {
    path.segments.iter().any(|s| s.ident == ident)
}
