//! `ui!` proc-macro — a JSX-style DSL that desugars to component calls.
//!
//! Grammar (informal):
//!
//! ```text
//! ui!         := node*
//! node        := component
//!              | 'if' rust_expr '{' node* '}' ('else' if_or_block)?
//!              | 'for' pat 'in' rust_expr '{' node* '}'
//!              | rust_expr       (anything else parses as a Rust expression
//!                                 and gets passed through to ChildList)
//! component   := ident '(' prop_list? ')' children?
//!              | ident children
//! prop_list   := prop (',' prop)* ','?
//! prop        := ident '=' rust_expr
//! children    := '{' node* '}'
//! ```
//!
//! ## Component recognition
//!
//! An identifier is parsed as a component invocation **only** when
//! immediately followed by `(` or `{`. Capitalization is purely a
//! convention; the parser doesn't consult it. A bare `Foo` (no parens,
//! no brace) is parsed as a plain Rust expression — useful for things
//! like dropping a precomputed `Element` into a children slot.
//!
//! ## Children — what's legal inside `{ … }`
//!
//! A children block is a sequence of *nodes*, and a node is any of:
//!
//!  - **a component or primitive invocation** — `Foo(...)`,
//!    `Foo(...) { ... }`, `Foo { ... }`, `view { ... }`, `text(...) { ... }`;
//!  - **control flow** — `if cond { … } else { … }`, `for x in iter { … }`,
//!    `match scrutinee { … }` (a reactive `if`/`match`, i.e. a condition
//!    that calls `.get()`, desugars to `when(...)`);
//!  - **any Rust expression** that yields a `ChildList`-compatible value
//!    (anything `IntoElement`). This is the escape hatch and it is *not*
//!    restricted to bare identifiers: a precomputed `let el = …; … el …`,
//!    a **helper-fn call** (`my_section()`), a `Vec<Element>` splat, or an
//!    iterator chain all work as children — and equally as the body of a
//!    `for`. If a child "isn't allowed," it's almost always because the
//!    expression's *type* isn't `IntoElement`, not because the position
//!    forbids expressions.
//!
//! The one thing to keep straight: a bare identifier becomes a component
//! only when followed by `(` or `{` (see *Component recognition*). So
//! `widget` is an expression child, while `widget()` and `widget { … }`
//! are invocations.
//!
//! ## Dispatch
//!
//! Each parsed component emits one of:
//!  - `text(expr)` for `Text` — content from a single-expr children block
//!    or from a `content = expr` prop.
//!  - `button(label, on_click)` for `Button`.
//!  - `view(children)` for `View`.
//!  - For any other identifier `Foo`, a struct-literal dispatch through the
//!    `BuildElement` trait — `BuildElement::build(Foo { field: (v).into(),
//!    ..<Foo as BuildElement>::defaults() })`. `Foo` names the props type
//!    (via a `pub type Foo = FooProps` alias that `#[component]` / the
//!    component library provides). No per-component `macro_rules!` — see
//!    `emit_user`.
//!
//! Reactive `if` (conditions containing `.get()`) is rewritten to
//! `when(cond, then, otherwise)`; non-reactive `if` is emitted verbatim.
//! `for` desugars to a `Vec<Element>` built by mapping over the
//! iterable.
//!
//! ## Attribute coercion
//!
//! String literal attribute values get an implicit `.into()` so
//! `label = "Score"` flows into a `String` field. Other attribute values
//! pass through verbatim — we don't apply generalized `.into()` because
//! of Rust inference fragility on non-literal types.

use proc_macro2::{Delimiter, Span, Spacing, TokenStream as TokenStream2, TokenTree};
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, parenthesized, Expr, Ident, Token};

/// Top-level entry: a `ui! { ... }` invocation parses to a list of elements.
pub struct Ui {
    elements: Vec<UiNode>,
}

/// A single node in the UI tree. Either a component invocation we parsed,
/// or a raw Rust expression that goes through ChildList passthrough.
enum UiNode {
    Component {
        name: Ident,
        props: Vec<Prop>,
        children: Option<Vec<UiNode>>,
        /// Trailing `.method(args)` chains. Used to attach builder
        /// methods like `.bind(r)` to the constructed primitive
        /// without burying them in the prop list. Stored as raw
        /// token streams and appended verbatim to the emitted call.
        chain: Vec<TokenStream2>,
    },
    /// A `for` loop whose body is itself a UI block.
    For {
        pat: syn::Pat,
        iter: Expr,
        /// Optional `, key = EXPR` clause between the iterable and the
        /// body. Required when `iter` is a reactive collection (a
        /// `Signal<Vec<_>>`) — the type system rejects a keyless reactive
        /// loop; harmless on a static loop. The expression is evaluated
        /// per item with the loop pattern in scope (e.g. `key = item.id`).
        key: Option<Expr>,
        body: Vec<UiNode>,
        /// Trailing `.method(args)` chain after the for-block's
        /// closing brace. Author syntax:
        /// `for i in iter { body }.style(expr)`. Each chain entry
        /// is applied to the Virtualizer's emission — the
        /// `.style(expr)` slot pins the row container's flex
        /// style; future chains can set `.horizontal()` /
        /// `.overscan(...)` / etc.
        chain: Vec<TokenStream2>,
    },
    /// An `if` / `if let` / `match`: parsed as a raw Rust expression with
    /// `ui!` recursively applied to each branch's contents.
    /// Branches always evaluate to a single UI node (or nothing for absent else).
    If {
        cond: Expr,
        then_body: Vec<UiNode>,
        else_body: Option<Vec<UiNode>>,
    },
    /// A reactive `match` over an arbitrary scrutinee. When the
    /// scrutinee reads a signal (heuristic: `.get()` in its tokens),
    /// the emitter lowers to a `runtime_core::switch(...)` call so
    /// the active arm re-evaluates whenever the scrutinee changes.
    /// Non-reactive `match` emits plain Rust `match`.
    ///
    /// Each arm's body is a UI block ({ child child child ... }) just
    /// like `if`'s branches.
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
    },
    /// Arbitrary Rust expression to be flattened via ChildList.
    Expr(Expr),
}

struct MatchArm {
    pat: syn::Pat,
    /// Optional `if guard` after the pattern.
    guard: Option<Expr>,
    body: Vec<UiNode>,
}

struct Prop {
    name: Ident,
    value: Expr,
    /// Optional `=> output_signal` clause for structured actions.
    /// Set when a prop is written as `on_click = method(sig) =>
    /// out_signal` — the `=>` token follows the prop's value
    /// expression and an output signal expression follows the `=>`.
    /// `emit_button` reads this to construct a fully-populated
    /// `Action` directly (no `action!`/`bind_press!` macro needed).
    arrow_target: Option<Expr>,
}

/// Recognized accessibility attribute names. Each maps 1:1 to a
/// `Bound<H>` setter of the same name (`a11y_label` → `.a11y_label(..)`,
/// `accessibility` → `.accessibility(..)`, etc.), so both `ui!` and
/// `jsx!` lower them by emitting `.<name>(<value>)`. Keeping the list
/// here (the single source of truth) keeps the two macros in lockstep
/// with the `Element` accessibility surface. When a new author-facing
/// a11y field lands, add its attr name here.
pub(crate) fn is_a11y_attr(name: &str) -> bool {
    matches!(
        name,
        "accessibility"
            | "a11y_label"
            | "a11y_hint"
            | "a11y_role"
            | "a11y_hidden"
            | "a11y_traits"
            | "live_region"
    )
}

impl Parse for Ui {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let elements = parse_ui_nodes(input)?;
        Ok(Ui { elements })
    }
}

/// Parses a sequence of `UiNode`s until end-of-input.
fn parse_ui_nodes(input: ParseStream) -> syn::Result<Vec<UiNode>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        out.push(parse_ui_node(input)?);
        // Optional commas between elements — purely cosmetic.
        let _ = input.parse::<Token![,]>();
    }
    Ok(out)
}

fn parse_ui_node(input: ParseStream) -> syn::Result<UiNode> {
    // Control flow keywords first.
    if input.peek(Token![if]) {
        return parse_if(input);
    }
    if input.peek(Token![for]) {
        return parse_for(input);
    }
    if input.peek(Token![match]) {
        return parse_match(input);
    }
    // Identifier followed by `(` or `{` is a component invocation:
    //   Foo()              Foo(props)              Foo { children }
    //   Foo(props) { children }
    // A bare `Foo` (no parens, no brace) is NOT a component — it parses
    // as a normal Rust expression. Capitalization is purely a convention;
    // the parser doesn't consult it.
    if input.peek(Ident) && next_is_component_invocation(input) {
        return parse_component(input);
    }
    // Fallback: parse a Rust expression. Goes through ChildList::append_to.
    let expr: Expr = input.parse()?;
    Ok(UiNode::Expr(expr))
}

/// Peeks past an identifier to see whether the *next* token is `(` or `{` —
/// the two shapes that mark a component invocation. We have to fork the
/// stream to do the lookahead.
///
/// Treats two identifier shapes as component invocations:
/// 1. **PascalCase** — any identifier starting with an uppercase ASCII
///    letter (the user-component convention). `Foo(...)` and `Foo { ... }`
///    are tag invocations.
/// 2. **Lowercase framework primitives** — `view`, `text`, `button`,
///    `text_input`, etc., recognized via `primitives::canonical_primitive`.
///    To avoid breaking bare-fn-call sites like `icon(LIGHT_LOGO)` that
///    pre-date the lowercase-tag convention, a lowercase primitive only
///    counts as a tag invocation if its `(...)` is **empty** or its first
///    token is `Ident =` (the prop-list shape). Otherwise it falls through
///    to the expression parser as `runtime_core::icon(LIGHT_LOGO)`.
///
/// Everything else (lowercase non-primitive identifiers like
/// `count_label(count)`) falls through to the expression parser, so an
/// embedded reactive method call inside `text { ... }` doesn't get
/// mis-parsed as a tag.
fn next_is_component_invocation(input: ParseStream) -> bool {
    let fork = input.fork();
    let ident = match fork.parse::<Ident>() {
        Ok(i) => i,
        Err(_) => return false,
    };
    let name = ident.to_string();
    let first_upper = name
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false);

    if first_upper {
        return fork.peek(syn::token::Paren) || fork.peek(syn::token::Brace);
    }

    // Lowercase: only treat as tag if it's a known primitive AND the call
    // shape is unambiguously tag-like (empty parens, `{ children }`, or
    // parens whose first token is `Ident =`).
    if crate::primitives::canonical_primitive(&name).is_none() {
        return false;
    }
    if fork.peek(syn::token::Brace) {
        return true;
    }
    if !fork.peek(syn::token::Paren) {
        return false;
    }
    // Look inside the parens for the prop-list shape. `step` lets us walk
    // the cursor without committing the parent stream.
    fork.step(|cursor| {
        let (group_cursor, _span, _after) = cursor
            .group(proc_macro2::Delimiter::Parenthesis)
            .ok_or_else(|| cursor.error("expected `(`"))?;
        // Empty `()` → no props → tag form.
        if group_cursor.eof() {
            return Ok((true, *cursor));
        }
        // First token is an identifier followed by `=` → prop-list shape.
        let mut walk = group_cursor;
        if let Some((tt, rest)) = walk.token_tree() {
            if matches!(tt, proc_macro2::TokenTree::Ident(_)) {
                walk = rest;
                if let Some((tt2, _)) = walk.token_tree() {
                    if matches!(tt2, proc_macro2::TokenTree::Punct(ref p) if p.as_char() == '=') {
                        return Ok((true, *cursor));
                    }
                }
            }
        }
        Ok((false, *cursor))
    })
    .unwrap_or(false)
}

fn parse_component(input: ParseStream) -> syn::Result<UiNode> {
    let name: Ident = input.parse()?;

    // Optional `(prop = expr, ...)` props list.
    let props = if input.peek(syn::token::Paren) {
        let content;
        parenthesized!(content in input);
        let pairs: Punctuated<Prop, Token![,]> = content.parse_terminated(Prop::parse, Token![,])?;
        pairs.into_iter().collect()
    } else {
        Vec::new()
    };

    // Optional `{ children }` block.
    let children = if input.peek(syn::token::Brace) {
        let content;
        braced!(content in input);
        Some(parse_ui_nodes(&content)?)
    } else {
        None
    };

    // Optional trailing `.method(args)` chain. Each segment is parsed
    // as `. ident ( token_stream )` and stored verbatim — we don't
    // interpret the args, just forward them. Supports zero or more
    // chained calls, e.g. `Button(...).bind(r).with_style(...)`.
    let chain = parse_method_chain(input)?;

    Ok(UiNode::Component { name, props, children, chain })
}

/// Parses a sequence of trailing `.method(args)` calls. Stops at the
/// first token that isn't `.`. Each call's args are captured as an
/// opaque `TokenStream2` and replayed verbatim during emission.
fn parse_method_chain(input: ParseStream) -> syn::Result<Vec<TokenStream2>> {
    let mut chain = Vec::new();
    while input.peek(Token![.]) {
        let _: Token![.] = input.parse()?;
        let method: Ident = input.parse()?;
        let args_content;
        parenthesized!(args_content in input);
        let args: TokenStream2 = args_content.parse()?;
        chain.push(quote! { . #method ( #args ) });
    }
    Ok(chain)
}

fn parse_if(input: ParseStream) -> syn::Result<UiNode> {
    let _if_token: Token![if] = input.parse()?;
    // Parse the condition as a Rust expression. `Expr::parse_without_eager_brace`
    // stops the parser from consuming the trailing `{` as a struct-literal.
    let cond: Expr = Expr::parse_without_eager_brace(input)?;
    let then_content;
    braced!(then_content in input);
    let then_body = parse_ui_nodes(&then_content)?;

    let else_body = if input.peek(Token![else]) {
        let _: Token![else] = input.parse()?;
        // Allow chained `else if` by wrapping the rest as a single If node.
        if input.peek(Token![if]) {
            let nested = parse_if(input)?;
            Some(vec![nested])
        } else {
            let else_content;
            braced!(else_content in input);
            Some(parse_ui_nodes(&else_content)?)
        }
    } else {
        None
    };

    Ok(UiNode::If { cond, then_body, else_body })
}

fn parse_for(input: ParseStream) -> syn::Result<UiNode> {
    let _for_token: Token![for] = input.parse()?;
    let pat = syn::Pat::parse_single(input)?;
    let _in_token: Token![in] = input.parse()?;
    let iter: Expr = Expr::parse_without_eager_brace(input)?;
    // Optional `, key = EXPR` clause: the reconciliation key for a
    // reactive list. `parse_without_eager_brace` stopped at the comma
    // (a comma can't continue an expression), so peek for it here. The
    // key expression itself is parsed brace-agnostically so it stops at
    // the body's opening `{`.
    let key = if input.peek(Token![,]) {
        let _comma: Token![,] = input.parse()?;
        let kw: Ident = input.parse()?;
        if kw != "key" {
            return Err(syn::Error::new(
                kw.span(),
                "expected `key` after `,` in a `for` loop (the reactive-list \
                 reconciliation key), e.g. `for item in items, key = item.id { … }`",
            ));
        }
        let _eq: Token![=] = input.parse()?;
        Some(Expr::parse_without_eager_brace(input)?)
    } else {
        None
    };
    let body_content;
    braced!(body_content in input);
    let body = parse_ui_nodes(&body_content)?;
    // Optional trailing `.method(args)` chain after the closing
    // brace — same shape components support. Each entry is replayed
    // verbatim by the Virtualizer-emitting path so authors can pin
    // the row container's style / flex direction / overscan / etc.
    // Example: `for i in count(sig) { ... }.style(row_style())`.
    let chain = parse_method_chain(input)?;
    Ok(UiNode::For { pat, iter, key, body, chain })
}

/// Parse `match scrutinee { pat => { ui_nodes }, pat if guard => { ui_nodes }, ... }`.
///
/// Each arm's body must be a brace-delimited UI block; we don't
/// accept the shorter `pat => single_node` form because the parser
/// would have to decide between "single UiNode" and "single Rust
/// expression that happens to be a tuple, etc." — the brace
/// requirement removes the ambiguity at zero ergonomic cost.
fn parse_match(input: ParseStream) -> syn::Result<UiNode> {
    let _match_token: Token![match] = input.parse()?;
    let scrutinee: Expr = Expr::parse_without_eager_brace(input)?;
    let body_content;
    braced!(body_content in input);

    let mut arms = Vec::new();
    while !body_content.is_empty() {
        let pat = syn::Pat::parse_multi_with_leading_vert(&body_content)?;
        let guard = if body_content.peek(Token![if]) {
            let _: Token![if] = body_content.parse()?;
            Some(Expr::parse_without_eager_brace(&body_content)?)
        } else {
            None
        };
        let _: Token![=>] = body_content.parse()?;
        let arm_content;
        braced!(arm_content in &body_content);
        let body = parse_ui_nodes(&arm_content)?;
        arms.push(MatchArm { pat, guard, body });
        // Optional comma between arms.
        let _ = body_content.parse::<Token![,]>();
    }
    Ok(UiNode::Match { scrutinee, arms })
}

impl Parse for Prop {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let _: Token![=] = input.parse()?;
        let value: Expr = input.parse()?;
        // Optional `=> rhs` clause for structured action props
        // (e.g. `on_click = method(sig) => out_signal`). The Rust
        // expression parser stops at the `=>` because it isn't a
        // valid binary operator — we eat it here and parse the
        // right-hand side as a separate expression so callers can
        // pick it up.
        let arrow_target = if input.peek(Token![=>]) {
            input.parse::<Token![=>]>()?;
            Some(input.parse::<Expr>()?)
        } else {
            None
        };
        Ok(Prop { name, value, arrow_target })
    }
}

// =============================================================================
// Emit
// =============================================================================

/// Walk a parsed `ui! { ... }` and append every component-position
/// ident — i.e. every `UiNode::Component { name }` — into `out` along
/// with its source line. Recurses into nested children, for/if/match
/// bodies. Arbitrary expression-position calls (`UiNode::Expr`) are
/// NOT captured: per the MCP spec (§6.3) a "component" is something
/// that appears as a child in JSX position, not any function call.
///
/// Used by `mcp_emit` (under `feature = "catalog"`) to build the
/// `composes` edge list for each `#[component]` entry. Kept here so
/// the AST stays encapsulated in this module.
#[cfg(feature = "catalog")]
pub(crate) fn collect_component_refs(ui: &Ui, out: &mut Vec<(String, u32)>) {
    collect_from_nodes(&ui.elements, out);
}

#[cfg(feature = "catalog")]
fn collect_from_nodes(nodes: &[UiNode], out: &mut Vec<(String, u32)>) {
    for node in nodes {
        match node {
            UiNode::Component { name, children, .. } => {
                let line = name.span().start().line as u32;
                out.push((name.to_string(), line));
                if let Some(c) = children {
                    collect_from_nodes(c, out);
                }
            }
            UiNode::For { body, .. } => collect_from_nodes(body, out),
            UiNode::If { then_body, else_body, .. } => {
                collect_from_nodes(then_body, out);
                if let Some(e) = else_body {
                    collect_from_nodes(e, out);
                }
            }
            UiNode::Match { arms, .. } => {
                for arm in arms {
                    collect_from_nodes(&arm.body, out);
                }
            }
            UiNode::Expr(_) => {}
        }
    }
}

/// Top-level emit: produce a single expression that yields a `Element`.
/// If the `ui!` body has exactly one element, emit it directly. Otherwise
/// wrap in `view(children![...])`. The whole expression is coerced via
/// `IntoElement` so the macro's caller (typically a `#[component]`
/// function returning `Element`) gets the right type whether the
/// inner expression is a `Bound<H>` (from a primitive constructor like
/// `view(...)`) or a plain `Element` (from a user component's macro
/// expansion).
/// Where a node is being emitted, which decides how control-flow
/// lowers:
///
/// - [`Ctx::Child`] — the node sits in a children list (`View { … }`,
///   a component's children, a `for`/`if`/`match` body, the top-level
///   when there's more than one element). Here a control-flow node may
///   produce a flat `Vec<Element>` (0 / 1 / N siblings); the
///   surrounding `ChildList::append_to` flattens it. Static `if`/`match`
///   branches therefore emit **flat siblings** — no wrapper `View`, and
///   a missing `else` contributes nothing (not an empty `View`).
///
/// - [`Ctx::Single`] — the node must be exactly one `Element`: the
///   sole top-level element (coerced via `IntoElement`), or a
///   `when`/`switch` branch / virtualizer row built by
///   [`emit_block_as_primitive`]. Control-flow that would otherwise be a
///   `Vec` (a `for`, a flattened `if`) is wrapped in a single `View`
///   here.
///
/// Primitives and bare expressions are a single value either way, so
/// they ignore the context.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Child,
    Single,
}

pub fn emit(ui: Ui, input: &TokenStream2) -> TokenStream2 {
    let body = match ui.elements.len() {
        0 => quote! { ::runtime_core::view(::std::vec::Vec::new()) },
        // Sole element: it is coerced to one `Element` below, so emit
        // it in single-slot context.
        1 => emit_node(&ui.elements[0], Ctx::Single),
        _ => {
            let kids = ui.elements.iter().map(|n| emit_node(n, Ctx::Child));
            quote! {
                ::runtime_core::view({
                    let mut __c: ::std::vec::Vec<::runtime_core::Element>
                        = ::std::vec::Vec::new();
                    #( ::runtime_core::ChildList::append_to(#kids, &mut __c); )*
                    __c
                })
            }
        }
    };
    emit_shell(input, body)
}

/// Wrap an emission tail (the real build chain on the happy path, or the
/// diagnostic + empty view on the recovery path) in the SHARED shell that
/// carries the salvage closure.
///
/// Why the happy path carries a salvage copy at all: rust-analyzer's
/// completion inside a macro call expands the buffer TWICE — once as
/// written ("real") and once with a placeholder ident spliced at the
/// cursor ("speculative") — and then resolves nodes of the speculative
/// expansion against the real expansion's HIR **by text range**. That
/// only works when the two expansions are textually aligned up to the
/// cursor. A mid-typing state like a dangling `c.` above an existing
/// statement token-glues into VALID code (`c.` + `c.reset()` ⇒
/// `c.c.reset()`), so the real buffer takes the happy path while the
/// speculative one hits recovery — two structurally unrelated expansions,
/// and the receiver's range lands on an arbitrary expression of the real
/// one (observed: dot-completion on a `CounterHandle` offering `Ref`
/// methods, because the offset happened to cover `counter`). Emitting the
/// SAME salvage closure, at the SAME offset, in BOTH paths restores the
/// alignment for every (valid, mid-typing) buffer pair. Span line info
/// can't help instead: rust-analyzer's proc-macro server reports 1:0 for
/// every span (verified empirically), so the glue is undetectable at
/// macro level.
///
/// The shell is emitted ONLY under an IDE-style expansion host (detected
/// by [`host_has_no_span_lines`]): under real rustc the happy expansion
/// is byte-identical to the pre-shell output. That keeps real builds
/// clean three ways — no `unexpected_cfgs`-style lint games in consumer
/// crates, no type-checking of the salvage copy (a `move` closure in the
/// salvage would steal captures from the real chain and break VALID code
/// with E0382), and no salvage noise in `cargo expand` or in the error
/// output of genuinely broken builds. Flycheck runs rustc, so editor
/// diagnostics never see the salvage either; rust-analyzer's own
/// diagnostics don't include borrowck.
///
/// The diagnostic (recovery path) must come AFTER the closure: parse
/// error messages differ between the real and speculative buffers, and a
/// leading `compile_error!("…")` of a different length would shift every
/// salvage offset — breaking the very alignment this shell exists for.
pub(crate) fn emit_shell(input: &TokenStream2, rest: TokenStream2) -> TokenStream2 {
    if !host_has_no_span_lines(input) {
        return quote! { ::runtime_core::IntoElement::into_element(#rest) };
    }
    let salvaged = salvage_stmts(input.clone());
    quote! {
        ::runtime_core::IntoElement::into_element({
            #[allow(unused, unreachable_code, clippy::all)]
            let __ui_recover = || {
                #( #salvaged )*
            };
            #rest
        })
    }
}

/// True when the expansion host provides no real span positions — the
/// discriminator between rustc and IDE proc-macro servers. rust-analyzer's
/// server reports a degenerate zero-width `1:0` (or `0:0`) for EVERY
/// token span (verified empirically — which is also why a line-aware
/// parse can't detect the `c.`-glues-with-next-line shape and this shell
/// exists at all); under rustc ≥1.88 with proc-macro2's `span-locations`,
/// real tokens carry real positions, and a multi-token input can never be
/// all zero-width at column 0. proc-macro2's fallback spans (unit tests,
/// non-rustc hosts) are degenerate the same way, so tests exercise the
/// shell path. If rust-analyzer ever starts reporting real span lines,
/// this returns false there and completion inside broken `ui!` bodies
/// regresses to the bare-`compile_error!` baseline — revisit the gate
/// (a `#[cfg(rust_analyzer)]` emission, which currently trips
/// `unexpected_cfgs` in consumer crates, becomes the alternative).
fn host_has_no_span_lines(input: &TokenStream2) -> bool {
    input.clone().into_iter().all(|tt| {
        let (s, e) = (tt.span().start(), tt.span().end());
        s == e && s.line <= 1 && s.column == 0
    })
}

/// Emit a *recovery* expansion for a `ui!`/`jsx!` body that failed to
/// parse. Two jobs:
///
/// 1. Re-emit the real `compile_error!` (with the parser's span) so the
///    build still fails with the correct diagnostic at the correct place.
/// 2. Re-surface as much of the raw input as possible in
///    dead-but-type-checked positions, so rust-analyzer keeps full type
///    info (completion, hover, go-to-def) for the parts of the block that
///    *are* well-formed — i.e. everything except the token you're mid-way
///    through typing. Without this, a single in-progress expression turns
///    the entire `ui! { … }` into an opaque `compile_error!` and the IDE
///    goes dark for the whole block.
///
/// Three salvage forms, matched to what completion needs per position:
///
/// - **Prop values** (`label = <expr>`): the RHS as a value expression —
///   variable/method completion inside prop values.
/// - **Component invocations** (`PascalTag(…)`): a STRUCT LITERAL over
///   the tag (which aliases the props type) with every leading ident of
///   the prop list as a `field: todo!()` entry — this is what makes
///   rust-analyzer complete prop NAMES at `Counter(sta|)`, the single
///   most common mid-typing state. See [`salvage_component_invocations`].
/// - **Statement runs** (children blocks): iterated longest-valid-prefix
///   expression salvage, so ONE broken child doesn't eat its siblings —
///   bare exprs, half-typed tag names (value-position ident completion),
///   primitive calls, and the conditions of `if`/`for`/`match` headers
///   whose ui!-flavored bodies aren't valid Rust statements. See
///   [`salvage_statement_run`].
///
/// The salvaged expressions live inside a never-called closure: they're
/// analyzed but never executed, and `&(expr)` avoids moving out of
/// the user's bindings. The whole thing still evaluates to an `Element`
/// so the surrounding code type-checks as far as it can. The closure is
/// emitted through [`emit_shell`] — shared with the HAPPY path — so the
/// real and speculative expansions rust-analyzer compares stay textually
/// aligned regardless of which path each buffer takes (see `emit_shell`
/// for why that alignment is what makes completion resolve at all).
///
/// Everything emitted here is guaranteed-valid Rust syntax: salvage only
/// keeps token runs that successfully parse as a `syn::Expr` (plus the
/// synthesized struct literals, valid by construction). If it emitted
/// unparseable tokens, rust-analyzer's own expansion of `ui!` would fail
/// and we'd be worse off than the `compile_error!` baseline. Original
/// token SPANS are preserved throughout (tokens are reused, never
/// re-created) — that's what lets RA map completions back to the cursor.
pub(crate) fn emit_recovery(input: TokenStream2, err: &syn::Error) -> TokenStream2 {
    let diag = err.to_compile_error();
    emit_shell(
        &input,
        quote! {
            #diag
            ::runtime_core::view(::std::vec::Vec::new())
        },
    )
}

/// Walk a raw token stream and collect salvage STATEMENTS (most are
/// `let _ = &(expr);` wrappers; `let` statements are preserved verbatim),
/// preserving spans. Used by [`emit_shell`] for BOTH the happy and the
/// recovery expansion.
///
/// Strategy: within each token group, split on top-level commas. A
/// segment shaped `ident = <tokens>` (a prop assignment — the lone `=`
/// is `Spacing::Alone`, which rules out `==`/`=>`/`<=`) yields its RHS as
/// a candidate expression. We also recurse into every nested group so
/// children blocks and call arguments get salvaged too. We deliberately
/// do NOT try to parse whole segments as expressions: `Card { … }` is a
/// syntactically valid struct literal but semantically bogus (Card isn't
/// a struct), and emitting it would inject spurious type errors that
/// drown out the real completions.
///
/// PREFIX-STABILITY INVARIANT: because the happy and recovery paths both
/// carry this salvage, and rust-analyzer resolves its speculative
/// expansion against the real one BY TEXT RANGE, the salvage of two
/// inputs that agree up to a position must agree in output up to that
/// position. Every decomposition decision below is therefore made from
/// the FRONT of the token run — never by whether some enclosing run
/// happens to parse as a whole (which would let tokens after the cursor
/// change output before it). That's why control-flow headers and bare
/// calls decompose canonically even when the full expression is valid.
fn salvage_stmts(stream: TokenStream2) -> Vec<TokenStream2> {
    let mut out = Vec::new();
    salvage_from_stream(stream, &mut out, SalvageCtx::Children);
    out
}

/// Push an expression-shaped salvage as a `let _ = &(expr);` statement.
/// `&(expr)` avoids moving out of the user's bindings.
fn push_expr(out: &mut Vec<TokenStream2>, e: TokenStream2) {
    out.push(quote! { let _ = &(#e); });
}

/// Which grammatical position a token run being salvaged sits in. The
/// `ui!` grammar gives `Pascal ( … )` two meanings by position: in CHILD
/// position it is a component tag (struct-literal salvage → prop-NAME
/// completion), but inside a prop value or handler body it is ordinary
/// Rust — a tuple-struct/enum-variant CONSTRUCTOR call (`P2(w, h)`,
/// `Dir::North(steps)`), and struct-literal-salvaging those plants
/// `E0560 no such field` squiggles from rust-analyzer's pull diagnostics
/// on VALID code (user-hit via the always-on happy shell). So the
/// struct-literal salvage fires only in `Children` context.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SalvageCtx {
    /// Direct children of a tag body — `Pascal(…)` is a component tag.
    Children,
    /// Prop values, call arguments, handler bodies — plain Rust.
    Value,
}

fn salvage_from_stream(stream: TokenStream2, out: &mut Vec<TokenStream2>, ctx: SalvageCtx) {
    let mut segment: Vec<TokenTree> = Vec::new();
    let mut segments: Vec<Vec<TokenTree>> = Vec::new();
    for tt in stream {
        match &tt {
            TokenTree::Punct(p) if p.as_char() == ',' && p.spacing() == Spacing::Alone => {
                segments.push(std::mem::take(&mut segment));
            }
            _ => segment.push(tt),
        }
    }
    if !segment.is_empty() {
        segments.push(segment);
    }

    for seg in segments {
        // Component invocations (`PascalTag ( … )`) get a STRUCT-LITERAL
        // salvage so rust-analyzer offers prop-NAME completion at a
        // half-typed prop — see [`salvage_component_invocations`]. Only
        // in CHILD position, where the grammar says a Pascal call IS a
        // tag (see [`SalvageCtx`]).
        if ctx == SalvageCtx::Children {
            salvage_component_invocations(&seg, out);
        }
        // A prop assignment: `ident = <rhs>`. Salvage the RHS only — the
        // whole segment would parse as an assignment to an undefined
        // name and inject noise. STRUCTURED salvage first: a closure /
        // control-flow RHS whose brace body is what's broken keeps its
        // BINDINGS via scaffolding — `on_click = move || { if let Some(c)
        // = … { c.│ } }` must keep `c` bound, or dot-completion at the
        // cursor degrades to unknown-receiver noise (the bug that made
        // handle methods invisible until a prefix was typed).
        if let Some(rhs) = prop_value_tokens(&seg) {
            if let Some(composite) = try_scaffold(&rhs, SalvageCtx::Value) {
                push_expr(out, composite);
            } else if let Some(expr_ts) = parse_expr_prefix(rhs.clone()) {
                push_expr(out, expr_ts);
            } else {
                // Nothing parseable at this level — groups inside may
                // still hold salvageable content.
                for tt in &rhs {
                    if let TokenTree::Group(g) = tt {
                        salvage_from_stream(g.stream(), out, SalvageCtx::Value);
                    }
                }
            }
            continue;
        }
        // Otherwise treat the segment as a STATEMENT RUN — ui! children
        // are whitespace-separated, so one broken child must not eat its
        // siblings. Owns ALL group recursion for its tokens: a scaffolded
        // body must not ALSO be salvaged flat, or the bound copies get
        // shadowed by unbound duplicates full of unresolved-name noise.
        salvage_statement_run(seg, out, ctx);
    }
}

/// Try to rebuild `HEAD { BODY }` where the BODY is what failed to
/// parse: keep HEAD as REAL scaffolding — preserving pattern bindings
/// (`if let Some(c) = …`, `for x in …`) and closure parameters
/// (`move |e| …`) — around a recursively salvaged body. The candidate is
/// validated by parsing the actual composite, so a header whose block
/// isn't statement-shaped (`match` needs arms, a bogus head, …) returns
/// `None` and falls back to coarser salvage.
fn try_scaffold(toks: &[TokenTree], ctx: SalvageCtx) -> Option<TokenStream2> {
    let TokenTree::Group(g) = toks.last()? else {
        return None;
    };
    if g.delimiter() != Delimiter::Brace || toks.len() < 2 {
        return None;
    }
    let head: TokenStream2 = toks[..toks.len() - 1].iter().cloned().collect();
    let mut inner: Vec<TokenStream2> = Vec::new();
    salvage_from_stream(g.stream(), &mut inner, ctx);
    let candidate = quote! {
        #head {
            #( #inner )*
        }
    };
    syn::parse2::<Expr>(candidate.clone()).ok().map(|_| candidate)
}

/// Scaffold a plain `if`/`for` through the SAME type dispatch the real
/// `ui!` lowering uses, so the salvage accepts exactly what the DSL
/// accepts (see the call site in [`salvage_statement_run`] for the full
/// rationale — the plain-Rust form E0308-squiggles valid reactive
/// conditions under rust-analyzer). Returns the number of consumed
/// tokens on success; `None` (nothing emitted) when the construct is too
/// broken to scaffold this way.
///
/// For `if`, the whole `else if …`/`else` chain is consumed: the else
/// branch becomes the dispatch's second closure, with a chained
/// `else if` salvaged recursively inside it.
fn salvage_dispatch_construct(
    kw: &str,
    toks: &[TokenTree],
    out: &mut Vec<TokenStream2>,
    ctx: SalvageCtx,
) -> Option<usize> {
    let body_at = toks.iter().position(
        |t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace),
    )?;
    let TokenTree::Group(body) = &toks[body_at] else {
        return None;
    };

    if kw == "for" {
        let in_pos = toks[..body_at]
            .iter()
            .position(|t| matches!(t, TokenTree::Ident(i) if i == "in"))?;
        if in_pos < 1 || in_pos + 1 >= body_at {
            return None;
        }
        let pat: TokenStream2 = toks[1..in_pos].iter().cloned().collect();
        let iter: TokenStream2 = toks[in_pos + 1..body_at].iter().cloned().collect();
        let mut body_stmts: Vec<TokenStream2> = Vec::new();
        salvage_from_stream(body.stream(), &mut body_stmts, ctx);
        let candidate = quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::{StaticForEach as _, ReactiveForEach as _};
                (#iter).__idealyst_for_each(move |#pat| {
                    #( #body_stmts )*
                    ::std::vec::Vec::new()
                })
            }
        };
        return syn::parse2::<Expr>(candidate.clone()).ok().map(|_| {
            push_expr(out, candidate);
            body_at + 1
        });
    }

    // `if` — a nonempty condition is required.
    if body_at < 2 {
        return None;
    }
    let cond: TokenStream2 = toks[1..body_at].iter().cloned().collect();
    let mut then_stmts: Vec<TokenStream2> = Vec::new();
    salvage_from_stream(body.stream(), &mut then_stmts, ctx);

    let mut consumed = body_at + 1;
    let mut else_stmts: Vec<TokenStream2> = Vec::new();
    if matches!(toks.get(consumed), Some(TokenTree::Ident(i)) if i == "else") {
        match (toks.get(consumed + 1), toks.get(consumed + 2)) {
            (Some(TokenTree::Group(g)), _) if g.delimiter() == Delimiter::Brace => {
                salvage_from_stream(g.stream(), &mut else_stmts, ctx);
                consumed += 2;
            }
            (Some(TokenTree::Ident(i)), _) if i == "if" => {
                let end = if_chain_extent(toks, consumed + 1);
                salvage_statement_run(toks[consumed + 1..end].to_vec(), &mut else_stmts, ctx);
                consumed = end;
            }
            _ => {}
        }
    }
    let candidate = quote! {
        {
            #[allow(unused_imports)]
            use ::runtime_core::{StaticCond as _, ReactiveCond as _};
            (#cond).__idealyst_if(
                move || { #( #then_stmts )* ::std::vec::Vec::new() },
                move || { #( #else_stmts )* ::std::vec::Vec::new() },
            )
        }
    };
    syn::parse2::<Expr>(candidate.clone()).ok().map(|_| {
        push_expr(out, candidate);
        consumed
    })
}

/// Index just past an `if … { … } (else if … { … })* (else { … })?`
/// chain whose `if` sits at `start`. Bodies are single brace-group
/// tokens, so the scan can't be confused by nested constructs.
fn if_chain_extent(toks: &[TokenTree], start: usize) -> usize {
    let mut pos = start;
    loop {
        let Some(b) = toks[pos..].iter().position(
            |t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace),
        ) else {
            return toks.len();
        };
        pos += b + 1;
        match (toks.get(pos), toks.get(pos + 1)) {
            (Some(TokenTree::Ident(e)), Some(TokenTree::Group(g)))
                if e == "else" && g.delimiter() == Delimiter::Brace =>
            {
                return pos + 2;
            }
            (Some(TokenTree::Ident(e)), Some(TokenTree::Ident(i)))
                if e == "else" && i == "if" =>
            {
                pos += 2;
            }
            _ => return pos,
        }
    }
}

/// Salvage a statement-shaped token run: repeatedly take the longest
/// valid leading expression, emit it, and continue with the remainder;
/// when no prefix parses, drop one leading token and keep going. Two
/// head shapes are consumed WITHOUT expression salvage:
///
/// - `Ident { … }` (tag-with-children) — parses as a struct literal but
///   is semantically bogus from a children block (`Card { … }` would
///   type-check the wrong way); its braces were already recursed.
/// - `PascalTag ( … )` — parses as a fn call to the component fn, whose
///   arity/types won't match the prop list (noise); the struct-literal
///   salvage from [`salvage_component_invocations`] is the useful
///   surface for it.
fn salvage_statement_run(mut toks: Vec<TokenTree>, out: &mut Vec<TokenStream2>, ctx: SalvageCtx) {
    while !toks.is_empty() {
        let head_pair_to_skip = match (toks.first(), toks.get(1)) {
            (Some(TokenTree::Ident(_)), Some(TokenTree::Group(g)))
                if g.delimiter() == Delimiter::Brace =>
            {
                true
            }
            (Some(TokenTree::Ident(i)), Some(TokenTree::Group(g)))
                if g.delimiter() == Delimiter::Parenthesis && is_pascal(i) =>
            {
                true
            }
            _ => false,
        };
        if head_pair_to_skip {
            // The pair itself isn't expression-salvaged, but its group
            // still holds salvageable content: a BRACE group is a tag's
            // children block (Children ctx), a PAREN group is its prop
            // list (Value ctx — Pascal calls inside are constructors).
            if let Some(TokenTree::Group(g)) = toks.get(1) {
                let inner_ctx = if g.delimiter() == Delimiter::Brace {
                    SalvageCtx::Children
                } else {
                    SalvageCtx::Value
                };
                salvage_from_stream(g.stream(), out, inner_ctx);
            }
            toks.drain(..2);
            continue;
        }

        // `let` statements: a whole, valid `let … ;` is preserved
        // VERBATIM so the binding stays live for the statements after it
        // — `let c = counter.get().unwrap(); c.│` needs `c` bound in the
        // salvage copy or dot-completion has no receiver type. Decided
        // entirely from the front (`let` … first top-level `;`), so it's
        // prefix-stable. A broken/mid-typed `let` falls back to salvaging
        // the `=`-RHS prefix (binding lost — acceptable, the buffer is
        // already broken at exactly that statement).
        if matches!(toks.first(), Some(TokenTree::Ident(i)) if i == "let") {
            let semi = toks.iter().position(
                |t| matches!(t, TokenTree::Punct(p) if p.as_char() == ';'),
            );
            if let Some(semi) = semi {
                let stmt: TokenStream2 = toks[..=semi].iter().cloned().collect();
                if syn::parse2::<syn::Block>(quote! { { #stmt } }).is_ok() {
                    out.push(stmt);
                    toks.drain(..=semi);
                    continue;
                }
            }
            let upto = semi.unwrap_or(toks.len());
            if let Some(eq) = toks[..upto].iter().position(|t| {
                matches!(t, TokenTree::Punct(p)
                    if p.as_char() == '=' && p.spacing() == Spacing::Alone)
            }) {
                if let Some(ts) = parse_expr_prefix(toks[eq + 1..upto].to_vec()) {
                    push_expr(out, ts);
                }
            }
            let drain_to = if semi.is_some() { upto + 1 } else { upto };
            toks.drain(..drain_to);
            continue;
        }

        // Control-flow with a statement-shaped body scaffolds CANONICALLY
        // — even when the whole expression is valid. If only the broken
        // variant scaffolded, a buffer whose glued form parses (`c.` +
        // `c.reset()` ⇒ `c.c.reset()`) would emit `if … { c.bump(10); }`
        // whole while its speculative twin emits the scaffold, and the
        // two expansions would diverge BEFORE the cursor (see
        // [`emit_shell`] on why that kills completion). `match` is
        // excluded: its arms aren't statements, so a scaffold body would
        // flat-salvage the arms and strip their pattern bindings on
        // VALID code; it keeps whole-expression salvage below.
        //
        // WHICH scaffold form depends on the construct:
        // - Plain `if`/`for` go through the SAME type dispatch the real
        //   lowering uses (`__idealyst_if` / `__idealyst_for_each`), NOT
        //   a plain Rust `if`/`for` — `ui!` legally accepts a
        //   `ReadSignal<bool>` condition and a `Signal<Vec<_>>` iterable,
        //   which a plain `if`/`for` rejects, and rust-analyzer surfaces
        //   that as an E0308 squiggle ON VALID CODE via its pull-model
        //   diagnostics (user-hit: `if is_high` where
        //   `is_high = memo(…)`). The dispatch accepts exactly what the
        //   DSL accepts, so the salvage typechecks iff the real code
        //   does; it also keeps `for`-loop items TYPED for either world.
        // - `if let`/`while let`/`while` keep the plain-Rust scaffold:
        //   their conditions are plain Rust in valid code (a `let`
        //   pattern must already match its RHS type; `while` has no
        //   reactive form), and the `let` forms need the real construct
        //   to keep their pattern BINDINGS live.
        if let Some(TokenTree::Ident(kw)) = toks.first() {
            let kw = kw.to_string();
            let let_form =
                matches!(toks.get(1), Some(TokenTree::Ident(i)) if i == "let");
            if (kw == "if" || kw == "for") && !let_form {
                if let Some(consumed) = salvage_dispatch_construct(&kw, &toks, out, ctx) {
                    toks.drain(..consumed);
                    continue;
                }
                // Mid-typed condition / no body yet: fall through to the
                // keyword-condition branch below.
            } else if kw == "if" || kw == "while" {
                if let Some(body_at) = toks.iter().position(|t| {
                    matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                }) {
                    if let Some(composite) = try_scaffold(&toks[..=body_at], ctx) {
                        push_expr(out, composite);
                        toks.drain(..=body_at);
                        continue;
                    }
                }
            }
        }

        // Bare call in statement position (`ident ( … )`, lowercase, not
        // chained): decompose CANONICALLY into the callee name plus the
        // salvage of its arguments, even when the whole call is valid —
        // whether the interior parses must not change the output emitted
        // before it (prefix stability again). Chained calls
        // (`foo(x).bar()`) keep whole-expression salvage: decomposing
        // would orphan the chain, and the front-token rule stays
        // deterministic by peeking only at the token after the group.
        if let (Some(TokenTree::Ident(head)), Some(TokenTree::Group(g))) =
            (toks.first(), toks.get(1))
        {
            let chained = matches!(toks.get(2), Some(TokenTree::Punct(p))
                if p.as_char() == '.' || p.as_char() == '?');
            let keyword = matches!(
                head.to_string().as_str(),
                "if" | "while" | "for" | "match" | "move" | "return" | "break"
            );
            if g.delimiter() == Delimiter::Parenthesis
                && !is_pascal(head)
                && !keyword
                && !chained
            {
                push_expr(out, head.to_token_stream());
                salvage_from_stream(g.stream(), out, SalvageCtx::Value);
                toks.drain(..2);
                continue;
            }
        }

        // A LEADING brace group is descended, never block-expr-salvaged
        // whole: `{ for x in rows { … } }` parses as a block expression,
        // and swallowing it would re-emit DSL-flavored contents as plain
        // Rust (the exact type-error class the dispatch scaffolds above
        // exist to prevent) — and break prefix stability, since whether
        // the block parses depends on its interior.
        if matches!(toks.first(), Some(TokenTree::Group(g))
            if g.delimiter() == Delimiter::Brace)
        {
            if let TokenTree::Group(g) = &toks[0] {
                salvage_from_stream(g.stream(), out, ctx);
            }
            toks.remove(0);
            continue;
        }

        let mut n = toks.len();
        let mut consumed = 0;
        while n > 0 {
            let ts: TokenStream2 = toks[..n].iter().cloned().collect();
            if let Ok(expr) = syn::parse2::<Expr>(ts) {
                push_expr(out, expr.to_token_stream());
                consumed = n;
                break;
            }
            n -= 1;
        }
        if consumed > 0 {
            toks.drain(..consumed);
            continue;
        }
        // Nothing parseable starts here. STRUCTURED attempt first: if a
        // brace group follows a header, rebuild `HEAD { salvaged-body }`
        // so pattern bindings survive (`if let Some(c) = … { c.│ }`
        // keeps `c` bound — the difference between real dot-completion
        // and unknown-receiver noise at the cursor). Reached for
        // non-keyword headers (statement-position closures etc.) — the
        // control-flow keywords already tried this above.
        if let Some(body_at) = toks.iter().position(|t| {
            matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
        }) {
            if body_at > 0 {
                if let Some(composite) = try_scaffold(&toks[..=body_at], ctx) {
                    push_expr(out, composite);
                    toks.drain(..=body_at);
                    continue;
                }
            }
        }
        // Scaffolding didn't apply (no header/brace, or a non-statement
        // body like `match` arms). For control-flow headers, the
        // condition is still salvageable Rust — pull it out so
        // `if fro…` keeps completion, then step over the header. The
        // body group is recursed flat before draining.
        if let Some(TokenTree::Ident(kw)) = toks.first() {
            let kw = kw.to_string();
            if kw == "if" || kw == "while" || kw == "match" || kw == "for" {
                let body_at = toks
                    .iter()
                    .position(|t| {
                        matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                    })
                    .unwrap_or(toks.len());
                // `for pat in iter` — the expression is what follows
                // `in`; for the others it's everything after the keyword.
                // `if let PAT = expr` — a bare `let` expression is only
                // valid inside `if`/`while`, so salvage the expr after
                // `=` instead of the whole let (emitting `&(let …)` used
                // to produce a bogus-position error squiggle right on
                // the author's pattern).
                let cond_from = if kw == "for" {
                    toks[..body_at]
                        .iter()
                        .position(|t| matches!(t, TokenTree::Ident(i) if i == "in"))
                        .map(|i| i + 1)
                        .unwrap_or(1)
                } else if matches!(toks.get(1), Some(TokenTree::Ident(i)) if i == "let") {
                    toks[..body_at]
                        .iter()
                        .position(|t| {
                            matches!(t, TokenTree::Punct(p)
                                if p.as_char() == '=' && p.spacing() == Spacing::Alone)
                        })
                        .map(|i| i + 1)
                        .unwrap_or(1)
                } else {
                    1
                };
                if cond_from < body_at {
                    if let Some(ts) = parse_expr_prefix(toks[cond_from..body_at].to_vec()) {
                        push_expr(out, ts);
                    }
                }
                if let Some(TokenTree::Group(g)) = toks.get(body_at) {
                    salvage_from_stream(g.stream(), out, ctx);
                }
                let drain_to = (body_at + 1).min(toks.len());
                toks.drain(..drain_to);
                continue;
            }
        }
        // Mid-typed operator, stray punct, or an unrecognized head —
        // step past it, salvaging a group's interior on the way.
        if let TokenTree::Group(g) = &toks[0] {
            salvage_from_stream(g.stream(), out, ctx);
        }
        toks.remove(0);
    }
}

/// True when the ident starts uppercase — the `ui!` component-tag
/// convention (primitives are lowercase-only).
fn is_pascal(i: &proc_macro2::Ident) -> bool {
    i.to_string().chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// For every `PascalTag ( … )` pair in the segment, emit a struct-literal
/// salvage:
///
/// ```ignore
/// Counter { sta: ::core::todo!(), start: ::core::todo!(), ..Default::default() }
/// ```
///
/// The tag doubles as the props type (`#[component]` emits `pub type Tag =
/// TagProps`), so with the ORIGINAL ident spans preserved rust-analyzer
/// treats the half-typed `sta` as a field name of the real props struct
/// and completes `start` — the single most common mid-typing state.
/// `todo!()` types as `!` and coerces to every field type, so the
/// complete prop names add zero type-mismatch noise; only the half-typed
/// name shows an unknown-field error, and that sits under the cursor
/// where an error is expected anyway. `..Default::default()` is valid on
/// every props struct (the `BuildElement: Default` contract).
fn salvage_component_invocations(seg: &[TokenTree], out: &mut Vec<TokenStream2>) {
    // PascalCase enum constructors that pattern/expression code uses
    // constantly — `Some(c)` in an `if let` is NOT a component
    // invocation, and emitting `Some { c: todo!() … }` would plant a
    // type-error squiggle on the author's own pattern.
    const NOT_COMPONENTS: &[&str] = &["Some", "Ok", "Err", "None"];
    for pair in seg.windows(2) {
        let (TokenTree::Ident(name), TokenTree::Group(g)) = (&pair[0], &pair[1]) else {
            continue;
        };
        if g.delimiter() != Delimiter::Parenthesis || !is_pascal(name) {
            continue;
        }
        if NOT_COMPONENTS.contains(&name.to_string().as_str()) {
            continue;
        }
        // Leading ident of each top-level comma segment inside the parens
        // — covers `sta`, `start = 3`, and `start =` alike.
        let mut fields: Vec<proc_macro2::Ident> = Vec::new();
        let mut at_start = true;
        for tt in g.stream() {
            match &tt {
                TokenTree::Punct(p) if p.as_char() == ',' && p.spacing() == Spacing::Alone => {
                    at_start = true;
                }
                TokenTree::Ident(i) if at_start => {
                    fields.push(i.clone());
                    at_start = false;
                }
                _ => at_start = false,
            }
        }
        push_expr(
            out,
            quote! {
                #name {
                    #( #fields: ::core::todo!(), )*
                    ..::core::default::Default::default()
                }
            },
        );
    }
}

/// If `seg` begins with `ident =` (a lone `=`, not `==`/`=>`/…), return
/// the right-hand-side tokens. Otherwise `None`.
fn prop_value_tokens(seg: &[TokenTree]) -> Option<Vec<TokenTree>> {
    match (seg.first(), seg.get(1)) {
        (Some(TokenTree::Ident(_)), Some(TokenTree::Punct(p)))
            if p.as_char() == '=' && p.spacing() == Spacing::Alone =>
        {
            Some(seg[2..].to_vec())
        }
        _ => None,
    }
}

/// Parse the longest prefix of `toks` that forms a valid `syn::Expr`,
/// returning it re-tokenized (spans preserved). Trimming the tail lets us
/// recover `foo` from a half-typed `foo.` and `foo.bar` from `foo.bar(`.
fn parse_expr_prefix(mut toks: Vec<TokenTree>) -> Option<TokenStream2> {
    while !toks.is_empty() {
        let ts: TokenStream2 = toks.iter().cloned().collect();
        if let Ok(expr) = syn::parse2::<Expr>(ts) {
            return Some(expr.to_token_stream());
        }
        toks.pop();
    }
    None
}

fn emit_node(node: &UiNode, ctx: Ctx) -> TokenStream2 {
    match node {
        UiNode::Component { name, props, children, chain } => {
            emit_component(name, props, children.as_deref(), chain)
        }
        UiNode::If { cond, then_body, else_body } => {
            emit_if(cond, then_body, else_body.as_deref(), ctx)
        }
        UiNode::For { pat, iter, key, body, chain } => {
            emit_for(pat, iter, key.as_ref(), body, chain, ctx)
        }
        UiNode::Match { scrutinee, arms } => emit_match(scrutinee, arms, ctx),
        UiNode::Expr(e) => e.to_token_stream(),
    }
}

/// Emit a prop's value tokens. String literals get an implicit `.into()` so
/// `label = "Score"` can flow into a `String` field without `.to_string()`.
/// Other expressions pass through verbatim — we don't want generalized
/// .into() coercion because of inference fragility on non-literal types.
fn emit_attr_value(value: &Expr) -> TokenStream2 {
    if matches!(value, Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(_), .. })) {
        quote! { #value.into() }
    } else {
        quote! { #value }
    }
}

/// Emit a component invocation. Primitives (Text/Button/View/When) dispatch
/// to free functions in runtime_core; other names dispatch through the
/// `BuildElement` trait via a struct literal (see `emit_user`).
fn emit_component(
    name: &Ident,
    props: &[Prop],
    children: Option<&[UiNode]>,
    chain: &[TokenStream2],
) -> TokenStream2 {
    // Framework primitives are a fixed set, canonicalized to snake_case
    // (`view`, `text`, `text_input`, …) to match the `runtime_core::view(...)`
    // builder fn names and React's lowercase-intrinsic convention. PascalCase
    // call sites (`View(...)`) are still accepted during the migration window;
    // see `primitives::canonical_primitive`. Everything else is a user/library
    // `#[component]`, dispatched to its real `Name!` macro via `emit_user`
    // (real macro-name resolution → import-renames, qualified paths, and IDE
    // nav all work).
    //
    // For primitives, two props attach as method calls rather than
    // constructor args: `style = …` → `.with_style(…)`, and
    // `disabled = …` → `.disabled(…)` (button only). `Pressable` is
    // deliberately omitted: that tag is owned by idea-ui's styled
    // component; bare-primitive users call `runtime_core::pressable(...)`.
    let name_str = name.to_string();
    let canonical = crate::primitives::canonical_primitive(&name_str);
    let is_primitive = canonical.is_some();
    let supports_disabled = canonical == Some("button");

    let (style_prop, disabled_prop, test_id_prop, a11y_props, other_props): (
        Vec<&Prop>,
        Vec<&Prop>,
        Vec<&Prop>,
        Vec<&Prop>,
        Vec<&Prop>,
    ) = if is_primitive {
        let mut style = None;
        let mut disabled = None;
        let mut test_id = None;
        let mut a11y = Vec::new();
        let mut rest = Vec::with_capacity(props.len());
        for p in props {
            if p.name == "style" && style.is_none() {
                style = Some(p);
            } else if supports_disabled && p.name == "disabled" && disabled.is_none() {
                disabled = Some(p);
            } else if p.name == "test_id" && test_id.is_none() {
                // Robot/automation anchor. Maps to the always-present
                // `Element::with_test_id` (not the robot-gated builder
                // `.test_id`), so it works on every primitive and never depends
                // on the `robot` feature being on at macro-expansion time.
                // Without this, `view(test_id = …)` silently dropped the id.
                test_id = Some(p);
            } else if is_a11y_attr(&p.name.to_string()) {
                // `accessibility`, `a11y_label`, `a11y_role`, … attach as
                // post-fix `Bound` setter calls, like `style`/`disabled`.
                a11y.push(p);
            } else {
                rest.push(p);
            }
        }
        (
            style.into_iter().collect(),
            disabled.into_iter().collect(),
            test_id.into_iter().collect(),
            a11y,
            rest,
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), props.iter().collect())
    };

    let other_props: Vec<Prop> = other_props
        .into_iter()
        .map(|p| Prop {
            name: p.name.clone(),
            value: p.value.clone(),
            arrow_target: p.arrow_target.clone(),
        })
        .collect();

    let inner = match (canonical, name_str.as_str()) {
        (Some("text"), _) => emit_text(&other_props, children),
        (Some("button"), _) => emit_button(&other_props, children),
        (Some("view"), _) => emit_view(&other_props, children),
        (Some("when"), _) => emit_when(&other_props, children),
        (Some("icon"), _) => emit_icon(&other_props, children),
        (Some("image"), _) => emit_image(&other_props, children),
        (Some("text_input"), _) => emit_text_input(&other_props, children),
        (Some("toggle"), _) => emit_toggle(&other_props, children),
        (Some("scroll_view"), _) => emit_scroll_view(&other_props, children),
        (Some("slider"), _) => emit_slider(&other_props, children),
        (Some("activity_indicator"), _) => emit_activity_indicator(&other_props, children),
        (Some("flat_list"), _) => emit_flat_list(&other_props, children),
        (Some("graphics"), _) => emit_graphics(&other_props, children),
        (Some("link"), _) => emit_link(&other_props, children),
        (Some("overlay"), _) => emit_overlay(&other_props, children),
        (Some("anchored_overlay"), _) => emit_anchored_overlay(&other_props, children),
        (Some("presence"), _) => emit_presence(&other_props, children),
        (_, "DrawerNavigator") => emit_drawer_navigator(&other_props, children),
        (_, "CardTabs") => emit_card_tabs(&other_props, children),
        _ => emit_user(name, props, children),
    };

    let with_style = if let Some(p) = style_prop.first() {
        let v = &p.value;
        quote! { (#inner).with_style(#v) }
    } else {
        inner
    };

    let with_disabled = if let Some(p) = disabled_prop.first() {
        let v = &p.value;
        quote! { (#with_style).disabled(#v) }
    } else {
        with_style
    };

    // Robot/automation `test_id`. The `Bound::test_id` builder is always
    // present (it just stores the id; only the registry that reads it is
    // `robot`-gated), so emitting it never depends on the feature.
    let with_test_id = if let Some(p) = test_id_prop.first() {
        let v = &p.value;
        quote! { (#with_disabled).test_id(#v) }
    } else {
        with_disabled
    };

    // Chain the accessibility setters. Each recognized attr name
    // (`a11y_label`, `a11y_role`, `accessibility`, …) maps 1:1 to a
    // `Bound` setter of the same name, so we emit `.<name>(<value>)`.
    let with_a11y = if a11y_props.is_empty() {
        with_test_id
    } else {
        let setters = a11y_props.iter().map(|p| {
            let name = &p.name;
            let v = &p.value;
            quote! { .#name(#v) }
        });
        quote! { (#with_test_id) #(#setters)* }
    };

    // Append any trailing `.method(args)` calls verbatim. The
    // expression is parenthesized once so the chain attaches to the
    // final value of the inner expression, not to its head.
    if chain.is_empty() {
        with_a11y
    } else {
        quote! { (#with_a11y) #(#chain)* }
    }
}

/// One piece of an f-string text literal: a literal fragment or a
/// `{name}` / `{name:spec}` interpolation slot.
enum FPiece {
    Lit(String),
    Slot {
        name: String,
        /// The `format!` template applied to the slot value —
        /// `"{}"` for a bare `{name}`, `"{:spec}"` when a spec was given.
        fmt: String,
    },
}

/// Parse a text-position string literal as an f-string.
///
/// - `Ok(None)` — the literal contains NO valid `{ident}` placeholder:
///   it is prose and stays verbatim, whatever braces it contains. This
///   is the compatibility rule: interpolation only activates on clear
///   intent (`"use { to open"` and legacy `"{{"`-escaped strings never
///   change meaning).
/// - `Ok(Some(pieces))` — at least one valid placeholder; `{{`/`}}`
///   unescape, and every other brace use must be well-formed.
/// - `Err(msg)` — the literal HAS a valid placeholder but also a brace
///   error (positional `{}`/`{0}`, `{:?}` Debug spec, unmatched brace,
///   non-identifier) — loud, because intent is clearly interpolation.
fn parse_fstring(value: &str) -> Result<Option<Vec<FPiece>>, String> {
    let mut pieces: Vec<FPiece> = vec![FPiece::Lit(String::new())];
    let mut first_err: Option<String> = None;
    let mut slots = 0usize;
    let mut push_ch = |pieces: &mut Vec<FPiece>, c: char| match pieces.last_mut() {
        Some(FPiece::Lit(s)) => s.push(c),
        _ => pieces.push(FPiece::Lit(c.to_string())),
    };
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    push_ch(&mut pieces, '{');
                    continue;
                }
                let mut inner = String::new();
                let mut closed = false;
                for ic in chars.by_ref() {
                    if ic == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(ic);
                }
                if !closed {
                    if first_err.is_none() {
                        first_err =
                            Some("unmatched `{` — escape a literal brace as `{{`".to_string());
                    }
                    continue;
                }
                let (name, spec) = match inner.split_once(':') {
                    Some((n, s)) => (n, Some(s)),
                    None => (inner.as_str(), None),
                };
                let ident_ok = !name.is_empty()
                    && name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                if !ident_ok {
                    if first_err.is_none() {
                        first_err = Some(format!(
                            "`{{{inner}}}` — text f-strings take NAMED placeholders only \
                             (`{{count}}`, `{{count:.2}}`). For positional args or \
                             expressions use `text {{ move || format!(…) }}`."
                        ));
                    }
                    continue;
                }
                // A valid IDENT signals interpolation intent even when the
                // spec is unsupported — count it BEFORE the spec check so
                // `"{count:?}"` errors loudly instead of silently
                // rendering verbatim (only ident-less braces get the
                // prose tolerance).
                slots += 1;
                if let Some(s) = spec {
                    if s.contains('?') {
                        if first_err.is_none() {
                            first_err = Some(format!(
                                "`{{{inner}}}` — Debug formatting (`:?`) isn't available in \
                                 text f-strings (slots format via Display); use \
                                 `text {{ move || format!(…) }}`."
                            ));
                        }
                        continue;
                    }
                }
                let fmt = match spec {
                    Some(s) => format!("{{:{s}}}"),
                    None => "{}".to_string(),
                };
                pieces.push(FPiece::Slot { name: name.to_string(), fmt });
                pieces.push(FPiece::Lit(String::new()));
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                // A lone `}` in prose is tolerated verbatim (only an
                // f-string with slots unescapes, and a stray `}` there
                // renders as itself — matching `}}`).
                push_ch(&mut pieces, '}');
            }
            other => push_ch(&mut pieces, other),
        }
    }
    if slots == 0 {
        return Ok(None);
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(Some(pieces))
}

/// If `expr` is a text-position string literal with `{name}`
/// placeholders, emit the typed-slot interpolation (`TextSlotPart` list →
/// `__idealyst_text_from_parts`). Returns `None` for non-literals and
/// placeholder-free literals (caller falls through to the existing
/// paths). See `parse_fstring` for the exact literal semantics and
/// `runtime_core::sources` for the slot/type dispatch.
pub(crate) fn try_emit_fstring(expr: &Expr) -> Option<TokenStream2> {
    let Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(lit), .. }) = expr else {
        return None;
    };
    try_emit_fstring_lit(lit)
}

/// Literal-direct form of [`try_emit_fstring`] — `jsx!` prop values
/// carry bare `LitStr`s rather than `Expr`s.
pub(crate) fn try_emit_fstring_lit(lit: &syn::LitStr) -> Option<TokenStream2> {
    let pieces = match parse_fstring(&lit.value()) {
        Ok(Some(p)) => p,
        Ok(None) => return None,
        Err(msg) => {
            let msg = format!("text f-string error: {msg}");
            return Some(quote::quote_spanned! { lit.span() => ::std::compile_error!(#msg) });
        }
    };
    let parts = pieces.iter().filter_map(|p| match p {
        FPiece::Lit(s) if s.is_empty() => None,
        FPiece::Lit(s) => Some(quote! { ::runtime_core::TextSlotPart::Lit(#s) }),
        FPiece::Slot { name, fmt } => {
            // The ident carries the LITERAL's span — errors on an
            // unresolved `{name}` point at the string (stable Rust has
            // no sub-literal spans).
            let ident = proc_macro2::Ident::new(name, lit.span());
            Some(quote! {
                ::runtime_core::TextSlotPart::Slot({
                    #[allow(unused_imports)]
                    use ::runtime_core::{StaticTextSlot as _, ReactiveTextSlot as _};
                    (#ident).__idealyst_text_slot(
                        move |__v: &dyn ::std::fmt::Display| ::std::format!(#fmt, __v),
                    )
                })
            })
        }
    });
    Some(quote! {
        ::runtime_core::__idealyst_text_from_parts(::std::vec![ #( #parts ),* ])
    })
}

fn emit_text(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    // Text takes its content from either a `content` prop or a
    // children block. Children win when both are present.
    //
    // Three emission modes, in priority order:
    //
    //   1. **Structured `Derived<String>`** when the body is a
    //      function-call expression whose args look like bare
    //      signal references (e.g. `text { count_label(count) }`).
    //      Emits a `TextSource::Bound(Derived<String> { method,
    //      inputs, initial, compute })` — generator backends
    //      (Roku) read the structure; runtime backends use
    //      `compute`. This is the path that used to require
    //      a structured-binding wrapper around the call.
    //
    //   2. **Reactive closure** when the body doesn't match (1)
    //      but contains `.get()` somewhere — the body is wrapped
    //      in a `move ||` closure so `IntoTextSource` routes it
    //      through an opaque `Derived<String>` and the framework
    //      sets up an Effect-driven update. Roku won't ship this
    //      shape to the device (no method name to dispatch).
    //
    //   3. **Static** for everything else — literal strings,
    //      build-time `format!()` calls, bare variables.
    //
    // Try (1) first; fall through to (2) / (3) on miss.
    if let Some(kids) = children {
        if kids.len() == 1 {
            if let UiNode::Expr(expr) = &kids[0] {
                if let Some(structured) = try_emit_derived_call::<String>(expr) {
                    return quote! {
                        ::runtime_core::text(
                            ::runtime_core::TextSource::Bound(#structured)
                        )
                    };
                }
                // Path (2): a CLOSURE child is reactive — the 0.1.0 type-driven
                // boundary. `text { move || … }` re-evaluates on each fire; the
                // walker installs an Effect via `IntoTextSource for Fn() -> String`.
                // We unwrap the author's closure body and re-wrap it with
                // `ToString::to_string` so the content may be any `Display` type
                // (`move || count.get()` where `count: Signal<i32>`), not just
                // `String`.
                if let Expr::Closure(closure) = expr {
                    let body = &closure.body;
                    return quote! {
                        ::runtime_core::text(move || ::std::string::ToString::to_string(&{ #body }))
                    };
                }
                // Path (2b): F-STRING — a literal child with `{name}`
                // placeholders interpolates, slots live-or-static by
                // TYPE. Placeholder-free literals fall through to the
                // static path untouched (see `parse_fstring`).
                if let Some(src) = try_emit_fstring(expr) {
                    return quote! { ::runtime_core::text(#src) };
                }
            }
        }
    }

    // Path (2) via the `content` prop: `text(content = move || …)`,
    // f-strings included.
    if children.is_none() {
        if let Some(p) = props.iter().find(|p| p.name == "content") {
            if let Expr::Closure(closure) = &p.value {
                let body = &closure.body;
                return quote! {
                    ::runtime_core::text(move || ::std::string::ToString::to_string(&{ #body }))
                };
            }
            if let Some(src) = try_emit_fstring(&p.value) {
                return quote! { ::runtime_core::text(#src) };
            }
        }
    }

    // Path (3) footgun guard (permanent): a bare, non-closure, non-macro text
    // content that reads a signal (`text { count.get() }`,
    // `text { format!("{}", x.get()) }`) was AUTO-WRAPPED reactive in 0.0.1 via a
    // `.get()` token scan. That heuristic is gone: reactivity is decided by TYPE
    // (a closure is live; a value is static). A bare `.get()` in text is now
    // *static* — which would be a **silent freeze** (renders once, never updates).
    // Rather than let that footgun exist, reject it LOUDLY and point at the
    // closure form. This guard reads no reactivity into anything — it turns one
    // specific silent mistake into a compile error; the reactive/static decision
    // is still the content's TYPE (closure vs value). Exempt: a closure child
    // (handled above; reactive) and a MACRO child/prop (`rx!(…)`)
    // whose VALUE type already carries reactivity (`Reactive<String>` /
    // `TextSource::JsBinding`). The rare false positive — a no-arg `.get()` that
    // is NOT a signal (`Cell`/`OnceCell::get()`) in bare text — is resolved by
    // binding to a `let` first: `let v = cell.get(); text { v }`.
    if text_content_reads_signal_bare(children, props) {
        return quote! {
            ::std::compile_error!(
                "0.1.0: reactive text must be a closure. Write \
                 `text { move || … }` (e.g. `text { move || format!(\"{}\", sig.get()) }`) \
                 instead of `text { …sig.get()… }` — a bare `.get()` in text is now static. \
                 `rx!(…)` values stay reactive by type."
            )
        };
    }

    let content: TokenStream2 = if let Some(kids) = children {
        match kids.len() {
            0 => quote! { "" },
            1 => emit_node(&kids[0], Ctx::Single),
            _ => {
                let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));
                quote! {
                    {
                        let mut __s = ::std::string::String::new();
                        #( __s.push_str(&::std::string::ToString::to_string(&#parts)); )*
                        __s
                    }
                }
            }
        }
    } else if let Some(p) = props.iter().find(|p| p.name == "content") {
        p.value.to_token_stream()
    } else {
        quote! { "" }
    };

    // Path (3): static — a literal, a build-time `format!()`, a bare value, or a
    // reactive-BY-TYPE value (`rx!`, a `Reactive<String>` /
    // `Signal<String>` handle) that `IntoTextSource` routes on its own.
    quote! { ::runtime_core::text(#content) }
}

/// Migration guard helper — true iff the text content is a **bare** expression
/// (not a closure, not a macro invocation) that reads a signal (`.get()`). Such
/// content was auto-wrapped reactive in 0.0.1 and is now silently static; the
/// caller rejects it with a `compile_error!` pointing at the closure form.
fn text_content_reads_signal_bare(children: Option<&[UiNode]>, props: &[Prop]) -> bool {
    fn expr_is_bare_signal_read(e: &Expr) -> bool {
        !matches!(e, Expr::Closure(_) | Expr::Macro(_))
            && expression_reads_signal(&e.to_token_stream())
    }
    if let Some(kids) = children {
        return kids.iter().any(|n| match n {
            UiNode::Expr(e) => expr_is_bare_signal_read(e),
            _ => false,
        });
    }
    if let Some(p) = props.iter().find(|p| p.name == "content") {
        return expr_is_bare_signal_read(&p.value);
    }
    false
}

/// Try to lower a call expression like `method(sig_a, sig_b)` into
/// a fully-populated `Derived<T>` constructor. Returns `Some(tokens)`
/// on match, `None` if the expression isn't a structured call.
///
/// Match criteria:
/// - Top-level expression is `syn::Expr::Call`.
/// - Function position is a single-segment path (`my_method`, not
///   `module::my_method` or `Foo::method`).
/// - At least one arg, and *every* arg is itself a single-segment
///   path expression (bare identifier — the signal). Mixing literal
///   args or method calls falls through, leaving the author to
///   explicitly wrap.
///
/// `T` is the value type the emitted `Derived` produces. The
/// expression's `compute` body re-evaluates the call against
/// `.get()` on each signal arg; for `T = String` we wrap the
/// return in `format!("{}", _)` so any `Display` return type
/// fits. Other `T`s pass through as-is.
fn try_emit_derived_call<T>(expr: &Expr) -> Option<TokenStream2>
where
    T: DerivedKind,
{
    let call = match expr {
        Expr::Call(c) => c,
        _ => return None,
    };
    // Function position must be a single-segment path.
    let func_ident = match &*call.func {
        Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
            if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                return None;
            }
            path.segments[0].ident.clone()
        }
        _ => return None,
    };
    // Each arg must be a bare path (signal reference).
    let args: Vec<&Expr> = call.args.iter().collect();
    if args.is_empty() {
        return None;
    }
    for a in &args {
        match a {
            Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
                if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                    return None;
                }
            }
            _ => return None,
        }
    }

    let method_lit = syn::LitStr::new(&func_ident.to_string(), func_ident.span());
    let get_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).get() }).collect();
    let id_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).id() }).collect();
    let initial_calls: Vec<TokenStream2> = args
        .iter()
        .map(|a| {
            quote! {
                ::runtime_core::__serde_json::to_value(&(#a).get())
                    .unwrap_or(::runtime_core::__serde_json::Value::Null)
            }
        })
        .collect();

    let compute_body = T::compute_body(&func_ident, &get_calls);
    let ty_tokens = T::type_tokens();

    Some(quote! {
        ::runtime_core::Derived::<#ty_tokens> {
            method:  #method_lit,
            inputs:  ::std::vec![ #(#id_calls),* ],
            initial: ::std::vec![ #(#initial_calls),* ],
            compute: ::std::rc::Rc::new(move || { #compute_body }),
        }
    })
}

/// Does `expr` have the "reactive call" shape — a single-segment
/// function call whose every argument is a bare single-segment path
/// (a signal reference)? This is the SAME syntactic shape
/// `try_emit_derived_call` / `try_emit_structured_match` accept, and
/// it is the shape that, by contract, reads signals: the structured
/// lowering calls `(arg).get()` on each argument.
///
/// `condition_is_reactive` only fires on a literal `.get()` substring,
/// so a scrutinee like `key(state)` (the signal read is hidden inside
/// `key`, the args are bare `Signal`s) is NOT caught by it. For `if`
/// this didn't matter — `if key(state) { … }` is always claimed by the
/// structured `try_emit_derived_call::<bool>` path, which makes it
/// reactive. But `match`'s structured path requires *literal* arm
/// keys, so `match key(state) { Enum::A => …, _ => … }` (enum/non-literal
/// arms) fell through `try_emit_structured_match` AND past
/// `condition_is_reactive`, landing on the static plain-`match` arm —
/// it built once and never re-ran when `state` changed. This predicate
/// closes that gap so the closure-`switch` reactive path also claims
/// the bare-call shape (matching `if`'s behavior).
fn is_reactive_call_shape(expr: &Expr) -> bool {
    let call = match expr {
        Expr::Call(c) => c,
        _ => return false,
    };
    // Function position must be a single-segment path with no generic args.
    match &*call.func {
        Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
            if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                return false;
            }
        }
        _ => return false,
    }
    if call.args.is_empty() {
        return false;
    }
    // Every arg must be a bare single-segment path (a signal reference).
    call.args.iter().all(|a| {
        matches!(
            a,
            Expr::Path(syn::ExprPath { qself: None, path, .. })
                if path.segments.len() == 1 && path.segments[0].arguments.is_empty()
        )
    })
}

/// Per-type hooks for `try_emit_derived_call`. `String` wraps the
/// call in `format!("{}", _)` so authors don't have to make their
/// `#[method]` return `String` directly; `bool` passes through
/// (the method must return a bool); etc.
trait DerivedKind {
    fn type_tokens() -> TokenStream2;
    fn compute_body(func: &Ident, get_calls: &[TokenStream2]) -> TokenStream2;
}

impl DerivedKind for String {
    fn type_tokens() -> TokenStream2 {
        quote! { ::std::string::String }
    }
    fn compute_body(func: &Ident, get_calls: &[TokenStream2]) -> TokenStream2 {
        quote! {
            ::std::format!("{}", #func( #(#get_calls),* ))
        }
    }
}

impl DerivedKind for bool {
    fn type_tokens() -> TokenStream2 {
        quote! { bool }
    }
    fn compute_body(func: &Ident, get_calls: &[TokenStream2]) -> TokenStream2 {
        quote! { #func( #(#get_calls),* ) }
    }
}

/// Heuristic: does the token stream contain `.get()`? Used to decide
/// whether `text { ... }` bodies should be wrapped in a reactive
/// closure. Matches the same heuristic `condition_is_reactive` uses
/// for `if` conditions, so authors who reach for `.get()` in their
/// content get the reactive behavior they expect.
fn expression_reads_signal(tokens: &TokenStream2) -> bool {
    let s = tokens.to_string();
    s.contains(".get()") || s.contains(". get ()")
}

/// Lower `on_click = method(sig)` or `on_click = method(sig) =>
/// out_signal` into a fully-populated `Action` constructor.
/// Returns `None` when `value` isn't a structured-call shape (the
/// caller falls back to the existing closure / IntoAction path).
///
/// Same shape match as `try_emit_derived_call`: function-position
/// single ident, every arg a bare path (signal reference). The
/// optional `arrow_target` (parsed by `Prop::parse` when the
/// author writes `=> out_signal`) becomes the Action's `output`
/// field; the closure inside `fire` writes the method's return
/// value to `out_signal` after invocation.
fn try_emit_structured_action(value: &Expr, arrow_target: Option<&Expr>) -> Option<TokenStream2> {
    let call = match value {
        Expr::Call(c) => c,
        _ => return None,
    };
    let func_ident = match &*call.func {
        Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
            if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                return None;
            }
            path.segments[0].ident.clone()
        }
        _ => return None,
    };
    let args: Vec<&Expr> = call.args.iter().collect();
    for a in &args {
        match a {
            Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
                if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                    return None;
                }
            }
            _ => return None,
        }
    }

    let method_lit = syn::LitStr::new(&func_ident.to_string(), func_ident.span());
    let get_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).get() }).collect();
    let id_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).id() }).collect();
    let initial_calls: Vec<TokenStream2> = args
        .iter()
        .map(|a| {
            quote! {
                ::runtime_core::__serde_json::to_value(&(#a).get())
                    .unwrap_or(::runtime_core::__serde_json::Value::Null)
            }
        })
        .collect();

    let (fire_body, output_tokens) = match arrow_target {
        Some(out) => (
            quote! { (#out).set(#func_ident( #(#get_calls),* )); },
            quote! { ::std::option::Option::Some((#out).id()) },
        ),
        None => (
            quote! { #func_ident( #(#get_calls),* ); },
            quote! { ::std::option::Option::None },
        ),
    };

    Some(quote! {
        ::runtime_core::Action {
            method:  #method_lit,
            inputs:  ::std::vec![ #(#id_calls),* ],
            initial: ::std::vec![ #(#initial_calls),* ],
            output:  #output_tokens,
            fire:    ::std::rc::Rc::new(move || { #fire_body }),
        }
    })
}

fn emit_button(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let label = props
        .iter()
        .find(|p| p.name == "label")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { "" });
    // on_click: three shapes, in priority order:
    //   1. `on_click = method(sig) => out_signal` — structured Action
    //   2. `on_click = method(sig)` — structured Action, fire-and-forget
    //   3. `on_click = closure_expression` — opaque coercion via IntoAction
    let on_click = match props.iter().find(|p| p.name == "on_click") {
        Some(p) => {
            if let Some(action) = try_emit_structured_action(&p.value, p.arrow_target.as_ref()) {
                action
            } else {
                p.value.to_token_stream()
            }
        }
        None => quote! { || {} },
    };
    let leading = if let Some(p) = props.iter().find(|p| p.name == "leading_icon") {
        let v = &p.value;
        quote! { .leading_icon(#v) }
    } else {
        quote! {}
    };
    let trailing = if let Some(p) = props.iter().find(|p| p.name == "trailing_icon") {
        let v = &p.value;
        quote! { .trailing_icon(#v) }
    } else {
        quote! {}
    };
    quote! { ::runtime_core::button(#label, #on_click) #leading #trailing }
}

fn emit_view(_props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    let kids = children.unwrap_or(&[]);
    let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));
    quote! {
        ::runtime_core::view({
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        })
    }
}

fn emit_when(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let cond = props.iter().find(|p| p.name == "cond").map(|p| p.value.to_token_stream()).unwrap_or_else(|| quote! { || false });
    let then_e = props.iter().find(|p| p.name == "then").map(|p| p.value.to_token_stream()).unwrap_or_else(|| quote! { || ::runtime_core::view(::std::vec::Vec::new()) });
    let other = props.iter().find(|p| p.name == "otherwise").map(|p| p.value.to_token_stream()).unwrap_or_else(|| quote! { || ::runtime_core::view(::std::vec::Vec::new()) });
    quote! { ::runtime_core::when(#cond, #then_e, #other) }
}

/// `Icon(data = ..., color = ..., stroke = ..., draw_in = ...)`.
/// `data` is required (an `IconData` value). Optional props:
/// - `color`: reactive closure returning a `Color`
/// - `stroke`: reactive closure returning f32 (0.0–1.0)
/// - `draw_in`: tuple `(duration_ms, easing)` for mount animation
fn emit_icon(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let data = props
        .iter()
        .find(|p| p.name == "data")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { compile_error!("Icon requires a `data` prop") });
    let color_call = if let Some(p) = props.iter().find(|p| p.name == "color") {
        let v = &p.value;
        quote! { .color(#v) }
    } else {
        quote! {}
    };
    let stroke_call = if let Some(p) = props.iter().find(|p| p.name == "stroke") {
        let v = &p.value;
        quote! { .stroke(#v) }
    } else {
        quote! {}
    };
    // `animate` takes a StrokeAnimation struct directly.
    // `draw_in` is shorthand for (duration, easing) tuple.
    let anim_call = if let Some(p) = props.iter().find(|p| p.name == "animate") {
        let v = &p.value;
        quote! { .animate(#v) }
    } else if let Some(p) = props.iter().find(|p| p.name == "draw_in") {
        let v = &p.value;
        quote! { .draw_in((#v).0, (#v).1) }
    } else {
        quote! {}
    };
    quote! { ::runtime_core::icon(#data) #color_call #stroke_call #anim_call }
}

/// `Image(src = ..., alt = ...)` or `Image(asset = &LOGO, alt = ...)`.
///
/// Exactly one source prop should be set:
/// - `src = ...` routes through [`image`](runtime_core::primitives::image::image)
///   for free-form URLs / closures.
/// - `asset = ...` routes through
///   [`image_asset`](runtime_core::primitives::image::image_asset)
///   for declarative `Asset<kinds::Image>` references. The expression
///   should evaluate to a `Copy` `Asset<kinds::Image>` (typically by
///   dereferencing a `static`: `asset = *LOGO`, or shorthand
///   `asset = &LOGO` which the macro auto-derefs).
fn emit_image(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let alt_call = if let Some(a) = props.iter().find(|p| p.name == "alt") {
        let v = emit_attr_value(&a.value);
        quote! { .alt(#v) }
    } else {
        quote! {}
    };
    if let Some(a) = props.iter().find(|p| p.name == "asset") {
        let v = a.value.to_token_stream();
        // Idiomatic call site is `asset = &LOGO` (a borrow of a
        // `static`), so we emit one `*` to read the `Copy` value out.
        // For direct expressions that already produce an
        // `Asset<kinds::Image>` by value, write `asset = &owned`.
        return quote! {
            ::runtime_core::primitives::image::image_asset(*#v) #alt_call
        };
    }
    let src = props
        .iter()
        .find(|p| p.name == "src")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { "" });
    quote! { ::runtime_core::primitives::image::image(#src) #alt_call }
}

/// `TextInput(value = signal, on_change = closure, placeholder = ...)`.
fn emit_text_input(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let value = props
        .iter()
        .find(|p| p.name == "value")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { ::runtime_core::Signal::new(::std::string::String::new()) });
    let on_change = props
        .iter()
        .find(|p| p.name == "on_change")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { |_| {} });
    let placeholder_call = if let Some(p) = props.iter().find(|p| p.name == "placeholder") {
        let v = emit_attr_value(&p.value);
        quote! { .placeholder(#v) }
    } else {
        quote! {}
    };
    let secure_call = if let Some(p) = props.iter().find(|p| p.name == "secure") {
        let v = p.value.to_token_stream();
        quote! { .secure(#v) }
    } else {
        quote! {}
    };
    quote! {
        ::runtime_core::primitives::text_input::text_input(#value, #on_change) #placeholder_call #secure_call
    }
}

/// `Toggle(value = signal, on_change = closure)`.
fn emit_toggle(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let value = props
        .iter()
        .find(|p| p.name == "value")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { ::runtime_core::Signal::new(false) });
    let on_change = props
        .iter()
        .find(|p| p.name == "on_change")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { |_| {} });
    quote! { ::runtime_core::primitives::toggle::toggle(#value, #on_change) }
}

/// `ScrollView(horizontal = bool) { children }`. Children list works
/// just like `View`.
fn emit_scroll_view(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    let kids = children.unwrap_or(&[]);
    let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));
    let horizontal_call = if let Some(p) = props.iter().find(|p| p.name == "horizontal") {
        let v = &p.value;
        quote! { .horizontal(#v) }
    } else {
        quote! {}
    };
    quote! {
        ::runtime_core::primitives::scroll_view::scroll_view({
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        }) #horizontal_call
    }
}

/// `Slider(value = signal, on_change = closure, min = f32, max = f32, step = f32)`.
fn emit_slider(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let value = props
        .iter()
        .find(|p| p.name == "value")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { ::runtime_core::Signal::new(0.0f32) });
    let on_change = props
        .iter()
        .find(|p| p.name == "on_change")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { |_| {} });
    let range_call = match (
        props.iter().find(|p| p.name == "min"),
        props.iter().find(|p| p.name == "max"),
    ) {
        (Some(mn), Some(mx)) => {
            let a = &mn.value;
            let b = &mx.value;
            quote! { .range(#a, #b) }
        }
        _ => quote! {},
    };
    let step_call = if let Some(p) = props.iter().find(|p| p.name == "step") {
        let v = &p.value;
        quote! { .step(#v) }
    } else {
        quote! {}
    };
    quote! {
        ::runtime_core::primitives::slider::slider(#value, #on_change)
            #range_call
            #step_call
    }
}


/// `Graphics(on_ready = ..., on_resize = ..., on_lost = ...)`.
/// `on_ready` is required; the others default to no-ops.
///
/// The framework provides a platform-native drawable surface via
/// `OnReadyEvent.surface`, which implements `raw_window_handle`'s
/// `HasWindowHandle + HasDisplayHandle`. The author plugs in their
/// GPU library of choice (`wgpu::Instance::create_surface(&surface)`,
/// or any other lib that takes those traits).
fn emit_graphics(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let on_ready = props
        .iter()
        .find(|p| p.name == "on_ready")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { |_event| {} });
    let on_resize_call = if let Some(p) = props.iter().find(|p| p.name == "on_resize") {
        let v = &p.value;
        quote! { .on_resize(#v) }
    } else {
        quote! {}
    };
    let on_lost_call = if let Some(p) = props.iter().find(|p| p.name == "on_lost") {
        let v = &p.value;
        quote! { .on_lost(#v) }
    } else {
        quote! {}
    };
    quote! {
        ::runtime_core::primitives::graphics::graphics(#on_ready)
            #on_resize_call
            #on_lost_call
    }
}

/// `ActivityIndicator(size = ..., color = ...)`.
fn emit_activity_indicator(
    props: &[Prop],
    _children: Option<&[UiNode]>,
) -> TokenStream2 {
    let size_call = if let Some(p) = props.iter().find(|p| p.name == "size") {
        let v = &p.value;
        quote! { .size(#v) }
    } else {
        quote! {}
    };
    let color_call = if let Some(p) = props.iter().find(|p| p.name == "color") {
        let v = &p.value;
        quote! { .color(#v) }
    } else {
        quote! {}
    };
    quote! {
        ::runtime_core::primitives::activity_indicator::activity_indicator()
            #size_call
            #color_call
    }
}

/// `FlatList(data = signal, key = |idx, item| ..., size = FlatListItemSize<T>, render = |idx, item| ...)`.
///
/// `size` accepts a `FlatListItemSize<T>` value (Known/Measured). Use
/// `runtime_core::primitives::flat_list::fixed_size(48.0)` for the
/// fixed-height common case.
fn emit_link(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    // Two shapes:
    //   - `Link(external = "https://…") { children }` — an off-app
    //     link. Lowers to `external_link(url, children)`: on web a real
    //     `<a target="_blank">`, on native a platform `open_url`. No
    //     `route` / `params`.
    //   - `Link(route = ..., params = ...) { children }` — in-app
    //     navigation. Mirrors the `link<P>(route, params, children)`
    //     constructor's three positional args. `route` is required at
    //     the type level; `params` defaults to `()`.
    let kids = children.unwrap_or(&[]);
    let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));
    let children_vec = quote! {
        {
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        }
    };

    if let Some(external) = props.iter().find(|p| p.name == "external") {
        let url = external.value.to_token_stream();
        return quote! {
            ::runtime_core::primitives::link::external_link(#url, #children_vec)
        };
    }

    let route = props
        .iter()
        .find(|p| p.name == "route")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { compile_error!("Link: missing required `route` prop (or use `external = \"https://…\"` for an off-app link)") });
    let params = props
        .iter()
        .find(|p| p.name == "params")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { () });

    quote! {
        ::runtime_core::primitives::link::link(#route, #params, #children_vec)
    }
}

/// `Overlay(placement = ..., backdrop = ..., backdrop_style = ...,
///          on_dismiss = ..., trap_focus = ...) { children }`.
/// Lowers to `overlay(children).placement(...).backdrop(...)…` chain.
/// Viewport-anchored only; for element-anchored cases use
/// `AnchoredOverlay` (handled by `emit_anchored_overlay`).
fn emit_overlay(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    let kids = children.unwrap_or(&[]);
    let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));

    let placement_call = props
        .iter()
        .find(|p| p.name == "placement")
        .map(|p| {
            let v = &p.value;
            quote! { .placement(#v) }
        })
        .unwrap_or_default();
    let backdrop_call = props
        .iter()
        .find(|p| p.name == "backdrop")
        .map(|p| {
            let v = &p.value;
            quote! { .backdrop(#v) }
        })
        .unwrap_or_default();
    let backdrop_style_call = props
        .iter()
        .find(|p| p.name == "backdrop_style")
        .map(|p| {
            let v = &p.value;
            quote! { .backdrop_style(#v) }
        })
        .unwrap_or_default();
    let on_dismiss_call = props
        .iter()
        .find(|p| p.name == "on_dismiss")
        .map(|p| {
            let v = &p.value;
            quote! { .on_dismiss(#v) }
        })
        .unwrap_or_default();
    let trap_focus_call = props
        .iter()
        .find(|p| p.name == "trap_focus")
        .map(|p| {
            let v = &p.value;
            quote! { .trap_focus(#v) }
        })
        .unwrap_or_default();

    quote! {
        ::runtime_core::primitives::overlay::overlay({
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        })
        #placement_call
        #backdrop_call
        #backdrop_style_call
        #on_dismiss_call
        #trap_focus_call
    }
}

/// `AnchoredOverlay(target = ..., side = ..., align = ..., offset = ...,
///                  backdrop = ..., backdrop_style = ...,
///                  on_dismiss = ..., trap_focus = ...) { children }`.
/// Lowers to
/// `anchored_overlay(target, children).side(...).align(...)…` chain.
/// Element-anchored only; for viewport-anchored cases use `Overlay`.
fn emit_anchored_overlay(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    let kids = children.unwrap_or(&[]);
    let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));

    // `target` is required to build the primitive — pass it
    // positionally rather than as a `.target(...)` chain call so the
    // type system enforces it.
    let target_value = props
        .iter()
        .find(|p| p.name == "target")
        .map(|p| {
            let v = &p.value;
            quote! { #v }
        })
        .unwrap_or_else(|| {
            quote! {
                compile_error!("AnchoredOverlay requires a `target = ...` prop")
            }
        });

    let side_call = props
        .iter()
        .find(|p| p.name == "side")
        .map(|p| {
            let v = &p.value;
            quote! { .side(#v) }
        })
        .unwrap_or_default();
    let align_call = props
        .iter()
        .find(|p| p.name == "align")
        .map(|p| {
            let v = &p.value;
            quote! { .align(#v) }
        })
        .unwrap_or_default();
    let offset_call = props
        .iter()
        .find(|p| p.name == "offset")
        .map(|p| {
            let v = &p.value;
            quote! { .offset(#v) }
        })
        .unwrap_or_default();
    let backdrop_call = props
        .iter()
        .find(|p| p.name == "backdrop")
        .map(|p| {
            let v = &p.value;
            quote! { .backdrop(#v) }
        })
        .unwrap_or_default();
    let backdrop_style_call = props
        .iter()
        .find(|p| p.name == "backdrop_style")
        .map(|p| {
            let v = &p.value;
            quote! { .backdrop_style(#v) }
        })
        .unwrap_or_default();
    let on_dismiss_call = props
        .iter()
        .find(|p| p.name == "on_dismiss")
        .map(|p| {
            let v = &p.value;
            quote! { .on_dismiss(#v) }
        })
        .unwrap_or_default();
    let trap_focus_call = props
        .iter()
        .find(|p| p.name == "trap_focus")
        .map(|p| {
            let v = &p.value;
            quote! { .trap_focus(#v) }
        })
        .unwrap_or_default();

    quote! {
        ::runtime_core::primitives::overlay::anchored_overlay(
            #target_value,
            {
                let mut __c: ::std::vec::Vec<::runtime_core::Element>
                    = ::std::vec::Vec::new();
                #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
                __c
            },
        )
        #side_call
        #align_call
        #offset_call
        #backdrop_call
        #backdrop_style_call
        #on_dismiss_call
        #trap_focus_call
    }
}

/// `Presence(present = ..., enter = ..., exit = ...) { child }`.
/// Lowers to a `presence(move || <child primitive>)` call, chained
/// with the optional `.present(...)`, `.enter(...)`, `.exit(...)`
/// builder methods. The children block builds the child primitive —
/// if it has multiple nodes they wrap in a View, exactly like
/// `emit_block_as_primitive` does for `when` / `switch` branches.
///
/// The child expression is captured by-move into the closure so its
/// reactive scope subscribes correctly on each (re)mount.
fn emit_presence(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    let child_expr = emit_block_as_primitive(children.unwrap_or(&[]));

    let present_call = props
        .iter()
        .find(|p| p.name == "present")
        .map(|p| {
            let v = &p.value;
            quote! { .present(#v) }
        })
        .unwrap_or_default();
    let enter_call = props
        .iter()
        .find(|p| p.name == "enter")
        .map(|p| {
            let v = &p.value;
            quote! { .enter(#v) }
        })
        .unwrap_or_default();
    let exit_call = props
        .iter()
        .find(|p| p.name == "exit")
        .map(|p| {
            let v = &p.value;
            quote! { .exit(#v) }
        })
        .unwrap_or_default();

    quote! {
        ::runtime_core::primitives::presence::presence(move || #child_expr)
            #present_call
            #enter_call
            #exit_call
    }
}

fn emit_flat_list(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let data = props
        .iter()
        .find(|p| p.name == "data")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { ::runtime_core::Signal::new(::std::vec::Vec::new()) });
    let key = props
        .iter()
        .find(|p| p.name == "key")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { |idx, _item| idx as u64 });
    let size = props
        .iter()
        .find(|p| p.name == "size")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| {
            quote! { ::runtime_core::primitives::flat_list::fixed_size(48.0) }
        });
    let render = props
        .iter()
        .find(|p| p.name == "render")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { |_idx, _item| ::runtime_core::view(::std::vec::Vec::new()).into() });

    let overscan_call = if let Some(p) = props.iter().find(|p| p.name == "overscan") {
        let v = &p.value;
        quote! { .overscan(#v) }
    } else {
        quote! {}
    };
    let axis_call = if let Some(p) = props.iter().find(|p| p.name == "axis") {
        let v = &p.value;
        quote! { .axis(#v) }
    } else {
        quote! {}
    };
    let lanes_call = if let Some(p) = props.iter().find(|p| p.name == "lanes") {
        let v = &p.value;
        quote! { .lanes(#v) }
    } else {
        quote! {}
    };
    let spacing_call = match (
        props.iter().find(|p| p.name == "gap"),
        props.iter().find(|p| p.name == "main_spacing"),
        props.iter().find(|p| p.name == "cross_spacing"),
    ) {
        (Some(p), _, _) => {
            let v = &p.value;
            quote! { .gap(#v) }
        }
        (None, main, cross) => {
            let m = main
                .map(|p| p.value.to_token_stream())
                .unwrap_or_else(|| quote! { 0.0 });
            let c = cross
                .map(|p| p.value.to_token_stream())
                .unwrap_or_else(|| quote! { 0.0 });
            if main.is_some() || cross.is_some() {
                quote! { .spacing(#m, #c) }
            } else {
                quote! {}
            }
        }
    };

    // The third generic on flat_list is unused — fall through.
    quote! {
        ::runtime_core::primitives::flat_list::flat_list::<_, _, (), _>(#data, #key, #size, #render)
            #overscan_call
            #axis_call
            #lanes_call
            #spacing_call
    }
}

/// Emit a user-defined component invocation as a `BuildElement` struct
/// literal (see the function body for the full rationale). A children
/// block (if present) becomes the `children` field, a `Vec<Element>`.
fn emit_user(name: &Ident, props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    // A tag `Foo` dispatches through the `BuildElement` trait: a plain
    // struct literal plus a UFCS `build` call — NO per-component
    // `macro_rules!`. This resolves across crate boundaries by ordinary
    // path rules (no `#[macro_export]` / `#[macro_use]`), and because the
    // call site is a real struct literal rust-analyzer gives field-name
    // completion, hover, and go-to-def on every prop. `build` hides the
    // `&Props` vs `Props` signature split; `defaults()` supplies the
    // struct-update base (the type's `Default`, or the values declared via
    // `#[component(default(...))]`).
    //
    // The tag is used *as the type name* (not `FooProps`): `#[component]`
    // and idea-ui emit a `pub type Foo = FooProps` alias, so existing
    // `use …::Foo` imports keep working unchanged (they resolve to the
    // alias instead of the old macro). The tag carries its own span, so
    // go-to-def on `Foo` lands on that alias.
    let props_ty = name;

    // Each provided prop becomes a struct field, coerced via `.into()`.
    // The field's declared type pins the `.into()` target, so this is
    // the same uniform coercion the old invocation macros performed —
    // `"x"` → `String`/`Reactive<String>`, identity for matching types.
    let field_assignments = props.iter().map(|p| {
        let n = &p.name;
        let v = &p.value;
        quote! { #n: (#v).into(), }
    });

    // Children (from a `{ … }` block) flow into a `children` field as a
    // `Vec<Element>`. A component whose `children` field isn't that type
    // gets a type error at the call site — intentional.
    let children_field = children.map(|kids| {
        let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));
        quote! {
            children: {
                let mut __c: ::std::vec::Vec<::runtime_core::Element>
                    = ::std::vec::Vec::new();
                #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
                __c
            },
        }
    });

    quote! {
        ::runtime_core::BuildElement::build(
            #props_ty {
                #(#field_assignments)*
                #children_field
                ..<#props_ty as ::runtime_core::BuildElement>::defaults()
            }
        )
    }
}

/// An empty, **layout-neutral** `View`, coerced to `Element` — used as
/// the `else` branch of a single-slot `if`/`when` that has no author
/// `else`, so both arms have the same `Element` type.
///
/// The placeholder is `position: absolute` so it occupies no flex slot.
/// A plain in-flow empty `view` is still a flex item, so a parent's
/// `gap` (or centering) spaces it and the siblings *shift* when the
/// condition toggles — e.g. `if open { Popover/Tooltip/Modal }` made the
/// trigger jump as the overlay mounted/unmounted, even though the overlay
/// itself portals out of flow. An absolutely-positioned empty view is a
/// real (When-swappable) node that contributes nothing to layout when the
/// branch is absent, so a false `if` is truly weightless.
fn empty_view_primitive() -> TokenStream2 {
    quote! {
        ::runtime_core::IntoElement::into_element(
            ::runtime_core::view(::std::vec::Vec::new()).with_style(
                ::std::rc::Rc::new(::runtime_core::StyleSheet::r#static(
                    ::runtime_core::StyleRules {
                        position: ::core::option::Option::Some(::runtime_core::Position::Absolute),
                        ..::core::default::Default::default()
                    }
                ))
            )
        )
    }
}

fn emit_if(
    cond: &Expr,
    then_body: &[UiNode],
    else_body: Option<&[UiNode]>,
    ctx: Ctx,
) -> TokenStream2 {
    // Reactive lowerings return ONE `when(...)` Element — a reactive
    // branch is a single subtree, so multi-node branches wrap by
    // necessity (that's correct; the reactive anchor needs one root per
    // branch). `then`/`else` are single Primitives via
    // `emit_block_as_primitive`.
    //
    // 1. Structured call shape (`if is_even(count) { … }`): a
    //    fully-populated `Derived<bool>` so generator backends (Roku)
    //    can ship the binding declaratively.
    // 2. Closure-reactive (`.get()` in the condition): `when()` with the
    //    closure form; the framework's Effect path rebuilds on change.
    //
    // NON-`Copy` CAPTURE IN A NESTED `else if` ARM (field report "B5"):
    // a chained `if a {…} else if b {…} else { foo(app) }` lowers each
    // `else if` to a `when` nested INSIDE the parent's `otherwise`
    // closure. Every `when` branch closure is `Fn` (re-invoked whenever
    // its branch activates), and the parent's `otherwise` MUST rebuild
    // the inner `when` Element on each call — which MOVES any non-`Copy`
    // value the deeper arm uses into the inner closure. An `Fn` closure
    // can't move out a captured non-`Copy` value on repeat calls, so the
    // chain fails to compile with "cannot move out of value, a captured
    // variable in an `Fn` closure" (the error points at the deep arm).
    //
    // Note this is the SAME `Fn`-branch constraint a flat reactive
    // `switch`/`when` has — but there a single dispatcher owns the value,
    // so an in-arm `app.clone()` reads it by ref and compiles. In the
    // nested-`else if` form the move happens at the *construction* of the
    // inner `when` (one level up from the arm), so an in-arm `.clone()`
    // is too late to help.
    //
    // Author workarounds (no macro change avoids it without rewriting the
    // whole if-chain to a single flat `switch`, which would regress the
    // anchorless-`when` lowering many tests + the Android device fix rely
    // on — out of proportion for this ergonomic edge):
    //   - Move the non-`Copy` use into a flat `match` over a discriminant
    //     (`match pick(a, b) { 0 => …, 1 => …, _ => foo(app.clone()) }`).
    //     A flat `match` is a single `switch` dispatcher, so an in-arm
    //     `.clone()` works (verified in
    //     `tests/match_reactive_call_regression.rs::b5_*`).
    //   - Wrap the value in `Rc`/`Arc` and clone a handle per arm, or hoist
    //     it into an owner the arm borrows rather than moves.
    // 0. `if let PAT = EXPR { … }` — the condition is a refutable `let`, not a
    //    value. It can't be a `bool`/`Signal<bool>`, so neither the reactive
    //    paths nor the type-driven `__idealyst_if` dispatch apply (you can't
    //    call a method on a `let` expression). Emit a plain Rust `if let`,
    //    splicing the binding so the then-branch sees it. Always static — a
    //    reactive `if let` (re-binding on signal change) is not a supported
    //    construct; author it as `match sig.get() { … }` instead.
    if matches!(cond, Expr::Let(_)) {
        return emit_plain_if(cond, then_body, else_body, ctx);
    }
    if let Some(structured_cond) = try_emit_derived_call::<bool>(cond) {
        let then_expr = emit_block_as_primitive(then_body);
        let else_expr = else_body.map(emit_block_as_primitive).unwrap_or_else(empty_view_primitive);
        return quote! {
            ::runtime_core::when(#structured_cond, move || #then_expr, move || #else_expr)
        };
    }

    // 3. Reactive condition → `when(move || cond, …)`. Two shapes qualify, both
    //    lowered identically:
    //      (a) a `.get()` read anywhere in the condition (`if sig.get() > 0`,
    //          `if !flag.get()`). A `.get()` buried in a comparison/negation has
    //          a plain `bool` *type*, so the type-driven dispatch (path 4) would
    //          miss it — this keeps it live, exactly as 0.0.1 did.
    //      (b) a top-level CALL condition — `if use_focus()()`,
    //          `if state.is_active()`, `if is_active(state)`. A call may read a
    //          signal that isn't spelled `.get()` (a reactive hook returning
    //          `impl Fn() -> bool`, a predicate that reads a signal internally).
    //          In 0.0.1 those were treated as static and **silently frozen** —
    //          the documented `use_focus()` footgun. 0.1.0 makes any top-level
    //          `Call`/`MethodCall` reactive: the framework's Effect tracks
    //          whatever signals the call reads while evaluating, so it's live. A
    //          static call (reads no signal) yields an inert effect that runs
    //          once and never re-fires — correct and ~free.
    //
    //    We do NOT force EVERY condition reactive. A pure structural comparison
    //    (`if kind == Kind::Scope`, `if !name.is_empty()`, `if a && b`) reads no
    //    signal by construction, so wrapping it in a `move ||` closure would only
    //    impose `'static`/`Clone` capture ceremony (cloning an `Rc` per branch,
    //    threading owned strings, …) for zero reactive benefit. Those fall
    //    through to the plain static `if` (path 5, borrowed captures, flat-splat).
    //    The one residual gap — a non-`.get()` reactive read hidden inside such a
    //    comparison — is authored with the visible closure boundary
    //    `{ move || if cond { … } else { … } }` (an `Element::Dynamic`), the same
    //    escape hatch every reactive child uses. The trade-off of (a)/(b) is the
    //    `'static` move-capture the closure imposes: a branch that renders a
    //    borrowed/non-`Clone` value now needs a `.clone()` or `'static` capture —
    //    a loud, diagnosable compile error, never a silent freeze.
    if condition_is_reactive(cond) || matches!(cond, Expr::Call(_) | Expr::MethodCall(_)) {
        let then_expr = emit_block_as_primitive(then_body);
        let else_expr =
            else_body.map(emit_block_as_primitive).unwrap_or_else(empty_view_primitive);
        return quote! {
            ::runtime_core::when(move || #cond, move || #then_expr, move || #else_expr)
        };
    }

    // 4. Type-driven dispatch — ONLY for a condition whose TYPE could be a
    //    reactive bool: a bare path (`if del_visible`) or field access
    //    (`if state.open`). Those are the only shapes that might be a
    //    `Signal<bool>`/`Derived<bool>` rather than a plain `bool`. We emit
    //    `(COND).__idealyst_if(then, els)` with BOTH `StaticCond` and
    //    `ReactiveCond` in scope; Rust method resolution picks the impl from
    //    COND's type:
    //      - `bool` → `StaticCond` → the taken branch's flat node list;
    //      - `Signal<bool>` (e.g. `memo(...)`) / `Derived<bool>` →
    //        `ReactiveCond` → one reactive `when`.
    //    Both return `Vec<Element>`, so the branch thunks emit flat node lists
    //    (preserving no-wrapper flat-splat) and `ChildList`/`one_or_view`
    //    normalize per ctx.
    //
    // 5. EVERY OTHER condition shape — a literal, `&&`/`||`/`!`, a comparison —
    //    is a pure static `bool` (path 3 already claimed the reactive shapes),
    //    so it falls through to `emit_plain_if` (a plain Rust `if`, captures
    //    BORROWED, flat-splat in child slot).
    if !matches!(cond, Expr::Path(_) | Expr::Field(_)) {
        return emit_plain_if(cond, then_body, else_body, ctx);
    }
    let then_thunk = {
        let parts = then_body.iter().map(|n| emit_node(n, Ctx::Child));
        quote! {
            move || {
                let mut __c: ::std::vec::Vec<::runtime_core::Element>
                    = ::std::vec::Vec::new();
                #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
                __c
            }
        }
    };
    let else_thunk = match else_body {
        Some(eb) => {
            let parts = eb.iter().map(|n| emit_node(n, Ctx::Child));
            quote! {
                move || {
                    let mut __c: ::std::vec::Vec<::runtime_core::Element>
                        = ::std::vec::Vec::new();
                    #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
                    __c
                }
            }
        }
        None => quote! { move || ::std::vec::Vec::<::runtime_core::Element>::new() },
    };
    let dispatch = quote! {
        {
            #[allow(unused_imports)]
            use ::runtime_core::{StaticCond as _, ReactiveCond as _};
            (#cond).__idealyst_if(#then_thunk, #else_thunk)
        }
    };
    match ctx {
        // Children-slot: the dispatch IS a `Vec<Element>` — the surrounding
        // `ChildList::append_to` flattens it (static branch → flat siblings,
        // reactive branch → the single `when`).
        Ctx::Child => dispatch,
        // Single-slot: must be one Element. `one_or_view` returns the sole
        // element verbatim (a reactive `when`, or a single static node — NO
        // wrapper) and only wraps a genuinely multi-node static branch.
        Ctx::Single => quote! { ::runtime_core::one_or_view(#dispatch) },
    }
}

/// Emit a STATIC `if` as a plain Rust `if` — the lowering for a condition that
/// is *provably* `bool` (a literal, `&&`/`||`/`!`, a method/function call, …)
/// or an `if let` binding. Captures are borrowed (no `move`), so a branch may
/// freely reference a value also used after the `if` — the behavior a plain
/// Rust `if` has always had. Used by `emit_if` for the non-dispatch cases.
fn emit_plain_if(
    cond: &Expr,
    then_body: &[UiNode],
    else_body: Option<&[UiNode]>,
    ctx: Ctx,
) -> TokenStream2 {
    match ctx {
        // Single-slot: must be one Element. Plain Rust `if`, both arms
        // coerced to Element (missing `else` → empty View).
        Ctx::Single => {
            let then_expr = emit_block_as_primitive(then_body);
            let else_expr =
                else_body.map(emit_block_as_primitive).unwrap_or_else(empty_view_primitive);
            quote! { if #cond { #then_expr } else { #else_expr } }
        }
        // Children-slot: flatten to a `Vec<Element>`. The taken branch appends
        // its nodes as FLAT siblings (no wrapper View); a missing `else`
        // contributes nothing (no empty-View placeholder).
        Ctx::Child => {
            let then_parts = then_body.iter().map(|n| emit_node(n, Ctx::Child));
            let else_block = match else_body {
                Some(eb) => {
                    let else_parts = eb.iter().map(|n| emit_node(n, Ctx::Child));
                    quote! { #( ::runtime_core::ChildList::append_to(#else_parts, &mut __c); )* }
                }
                None => quote! {},
            };
            quote! {
                {
                    let mut __c: ::std::vec::Vec<::runtime_core::Element>
                        = ::std::vec::Vec::new();
                    if #cond {
                        #( ::runtime_core::ChildList::append_to(#then_parts, &mut __c); )*
                    } else {
                        #else_block
                    }
                    __c
                }
            }
        }
    }
}

/// Returns true iff the condition's token stream contains a `.get()` call,
/// using the same heuristic the component macro already uses elsewhere.
///
/// `proc_macro2` token-stream `to_string()` inserts whitespace between
/// tokens, and the exact spacing varies across versions and depending
/// on how the input was reconstructed. To avoid false negatives we
/// strip *all* whitespace first and then look for the literal
/// `.get()` substring. This correctly fires for both simple
/// scrutinees like `screen.get()` and compound ones like
/// `(a.get(), b.get())`.
fn condition_is_reactive(cond: &Expr) -> bool {
    // Detects a VISIBLE signal read in the condition tokens — an inline
    // `.get()` such as `if sig.get() > 1` or `match screen.get()`. This is the
    // one syntactic reactive path the macro keeps, because such a condition's
    // *type* is a plain `bool`/enum and would otherwise dispatch to the static
    // `StaticCond` impl. It is NOT a guess about hidden behavior: an opaque
    // call like `if del_visible()` is left for the type-driven `StaticCond` /
    // `ReactiveCond` dispatch in `emit_if` — a bare `fn() -> bool` is static
    // and free, while a reactive `Signal<bool>` (e.g. `memo(...)`) is reactive
    // by type. (Same philosophy as the for-loop's `StaticForEach` /
    // `ReactiveForEach`, which replaced the old `.get()`-iterable heuristic.)
    let raw = cond.to_token_stream().to_string();
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains(".get()")
}

/// Emit a `match` UI node. Same reactivity heuristic as `emit_if`:
/// if the scrutinee reads a signal, lower to `runtime_core::switch`
/// so the arm re-evaluates on signal changes (and surviving subtrees
/// stay mounted across unrelated signal updates, courtesy of the
/// PartialEq dedup in `build_switch`). Otherwise emit a plain Rust
/// `match`.
///
/// The `switch` lowering binds the scrutinee value as `__v: &S` and
/// dispatches via a regular `match`, so Rust's match ergonomics
/// handle the implicit `&` coercion on patterns. The user writes
/// `Screen::Summary => ui!{...}` and it matches the borrowed value.
fn emit_match(scrutinee: &Expr, arms: &[MatchArm], ctx: Ctx) -> TokenStream2 {
    // Priority order, same as emit_if:
    //   1. Structured match — scrutinee is a `method(sig, ...)` call
    //      and every arm is `LITERAL => body` (or `_ => body` for
    //      the default). Lower to a structured `Element::Switch`
    //      so generator backends (Roku) can ship the binding
    //      declaratively.
    //   2. Reactive closure — scrutinee reads a signal, EITHER via a
    //      literal `.get()` (`match screen.get()`) OR via the
    //      reactive-call shape `match key(state)` where `key` reads the
    //      bare signal args internally (matching the structured `if`
    //      path, so a non-literal-arm `match` is reactive like
    //      `if key(state)`). Lower to `runtime_core::switch(..)` — one
    //      Element per arm.
    //   3. Plain Rust `match` — no reactivity. Flattens in children-slot.
    if let Some(structured) = try_emit_structured_match(scrutinee, arms) {
        return structured;
    }

    // Reactive scrutinee — two shapes funnel into the closure-`switch`:
    //   (a) `.get()` appears literally in the scrutinee
    //       (`match screen.get() { … }`), caught by
    //       `condition_is_reactive`.
    //   (b) the "reactive call" shape `match key(state) { … }` where the
    //       signal read lives inside `key` and the args are bare signals.
    //       `try_emit_structured_match` (above) already claims this shape
    //       WHEN every arm key is a literal; the enum/non-literal-arm case
    //       falls through to here. Without (b), `match key(state)` over an
    //       enum lowered to a STATIC plain `match` and never re-rendered
    //       on signal change — unlike the equivalent `if key(state) { … }`,
    //       which the structured `if` path always makes reactive.
    let reactive_call = is_reactive_call_shape(scrutinee);
    if condition_is_reactive(scrutinee) || reactive_call {
        // Build the scrutinee the closure evaluates. For shape (b) we
        // rewrite `key(state)` → `key((state).get())` so the closure
        // subscribes to each signal arg (the structured paths do the
        // same). Shape (a) already reads via `.get()`, so it's verbatim.
        let scrutinee_expr: TokenStream2 = if reactive_call {
            // Safe to unwrap: `is_reactive_call_shape` guaranteed `Expr::Call`.
            if let Expr::Call(call) = scrutinee {
                let func = &call.func;
                let get_args = call.args.iter().map(|a| quote! { (#a).get() });
                quote! { #func( #(#get_args),* ) }
            } else {
                quote! { #scrutinee }
            }
        } else {
            quote! { #scrutinee }
        };

        // Reactive: each arm is a single subtree (`switch` rebuilds the
        // active arm). Multi-node arms wrap by necessity.
        let arm_tokens: Vec<TokenStream2> = arms
            .iter()
            .map(|arm| {
                let pat = &arm.pat;
                let body = emit_block_as_primitive(&arm.body);
                let body_coerced =
                    quote! { ::runtime_core::IntoElement::into_element(#body) };
                match &arm.guard {
                    Some(g) => quote! { #pat if #g => #body_coerced },
                    None => quote! { #pat => #body_coerced },
                }
            })
            .collect();
        return quote! {
            ::runtime_core::switch(
                move || #scrutinee_expr,
                move |__v| match __v {
                    #( #arm_tokens, )*
                },
            )
        };
    }

    // Static `match` — no reactivity.
    match ctx {
        // Single-slot: one Element. Each arm coerced via IntoElement.
        Ctx::Single => {
            let arm_tokens: Vec<TokenStream2> = arms
                .iter()
                .map(|arm| {
                    let pat = &arm.pat;
                    let body = emit_block_as_primitive(&arm.body);
                    let body_coerced =
                        quote! { ::runtime_core::IntoElement::into_element(#body) };
                    match &arm.guard {
                        Some(g) => quote! { #pat if #g => #body_coerced },
                        None => quote! { #pat => #body_coerced },
                    }
                })
                .collect();
            quote! {
                match #scrutinee {
                    #( #arm_tokens, )*
                }
            }
        }
        // Children-slot: each arm appends its nodes as FLAT siblings into
        // a shared vec (no per-arm wrapper View). The whole `match`
        // evaluates to that `Vec<Element>`.
        Ctx::Child => {
            let arm_tokens: Vec<TokenStream2> = arms
                .iter()
                .map(|arm| {
                    let pat = &arm.pat;
                    let parts = arm.body.iter().map(|n| emit_node(n, Ctx::Child));
                    let appends = quote! {
                        #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
                    };
                    match &arm.guard {
                        Some(g) => quote! { #pat if #g => { #appends } },
                        None => quote! { #pat => { #appends } },
                    }
                })
                .collect();
            quote! {
                {
                    let mut __c: ::std::vec::Vec<::runtime_core::Element>
                        = ::std::vec::Vec::new();
                    match #scrutinee {
                        #( #arm_tokens )*
                    }
                    __c
                }
            }
        }
    }
}

/// Lower `match method(sig) { 0 => body0, 1 => body1, _ => default }`
/// directly into a `Element::Switch` carrying a structured
/// `Derived<serde_json::Value>` discriminant + literal-keyed arms.
/// Returns `None` if any arm has a non-literal pattern, a guard,
/// or the scrutinee isn't a structured call shape — the caller
/// falls back to the closure-driven `switch()` builder.
///
/// Patterns supported as arm keys: integer / bool / string / float
/// literals. Range patterns, struct destructuring, `|` alternation,
/// and guards aren't supported in the structured path — Roku's
/// runtime equality is JSON-value comparison and arm matching is
/// linear, so anything richer than a literal is a closure-path
/// affair.
fn try_emit_structured_match(scrutinee: &Expr, arms: &[MatchArm]) -> Option<TokenStream2> {
    let call = match scrutinee {
        Expr::Call(c) => c,
        _ => return None,
    };
    let func_ident = match &*call.func {
        Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
            if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                return None;
            }
            path.segments[0].ident.clone()
        }
        _ => return None,
    };
    let args: Vec<&Expr> = call.args.iter().collect();
    if args.is_empty() {
        return None;
    }
    for a in &args {
        match a {
            Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
                if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                    return None;
                }
            }
            _ => return None,
        }
    }

    // Walk arms. Every arm must be (lit-pat | wildcard) with no guard.
    let mut literal_arms: Vec<(TokenStream2, &Vec<UiNode>)> = Vec::new();
    let mut default: Option<&Vec<UiNode>> = None;
    for arm in arms {
        if arm.guard.is_some() {
            return None;
        }
        match &arm.pat {
            syn::Pat::Lit(syn::PatLit { lit, .. }) => {
                let v = lit_to_value_tokens(lit)?;
                literal_arms.push((v, &arm.body));
            }
            syn::Pat::Wild(_) => {
                if default.is_some() {
                    return None;
                }
                default = Some(&arm.body);
            }
            _ => return None,
        }
    }
    let default = default?;

    let method_lit = syn::LitStr::new(&func_ident.to_string(), func_ident.span());
    let get_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).get() }).collect();
    let id_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).id() }).collect();
    let initial_calls: Vec<TokenStream2> = args
        .iter()
        .map(|a| {
            quote! {
                ::runtime_core::__serde_json::to_value(&(#a).get())
                    .unwrap_or(::runtime_core::__serde_json::Value::Null)
            }
        })
        .collect();

    let arm_tokens: Vec<TokenStream2> = literal_arms
        .into_iter()
        .map(|(pat_value, body)| {
            let body_expr = emit_block_as_primitive(body);
            quote! {
                (
                    #pat_value,
                    ::std::boxed::Box::new(move || #body_expr)
                        as ::std::boxed::Box<dyn ::std::ops::Fn() -> ::runtime_core::Element>,
                )
            }
        })
        .collect();
    let default_expr = emit_block_as_primitive(default);

    Some(quote! {
        ::runtime_core::Element::Switch {
            discriminant: ::runtime_core::Derived::<::runtime_core::__serde_json::Value> {
                method:  #method_lit,
                inputs:  ::std::vec![ #(#id_calls),* ],
                initial: ::std::vec![ #(#initial_calls),* ],
                compute: ::std::rc::Rc::new(move || {
                    ::runtime_core::__serde_json::to_value(&#func_ident( #(#get_calls),* ))
                        .unwrap_or(::runtime_core::__serde_json::Value::Null)
                }),
            },
            arms:    ::std::vec![ #(#arm_tokens),* ],
            default: ::std::boxed::Box::new(move || #default_expr),
            style:   ::std::option::Option::None,
        }
    })
}

/// Emit a `serde_json::Value` constructor for a literal pattern.
/// Returns `None` for unsupported literal kinds (chars, byte
/// strings — we don't ship those over the Roku wire today).
fn lit_to_value_tokens(lit: &syn::Lit) -> Option<TokenStream2> {
    match lit {
        syn::Lit::Int(i) => {
            let i = i.clone();
            Some(quote! { ::runtime_core::__serde_json::Value::from(#i) })
        }
        syn::Lit::Bool(b) => {
            let b = b.value;
            Some(quote! { ::runtime_core::__serde_json::Value::from(#b) })
        }
        syn::Lit::Str(s) => {
            let s = s.value();
            Some(quote! { ::runtime_core::__serde_json::Value::from(#s) })
        }
        syn::Lit::Float(f) => {
            let f = f.clone();
            Some(quote! { ::runtime_core::__serde_json::Value::from(#f as f64) })
        }
        _ => None,
    }
}

fn emit_for(
    pat: &syn::Pat,
    iter: &Expr,
    key: Option<&Expr>,
    body: &[UiNode],
    chain: &[TokenStream2],
    ctx: Ctx,
) -> TokenStream2 {
    // The natural form of a `for` is a flat children list — all
    // `ChildList`-appendable, so it drops straight into a children slot.
    // In single-slot position (sole top-level element, or a `when` /
    // `switch` branch that is just a loop) the result must be ONE
    // `Element`: the Virtualizer / reactive-range `each` forms already
    // ARE one primitive (`is_single`), so they pass through; the `Vec`
    // forms (Repeat, type-driven dispatch) are wrapped in a `View`.
    let (child_form, is_single) = emit_for_children(pat, iter, key, body, chain);
    match ctx {
        Ctx::Child => child_form,
        Ctx::Single if is_single => child_form,
        Ctx::Single => quote! {
            ::runtime_core::view({
                let mut __c: ::std::vec::Vec<::runtime_core::Element>
                    = ::std::vec::Vec::new();
                ::runtime_core::ChildList::append_to(#child_form, &mut __c);
                __c
            })
        },
    }
}

/// Returns `(tokens, is_single)` where `is_single` is true when the
/// emitted form is already exactly one `Element` (Virtualizer /
/// reactive-range `each`) and false when it's a `Vec<Element>`
/// (Repeat / type-driven dispatch) that a single-slot caller must wrap.
fn emit_for_children(
    pat: &syn::Pat,
    iter: &Expr,
    key: Option<&Expr>,
    body: &[UiNode],
    chain: &[TokenStream2],
) -> (TokenStream2, bool) {
    // Reactive-list path: `for IDENT in count_method(sig) { body }` —
    // lower to a `Element::Virtualizer` carrying a structured
    // `Derived<usize>` for the count plus a row template. This is
    // what `bind_repeat!` used to do explicitly. Try this BEFORE
    // the static range path so a method-call iterator wins over
    // any static-range matching.
    if let Some(virt) = try_emit_for_virtualizer(pat, iter, body, chain) {
        return (virt, true);
    }

    // Reactive COUNT range — `for i in A..B` where a bound reads a
    // signal (e.g. `for i in 0..n.get()`). The iterable is a `Range`,
    // which is a *static* type, so the type-driven path below would
    // snapshot it once. We special-case a syntactic range whose bounds
    // read a signal and wrap the loop in a reactive `each` so the count
    // re-evaluates on change.
    //
    // This `.get()` check is deliberately scoped to RANGE BOUNDS only —
    // it is NOT the old general iterable heuristic (that's gone; the
    // type-driven path decides reactivity for every other iterable).
    // A `.get()` somewhere in a non-range iterable can no longer make a
    // loop accidentally reactive.
    if matches!(iter, Expr::Range(_)) && condition_is_reactive(iter) {
        let parts: Vec<TokenStream2> = body.iter().map(|n| emit_node(n, Ctx::Child)).collect();
        // A reactive range is keyed-by-position: the row's identity IS
        // its index, so the enumeration counter is the natural key (or
        // the author's `key` expr if they wrote one). Keying — rather
        // than a full rebuild — means growing/shrinking the count keeps
        // the surviving rows' component-local state, just like a keyed
        // `Signal<Vec<_>>` loop.
        let key_expr = match key {
            Some(k) => quote! { #k },
            None => quote! { __idx },
        };
        let each = quote! {
            ::runtime_core::each_keyed(move || {
                let mut __c: ::std::vec::Vec<(
                    ::runtime_core::EachKey,
                    ::runtime_core::EachRowBuild,
                )> = ::std::vec::Vec::new();
                let mut __idx: usize = 0;
                for #pat in #iter {
                    let __key = ::runtime_core::EachKey::new(#key_expr);
                    __idx += 1;
                    let __build: ::runtime_core::EachRowBuild = ::std::boxed::Box::new(move || {
                        let mut __row: ::std::vec::Vec<::runtime_core::Element>
                            = ::std::vec::Vec::new();
                        #( ::runtime_core::ChildList::append_to(#parts, &mut __row); )*
                        __row
                    });
                    __c.push((__key, __build));
                }
                __c
            })
        };
        let form = if chain.is_empty() {
            each
        } else {
            quote! { (#each) #(#chain)* }
        };
        return (form, true);
    }

    // Static range fast path: `for i in 0..n { single_node }` → batched
    // `Element::Repeat`, expanded by the walker into the parent's
    // children via `insert_many` (DocumentFragment batching on web).
    // Single-node bodies only: `Repeat` is one-node-per-index, so a
    // multi-node body would need a wrapper View — refused, since
    // children are a flat vector. Multi-node / non-range loops fall
    // through to the type-driven path below.
    if body.len() == 1 {
        let body_expr = emit_block_as_primitive(body);
        if let Some(repeat) = try_emit_for_repeat(pat, iter, &body_expr) {
            return (repeat, false);
        }
    }

    // Type-driven dispatch — the heuristic-free path for every other
    // iterable. We emit a `__idealyst_for_each*` call with BOTH
    // `StaticForEach` and `ReactiveForEach` in scope; Rust method
    // resolution picks the impl from ITER's *type*:
    //   - `Signal<C>` (a signal of a cloneable iterable) → a keyed
    //     reactive `Element::Each`,
    //   - any other `IntoIterator` (Vec, &Vec, array, HashMap, …) → a
    //     flat, built-once `Vec<Element>`.
    //
    // No `.get()` substring is inspected — the type decides — so a
    // `HashMap::get()` (or any incidental `.get()`) can never make a
    // loop accidentally reactive, and a real signal iterable is never
    // silently missed.
    //
    // Key handling: with a `, key = …` clause we emit the *keyed* method
    // (`__idealyst_for_each_keyed`), which both the static and reactive
    // impls provide. WITHOUT a key we emit the keyless method — defined
    // only on `StaticForEach` (and on `ReactiveForEach` behind a
    // never-satisfied bound). So a keyless `for x in vec { … }` compiles
    // (static) while a keyless `for x in signal { … }` is a COMPILE
    // ERROR carrying the `ReactiveListKeyed` diagnostic: a reactive list
    // must be keyed so per-row state survives rebuilds.
    let parts: Vec<TokenStream2> = body.iter().map(|n| emit_node(n, Ctx::Child)).collect();
    let dispatch = if let Some(k) = key {
        quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::{StaticForEach as _, ReactiveForEach as _};
                (#iter).__idealyst_for_each_keyed(
                    move |#pat| #k,
                    move |#pat| {
                        let mut __row: ::std::vec::Vec<::runtime_core::Element>
                            = ::std::vec::Vec::new();
                        #( ::runtime_core::ChildList::append_to(#parts, &mut __row); )*
                        __row
                    },
                )
            }
        }
    } else {
        quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::{StaticForEach as _, ReactiveForEach as _};
                (#iter).__idealyst_for_each(move |#pat| {
                    let mut __row: ::std::vec::Vec<::runtime_core::Element>
                        = ::std::vec::Vec::new();
                    #( ::runtime_core::ChildList::append_to(#parts, &mut __row); )*
                    __row
                })
            }
        }
    };
    let form = if chain.is_empty() {
        dispatch
    } else {
        quote! { (#dispatch) #(#chain)* }
    };
    (form, false)
}

/// Try to lower `for IDENT in count_method(sig, ...) { body }` to a
/// `Element::Virtualizer` carrying a structured `Derived<usize>`
/// (the count) + a captured row template. The IDENT inside the
/// body becomes a `Signal<i32>` carrying the row's index — same
/// trick `bind_repeat!` used. Returns `None` if the iterator
/// isn't a structured call shape.
fn try_emit_for_virtualizer(
    pat: &syn::Pat,
    iter: &Expr,
    body: &[UiNode],
    chain: &[TokenStream2],
) -> Option<TokenStream2> {
    // Pattern must be a bare ident.
    let row_ident = match pat {
        syn::Pat::Ident(p) if p.subpat.is_none() && p.by_ref.is_none() => &p.ident,
        _ => return None,
    };
    // Iterator must be a structured call shape — same recognition
    // criteria as `try_emit_derived_call` and friends.
    let call = match iter {
        Expr::Call(c) => c,
        _ => return None,
    };
    let func_ident = match &*call.func {
        Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
            if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                return None;
            }
            path.segments[0].ident.clone()
        }
        _ => return None,
    };
    let args: Vec<&Expr> = call.args.iter().collect();
    if args.is_empty() {
        return None;
    }
    for a in &args {
        match a {
            Expr::Path(syn::ExprPath { qself: None, path, .. }) => {
                if path.segments.len() != 1 || !path.segments[0].arguments.is_empty() {
                    return None;
                }
            }
            _ => return None,
        }
    }

    let method_lit = syn::LitStr::new(&func_ident.to_string(), func_ident.span());
    let get_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).get() }).collect();
    let id_calls: Vec<TokenStream2> = args.iter().map(|a| quote! { (#a).id() }).collect();
    let initial_calls: Vec<TokenStream2> = args
        .iter()
        .map(|a| {
            quote! {
                ::runtime_core::__serde_json::to_value(&(#a).get())
                    .unwrap_or(::runtime_core::__serde_json::Value::Null)
            }
        })
        .collect();

    let body_expr = emit_block_as_primitive(body);

    Some(quote! {
        {
            // Allocate the per-row index signal at snapshot time
            // (initial value 0). The device-side runtime mints a
            // fresh synthetic signal per cloned row and remaps
            // references to this id so a structured `method(i)` binding inside
            // the body dispatches with each row's actual index.
            let #row_ident: ::runtime_core::Signal<i32> =
                ::runtime_core::signal(0i32);
            let __row_index_id: ::std::option::Option<u64> =
                ::std::option::Option::Some(::runtime_core::Signal::<i32>::id(&#row_ident));
            // Build the row template against the index-0 placeholder.
            let __row_template: ::runtime_core::Element =
                ::runtime_core::IntoElement::into_element(#body_expr);
            // Wrap in a `Bound<VirtualizerHandle>` so the trailing
            // chain (e.g. `.with_style(...)`, `.horizontal(true)`)
            // applies to the Bound's methods. The structured
            // emission populates every field of the underlying
            // `Element::Virtualizer`; chain methods can mutate
            // them after construction.
            #[allow(unused_mut)]
            let mut __vh = ::runtime_core::primitives::virtualizer::virtualizer(
                ::std::boxed::Box::new(|| 0usize),
                ::std::boxed::Box::new(|i| i as u64),
                ::runtime_core::primitives::virtualizer::ItemSize::Known(
                    ::std::rc::Rc::new(|_| 40.0)
                ),
                ::std::rc::Rc::new(move |__idx: usize| {
                    // Real per-row builder for runtime backends (web,
                    // iOS, Android, wgpu). Each row gets its OWN index
                    // signal, seeded to the row's index, declared fresh
                    // on every call — `build_virtualizer` runs this
                    // inside a per-item `Scope`, so reactive reads of the
                    // loop variable resolve to *that* row's index with no
                    // cross-row signal sharing. Generator backends (Roku)
                    // ignore this closure and clone `row_template`,
                    // remapping `row_index_signal_id` per device row.
                    //
                    // Previously this was an empty-View placeholder, so
                    // `for i in count(sig) { … }` silently rendered blank
                    // rows on every runtime backend.
                    let #row_ident: ::runtime_core::Signal<i32> =
                        ::runtime_core::signal(__idx as i32);
                    ::runtime_core::IntoElement::into_element(#body_expr)
                }),
            );
            // Patch the structured-only fields on the underlying
            // Element::Virtualizer. The `virtualizer()` builder
            // doesn't know about them (it only handles the closure
            // shape) so we mutate them directly here.
            if let ::runtime_core::Element::Virtualizer {
                item_count, row_template, row_index_signal_id, ..
            } = &mut __vh.primitive_mut() {
                *item_count = ::runtime_core::Derived::<usize> {
                    method:  #method_lit,
                    inputs:  ::std::vec![ #(#id_calls),* ],
                    initial: ::std::vec![ #(#initial_calls),* ],
                    compute: ::std::rc::Rc::new(move || {
                        #func_ident( #(#get_calls),* ) as usize
                    }),
                };
                *row_template = ::std::option::Option::Some(::std::boxed::Box::new(__row_template));
                *row_index_signal_id = __row_index_id;
            }
            __vh #(#chain)*
        }
    })
}

/// Try to lower `for PAT in RANGE { body }` to a single
/// `Element::Repeat`. Returns `Some(tokens)` only when the
/// shape is one we can statically recognize:
///
/// - `iter` is a syntactic range expression with both bounds.
/// - `pat` is a simple identifier (so we can pass it as the
///   `row_builder` closure's `i` argument). Patterns like
///   tuples or destructuring aren't supported here — fall
///   back to the generic loop.
///
/// The emitted closure shifts the loop index by the range's
/// lower bound so author code that writes `for i in 5..10`
/// sees `i` ranging 5..10 inside the body, not 0..5.
fn try_emit_for_repeat(
    pat: &syn::Pat,
    iter: &Expr,
    body_expr: &TokenStream2,
) -> Option<TokenStream2> {
    // The pattern must be a single ident — anything else (tuple
    // destructuring, references, etc.) means the author is doing
    // something we can't trivially rebind through a `Fn(usize)`.
    let ident = match pat {
        syn::Pat::Ident(p) if p.subpat.is_none() && p.by_ref.is_none() => &p.ident,
        _ => return None,
    };

    // The iterator must be a range literal with both bounds.
    let range = match iter {
        Expr::Range(r) => r,
        _ => return None,
    };
    let start = range.start.as_ref()?;
    let end = range.end.as_ref()?;
    // Inclusive ranges (`a..=b`) need a +1 adjustment; for simplicity
    // we only handle exclusive ranges. Authors using inclusive ranges
    // hit the fallback path with no behavior change.
    if matches!(range.limits, syn::RangeLimits::Closed(_)) {
        return None;
    }

    // Build the closure body. We bind the user's chosen identifier
    // to `start + __i`, where `__i` is the closure's `usize` parameter
    // (always 0..count). This preserves the original visible semantics
    // of `for i in 5..10 { use(i) }` inside the row builder.
    Some(quote! {
        ::std::vec![
            ::runtime_core::Element::Repeat {
                // `(end - start)` evaluated as `usize`. Author code
                // commonly writes `0..n` where `n: usize`; this works
                // with any integer type via the `usize::try_from`
                // fallback in `Element::Repeat`'s constructor, but
                // we accept the simpler cast here because the macro's
                // surface is `usize`-typed loops.
                count: (#end - #start) as usize,
                row_builder: ::std::boxed::Box::new(move |__i: usize| {
                    let #ident = (#start) + __i;
                    ::runtime_core::IntoElement::into_element(#body_expr)
                }),
            }
        ]
    })
}

/// Emit a block of UI nodes as a single `Element`-producing expression.
/// Used for if/else/for branches where we need exactly one primitive value.
/// The result is coerced via `IntoElement` so the branch can produce
/// either a `Bound<H>` (from a primitive constructor) or a `Element`
/// (from a user component) and the surrounding `when()` / `if`
/// expression always sees `Element`.
fn emit_block_as_primitive(nodes: &[UiNode]) -> TokenStream2 {
    let body = match nodes.len() {
        0 => quote! { ::runtime_core::view(::std::vec::Vec::new()) },
        // Sole node must itself be one Element: single-slot context.
        1 => emit_node(&nodes[0], Ctx::Single),
        // Multiple nodes genuinely need a wrapper to collapse to one
        // value; the wrapper's children are a list, so each is Child.
        _ => {
            let parts = nodes.iter().map(|n| emit_node(n, Ctx::Child));
            quote! {
                ::runtime_core::view({
                    let mut __c: ::std::vec::Vec<::runtime_core::Element>
                        = ::std::vec::Vec::new();
                    #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
                    __c
                })
            }
        }
    };
    quote! { ::runtime_core::IntoElement::into_element(#body) }
}

/// `DrawerNavigator` `ui!` sugar — retired. This branch used to emit
/// `::runtime_core::DrawerNavigator::new(...)`, but no such author-facing
/// type exists (nor is one re-exported from `runtime_core`): the navigator
/// system converged on the SDK-crate builders `stack-navigator`
/// (`StackNavigator`) and `swap-navigator` (`SwapNavigator`), which are NOT
/// used through `ui!` sugar. The old sugar therefore couldn't compile — an
/// author reaching for it got an inscrutable "cannot find `DrawerNavigator`
/// in `runtime_core`". We now fail with a pointed message instead.
///
/// (Backend-side `NodeKind::DrawerNavigator` plumbing in the GPU engine is
/// left in place — whether a drawer navigator returns via the SDK-crate
/// pattern is a framework-author call, tracked separately; this only removes
/// the dead author-facing macro path.)
fn emit_drawer_navigator(_props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    quote! {
        ::std::compile_error!(
            "DrawerNavigator has no `ui!` sugar. Use a navigator SDK crate's builder API: \
             `stack_navigator::StackNavigator::new(&ROUTE).screen(...)` for a push/pop stack, \
             or `swap_navigator::SwapNavigator::new(&ROUTE)` for tab/drawer-style swapping. \
             See the `navigation` guide (read_guide) and the `stack_two_screens` recipe."
        )
    }
}

/// Emit a `CardTabs { Tab(label = "...") { ... } ... }` invocation
/// as a user-component call carrying `tabs = vec![(label,
/// render_closure), ...]`. Each Tab's body is wrapped in a render
/// closure so the panel can be invoked lazily — the active panel
/// builds at switch time, not eagerly at mount.
///
/// Non-`Tab` children fail compilation with a pointed message so
/// the constraint reads at the call site.
fn emit_card_tabs(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    let kids = children.unwrap_or(&[]);
    let mut tab_pairs: Vec<TokenStream2> = Vec::new();
    for kid in kids {
        match kid {
            UiNode::Component {
                name,
                props: tab_props,
                children: tab_children,
                chain: _,
            } if name.to_string().to_ascii_lowercase() == "tab" => {
                // `label` is required.
                let label = match tab_props.iter().find(|p| p.name == "label") {
                    Some(p) => p.value.clone(),
                    None => {
                        return quote! {
                            ::std::compile_error!(
                                "CardTabs: each Tab requires a `label` prop"
                            )
                        };
                    }
                };
                // Build the body Element from the Tab's children
                // and wrap it in a render closure. The closure is
                // `Rc<dyn Fn() -> Element>` so it can be cheaply
                // cloned into a `switch` branches closure that
                // dispatches by index.
                let body_nodes: &[UiNode] = tab_children.as_deref().unwrap_or(&[]);
                let body_expr = emit_block_as_primitive(body_nodes);
                tab_pairs.push(quote! {
                    (
                        ::std::string::String::from(#label),
                        ::std::rc::Rc::new(move || #body_expr)
                            as ::std::rc::Rc<dyn Fn() -> ::runtime_core::Element>,
                    )
                });
            }
            UiNode::Component { name, .. } => {
                let got = name.to_string();
                let msg = format!(
                    "CardTabs children must be Tab(...) elements; got `{}`",
                    got
                );
                return quote! { ::std::compile_error!(#msg) };
            }
            _ => {
                return quote! {
                    ::std::compile_error!(
                        "CardTabs children must be Tab(...) elements"
                    )
                };
            }
        }
    }

    // Pass any other props through to the `cardtabs!` invocation
    // unchanged — same shape as `emit_user`.
    let other_prop_assignments = props.iter().map(|p| {
        let n = &p.name;
        let v = emit_attr_value(&p.value);
        quote! { #n = #v }
    });

    quote! {
        cardtabs!(
            #(#other_prop_assignments,)*
            tabs = vec![#(#tab_pairs),*]
        )
    }
}

// Silence "unused" complaints on items we may need later.
#[allow(dead_code)]
fn _unused(_: Span) {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a `ui!` body and emit it, returning the emitted token stream
    /// as a string for substring assertions. NOTE: the output includes the
    /// `__ui_recover` salvage shell (same as a real expansion) — counted
    /// assertions must account for the salvage copy of the input.
    fn parse_and_emit(input: TokenStream2) -> String {
        let ui: Ui = syn::parse2(input.clone()).expect("parse ui");
        emit(ui, &input).to_string()
    }

    #[test]
    fn empty_ui_emits_empty_view() {
        let out = parse_and_emit(quote! {});
        assert!(out.contains("view"));
        assert!(out.contains("Vec :: new"));
    }

    /// Regression (arena drift, 2026-07-22): the `DrawerNavigator` `ui!`
    /// sugar emitted `::runtime_core::DrawerNavigator::new(...)` — a type
    /// that does not exist — so any use failed with an inscrutable
    /// "cannot find DrawerNavigator in runtime_core". It must now emit a
    /// `compile_error!` that points at the real navigator builders, and
    /// must NOT emit the dead `runtime_core::DrawerNavigator` path.
    #[test]
    fn regression_drawer_navigator_sugar_emits_guiding_compile_error() {
        let out = parse_and_emit(quote! {
            DrawerNavigator(initial = HOME) {
                Screen(route = HOME) { HomeScreen() }
            }
        });
        assert!(
            out.contains("compile_error"),
            "DrawerNavigator sugar must emit a compile_error, got: {out}"
        );
        assert!(
            out.contains("StackNavigator") || out.contains("SwapNavigator"),
            "the error must point at the real navigator builders"
        );
        assert!(
            !out.contains("DrawerNavigator :: new"),
            "the dead `runtime_core::DrawerNavigator::new` path must be gone"
        );
    }

    #[test]
    fn text_with_children_block_emits_text_call() {
        let out = parse_and_emit(quote! { text { "hello" } });
        assert!(out.contains(":: runtime_core :: text"));
        assert!(out.contains("\"hello\""));
    }

    #[test]
    fn text_with_content_prop_emits_text_call() {
        let out = parse_and_emit(quote! { text(content = "hi") });
        assert!(out.contains(":: runtime_core :: text"));
        assert!(out.contains("\"hi\""));
    }

    #[test]
    fn user_component_dispatches_via_build_element() {
        // `Counter(...)` lowers to a struct literal + UFCS build call,
        // keyed on the `Counter` -> `CounterProps` convention — no
        // per-component macro.
        let out = parse_and_emit(quote! { Counter(label = "x", value = score) });
        // The tag is used as the type name (a `pub type Counter = …Props`
        // alias bridges to the real props), so the literal is `Counter { … }`.
        assert!(out.contains("Counter {"), "got: {out}");
        assert!(out.contains("BuildElement :: build"), "got: {out}");
        assert!(!out.contains("Counter !"), "no macro dispatch; got: {out}");
    }

    #[test]
    fn user_component_attr_values_coerced_via_into() {
        let out = parse_and_emit(quote! { Counter(label = "x", value = score) });
        // Each prop is a struct field coerced with `.into()`; the field's
        // declared type pins the target (so `"x"` lands in a String /
        // Reactive<String>). Both literal and non-literal values coerce.
        assert!(out.contains("(\"x\") . into ()"), "got: {out}");
        assert!(out.contains("(score) . into ()"), "got: {out}");
    }

    #[test]
    fn user_component_uses_defaults_struct_update_base() {
        // Omitted props come from `..<CounterProps as BuildElement>::defaults()`.
        let out = parse_and_emit(quote! { Counter(value = score) });
        assert!(out.contains("BuildElement > :: defaults ()"), "got: {out}");
    }

    #[test]
    fn user_component_with_children_emits_children_field() {
        let out = parse_and_emit(quote! {
            Card(title = "T") {
                Counter(value = s)
            }
        });
        assert!(out.contains("Card {"), "got: {out}");
        assert!(out.contains("BuildElement :: build"), "got: {out}");
        assert!(out.contains("children :"), "children is a struct field now; got: {out}");
        assert!(out.contains("ChildList :: append_to"), "got: {out}");
    }

    #[test]
    fn lowercase_call_falls_through_to_expression() {
        // Capitalization is the disambiguator: an uppercase-first ident
        // followed by `(` / `{` is a component invocation; a
        // lowercase-first ident is a Rust function call that goes
        // through expression parsing (see
        // `next_is_component_invocation`). This is what lets reactive
        // helper calls like `count_label(count)` work inside
        // `text { ... }` without the parser trying to grab `count` as
        // a prop name.
        let out = parse_and_emit(quote! { mycomp(x = 1) });
        // Should be wrapped via IntoElement::into_element (the
        // expression-passthrough path), NOT dispatched to a
        // `mycomp!` invocation macro.
        assert!(
            !out.contains("mycomp !"),
            "lowercase ident should not dispatch to `mycomp!`; got: {}",
            out,
        );
        assert!(
            out.contains("into_element") && out.contains("mycomp"),
            "expected expression-passthrough; got: {}",
            out,
        );
    }

    #[test]
    fn bare_ident_is_passed_through_as_rust_expr() {
        // `extra` alone (no parens, no brace) — parses as a Rust
        // expression and goes through ChildList passthrough.
        let out = parse_and_emit(quote! { extra });
        // No `extra !` macro invocation should appear.
        assert!(!out.contains("extra !"));
    }

    #[test]
    fn reactive_if_rewrites_to_when() {
        let out = parse_and_emit(quote! {
            if flag.get() {
                text { "on" }
            } else {
                text { "off" }
            }
        });
        assert!(out.contains(":: runtime_core :: when"));
        assert!(out.contains("move ||"));
    }

    #[test]
    fn non_reactive_if_emits_plain_if() {
        let out = parse_and_emit(quote! {
            if some_bool {
                text { "on" }
            } else {
                text { "off" }
            }
        });
        // No eager `when()` in the emission: a bare-path condition goes
        // through the type-driven `__idealyst_if` dispatch, and reactivity
        // is decided by the condition's TYPE (`bool` → StaticCond → plain
        // branch; a signal type → ReactiveCond → reactive `when` at
        // runtime). This test predates the dispatch and used to assert a
        // literal `if some_bool` — stale since path-4 landed.
        assert!(!out.contains(":: runtime_core :: when"));
        assert!(out.contains("(some_bool) . __idealyst_if"));
    }

    #[test]
    fn for_loop_emits_type_driven_dispatch() {
        let out = parse_and_emit(quote! {
            for n in items {
                text { "x" }
            }
        });
        // A keyless `for` lowers to the type-driven `__idealyst_for_each`
        // dispatch (StaticForEach / ReactiveForEach), not a literal `for`.
        // Each iteration appends flat siblings into a row Vec.
        assert!(out.contains("__idealyst_for_each"));
        // …and NOT the keyed variant — keyless stays keyless (the type
        // system, not the macro, rejects a keyless reactive loop).
        assert!(!out.contains("__idealyst_for_each_keyed"));
        assert!(out.contains("move | n |"));
        assert!(out.contains("ChildList :: append_to"));
    }

    #[test]
    fn for_loop_with_key_emits_keyed_dispatch() {
        let out = parse_and_emit(quote! {
            for n in items, key = n.id {
                text { "x" }
            }
        });
        // A `, key = …` clause lowers to the KEYED dispatch, passing a
        // key closure (the key expr) alongside the row builder.
        assert!(out.contains("__idealyst_for_each_keyed"));
        // The key closure carries the author's key expression.
        assert!(out.contains("n . id"));
        assert!(out.contains("ChildList :: append_to"));
    }

    #[test]
    fn else_if_chain_parses() {
        // Should parse without error; emitted shape contains nested
        // if/else.
        let out = parse_and_emit(quote! {
            if a.get() { text { "a" } } else if b.get() { text { "b" } } else { text { "c" } }
        });
        assert!(out.contains(":: runtime_core :: when"));
    }

    #[test]
    fn multiple_children_get_wrapped_in_children() {
        let out = parse_and_emit(quote! {
            Card {
                Counter(value = s)
                Counter(value = t)
            }
        });
        // Both Counter calls appear, and the wrapping ChildList::append_to
        // ensures they flatten into Vec<Element>.
        // Both children lower to their own Counter struct literal — twice
        // each: once in the real build chain, once in the `__ui_recover`
        // salvage shell every expansion now carries (see `emit_shell`).
        assert!(out.contains("(s) . into ()"), "got: {out}");
        assert!(out.contains("(t) . into ()"), "got: {out}");
        assert_eq!(out.matches("Counter {").count(), 4, "got: {out}");
    }

    // ---- error-recovery expansion ----

    /// `Ui` has no `Debug`, so `unwrap_err` won't compile. This grabs the
    /// parse error (panicking if the body unexpectedly parses).
    fn parse_err(input: TokenStream2) -> syn::Error {
        match syn::parse2::<Ui>(input) {
            Ok(_) => panic!("expected the body to fail to parse"),
            Err(e) => e,
        }
    }

    #[test]
    fn recovery_keeps_diagnostic_and_salvages_complete_props() {
        // `label = broken .` is a half-typed expression, so the whole
        // body fails to parse. Recovery must (a) keep the real
        // compile_error, and (b) re-surface the *complete* prop values so
        // rust-analyzer keeps type info for everything but the token being
        // typed.
        let input = quote! { Button(tone = good_tone, label = broken .) };
        let err = parse_err(input.clone());
        let ts = emit_recovery(input, &err);
        let s = ts.to_string();
        assert!(s.contains("compile_error"), "must keep the real diagnostic: {s}");
        assert!(s.contains("good_tone"), "should salvage the complete prop value: {s}");
        // Prefix recovery pulls `broken` out of the half-typed `broken .`.
        assert!(s.contains("broken"), "should salvage the parseable prefix: {s}");
    }

    #[test]
    fn recovery_output_is_valid_rust_expression() {
        // The recovery expansion must itself parse as an expression — if it
        // didn't, rust-analyzer couldn't expand `ui!` at all and we'd lose
        // more than the bare-`compile_error!` baseline.
        let input = quote! { Button(tone = good_tone, label = broken .) };
        let err = parse_err(input.clone());
        let ts = emit_recovery(input, &err);
        syn::parse2::<Expr>(ts).expect("recovery output must be a valid expression");
    }

    #[test]
    fn recovery_with_nothing_salvageable_is_still_valid() {
        // A broken child with no salvageable prop value: recovery still
        // produces a valid, compile_error-bearing expression (empty salvage
        // closure), never unparseable tokens.
        let input = quote! { text { foo. } };
        let err = parse_err(input.clone());
        let ts = emit_recovery(input, &err);
        assert!(ts.to_string().contains("compile_error"));
        syn::parse2::<Expr>(ts).expect("recovery output must be a valid expression");
    }

    #[test]
    fn recovery_salvages_across_nested_groups() {
        // The complete sibling prop deep inside a children block must be
        // salvaged even when a sibling is mid-typed.
        let input = quote! {
            Card(title = "t") {
                Counter(value = signal.get(), label = oops .)
            }
        };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(s.contains("signal . get ()"), "nested complete prop should be salvaged: {s}");
    }

    #[test]
    fn recovery_half_typed_prop_emits_struct_literal_for_name_completion() {
        // THE most common mid-typing state: `Counter(sta` (auto-closed
        // paren). The recovery must put `sta` in FIELD-NAME position of a
        // `Counter { … }` struct literal — the tag aliases the props
        // struct, so rust-analyzer completes `sta` → `start` there.
        let input = quote! { Counter(sta) };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(
            s.contains("Counter { sta : :: core :: todo ! ()"),
            "half-typed prop must land in struct-literal field position: {s}"
        );
        assert!(
            s.contains(".. :: core :: default :: Default :: default ()"),
            "struct literal must be completed by a Default base: {s}"
        );
    }

    #[test]
    fn recovery_struct_literal_includes_complete_and_partial_prop_names() {
        // `Counter(start = 3, la` — the complete prop AND the half-typed
        // one both become fields (todo!() coerces, so the complete name
        // adds no type noise).
        let input = quote! { Counter(start = 3, la) };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(s.contains("start : :: core :: todo ! ()"), "{s}");
        assert!(s.contains("la : :: core :: todo ! ()"), "{s}");
    }

    #[test]
    fn recovery_salvages_bare_child_expressions_and_tags() {
        // Bare expressions in child position (and a half-typed tag name)
        // salvage as value-position expressions: `Cou` completes to the
        // component fn, `count.get()` keeps hover/completion.
        let input = quote! {
            view() {
                count.get()
                Cou
                broken .
            }
        };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(s.contains("count . get ()"), "bare child expr salvaged: {s}");
        assert!(s.contains("(Cou)"), "half-typed tag salvaged in value position: {s}");
    }

    #[test]
    fn recovery_scaffolds_if_with_unparseable_body() {
        // A broken sibling must not eat the `if` next to it. The body
        // (`text { "x" }`) can't parse as a syn statement, so the whole
        // if-expr can't salvage verbatim — instead the construct is
        // scaffolded through the `__idealyst_if` TYPE DISPATCH (not a
        // plain Rust `if`: `ui!` accepts a `ReadSignal<bool>` condition,
        // which a plain `if` would E0308-squiggle on valid code), with
        // the condition analyzable in receiver position and the body
        // salvaged inside the then-closure.
        let input = quote! {
            view() {
                if frozen { text { "x" } }
                broken .
            }
        };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(
            s.contains("(frozen) . __idealyst_if"),
            "if scaffolded via type dispatch: {s}"
        );
        assert!(s.contains("\"x\""), "body content salvaged inside: {s}");
        assert!(!s.contains("if frozen"), "no plain-Rust if in the salvage: {s}");
    }

    #[test]
    fn regression_salvage_reactive_if_else_types_via_dispatch() {
        // User-hit (idealyst-test app): `if is_high { … } else { … }`
        // where `is_high: ReadSignal<bool>` — valid DSL, but the salvage
        // shell used to re-emit it as a plain Rust `if`, and
        // rust-analyzer's pull-model diagnostics squiggled the user's
        // valid code with `E0308: expected bool, found ReadSignal<bool>`.
        // The salvage must route the condition through `__idealyst_if`
        // (StaticCond for bool / ReactiveCond for signal types) so it
        // typechecks exactly when the real lowering does. Both branches'
        // content must survive inside the dispatch closures.
        let out = parse_and_emit(quote! {
            view() {
                if is_high {
                    text { "High!" }
                } else {
                    text { "Keep clicking" }
                }
                if frozen {
                    text { "never" }
                }
            }
        });
        assert!(
            out.contains("(is_high) . __idealyst_if"),
            "reactive-capable dispatch in the salvage: {out}"
        );
        assert!(out.contains("(frozen) . __idealyst_if"), "{out}");
        assert!(!out.contains("if is_high"), "no plain-Rust if on the condition: {out}");
        assert!(out.contains("\"High!\""), "{out}");
        assert!(out.contains("\"Keep clicking\""), "else branch salvaged into the dispatch: {out}");
    }

    #[test]
    fn regression_salvage_constructor_calls_not_struct_literaled() {
        // User-facing squiggle class (caught by the pull-diagnostics
        // audit): `P2(w, h)` / `Dir::North(steps)` inside a HANDLER are
        // tuple-struct/enum-variant constructor calls, not component
        // tags — struct-literal-salvaging them planted `E0560 no such
        // field` on valid code. Position decides (see `SalvageCtx`):
        // prop values and handler bodies never struct-literal-salvage,
        // while a Pascal call in CHILD position still does (that's the
        // prop-name completion path).
        let out = parse_and_emit(quote! {
            view() {
                Counter(start = 3)
                button(label = "x", on_click = move || {
                    consume(P2(w, h), Dir::North(steps));
                })
            }
        });
        assert!(!out.contains("P2 {"), "constructor call must stay a call: {out}");
        assert!(!out.contains("North {"), "variant call must stay a call: {out}");
        assert!(out.contains("P2 (w , h)"), "constructor salvaged as an expression: {out}");
        // Child-position tag still gets the struct-literal salvage (the
        // real chain emits `Counter { start: (3).into(), … }`; the salvage
        // copy adds the todo!() form — assert the salvage-specific shape).
        assert!(
            out.contains("Counter { start : :: core :: todo ! ()"),
            "child-position tag keeps prop-name completion: {out}"
        );
    }

    #[test]
    fn regression_salvage_for_loop_types_via_dispatch() {
        // Same class as the reactive-if bug: `for x in rows { … }` where
        // `rows: Signal<Vec<_>>` is valid DSL but not a valid plain Rust
        // for-loop. The salvage scaffolds it through
        // `__idealyst_for_each`, which also keeps `x` TYPED (closure
        // param inferred through the same traits the real lowering uses)
        // for completion inside the body.
        let out = parse_and_emit(quote! {
            view() {
                for x in rows {
                    text { x }
                }
            }
        });
        assert!(
            out.contains("(rows) . __idealyst_for_each (move | x |"),
            "for-loop salvaged via dispatch with the pattern as closure param: {out}"
        );
        assert!(!out.contains("for x in rows"), "no plain-Rust for in the salvage: {out}");
    }

    // ---- f-string text interpolation ----

    #[test]
    fn fstring_text_lowers_to_typed_slots() {
        let out = parse_and_emit(quote! { text { "count: {count}   doubled: {doubled}" } });
        assert!(out.contains("__idealyst_text_from_parts"), "{out}");
        assert!(out.contains("TextSlotPart :: Lit (\"count: \")"), "{out}");
        assert!(out.contains("(count) . __idealyst_text_slot"), "{out}");
        assert!(out.contains("(doubled) . __idealyst_text_slot"), "{out}");
        // Slot classification is by TYPE via the trait pair, not by any
        // token heuristic — both traits must be in scope at the slot.
        assert!(out.contains("StaticTextSlot as _"), "{out}");
        assert!(out.contains("ReactiveTextSlot as _"), "{out}");
    }

    #[test]
    fn fstring_format_spec_passes_through() {
        let out = parse_and_emit(quote! { text { "ratio: {ratio:.2}" } });
        assert!(out.contains("format ! (\"{:.2}\""), "spec forwarded to format!: {out}");
    }

    #[test]
    fn fstring_escapes_unescape_only_when_interpolating() {
        // With a slot present, `{{`/`}}` unescape…
        let out = parse_and_emit(quote! { text { "{{x}} = {count}" } });
        assert!(out.contains("TextSlotPart :: Lit (\"{x} = \")"), "{out}");
        // …but a placeholder-free literal stays VERBATIM, braces and all
        // (prose tolerance — no silent meaning change for existing text).
        let out = parse_and_emit(quote! { text { "use {{ and }} freely" } });
        assert!(!out.contains("__idealyst_text_from_parts"), "{out}");
        assert!(out.contains("\"use {{ and }} freely\""), "{out}");
        // Same for an unterminated `{` with no valid placeholder.
        let out = parse_and_emit(quote! { text { "open { brace prose" } });
        assert!(!out.contains("__idealyst_text_from_parts"), "{out}");
        assert!(!out.contains("compile_error"), "{out}");
    }

    #[test]
    fn fstring_positional_placeholder_is_error_when_interpolating() {
        // `{}` next to a real `{count}` slot = clearly meant as a format
        // string → loud error (positional needs the closure form).
        let out = parse_and_emit(quote! { text { "{} of {count}" } });
        assert!(out.contains("compile_error"), "{out}");
        // A lone `{}` with no named slot is prose-tolerated verbatim.
        let out = parse_and_emit(quote! { text { "{} items" } });
        assert!(!out.contains("compile_error"), "{out}");
        assert!(out.contains("\"{} items\""), "{out}");
    }

    #[test]
    fn fstring_debug_spec_is_error_even_alone() {
        // A valid ident with an unsupported spec signals interpolation
        // intent — must error, not silently render "{count:?}".
        let out = parse_and_emit(quote! { text { "{count:?}" } });
        assert!(out.contains("compile_error"), "{out}");
    }

    #[test]
    fn fstring_content_prop_interpolates_too() {
        let out = parse_and_emit(quote! { text(content = "hi {name}") });
        assert!(out.contains("__idealyst_text_from_parts"), "{out}");
        assert!(out.contains("(name) . __idealyst_text_slot"), "{out}");
    }

    #[test]
    fn recovery_preserves_if_let_bindings_inside_closure_props() {
        // THE dot-completion case: mid-typing `c.` inside an `if let`
        // inside an `on_click` closure inside `ui!`. The recovery must
        // rebuild the closure AND the `if let` as scaffolding so `c`
        // stays BOUND — otherwise rust-analyzer sees an unresolved
        // receiver at the cursor and completion degrades to
        // unknown-receiver trait noise (handle methods invisible until a
        // prefix is typed).
        let input = quote! {
            view() {
                button(label = "x", on_click = move || {
                    if let Some(c) = counter.get() {
                        c.bump(10);
                        c.
                    }
                })
            }
        };
        let err = parse_err(input.clone());
        let ts = emit_recovery(input, &err);
        let s = ts.to_string();
        assert!(
            s.contains("move || { let _ = & (if let Some (c) = counter . get ()"),
            "closure + if-let scaffolding preserved: {s}"
        );
        assert!(s.contains("& (c . bump (10))"), "bound statement salvaged in place: {s}");
        assert!(s.contains("& (c)"), "the cursor token survives, bound: {s}");
        // No unbound duplicates: the statements exist ONLY inside the
        // scaffold, so `c` resolves everywhere it appears.
        assert_eq!(
            s.matches("c . bump").count(),
            1,
            "no flat unbound duplicate of the body: {s}"
        );
        syn::parse2::<Expr>(ts).expect("recovery output must be a valid expression");
    }

    #[test]
    fn recovery_preserves_let_statements_verbatim() {
        // `let c = counter.get().unwrap(); c.│` — the whole valid `let`
        // must survive VERBATIM in the salvage so `c` stays bound and
        // dot-completion at the cursor has a receiver type. Wrapping it
        // as `let _ = &(…)` (or dropping it) would orphan every statement
        // after it.
        let input = quote! {
            view() {
                button(label = "x", on_click = move || {
                    let c = counter.get().unwrap();
                    c.bump(1);
                    c.
                })
            }
        };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(
            s.contains("let c = counter . get () . unwrap () ;"),
            "valid let preserved verbatim: {s}"
        );
        assert!(s.contains("& (c . bump (1))"), "later statement stays bound: {s}");
        assert!(s.contains("& (c)"), "cursor token survives, bound: {s}");
    }

    #[test]
    fn recovery_never_emits_bare_let_expressions() {
        // `if let PAT = expr` with an unsalvageable body must NOT fall
        // back to emitting `&(let PAT = expr)` — a let-expression is
        // only valid inside if/while, and the bogus-position error used
        // to squiggle the author's own pattern. The `=`-RHS is the safe
        // salvage.
        let input = quote! {
            view() {
                if let Some(c) = counter.get() { text { c } }
                broken .
            }
        };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(!s.contains("& (let"), "no bare let-expression salvage: {s}");
    }

    #[test]
    fn recovery_lowercase_primitive_call_decomposes_canonically() {
        // `text(conten` — lowercase tags are free FNS: the callee is
        // salvaged as a bare path (fn-name completion/hover) and the
        // arguments are salvaged from inside the group (`conten` keeps
        // ident completion in value position). The call is NEVER emitted
        // whole — whether its interior parses must not change the output
        // emitted before it (the prefix-stability rule in
        // [`salvage_stmts`]) — and never a struct literal (primitives
        // aren't structs).
        let input = quote! { text(conten) something . };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(s.contains("& (text)"), "callee salvaged as a bare path: {s}");
        assert!(s.contains("& (conten)"), "argument salvaged from the group: {s}");
        assert!(!s.contains("text (conten)"), "call never emitted whole: {s}");
        assert!(!s.contains("text {"), "no struct literal for lowercase tags: {s}");
    }

    #[test]
    fn recovery_braced_tag_guard_still_holds() {
        // `Card { … }` (tag with children) must NOT be salvaged as a
        // struct literal — that parse is semantically bogus. Its children
        // are still recursed individually.
        let input = quote! {
            Card { inner_value }
            broken .
        };
        let err = parse_err(input.clone());
        let s = emit_recovery(input, &err).to_string();
        assert!(!s.contains("Card {"), "braced tag must not become a struct literal: {s}");
        assert!(s.contains("inner_value"), "children still recursed: {s}");
    }

    #[test]
    fn regression_happy_and_speculative_expansions_align_at_cursor() {
        // THE mid-block glue case (user-hit): a dangling `c.` typed on a
        // line ABOVE `c.reset();` token-glues into `c.c.reset()` — VALID
        // Rust — so the buffer as written expands through the happy path,
        // while rust-analyzer's speculative buffer (placeholder ident
        // spliced at the cursor) fails to parse and expands through
        // recovery. RA resolves speculative nodes against the real
        // expansion's HIR by TEXT RANGE, so completion only works if the
        // two expansions are textually identical up to the cursor. That
        // is exactly what the shared `emit_shell` guarantees; this pins
        // it: the common prefix of the two expansion strings must extend
        // past the receiver of the half-typed member access.
        let glued = quote! {
            view() {
                Counter(start = 3, bind_to = counter)
                button(label = "x", on_click = move || {
                    if let Some(c) = counter.get() {
                        c.bump(10);
                        c.c.reset();
                    }
                })
            }
        };
        let speculative = quote! {
            view() {
                Counter(start = 3, bind_to = counter)
                button(label = "x", on_click = move || {
                    if let Some(c) = counter.get() {
                        c.bump(10);
                        c.intellijRulezz
                        c.reset();
                    }
                })
            }
        };
        let happy = {
            let ui: Ui = syn::parse2(glued.clone()).expect("glued input is valid");
            emit(ui, &glued).to_string()
        };
        let err = parse_err(speculative.clone());
        let recovered = emit_recovery(speculative, &err).to_string();

        let common: usize = happy
            .bytes()
            .zip(recovered.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let receiver_at = recovered
            .find("(c . intellijRulezz)")
            .expect("speculative salvage must keep the half-typed access")
            + "(c ".len();
        assert!(
            common >= receiver_at,
            "expansions must align past the completion receiver \
             (aligned {common} bytes, receiver at {receiver_at}):\n\
             HAPPY: {happy}\nRECOVERED: {recovered}"
        );
        // And the receiver `c` must be BOUND in both copies — the happy
        // shell salvages the whole glued statement, the recovery shell
        // scaffolds the if-let around the split statements.
        assert!(happy.contains("if let Some (c) = counter . get ()"));
        assert!(recovered.contains("if let Some (c) = counter . get ()"));
    }

    #[test]
    fn happy_path_carries_salvage_shell_under_ide_host() {
        // The salvage copy in a VALID expansion must be present when the
        // host has no real span info (IDE proc-macro servers; also
        // proc-macro2 fallback spans, which is why this test sees it) —
        // that's what keeps rust-analyzer's real/speculative expansions
        // aligned. Under rustc (real spans) the shell is skipped
        // entirely; `host_has_no_span_lines` documents why.
        let out = parse_and_emit(quote! { text { "hello" } });
        assert!(out.contains("__ui_recover"), "{out}");
        // The real chain must still be there, after the shell.
        assert!(out.contains(":: runtime_core :: text"), "{out}");
    }

    #[test]
    fn recovery_new_salvage_forms_are_valid_expressions() {
        // The invariant that makes recovery safe at all: output must
        // parse, or rust-analyzer loses the whole block.
        for input in [
            quote! { Counter(sta) },
            quote! { Counter(start = 3, la) },
            quote! { view() { count.get() Cou broken . } },
            quote! { view() { if frozen { text { "x" } } broken . } },
        ] {
            let err = parse_err(input.clone());
            let ts = emit_recovery(input, &err);
            syn::parse2::<Expr>(ts).expect("recovery output must be a valid expression");
        }
    }
}
