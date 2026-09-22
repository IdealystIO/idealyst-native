//! The `ui!` grammar and its syntax tree.
//!
//! ```text
//! ui!         := node*
//! node        := component
//!              | 'if' rust_expr '{' node* '}' ('else' if_or_block)?
//!              | 'for' pat 'in' rust_expr (',' 'key' '=' rust_expr)? '{' node* '}'
//!              | 'match' rust_expr '{' arm* '}'
//!              | rust_expr       (anything else parses as a Rust expression
//!                                 and gets passed through to ChildList)
//! component   := ident '(' prop_list? ')' children? chain*
//!              | ident children
//! prop_list   := prop (',' prop)* ','?
//! prop        := ident '=' rust_expr ('=>' rust_expr)?
//! children    := '{' node* '}'
//! chain       := '.' ident '(' args? ')'
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
//! Which of those is a PRIMITIVE and which a `#[component]` is decided
//! by [`crate::primitives::canonical_primitive`], not here: the parser
//! produces one node kind for both and the emission dispatches.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, parenthesized, Expr, Ident, Token};

/// Top-level entry: a `ui! { ... }` invocation parses to a list of elements.
pub struct Ui {
    pub elements: Vec<UiNode>,
}

/// A single node in the UI tree. Either a component invocation we parsed,
/// or a raw Rust expression that goes through ChildList passthrough.
///
/// `Clone` because the split pass ([`crate::split`]) rewrites a scope's
/// nodes (substituting hoisted slot locals) rather than mutating the
/// parsed tree in place.
#[derive(Clone)]
pub enum UiNode {
    Component {
        name: Ident,
        props: Vec<Prop>,
        children: Option<Vec<UiNode>>,
        /// Trailing `.method(args)` chains. Used to attach builder
        /// methods like `.bind(r)` to the constructed primitive
        /// without burying them in the prop list. Stored as raw
        /// token streams and appended verbatim to the emitted call.
        chain: Vec<TokenStream2>,
        /// This node's index within its site, stamped by
        /// [`crate::number::number_elements`] after parsing and carried
        /// through the split pass's clone. `0` until then.
        ///
        /// It is the number the emission tags the built `Element` with
        /// and the number the build-time descriptor calls this node, so
        /// it is on the node rather than in a counter — see
        /// [`crate::number`].
        node: u32,
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

#[derive(Clone)]
pub struct MatchArm {
    pub pat: syn::Pat,
    /// Optional `if guard` after the pattern.
    pub guard: Option<Expr>,
    pub body: Vec<UiNode>,
}

#[derive(Clone)]
pub struct Prop {
    pub name: Ident,
    pub value: Expr,
    /// Optional `=> output_signal` clause for structured actions.
    /// Set when a prop is written as `on_click = method(sig) =>
    /// out_signal` — the `=>` token follows the prop's value
    /// expression and an output signal expression follows the `=>`.
    /// `emit_button` reads this to construct a fully-populated
    /// `Action` directly (no `action!`/`bind_press!` macro needed).
    pub arrow_target: Option<Expr>,
}

/// Recognized accessibility attribute names. Each maps 1:1 to a
/// `Bound<H>` setter of the same name (`a11y_label` → `.a11y_label(..)`,
/// `accessibility` → `.accessibility(..)`, etc.), so both `ui!` and
/// `jsx!` lower them by emitting `.<name>(<value>)`. Keeping the list
/// here (the single source of truth) keeps the two macros in lockstep
/// with the `Element` accessibility surface. When a new author-facing
/// a11y field lands, add its attr name here.
pub fn is_a11y_attr(name: &str) -> bool {
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

    Ok(UiNode::Component { name, props, children, chain, node: 0 })
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
pub fn collect_component_refs(ui: &Ui, out: &mut Vec<(String, u32)>) {
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