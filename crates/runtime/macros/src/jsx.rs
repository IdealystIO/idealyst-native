//! `jsx!` proc-macro — a JSX-flavored variant of `ui!`.
//!
//! Designed to feel familiar to React developers. Same emission backend as
//! `ui!` (`BuildElement` struct-literal dispatch for components, primitive
//! free functions, ChildList passthrough), but with angle-bracket syntax
//! and a couple of JSX-isms.
//!
//! Grammar (informal):
//!
//! ```text
//! jsx!         := node*
//! node         := element
//!               | 'if' rust_expr '{' node* '}' ('else' if_or_block)?
//!               | 'for' pat 'in' rust_expr '{' node* '}'
//!               | '{' rust_expr '}'                 // child expression
//! element      := '<' Name attr* '/>'               // self-closing
//!               | '<' Name attr* '>' node* '</' Name '>'
//! attr         := ident '=' attr_value
//! attr_value   := str_lit                           // bare string literal
//!               | '{' rust_expr '}'                 // braced expression
//! ```
//!
//! ## JSX-flavored conventions
//!
//! - **Closing tags must match.** `<Card>...</Card>` is required; a mismatched
//!   close is a compile error. `</>` shorthand is not supported.
//! - **String attrs are bare**, expression attrs are braced:
//!     `title="hi"     value={signal}     on_click={move || ...}`
//! - **`ref={r}` is a special attribute**: lifted out of the prop list and
//!   emitted as `.bind(r)` on the constructed element — the same way `style`
//!   on primitives is lifted into `.with_style(...)`.
//! - **Text content** still goes through the `Text` wrapper:
//!     `<Text>"hello"</Text>` or `<Text>{format!("score: {}", n)}</Text>`.
//!   Bare strings between tags are *not* allowed (matches `ui!`).
//! - **Reactive `if`** (condition containing `.get()`) is rewritten to
//!   `when(...)`, same as `ui!`. `for` desugars to a `Vec<Element>`.
//!
//! The emitter is shared with `ui!` wherever possible — primitive
//! dispatch (`Text`/`Button`/`View`/`When`) and user-component dispatch
//! (`BuildElement::build(Foo { … })`) both go through the existing logic.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, ToTokens};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::{braced, Expr, Ident, Token};

/// Top-level entry. A `jsx! { ... }` invocation parses into a flat list of
/// elements. If there's exactly one, we emit it directly; otherwise we wrap
/// in `view(children![...])`, identical to `ui!`.
pub struct Jsx {
    elements: Vec<JsxNode>,
}

/// A single JSX node.
enum JsxNode {
    /// `<Name attr=... ref={r}>children</Name>` or `<Name />`.
    Element {
        name: Ident,
        props: Vec<Prop>,
        /// Lifted from the prop list. Maps to `.bind(...)` on the
        /// constructed value.
        ref_expr: Option<Expr>,
        /// `None` for self-closing; `Some(vec)` for `<Foo>...</Foo>` even
        /// if the children list is empty (`<Foo></Foo>`).
        children: Option<Vec<JsxNode>>,
    },
    /// `<>children</>` — a group of siblings without a container element.
    /// Emits as `Vec<Element>`; the surrounding context's `ChildList`
    /// impl flattens it (multi-child position) or it gets wrapped in
    /// `view(...)` when a single `Element` is required (if/else branch,
    /// top-level single-node).
    Fragment {
        children: Vec<JsxNode>,
    },
    /// A `for` loop whose body is itself a jsx block.
    For {
        pat: syn::Pat,
        iter: Expr,
        /// Optional `, key = EXPR` clause — required when `iter` is a
        /// reactive collection (a `Signal<Vec<_>>`); see [`emit_for`].
        key: Option<Expr>,
        body: Vec<JsxNode>,
    },
    /// An `if` / `if else` chain at the statement level. Recursively jsx'd.
    If {
        cond: Expr,
        then_body: Vec<JsxNode>,
        else_body: Option<Vec<JsxNode>>,
    },
    /// A braced Rust expression child: `{some_expr}`. Routed through
    /// `ChildList::append_to` so it can yield zero, one, or many
    /// primitives.
    Expr(Expr),
}

struct Prop {
    name: Ident,
    value: PropValue,
}

/// A prop value is either a bare string literal (`title="hi"`) or a
/// braced Rust expression (`value={signal}`). String literals get the
/// implicit `.into()` for user components, mirroring `ui!`'s coercion.
enum PropValue {
    Str(syn::LitStr),
    Expr(Expr),
}

impl Parse for Jsx {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let elements = parse_nodes(input)?;
        Ok(Jsx { elements })
    }
}

fn parse_nodes(input: ParseStream) -> syn::Result<Vec<JsxNode>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        // Stop at a closing tag — the caller (an element-children parser)
        // handles `</Name>`.
        if is_close_tag_ahead(input) {
            break;
        }
        out.push(parse_node(input)?);
    }
    Ok(out)
}

fn parse_node(input: ParseStream) -> syn::Result<JsxNode> {
    // Statement-level control flow.
    if input.peek(Token![if]) {
        return parse_if(input);
    }
    if input.peek(Token![for]) {
        return parse_for(input);
    }
    // `<Name ...>` — an element.
    if input.peek(Token![<]) {
        return parse_element(input);
    }
    // `{expr}` — a braced child expression. Passed through ChildList.
    if input.peek(syn::token::Brace) {
        let content;
        braced!(content in input);
        let expr: Expr = content.parse()?;
        if !content.is_empty() {
            return Err(content.error("expected a single Rust expression inside `{...}`"));
        }
        return Ok(JsxNode::Expr(expr));
    }
    // Bare string literal — only meaningful as `Text`'s single child:
    //     <Text>"hello"</Text>
    // We accept it anywhere a child is allowed; the parent element decides
    // what to do with it. Text's emitter routes it through `text(expr)`.
    if input.peek(syn::LitStr) {
        let s: syn::LitStr = input.parse()?;
        let expr = Expr::Lit(syn::ExprLit { attrs: Vec::new(), lit: syn::Lit::Str(s) });
        return Ok(JsxNode::Expr(expr));
    }
    Err(input.error(
        "expected a JSX element (`<Name ...>`), `{expr}`, a string literal, or `if`/`for`",
    ))
}

/// Detects `</...>` without consuming. The element-children parser uses
/// this to stop before reading into the parent's closing tag.
fn is_close_tag_ahead(input: ParseStream) -> bool {
    let fork = input.fork();
    if fork.parse::<Token![<]>().is_err() {
        return false;
    }
    fork.peek(Token![/])
}

fn parse_element(input: ParseStream) -> syn::Result<JsxNode> {
    let _lt: Token![<] = input.parse()?;

    // Fragment open: `<>` — no name, no attrs. Emits a Vec<Element> so
    // siblings flatten into the parent's child list without an extra
    // container element.
    if input.peek(Token![>]) {
        let _gt: Token![>] = input.parse()?;
        let children = parse_nodes(input)?;
        // Closing `</>`.
        let _lt: Token![<] = input.parse()?;
        let _slash: Token![/] = input.parse()?;
        let _gt: Token![>] = input.parse()?;
        return Ok(JsxNode::Fragment { children });
    }

    let name: Ident = input.parse()?;

    // Parse attributes until we hit `/>` (self-close) or `>` (open).
    let mut props: Vec<Prop> = Vec::new();
    let mut ref_expr: Option<Expr> = None;

    loop {
        if input.peek(Token![/]) && input.peek2(Token![>]) {
            let _slash: Token![/] = input.parse()?;
            let _gt: Token![>] = input.parse()?;
            return Ok(JsxNode::Element {
                name,
                props,
                ref_expr,
                children: None,
            });
        }
        if input.peek(Token![>]) {
            let _gt: Token![>] = input.parse()?;
            break;
        }
        // Otherwise: an attribute. Use `parse_any` so keywords like `ref`
        // are accepted as attribute names — `ref` is a Rust keyword, but
        // we want JSX users to be able to write `ref={r}` naturally.
        let attr_name: Ident = Ident::parse_any(input)?;
        let _eq: Token![=] = input.parse()?;
        let value = parse_prop_value(input)?;

        // `ref` is special — lift it out of the prop list. Only braced
        // expressions are meaningful here (`ref={my_ref}`); we accept the
        // string-literal form too rather than error, since the error is
        // less helpful than letting the typechecker complain.
        if attr_name == "ref" {
            let expr = match value {
                PropValue::Expr(e) => e,
                PropValue::Str(s) => {
                    let lit = syn::Expr::Lit(syn::ExprLit {
                        attrs: Vec::new(),
                        lit: syn::Lit::Str(s),
                    });
                    lit
                }
            };
            if ref_expr.is_some() {
                return Err(syn::Error::new(attr_name.span(), "duplicate `ref` attribute"));
            }
            ref_expr = Some(expr);
        } else {
            props.push(Prop { name: attr_name, value });
        }
    }

    // We've consumed the opening `>`. Now children until `</Name>`.
    let children = parse_nodes(input)?;

    // Closing tag: `</Name>`, must match.
    let _lt: Token![<] = input.parse()?;
    let _slash: Token![/] = input.parse()?;
    let close_name: Ident = input.parse()?;
    let _gt: Token![>] = input.parse()?;
    if close_name != name {
        return Err(syn::Error::new(
            close_name.span(),
            format!(
                "mismatched closing tag: expected `</{}>`, got `</{}>`",
                name, close_name
            ),
        ));
    }

    Ok(JsxNode::Element {
        name,
        props,
        ref_expr,
        children: Some(children),
    })
}

fn parse_prop_value(input: ParseStream) -> syn::Result<PropValue> {
    // Bare string literal — JSX's `attr="foo"`.
    if input.peek(syn::LitStr) {
        let s: syn::LitStr = input.parse()?;
        return Ok(PropValue::Str(s));
    }
    // Braced expression — JSX's `attr={expr}`.
    if input.peek(syn::token::Brace) {
        let content;
        braced!(content in input);
        let expr: Expr = content.parse()?;
        if !content.is_empty() {
            return Err(content.error("expected a single Rust expression inside `{...}`"));
        }
        return Ok(PropValue::Expr(expr));
    }
    Err(input.error("expected a string literal (`\"...\"`) or a braced expression (`{...}`)"))
}

fn parse_if(input: ParseStream) -> syn::Result<JsxNode> {
    let _if: Token![if] = input.parse()?;
    let cond: Expr = Expr::parse_without_eager_brace(input)?;
    let then_content;
    braced!(then_content in input);
    let then_body = parse_nodes(&then_content)?;

    let else_body = if input.peek(Token![else]) {
        let _: Token![else] = input.parse()?;
        if input.peek(Token![if]) {
            Some(vec![parse_if(input)?])
        } else {
            let else_content;
            braced!(else_content in input);
            Some(parse_nodes(&else_content)?)
        }
    } else {
        None
    };

    Ok(JsxNode::If { cond, then_body, else_body })
}

fn parse_for(input: ParseStream) -> syn::Result<JsxNode> {
    let _for: Token![for] = input.parse()?;
    let pat = syn::Pat::parse_single(input)?;
    let _in: Token![in] = input.parse()?;
    let iter: Expr = Expr::parse_without_eager_brace(input)?;
    // Optional `, key = EXPR` reconciliation key (see `ui::parse_for`).
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
    let body = parse_nodes(&body_content)?;
    Ok(JsxNode::For { pat, iter, key, body })
}

// =============================================================================
// MCP catalog: composes-edge extraction
// =============================================================================

/// Walk a parsed `jsx! { ... }` and append every element-name ident
/// (`<Name ...>`) into `out`. Fragments (`<>...</>`) have no name and
/// are skipped — only their children are walked. Recurses into nested
/// element children, for/if bodies. Braced-expression children
/// (`{expr}`) are NOT captured.
///
/// Mirrors `ui::collect_component_refs`; see that doc-comment for why
/// we only capture JSX-position idents.
#[cfg(feature = "mcp")]
pub(crate) fn collect_component_refs(jsx: &Jsx, out: &mut Vec<(String, u32)>) {
    collect_from_nodes(&jsx.elements, out);
}

#[cfg(feature = "mcp")]
fn collect_from_nodes(nodes: &[JsxNode], out: &mut Vec<(String, u32)>) {
    for node in nodes {
        match node {
            JsxNode::Element { name, children, .. } => {
                let line = name.span().start().line as u32;
                out.push((name.to_string(), line));
                if let Some(c) = children {
                    collect_from_nodes(c, out);
                }
            }
            JsxNode::Fragment { children } => collect_from_nodes(children, out),
            JsxNode::For { body, .. } => collect_from_nodes(body, out),
            JsxNode::If { then_body, else_body, .. } => {
                collect_from_nodes(then_body, out);
                if let Some(e) = else_body {
                    collect_from_nodes(e, out);
                }
            }
            JsxNode::Expr(_) => {}
        }
    }
}

// =============================================================================
// Emit
// =============================================================================

pub fn emit(jsx: Jsx) -> TokenStream2 {
    let body = match jsx.elements.len() {
        0 => quote! { ::runtime_core::view(::std::vec::Vec::new()) },
        // Single top-level node — but a fragment at this position has
        // to be wrapped in a view, because the macro contract is to
        // return a `Element`, not a `Vec<Element>`.
        1 => match &jsx.elements[0] {
            JsxNode::Fragment { children } => emit_fragment_as_view(children),
            n => emit_node(n),
        },
        _ => {
            let kids = jsx.elements.iter().map(emit_node);
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
    quote! { ::runtime_core::IntoElement::into_element(#body) }
}

/// Wraps a fragment's children in `view(...)` so it can stand in where a
/// single `Element` is required (top-level single-node, if/else branch).
/// In multi-child positions this isn't needed — the bare `Vec<Element>`
/// emission flows through `ChildList::append_to` and flattens inline.
fn emit_fragment_as_view(children: &[JsxNode]) -> TokenStream2 {
    let parts = children.iter().map(emit_node);
    quote! {
        ::runtime_core::view({
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        })
    }
}

fn emit_node(node: &JsxNode) -> TokenStream2 {
    match node {
        JsxNode::Element { name, props, ref_expr, children } => {
            emit_element(name, props, ref_expr.as_ref(), children.as_deref())
        }
        JsxNode::Fragment { children } => emit_fragment_as_vec(children),
        JsxNode::If { cond, then_body, else_body } => {
            emit_if(cond, then_body, else_body.as_deref())
        }
        JsxNode::For { pat, iter, key, body } => emit_for(pat, iter, key.as_ref(), body),
        JsxNode::Expr(e) => e.to_token_stream(),
    }
}

/// Emits a fragment as a bare `Vec<Element>`. Used in child-list
/// positions where `ChildList::append_to` will flatten the Vec inline,
/// achieving the "no wrapper container" behavior fragments promise.
fn emit_fragment_as_vec(children: &[JsxNode]) -> TokenStream2 {
    let parts = children.iter().map(emit_node);
    quote! {
        {
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        }
    }
}

/// Returns the tokens for a prop value verbatim (no `.into()`). The
/// caller adds coercion where needed — `emit_user` wraps each field as
/// `(value).into()` in the props struct literal; primitives coerce in
/// their own emitters.
fn emit_attr_value_raw(value: &PropValue) -> TokenStream2 {
    match value {
        PropValue::Str(s) => quote! { #s },
        PropValue::Expr(e) => quote! { #e },
    }
}

fn emit_element(
    name: &Ident,
    props: &[Prop],
    ref_expr: Option<&Expr>,
    children: Option<&[JsxNode]>,
) -> TokenStream2 {
    // Primitives are canonicalized to snake_case (parity with `ui!`).
    // Both `<View>` and `<view>` are accepted during the migration window.
    let name_str = name.to_string();
    let canonical = crate::primitives::canonical_primitive(&name_str);
    let is_primitive = canonical.is_some();

    // Same trick as `ui!`: pull `style` out of the prop list for primitives
    // and emit `.with_style(...)`. User components pass `style = ...` as an
    // ordinary field in their props struct literal.
    let (style_value, other_props): (Option<&PropValue>, Vec<&Prop>) = if is_primitive {
        let mut style = None;
        let mut rest = Vec::with_capacity(props.len());
        for p in props {
            if p.name == "style" && style.is_none() {
                style = Some(&p.value);
            } else {
                rest.push(p);
            }
        }
        (style, rest)
    } else {
        (None, props.iter().collect())
    };

    let inner = match canonical {
        Some("text") => emit_text(&other_props, children),
        Some("button") => emit_button(&other_props, children),
        Some("view") => emit_view(&other_props, children),
        Some("when") => emit_when(&other_props, children),
        _ => emit_user(name, props, children),
    };

    let with_style = if let Some(v) = style_value {
        let val = emit_attr_value_raw(v);
        quote! { (#inner).with_style(#val) }
    } else {
        inner
    };

    if let Some(r) = ref_expr {
        quote! { (#with_style).bind(#r) }
    } else {
        with_style
    }
}

fn emit_text(props: &[&Prop], children: Option<&[JsxNode]>) -> TokenStream2 {
    if let Some(kids) = children {
        match kids.len() {
            0 => quote! { ::runtime_core::text("") },
            1 => {
                let e = emit_node(&kids[0]);
                quote! { ::runtime_core::text(#e) }
            }
            _ => {
                let parts = kids.iter().map(|k| emit_node(k));
                quote! {
                    ::runtime_core::text({
                        let mut __s = ::std::string::String::new();
                        #( __s.push_str(&::std::string::ToString::to_string(&#parts)); )*
                        __s
                    })
                }
            }
        }
    } else if let Some(p) = props.iter().find(|p| p.name == "content") {
        let v = emit_attr_value_raw(&p.value);
        quote! { ::runtime_core::text(#v) }
    } else {
        quote! { ::runtime_core::text("") }
    }
}

fn emit_button(props: &[&Prop], _children: Option<&[JsxNode]>) -> TokenStream2 {
    let label = props
        .iter()
        .find(|p| p.name == "label")
        .map(|p| emit_attr_value_raw(&p.value))
        .unwrap_or_else(|| quote! { "" });
    let on_click = props
        .iter()
        .find(|p| p.name == "on_click")
        .map(|p| emit_attr_value_raw(&p.value))
        .unwrap_or_else(|| quote! { || {} });
    quote! { ::runtime_core::button(#label, #on_click) }
}

fn emit_view(_props: &[&Prop], children: Option<&[JsxNode]>) -> TokenStream2 {
    let kids = children.unwrap_or(&[]);
    let parts = kids.iter().map(emit_node);
    quote! {
        ::runtime_core::view({
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        })
    }
}

fn emit_when(props: &[&Prop], _children: Option<&[JsxNode]>) -> TokenStream2 {
    let cond = props
        .iter()
        .find(|p| p.name == "cond")
        .map(|p| emit_attr_value_raw(&p.value))
        .unwrap_or_else(|| quote! { || false });
    let then_e = props
        .iter()
        .find(|p| p.name == "then")
        .map(|p| emit_attr_value_raw(&p.value))
        .unwrap_or_else(|| quote! { || ::runtime_core::view(::std::vec::Vec::new()) });
    let other = props
        .iter()
        .find(|p| p.name == "otherwise")
        .map(|p| emit_attr_value_raw(&p.value))
        .unwrap_or_else(|| quote! { || ::runtime_core::view(::std::vec::Vec::new()) });
    quote! { ::runtime_core::when(#cond, #then_e, #other) }
}

fn emit_user(name: &Ident, props: &[Prop], children: Option<&[JsxNode]>) -> TokenStream2 {
    // Tag `Foo` dispatches via the `BuildElement` trait — a plain struct
    // literal plus a UFCS `build` call, no per-component macro (parity
    // with `ui!`; see `ui::emit_user` for the rationale). The tag is used
    // as the type name (a `pub type Foo = FooProps` alias bridges to the
    // real props), so `use …::Foo` imports keep working unchanged.
    let props_ty = name;

    let field_assignments = props.iter().map(|p| {
        let n = &p.name;
        let v = emit_attr_value_raw(&p.value);
        quote! { #n: (#v).into(), }
    });

    let children_field = children.map(|kids| {
        let parts = kids.iter().map(emit_node);
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

fn emit_if(cond: &Expr, then_body: &[JsxNode], else_body: Option<&[JsxNode]>) -> TokenStream2 {
    let then_expr = emit_block_as_primitive(then_body);
    let else_expr = match else_body {
        Some(body) => emit_block_as_primitive(body),
        None => quote! { ::runtime_core::view(::std::vec::Vec::new()) },
    };

    if condition_is_reactive(cond) {
        quote! {
            ::runtime_core::when(
                move || #cond,
                move || #then_expr,
                move || #else_expr,
            )
        }
    } else {
        quote! {
            if #cond { #then_expr } else { #else_expr }
        }
    }
}

fn condition_is_reactive(cond: &Expr) -> bool {
    let tokens = cond.to_token_stream().to_string();
    tokens.contains(".get()") || tokens.contains(". get ()")
}

/// Lower a `jsx!` `for` to the same type-driven dispatch `ui!` uses, so
/// reactivity is decided by the iterable's *type* (a `Signal<C>` →
/// keyed reactive `Element::Each`; any other `IntoIterator` → a flat,
/// built-once `Vec<Element>`). Each iteration contributes ONE element
/// (the body, view-wrapped if multi-node — `jsx!`'s existing per-row
/// semantics, unlike `ui!`'s sibling flattening).
///
/// Key handling matches `ui!`: with `, key = …` we emit the KEYED method
/// (both impls provide it); without one we emit the keyless method —
/// which only `StaticForEach` really has, so a keyless `for x in vec`
/// compiles while a keyless `for x in signal` is a COMPILE ERROR carrying
/// the `ReactiveListKeyed` diagnostic.
///
/// (Reactive *count ranges* — `for i in 0..n.get()` — are a `ui!`-only
/// nicety; in `jsx!` a range is `IntoIterator`, so it stays static, same
/// as before.)
fn emit_for(pat: &syn::Pat, iter: &Expr, key: Option<&Expr>, body: &[JsxNode]) -> TokenStream2 {
    let body_expr = emit_block_as_primitive(body);
    if let Some(k) = key {
        quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::{StaticForEach as _, ReactiveForEach as _};
                (#iter).__idealyst_for_each_keyed(
                    move |#pat| #k,
                    move |#pat| {
                        let mut __row: ::std::vec::Vec<::runtime_core::Element>
                            = ::std::vec::Vec::new();
                        ::runtime_core::ChildList::append_to(#body_expr, &mut __row);
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
                    ::runtime_core::ChildList::append_to(#body_expr, &mut __row);
                    __row
                })
            }
        }
    }
}

fn emit_block_as_primitive(nodes: &[JsxNode]) -> TokenStream2 {
    let body = match nodes.len() {
        0 => quote! { ::runtime_core::view(::std::vec::Vec::new()) },
        // A single fragment in a branch position still needs a view
        // wrapper — `when(...)`'s closures return `Element`, not Vec.
        1 => match &nodes[0] {
            JsxNode::Fragment { children } => emit_fragment_as_view(children),
            n => emit_node(n),
        },
        _ => {
            let parts = nodes.iter().map(emit_node);
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

#[allow(dead_code)]
fn _unused(_: Span) {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_emit(input: TokenStream2) -> String {
        let jsx: Jsx = syn::parse2(input).expect("parse jsx");
        emit(jsx).to_string()
    }

    fn parse_err(input: TokenStream2) -> String {
        match syn::parse2::<Jsx>(input) {
            Ok(j) => panic!("expected parse error, got: {}", emit(j)),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn empty_jsx_emits_empty_view() {
        let out = parse_and_emit(quote! {});
        assert!(out.contains("view"));
        assert!(out.contains("Vec :: new"));
    }

    #[test]
    fn self_closing_text_with_content_attr() {
        let out = parse_and_emit(quote! { <Text content="hi" /> });
        assert!(out.contains(":: runtime_core :: text"));
        assert!(out.contains("\"hi\""));
    }

    #[test]
    fn text_with_string_child() {
        let out = parse_and_emit(quote! { <Text>{"hello"}</Text> });
        assert!(out.contains(":: runtime_core :: text"));
        assert!(out.contains("\"hello\""));
    }

    #[test]
    fn user_component_self_closing() {
        let out = parse_and_emit(quote! { <Counter label="x" value={score} /> });
        // Struct-literal + BuildElement::build dispatch (parity with ui!).
        assert!(out.contains("Counter {"), "got: {out}");
        assert!(out.contains("BuildElement :: build"), "got: {out}");
        // Each field is coerced via `.into()` (literal and braced expr).
        assert!(out.contains("(\"x\") . into ()"), "got: {out}");
        assert!(out.contains("(score) . into ()"), "got: {out}");
    }

    #[test]
    fn user_component_with_children() {
        let out = parse_and_emit(quote! {
            <Card title="T">
                <Counter value={s} />
            </Card>
        });
        assert!(out.contains("Card {"), "got: {out}");
        assert!(out.contains("children :"), "got: {out}");
        assert!(out.contains("Counter {"), "got: {out}");
    }

    #[test]
    fn ref_attr_emits_bind_call() {
        let out = parse_and_emit(quote! {
            <Button label="Click" on_click={f} ref={my_ref} />
        });
        assert!(out.contains(":: runtime_core :: button"));
        assert!(out.contains(". bind (my_ref)"));
    }

    #[test]
    fn ref_attr_on_user_component() {
        let out = parse_and_emit(quote! {
            <Counter value={s} ref={r} />
        });
        assert!(out.contains("Counter {"), "got: {out}");
        assert!(out.contains(". bind (r)"), "got: {out}");
    }

    #[test]
    fn mismatched_closing_tag_is_error() {
        let err = parse_err(quote! { <Card></Counter> });
        assert!(err.contains("mismatched closing tag"), "got: {err}");
    }

    #[test]
    fn reactive_if_rewrites_to_when() {
        let out = parse_and_emit(quote! {
            if flag.get() {
                <Text content="on" />
            } else {
                <Text content="off" />
            }
        });
        assert!(out.contains(":: runtime_core :: when"));
        assert!(out.contains("move ||"));
    }

    #[test]
    fn non_reactive_if_emits_plain_if() {
        let out = parse_and_emit(quote! {
            if some_bool {
                <Text content="on" />
            } else {
                <Text content="off" />
            }
        });
        assert!(!out.contains(":: runtime_core :: when"));
        assert!(out.contains("if some_bool"));
    }

    #[test]
    fn for_loop_emits_type_driven_dispatch() {
        let out = parse_and_emit(quote! {
            for n in items {
                <Text content="x" />
            }
        });
        // Keyless `for` lowers to the type-driven `__idealyst_for_each`
        // dispatch (StaticForEach / ReactiveForEach), not a literal `for`.
        assert!(out.contains("__idealyst_for_each"));
        assert!(!out.contains("__idealyst_for_each_keyed"));
        assert!(out.contains("ChildList :: append_to"));
    }

    #[test]
    fn for_loop_with_key_emits_keyed_dispatch() {
        let out = parse_and_emit(quote! {
            for n in items, key = n.id {
                <Text content="x" />
            }
        });
        assert!(out.contains("__idealyst_for_each_keyed"));
        assert!(out.contains("n . id"));
    }

    #[test]
    fn braced_child_expr_passes_through() {
        let out = parse_and_emit(quote! {
            <View>
                {existing_primitive}
            </View>
        });
        assert!(out.contains("existing_primitive"));
        assert!(out.contains("ChildList :: append_to"));
    }

    #[test]
    fn style_attr_on_primitive_emits_with_style() {
        let out = parse_and_emit(quote! {
            <Text style={banner_style()}>{"hi"}</Text>
        });
        assert!(out.contains(". with_style (banner_style"));
    }

    #[test]
    fn multiple_top_level_elements_wrap_in_view() {
        let out = parse_and_emit(quote! {
            <Text content="a" />
            <Text content="b" />
        });
        assert!(out.contains(":: runtime_core :: view"));
        let count = out.matches(":: runtime_core :: text").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn explicit_empty_children_block_works() {
        // <Foo></Foo> should be equivalent to <Foo />, just with an empty
        // children list emitted.
        let out = parse_and_emit(quote! { <View></View> });
        assert!(out.contains(":: runtime_core :: view"));
    }

    #[test]
    fn duplicate_ref_is_error() {
        let err = parse_err(quote! { <Button ref={a} ref={b} /> });
        assert!(err.contains("duplicate `ref`"), "got: {err}");
    }

    #[test]
    fn fragment_as_top_level_wraps_in_view() {
        // A bare fragment at the macro root has to become a Element,
        // so it wraps in view(). The two text children flow through
        // ChildList::append_to.
        let out = parse_and_emit(quote! {
            <>
                <Text content="a" />
                <Text content="b" />
            </>
        });
        assert!(out.contains(":: runtime_core :: view"));
        let count = out.matches(":: runtime_core :: text").count();
        assert_eq!(count, 2);
    }

    #[test]
    fn fragment_inside_element_flattens_via_childlist() {
        // <View><><Text/><Text/></></View> — the fragment emits a
        // Vec<Element> that ChildList::append_to extends into the
        // view's children inline. No nested view container.
        let out = parse_and_emit(quote! {
            <View>
                <>
                    <Text content="a" />
                    <Text content="b" />
                </>
            </View>
        });
        // Outer view emission appears once; no inner view wrapping
        // the fragment.
        let view_count = out.matches(":: runtime_core :: view").count();
        assert_eq!(view_count, 1, "expected exactly one view emission, got: {out}");
        let text_count = out.matches(":: runtime_core :: text").count();
        assert_eq!(text_count, 2);
    }

    #[test]
    fn fragment_in_if_branch_wraps_in_view() {
        // `when(...)`'s branch closures return `Element`, so a fragment
        // used as a branch body needs the view wrapper.
        let out = parse_and_emit(quote! {
            if flag.get() {
                <>
                    <Text content="a" />
                    <Text content="b" />
                </>
            } else {
                <Text content="off" />
            }
        });
        assert!(out.contains(":: runtime_core :: when"));
        // Both branches should reach a view + text, but the key thing is
        // the fragment branch gets wrapped — i.e. there's a view call
        // for the fragment side, not just a bare Vec.
        let view_count = out.matches(":: runtime_core :: view").count();
        assert!(view_count >= 1, "expected at least one view from fragment wrap, got: {out}");
    }

    #[test]
    fn empty_fragment_is_valid() {
        // `<></>` — degenerate but should parse. Empty Vec, wraps to
        // an empty view at top level.
        let out = parse_and_emit(quote! { <></> });
        assert!(out.contains(":: runtime_core :: view"));
    }

    #[test]
    fn fragment_with_mismatched_close_does_not_match_name() {
        // `<>` opens a fragment; `</Foo>` is *not* its close — the
        // children parser keeps going past `</>` only. A `</Foo>` after
        // `<>` should be unmatched. We expect a parse error.
        let err = parse_err(quote! { <><Text content="a" /></Foo> });
        // The exact message isn't load-bearing — just confirm it errors.
        assert!(!err.is_empty());
    }

    #[test]
    fn user_component_with_fragment_child() {
        // <Card><>...</></Card> — fragment flattens into the user
        // component's children prop list.
        let out = parse_and_emit(quote! {
            <Card title="t">
                <>
                    <Counter value={s} />
                    <Counter value={t} />
                </>
            </Card>
        });
        assert!(out.contains("Card {"), "got: {out}");
        assert!(out.contains("(s) . into ()"), "got: {out}");
        assert!(out.contains("(t) . into ()"), "got: {out}");
        assert_eq!(out.matches("Counter {").count(), 2, "got: {out}");
    }
}
