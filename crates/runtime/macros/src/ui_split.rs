//! The SHARED front half of both `ui!` lowerings: splitting a parsed
//! [`UiNode`] tree into a **static** part and an ordered list of
//! **dynamic slots**.
//!
//! Both lowerings run this pass and honour its decisions, which is what
//! makes them comparable at all: if `direct` and `template` disagreed on
//! what counts as static, a parity suite would be comparing two
//! different programs.
//!
//! # Scopes
//!
//! A *template scope* is one Rust scope's worth of nodes. The top level
//! of a `ui!` body is a scope; so is every construct whose body the
//! emitter puts inside a fresh Rust scope — an `if`/`match` branch, a
//! `for` row builder, a `presence` child thunk. Children of a `view` (or
//! of a component, or of `scroll_view`/`link`/`overlay`/…) are NOT a new
//! scope: they are built inline in the parent's block, so they share the
//! parent's slot list.
//!
//! Each scope has its own slot list. A nested scope's slots are
//! evaluated when that scope runs — per branch activation, per row —
//! which is exactly what the direct lowering has always done, because
//! the author's expressions live inside the branch/row closure.
//!
//! # Placement — and why evaluation order is the whole point
//!
//! ```text
//! Prelude   evaluated at the head of its scope, in SOURCE ORDER,
//!           before anything is constructed.
//! Construct evaluated where the construction splices it.
//! ```
//!
//! The direct lowering used to evaluate a node's props *after* its
//! children (`view(style = f()) { Badge(label = g()) }` ran `g()` then
//! `f()`, because `style` lowers to a trailing `.with_style(f())`). The
//! template lowering cannot reproduce that: its slots are an array,
//! evaluated left to right. Rather than contort the builder, BOTH
//! lowerings now hoist every `Prelude` slot into a `let` at the head of
//! its scope, in source order. That is the one behavioural change the
//! slot rewrite makes, and it makes the two lowerings' observable
//! evaluation order identical by construction.
//!
//! `Construct` placement is reserved for expressions whose *evaluation*
//! has no observable effect, so leaving them where the construction
//! splices them cannot diverge:
//!
//! - **closure literals** — constructing a closure only captures; the
//!   body does not run. (They must also stay put for a second reason:
//!   `let f; f = |e| e.x;` does not compile, because a closure
//!   parameter's type comes from the expected type at the *use* site.)
//! - **macro invocations** (`rx!(…)`, a nested `ui! { … }`) — value
//!   producers whose expansion the split pass cannot see into.
//! - **the "reactive call" shape** `f(sig_a, sig_b)` — the emitter
//!   rewrites it to `f((sig_a).get(), …)` *inside* a closure, so the
//!   author's tokens are never evaluated where they stand.
//! - **control-flow conditions / scrutinees / iterators** — the emitter
//!   wraps a reactive one in `move || …` (a closure literal again), and
//!   a static one is evaluated exactly once at the same point in both
//!   lowerings. A static `match` scrutinee additionally MUST stay put:
//!   hoisting `match self.kind { … }` to a `let` forces a move where
//!   Rust's match ergonomics borrow.
//! Everything else is `Prelude`.
//!
//! # Why hoisting is safe for inference
//!
//! A prelude entry is emitted as a **deferred-init** pair:
//!
//! ```ignore
//! let __ui_s0;
//! __ui_s0 = "x".into();
//! ```
//!
//! not `let __ui_s0 = …;`. Rust's inference is whole-body, so the local
//! is one inference variable shared by the assignment and the use site —
//! a `.into()` whose target is pinned by a struct field, or a `collect()`
//! pinned by a trait impl, still resolves. A plain `let … = expr;` would
//! demand the expression be self-describing.
//!
//! # Unused props
//!
//! Several primitives silently ignore props they don't recognise
//! (`view(gap = 4)` reaches nothing). Hoisting such a prop would start
//! *evaluating* an expression the emitter drops. Rather than maintain a
//! per-primitive table of consumed prop names — a second source of truth
//! that would rot — [`Scope::prelude_for`] takes the emitted body and
//! keeps only the entries whose slot local actually appears in it. Slot
//! locals are globally unique per expansion ([`next_slot`]), so the scan
//! is exact and a nested scope's `__ui_s7` can never be mistaken for an
//! outer one's.

use std::cell::Cell;

use proc_macro2::{Span, TokenStream as TokenStream2, TokenTree};
use quote::{quote, ToTokens};
use syn::{Expr, Ident};

use crate::ui::{is_reactive_call_shape, MatchArm, Prop, UiNode};

// ===========================================================================
// Slot identity
// ===========================================================================

thread_local! {
    /// Per-expansion slot counter. Globally unique within one `ui!`
    /// expansion so [`Scope::prelude_for`]'s used-scan cannot confuse a
    /// nested scope's local with an outer one's. A proc-macro expansion
    /// is single-threaded, and [`reset_slot_counter`] runs at the head of
    /// every top-level emission.
    static NEXT_SLOT: Cell<usize> = const { Cell::new(0) };
}

/// Start a fresh expansion's slot numbering.
pub(crate) fn reset_slot_counter() {
    NEXT_SLOT.with(|c| c.set(0));
}

fn next_slot() -> usize {
    NEXT_SLOT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

/// The local a `Prelude` slot is bound to. `__`-prefixed and
/// call-site-spanned: author code cannot name it.
pub(crate) fn slot_ident(index: usize) -> Ident {
    Ident::new(&format!("__ui_s{index}"), Span::call_site())
}

// ===========================================================================
// Slot model
// ===========================================================================

/// When a slot's expression is evaluated. See the module docs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Placement {
    Prelude,
    Construct,
}

/// What the slot feeds. Carried into the template descriptor's
/// `SlotSig`, and useful in diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SlotRole {
    /// A named prop's value.
    PropValue,
    /// A `text` node's content (children block or `content` prop).
    TextContent,
    /// A bare expression in a children position.
    ExprChild,
    /// An `if` condition.
    Condition,
    /// A `match` scrutinee.
    Scrutinee,
    /// A `for` iterable.
    Iterator,
    /// A `for`'s `key = …` expression.
    Key,
}

impl SlotRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SlotRole::PropValue => "prop",
            SlotRole::TextContent => "text",
            SlotRole::ExprChild => "child",
            SlotRole::Condition => "cond",
            SlotRole::Scrutinee => "scrutinee",
            SlotRole::Iterator => "iter",
            SlotRole::Key => "key",
        }
    }
}

/// One dynamic expression pulled out of a scope.
pub(crate) struct SlotDef {
    /// Expansion-unique index; also the name of the `Prelude` local.
    pub index: usize,
    /// The prop name this slot feeds, when it has one. Unused for now —
    /// it is what a later patch protocol matches a slot against by name
    /// rather than by position.
    pub name: Option<&'static str>,
    pub role: SlotRole,
    pub placement: Placement,
    /// The author's expression, already carrying any rewrite the
    /// emission applies to it.
    pub expr: TokenStream2,
    /// A SYNTACTIC kind label (`"closure"`, `"path"`, `"call"`, …), not
    /// a Rust type: a proc macro has tokens, never resolved types. It
    /// exists so a descriptor's `SlotSig` can be checked for shape drift
    /// against the code that produced it.
    pub kind: &'static str,
}

/// One template scope: the nodes as the emitter should see them, plus
/// the slots pulled out of them.
pub(crate) struct Scope {
    pub slots: Vec<SlotDef>,
    /// The node list with every `Prelude` slot replaced by its local.
    pub nodes: Vec<UiNode>,
}

impl Scope {
    /// The `let` prelude for this scope, restricted to the slots the
    /// emitted `body` actually reads (see the module docs on unused
    /// props).
    pub(crate) fn prelude_for(&self, body: &TokenStream2) -> TokenStream2 {
        let used = used_idents(body);
        let lets = self.slots.iter().filter(|s| s.placement == Placement::Prelude).filter_map(
            |s| {
                let ident = slot_ident(s.index);
                if !used.contains(&ident.to_string()) {
                    return None;
                }
                let expr = &s.expr;
                // Deferred init, not `let x = e` — see the module docs.
                Some(quote! { let #ident; #ident = #expr; })
            },
        );
        quote! { #(#lets)* }
    }
}

/// Every `__ui_s*` ident appearing anywhere in `stream`.
fn used_idents(stream: &TokenStream2) -> std::collections::HashSet<String> {
    fn walk(stream: &TokenStream2, out: &mut std::collections::HashSet<String>) {
        for tt in stream.clone() {
            match tt {
                TokenTree::Ident(id) => {
                    let s = id.to_string();
                    if s.starts_with("__ui_s") {
                        out.insert(s);
                    }
                }
                TokenTree::Group(g) => walk(&g.stream(), out),
                _ => {}
            }
        }
    }
    let mut out = std::collections::HashSet::new();
    walk(stream, &mut out);
    out
}

// ===========================================================================
// Static classification
// ===========================================================================

/// A value the template descriptor can carry as DATA. These are exactly
/// the shapes a static edit should be able to change without
/// recompiling.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum StaticValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// An enum-like path (`tone::Danger`) or a style-token accessor
    /// (`t.color.text()`), recorded as its source text. The *value* is
    /// not reconstructible from the text by a generated applier, so the
    /// emission keeps a site-local resolver; the text is what a
    /// descriptor diff compares.
    Path(String),
}

/// Classify a prop value as descriptor DATA, or `None` when it is
/// dynamic.
///
/// Recognised (per the split contract):
/// - string / integer / float / bool literals, including a negated
///   numeric literal,
/// - `"lit".to_string()` and `"lit".into()` on a literal receiver — the
///   same value, spelled for a different target type,
/// - style-token accessors of the shape `t.a.b()` / `theme.x.y()`,
/// - enum-like paths: two or more segments, last one PascalCase, no
///   generic arguments (`tone::Danger`, `StackAxis::Row`).
pub(crate) fn classify_static(expr: &Expr) -> Option<StaticValue> {
    match expr {
        Expr::Lit(lit) => lit_value(&lit.lit, false),
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => match &*u.expr {
            Expr::Lit(lit) => lit_value(&lit.lit, true),
            _ => None,
        },
        Expr::Group(g) => classify_static(&g.expr),
        Expr::Paren(p) => classify_static(&p.expr),
        // `"lit".to_string()` / `"lit".into()` — a literal wearing a
        // target type. Also the `t.a.b()` style-token accessor.
        Expr::MethodCall(mc) if mc.args.is_empty() && mc.turbofish.is_none() => {
            let method = mc.method.to_string();
            if method == "to_string" || method == "into" || method == "to_owned" {
                return classify_static(&mc.receiver);
            }
            if style_token_path(expr).is_some() {
                return Some(StaticValue::Path(path_text(expr)));
            }
            None
        }
        Expr::Path(p) if p.qself.is_none() => {
            let segs = &p.path.segments;
            if segs.len() < 2 || segs.iter().any(|s| !s.arguments.is_empty()) {
                return None;
            }
            let last = segs.last().unwrap().ident.to_string();
            if last.chars().next().is_some_and(|c| c.is_uppercase()) {
                Some(StaticValue::Path(path_text(expr)))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn lit_value(lit: &syn::Lit, negate: bool) -> Option<StaticValue> {
    match lit {
        syn::Lit::Str(s) if !negate => Some(StaticValue::Str(s.value())),
        syn::Lit::Bool(b) if !negate => Some(StaticValue::Bool(b.value())),
        syn::Lit::Int(i) => {
            let v: i64 = i.base10_parse().ok()?;
            Some(StaticValue::Int(if negate { -v } else { v }))
        }
        syn::Lit::Float(f) => {
            let v: f64 = f.base10_parse().ok()?;
            Some(StaticValue::Float(if negate { -v } else { v }))
        }
        _ => None,
    }
}

/// `t.a.b()` / `theme.x.y()` — a no-arg method call whose receiver chain
/// is **one or more** field accesses rooted at a bare `t` or `theme`.
///
/// The field requirement is load-bearing, not cosmetic. Without it,
/// ANY no-arg method on a local named `t` matched — including
/// `t.clone()`. That misfire made `tone = t.clone()` "static" (spliced
/// inline) while the sibling `tone = t` stayed dynamic (hoisted into the
/// prelude), so the prelude moved `t` before the inline clone borrowed
/// it: `websites/idea-ui-docs/src/pages/foundations.rs` stopped
/// compiling with E0382. A token accessor always names a group
/// (`t.color.text()`); a bare `t.method()` never is one.
fn style_token_path(expr: &Expr) -> Option<()> {
    let Expr::MethodCall(mc) = expr else { return None };
    if !mc.args.is_empty() {
        return None;
    }
    // Conversions and clones are never token accessors, whatever the
    // receiver is named.
    if matches!(mc.method.to_string().as_str(), "clone" | "into" | "to_string" | "to_owned") {
        return None;
    }
    let mut cursor: &Expr = &mc.receiver;
    let mut fields = 0usize;
    loop {
        match cursor {
            Expr::Field(f) => {
                fields += 1;
                cursor = &f.base;
            }
            Expr::Path(p)
                if fields > 0
                    && p.qself.is_none()
                    && p.path.segments.len() == 1
                    && matches!(p.path.segments[0].ident.to_string().as_str(), "t" | "theme") =>
            {
                return Some(());
            }
            _ => return None,
        }
    }
}

/// Whitespace-squashed source text of an expression — the descriptor's
/// record of a path value.
fn path_text(expr: &Expr) -> String {
    expr.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect()
}

// ===========================================================================
// Placement decisions
// ===========================================================================

/// Props the emission splices somewhere the split pass must not touch.
/// Each entry has a reason in the module docs.
fn prop_stays_put(kind: Option<&str>, name: &str) -> bool {
    match (kind, name) {
        // Shape-inspected: `emit_text` decides literal / f-string /
        // closure / reactive-call from the tokens themselves.
        (Some("text"), "content") => true,
        // Shape-inspected: `emit_button` rewrites the reactive-call
        // shape into a fire closure.
        (Some("button"), "on_click") => true,
        _ => false,
    }
}

/// A coarse syntactic label for a slot expression.
pub(crate) fn expr_kind_of(expr: &Expr) -> &'static str {
    match expr {
        Expr::Closure(_) => "closure",
        Expr::Macro(_) => "macro",
        Expr::Path(_) => "path",
        Expr::Field(_) => "field",
        Expr::Call(_) => "call",
        Expr::MethodCall(_) => "method_call",
        Expr::Lit(_) => "literal",
        Expr::Reference(_) => "reference",
        Expr::Tuple(_) => "tuple",
        Expr::Struct(_) => "struct",
        Expr::Cast(_) => "cast",
        Expr::Binary(_) => "binary",
        Expr::Unary(_) => "unary",
        Expr::If(_) => "if",
        Expr::Match(_) => "match",
        Expr::Block(_) => "block",
        _ => "expr",
    }
}

/// `true` when the expression's *evaluation* has no observable effect,
/// so leaving it at its construction point cannot diverge from
/// evaluating it in the prelude.
fn effect_free_to_construct(expr: &Expr) -> bool {
    matches!(expr, Expr::Closure(_) | Expr::Macro(_)) || is_reactive_call_shape(expr)
}

// ===========================================================================
// The split
// ===========================================================================

/// Split one scope's node list.
pub(crate) fn split(nodes: &[UiNode]) -> Scope {
    let mut slots: Vec<SlotDef> = Vec::new();
    let rewritten: Vec<UiNode> = nodes.iter().map(|n| rewrite_node(&mut slots, n)).collect();
    Scope { slots, nodes: rewritten }
}

/// How a primitive treats its `{ … }` block.
enum ChildrenKind {
    /// A child list built inline in the parent's block — same scope.
    List,
    /// `text`: the block is content, shape-inspected by `emit_text`.
    Content,
    /// A child list built inside a closure — its own scope, split when
    /// that scope is emitted.
    NestedScope,
    /// The emitter ignores the block.
    Ignored,
}

fn children_kind(canonical: Option<&str>, is_primitive: bool) -> ChildrenKind {
    if !is_primitive {
        // A `#[component]`'s children become its `children` field, built
        // inline in the parent's block.
        return ChildrenKind::List;
    }
    match canonical {
        Some("view") | Some("scroll_view") | Some("link") | Some("overlay")
        | Some("anchored_overlay") => ChildrenKind::List,
        Some("text") => ChildrenKind::Content,
        // `emit_presence` wraps the block in `move || …`.
        Some("presence") => ChildrenKind::NestedScope,
        _ => ChildrenKind::Ignored,
    }
}

fn rewrite_node(slots: &mut Vec<SlotDef>, node: &UiNode) -> UiNode {
    match node {
        UiNode::Component { name, props, children, chain } => {
            let name_str = name.to_string();
            let canonical = crate::primitives::canonical_primitive(&name_str);
            let is_primitive = canonical.is_some();

            // Props first: they come first in source order, which is the
            // order the prelude must evaluate them in.
            let new_props: Vec<Prop> =
                props.iter().map(|p| rewrite_prop(slots, canonical, p)).collect();

            let new_children = match children {
                None => None,
                Some(kids) => Some(match children_kind(canonical, is_primitive) {
                    ChildrenKind::List => {
                        kids.iter().map(|k| rewrite_node(slots, k)).collect()
                    }
                    ChildrenKind::Content => {
                        // Record the content for the descriptor, but leave
                        // the tokens where `emit_text` can inspect them.
                        for k in kids {
                            if let UiNode::Expr(e) = k {
                                if classify_static(e).is_none() {
                                    slots.push(SlotDef {
                                        index: next_slot(),
                                        name: None,
                                        role: SlotRole::TextContent,
                                        placement: Placement::Construct,
                                        expr: e.to_token_stream(),
                                        kind: expr_kind_of(e),
                                    });
                                }
                            }
                        }
                        kids.clone()
                    }
                    ChildrenKind::NestedScope | ChildrenKind::Ignored => kids.clone(),
                }),
            };

            UiNode::Component {
                name: name.clone(),
                props: new_props,
                children: new_children,
                chain: chain.clone(),
            }
        }

        // Control flow: the construct itself is built where it stands,
        // and its bodies are their own scopes (split when those scopes
        // are emitted). Only the condition / iterable / scrutinee / key
        // is recorded here, always `Construct` — see the module docs.
        UiNode::If { cond, then_body, else_body } => {
            if classify_static(cond).is_none() && !matches!(cond, Expr::Let(_)) {
                slots.push(SlotDef {
                    index: next_slot(),
                    name: None,
                    role: SlotRole::Condition,
                    placement: Placement::Construct,
                    expr: cond.to_token_stream(),
                    kind: expr_kind_of(cond),
                });
            }
            UiNode::If {
                cond: cond.clone(),
                then_body: then_body.clone(),
                else_body: else_body.clone(),
            }
        }
        UiNode::For { pat, iter, key, body, chain } => {
            slots.push(SlotDef {
                index: next_slot(),
                name: None,
                role: SlotRole::Iterator,
                placement: Placement::Construct,
                expr: iter.to_token_stream(),
                kind: expr_kind_of(iter),
            });
            if let Some(k) = key {
                slots.push(SlotDef {
                    index: next_slot(),
                    name: None,
                    role: SlotRole::Key,
                    placement: Placement::Construct,
                    expr: k.to_token_stream(),
                    kind: expr_kind_of(k),
                });
            }
            UiNode::For {
                pat: pat.clone(),
                iter: iter.clone(),
                key: key.clone(),
                body: body.clone(),
                chain: chain.clone(),
            }
        }
        UiNode::Match { scrutinee, arms } => {
            slots.push(SlotDef {
                index: next_slot(),
                name: None,
                role: SlotRole::Scrutinee,
                placement: Placement::Construct,
                expr: scrutinee.to_token_stream(),
                kind: expr_kind_of(scrutinee),
            });
            UiNode::Match {
                scrutinee: scrutinee.clone(),
                arms: arms
                    .iter()
                    .map(|a| MatchArm {
                        pat: a.pat.clone(),
                        guard: a.guard.clone(),
                        body: a.body.clone(),
                    })
                    .collect(),
            }
        }

        UiNode::Expr(e) => {
            if classify_static(e).is_some() {
                return UiNode::Expr(e.clone());
            }
            if effect_free_to_construct(e) {
                slots.push(SlotDef {
                    index: next_slot(),
                    name: None,
                    role: SlotRole::ExprChild,
                    placement: Placement::Construct,
                    expr: e.to_token_stream(),
                    kind: expr_kind_of(e),
                });
                return UiNode::Expr(e.clone());
            }
            let index = next_slot();
            slots.push(SlotDef {
                index,
                name: None,
                role: SlotRole::ExprChild,
                placement: Placement::Prelude,
                expr: e.to_token_stream(),
                kind: expr_kind_of(e),
            });
            UiNode::Expr(local_expr(index))
        }
    }
}

fn rewrite_prop(slots: &mut Vec<SlotDef>, canonical: Option<&str>, p: &Prop) -> Prop {
    let name = p.name.to_string();
    let verbatim = || Prop {
        name: p.name.clone(),
        value: p.value.clone(),
        arrow_target: p.arrow_target.clone(),
    };

    // Descriptor data — stays inline (the emission's `.into()` target is
    // pinned at the use site, and a literal is what a static edit wants
    // to change).
    if classify_static(&p.value).is_some() {
        return verbatim();
    }

    let stays = p.arrow_target.is_some()
        || effect_free_to_construct(&p.value)
        || prop_stays_put(canonical, &name);

    slots.push(SlotDef {
        index: next_slot(),
        // Leaked so the descriptor can hold a `&'static str` without a
        // per-site allocation dance; prop names are a bounded set drawn
        // from the source text of one crate.
        name: Some(Box::leak(name.clone().into_boxed_str())),
        role: SlotRole::PropValue,
        placement: if stays { Placement::Construct } else { Placement::Prelude },
        expr: p.value.to_token_stream(),
        kind: expr_kind_of(&p.value),
    });

    if stays {
        return verbatim();
    }
    let index = slots.last().unwrap().index;
    Prop {
        name: p.name.clone(),
        value: local_expr(index),
        arrow_target: None,
    }
}

/// The slot index a `__ui_sN` local names, if `expr` is one.
///
/// The inverse of [`local_expr`]. The overlay descriptor needs it
/// because by the time a node is recorded its hoisted props have already
/// been rewritten to their locals — the slot index is recoverable only
/// from the name.
pub(crate) fn slot_index_of(expr: &Expr) -> Option<usize> {
    let Expr::Path(p) = expr else { return None };
    if p.qself.is_some() || p.path.segments.len() != 1 {
        return None;
    }
    p.path.segments[0].ident.to_string().strip_prefix("__ui_s")?.parse().ok()
}

/// `__ui_sN` as an `Expr`.
fn local_expr(index: usize) -> Expr {
    let ident = slot_ident(index);
    syn::parse_quote! { #ident }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn st(tokens: TokenStream2) -> Option<StaticValue> {
        classify_static(&syn::parse2::<Expr>(tokens).unwrap())
    }

    #[test]
    fn literals_are_static() {
        assert_eq!(st(quote! { "hi" }), Some(StaticValue::Str("hi".into())));
        assert_eq!(st(quote! { 4 }), Some(StaticValue::Int(4)));
        assert_eq!(st(quote! { -4 }), Some(StaticValue::Int(-4)));
        assert_eq!(st(quote! { 1.5 }), Some(StaticValue::Float(1.5)));
        assert_eq!(st(quote! { true }), Some(StaticValue::Bool(true)));
    }

    #[test]
    fn literal_conversions_are_the_same_static_value() {
        // `"x".to_string()` / `"x".into()` is the same descriptor datum
        // as `"x"` — only the target type differs.
        assert_eq!(st(quote! { "x".to_string() }), Some(StaticValue::Str("x".into())));
        assert_eq!(st(quote! { "x".into() }), Some(StaticValue::Str("x".into())));
    }

    #[test]
    fn enum_like_paths_are_static() {
        assert_eq!(st(quote! { tone::Danger }), Some(StaticValue::Path("tone::Danger".into())));
        assert_eq!(st(quote! { StackAxis::Row }), Some(StaticValue::Path("StackAxis::Row".into())));
    }

    #[test]
    fn style_token_accessors_are_static() {
        assert_eq!(st(quote! { t.color.text() }), Some(StaticValue::Path("t.color.text()".into())));
        assert_eq!(
            st(quote! { theme.space.md() }),
            Some(StaticValue::Path("theme.space.md()".into()))
        );
    }

    #[test]
    fn bare_and_generic_paths_are_dynamic() {
        // A one-segment path is a local, not an enum variant.
        assert_eq!(st(quote! { danger }), None);
        assert_eq!(st(quote! { Danger }), None);
        // A lowercase tail is a function/associated item, not a value.
        assert_eq!(st(quote! { tone::danger }), None);
        // Generic arguments mean it is not a plain variant path.
        assert_eq!(st(quote! { Foo::<u8>::Bar }), None);
        // Not rooted at `t` / `theme`.
        assert_eq!(st(quote! { cfg.color.text() }), None);
        // A style-token accessor with arguments is a real call.
        assert_eq!(st(quote! { t.color.text(1) }), None);
    }

    /// Regression (idea-ui-docs `foundations.rs`, E0382): a no-arg
    /// method on a local named `t` is NOT a style-token accessor. When
    /// it was, `tone = t.clone()` counted as descriptor data and stayed
    /// inline while the sibling `tone = t` was hoisted — so the prelude
    /// moved `t` before the inline clone borrowed it.
    #[test]
    fn regression_bare_method_on_t_is_not_a_style_token_path() {
        assert_eq!(st(quote! { t.clone() }), None);
        assert_eq!(st(quote! { theme.clone() }), None);
        // …and a field-free accessor is not one either.
        assert_eq!(st(quote! { t.color() }), None);
        // The real shape still is.
        assert!(st(quote! { t.color.text() }).is_some());
    }

    #[test]
    fn closures_and_macros_stay_at_their_construction_point() {
        let e: Expr = syn::parse2(quote! { move |x| x + 1 }).unwrap();
        assert!(effect_free_to_construct(&e));
        let e: Expr = syn::parse2(quote! { rx!(a.get()) }).unwrap();
        assert!(effect_free_to_construct(&e));
        // The reactive-call shape is rewritten inside a closure.
        let e: Expr = syn::parse2(quote! { is_even(count) }).unwrap();
        assert!(effect_free_to_construct(&e));
        // An ordinary call is not.
        let e: Expr = syn::parse2(quote! { compute(1, 2) }).unwrap();
        assert!(!effect_free_to_construct(&e));
    }

    #[test]
    fn slot_indices_are_unique_across_nested_scopes() {
        reset_slot_counter();
        let a = next_slot();
        let b = next_slot();
        assert_ne!(a, b);
        assert_eq!(slot_ident(a).to_string(), format!("__ui_s{a}"));
    }

    #[test]
    fn prelude_keeps_only_the_slots_the_body_reads() {
        reset_slot_counter();
        let scope = split(&[syn::parse2::<crate::ui::Ui>(quote! {
            view(style = compute_style(), gap = dropped_by_view())
        })
        .unwrap()
        .elements
        .remove(0)]);
        // Both props became prelude slots…
        assert_eq!(
            scope.slots.iter().filter(|s| s.placement == Placement::Prelude).count(),
            2
        );
        // …but a body that only reads the first drops the second, so
        // `gap`'s expression is never evaluated (as before the rewrite).
        let s0 = slot_ident(scope.slots[0].index);
        let body = quote! { view(()).with_style(#s0) };
        let prelude = scope.prelude_for(&body).to_string();
        assert!(prelude.contains("compute_style"), "{prelude}");
        assert!(!prelude.contains("dropped_by_view"), "{prelude}");
    }

    #[test]
    fn prelude_uses_deferred_init_so_use_site_inference_still_works() {
        reset_slot_counter();
        let scope = split(&[syn::parse2::<crate::ui::Ui>(quote! { Badge(label = name) })
            .unwrap()
            .elements
            .remove(0)]);
        let ident = slot_ident(scope.slots[0].index);
        let body = quote! { Badge { label: (#ident).into() } };
        let prelude: String =
            scope.prelude_for(&body).to_string().chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(prelude, format!("let{ident};{ident}=name;"));
    }
}
