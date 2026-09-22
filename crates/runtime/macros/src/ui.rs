//! `ui!` EMISSION — the parsed tree into builder calls.
//!
//! The grammar, the syntax tree, the split pass, node numbering and the
//! IDE-recovery shell all live in `runtime-macros-parse`, a plain
//! library, because the CLI needs the same answers at build time and
//! cannot depend on a proc-macro crate. This module is what is left: the
//! part that can only run inside rustc.
//!
//! ## Two halves, one decision each
//!
//! | question | answered in |
//! |---|---|
//! | what did the author write? | `runtime_macros_parse::ast` |
//! | which of it is data and which is code? | `runtime_macros_parse::split` |
//! | which node is node 7? | `runtime_macros_parse::number` |
//! | what tokens build it? | here |
//!
//! ## Template scopes
//!
//! A *template scope* is one Rust scope's worth of nodes: the `ui!`
//! body, and every body the emission puts inside a fresh Rust scope —
//! an `if`/`match` branch ([`emit_block_as_primitive`],
//! [`emit_child_scope`]), a `for` row builder, a `presence` child thunk.
//! A `view`'s (or component's) children are NOT a new scope: they are
//! built inline in the parent's block, so they share its slot list and
//! its prelude. The split pass decides those boundaries
//! (`runtime_macros_parse::split::children_kind`) and the emission honours them; the
//! build-time descriptor producer reads the same function, which is why
//! both see the same hoisted locals.
//!
//! ## Primitives vs components
//!
//! Framework primitives are a fixed snake_case set
//! (`runtime_macros_parse::primitives::canonical_primitive`); every other tag is a `#[component]`
//! dispatched to its own `Name!` macro, so import renames, qualified
//! paths and IDE navigation all work.

use runtime_macros_parse::ast::{is_a11y_attr, MatchArm, Prop, Ui, UiNode};
use runtime_macros_parse::split as ui_split;
use runtime_macros_parse::reactive_shape::is_reactive_call_shape;
use runtime_macros_parse::recovery::emit_shell;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, ToTokens};
use syn::{Expr, Ident};


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
pub(crate) enum Ctx {
    Child,
    Single,
}

pub fn emit(mut ui: Ui, input: &TokenStream2) -> TokenStream2 {
    ui_split::reset_slot_counter();
    // Stamp every node with its index within this site, BEFORE the split
    // pass clones the tree — the clone carries the stamps, and that is
    // what ties the `Element` a node builds to the node the build-time
    // descriptor calls by the same number. Unconditional: it is a walk
    // over already-parsed data, it emits nothing, and having the tree
    // numbered whether or not the feature is on means the two states
    // cannot diverge in the one place divergence would be silent.
    runtime_macros_parse::number_elements(&mut ui.elements);
    // Under `ui-overlay`, keys this site off its call span. A no-op
    // otherwise, and nothing is prepended to the body either way — a
    // tagged site adds per-NODE calls and nothing per site. See
    // `ui_overlay`'s module docs.
    crate::ui_overlay::begin_site();
    let body = emit_root_scope(&ui.elements);
    emit_shell(input, body)
}

/// Emit the `ui!` body's TOP-LEVEL scope: split it, evaluate the prelude,
/// then build.
fn emit_root_scope(elements: &[UiNode]) -> TokenStream2 {
    let scope = ui_split::split(elements);
    let body = match scope.nodes.len() {
        0 => quote! { ::runtime_core::view(::std::vec::Vec::new()) },
        // Sole element: it is coerced to one `Element` below, so emit
        // it in single-slot context.
        1 => emit_node(&scope.nodes[0], Ctx::Single),
        _ => {
            let kids = scope.nodes.iter().map(|n| emit_node(n, Ctx::Child));
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
    with_prelude(&scope, body)
}

/// Wrap `body` in its scope's slot prelude, or return it unchanged when
/// the scope hoisted nothing (so a fully-static tree's emission is
/// byte-identical to the pre-slot-rewrite output).
pub(crate) fn with_prelude(scope: &ui_split::Scope, body: TokenStream2) -> TokenStream2 {
    let prelude = scope.prelude_for(&body);
    if prelude.is_empty() {
        body
    } else {
        quote! { { #prelude #body } }
    }
}

/// Emit `nodes` as a FRESH template scope yielding a `Vec<Element>`.
///
/// Every construct whose body the emission puts inside its own Rust
/// scope — an `if`/`match` branch, a `for` row builder — routes its body
/// through here, so that body's dynamic expressions are hoisted where
/// the body runs (per branch activation, per row) rather than at the
/// enclosing scope's head. That is also what the direct lowering has
/// always done, since the author's expressions live inside the
/// branch/row closure.
pub(crate) fn emit_child_scope(nodes: &[UiNode]) -> TokenStream2 {
    let scope = ui_split::split(nodes);
    let parts: Vec<TokenStream2> =
        scope.nodes.iter().map(|n| emit_node(n, Ctx::Child)).collect();
    let body = quote! {
        {
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        }
    };
    with_prelude(&scope, body)
}


pub(crate) fn emit_node(node: &UiNode, ctx: Ctx) -> TokenStream2 {
    match node {
        UiNode::Component { name, props, children, chain, node } => {
            emit_component(name, props, children.as_deref(), chain, *node)
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
    // This node's index within its site, stamped on the parsed tree by
    // `runtime_macros_parse::number` — NOT a count of emissions. See
    // that module for why the difference matters.
    node: u32,
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

    // `new-core` deferrals: primitives whose subsystems haven't migrated
    // fail loudly with their migration phase (repo rule: no silent scope
    // cuts). Everything else lowers through the vocabulary glue —
    // including `overlay` / `anchored_overlay` / `presence` / `graphics`
    // / `flat_list`, whose glue wrappers
    // (`glue::primitives::{overlay,presence,graphics,flat_list}`) landed
    // with the P3-set handlers; their emissions below retarget unchanged.
    // (`web_view` left the deferral list with the P6 External-SDK wave:
    // it was never a first-party primitive — the WebView SDK now ships
    // the `WebView` tag contract itself, so both cores emit ordinary
    // component dispatch. See `canonical_primitive`'s note.)
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
                // Robot/automation anchor. Lowers to the always-present
                // `.test_id(…)` builder on both cores (never depends on the
                // `robot` feature being on at macro-expansion time). Without
                // this, `view(test_id = …)` silently dropped the id.
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
        _ => emit_user(name, props, children, node),
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

    // Robot/automation `test_id`. The `.test_id` builder is always
    // present on BOTH cores (it just stores the id; only the registry
    // that reads it is `robot`-gated), so one emission serves both
    // lowerings: old core = `Bound::test_id` → `Element::with_test_id`;
    // new core = the same tokens land on the glue wrapper's `.test_id`
    // forwarding into the vocabulary prim's identity slot, which the
    // mount handlers register into `runtime_vocabulary::robot` (the P5
    // identity seam, un-deferred).
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
    let built = if chain.is_empty() {
        with_a11y
    } else {
        quote! { (#with_a11y) #(#chain)* }
    };

    crate::ui_overlay::tag(built, node)
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
///   ALSO loud, even with no valid slot co-occurring, when a brace group
///   carries unambiguous interpolation intent — a binding followed by a
///   `.field` / `[index]` / `(call)` continuation (`{item.name}`,
///   `{v[0]}`, `{x.y()}`). Those can only be a broken attempt to
///   interpolate an expression, so they must not silently render verbatim
///   (see [`fstring_interp_intent`]).
fn parse_fstring(value: &str) -> Result<Option<Vec<FPiece>>, String> {
    let mut pieces: Vec<FPiece> = vec![FPiece::Lit(String::new())];
    let mut first_err: Option<String> = None;
    // Set when a brace group shows clear interpolation intent (a
    // path/index/call on a binding). Forces the `Err` path even when no
    // VALID `{ident}` slot co-occurs — closing the silent-verbatim gap
    // that let `text { "{item.name}" }` render the literal `{item.name}`.
    let mut force_err = false;
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
                    if fstring_interp_intent(name) {
                        // A binding + `.`/`[`/`(` continuation is
                        // unambiguous interpolation intent, not prose.
                        // Force the loud error even when no valid slot
                        // co-occurs, so the footgun can't build clean.
                        force_err = true;
                        if first_err.is_none() {
                            first_err = Some(format!(
                                "`{{{inner}}}` — field paths, indexing, and method calls \
                                 aren't interpolated in text f-strings (only a bare \
                                 `{{name}}` is). Use a reactive closure \
                                 `text {{ move || format!(\"{{}}\", {inner}) }}`, or bind a \
                                 local first: `let name = {inner}; text {{ \"{{name}}\" }}`."
                            ));
                        }
                    } else if first_err.is_none() {
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
    // A clear-interpolation-intent brace group errors LOUD regardless of
    // whether a valid slot co-occurred — it is never prose. Checked before
    // the `slots == 0` prose short-circuit so `text { "{item.name}" }`
    // fails to compile instead of rendering the literal `{item.name}`.
    if force_err {
        if let Some(e) = first_err {
            return Err(e);
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

/// Does a brace group's pre-spec content show unambiguous interpolation
/// intent — a binding (`ident`) immediately followed by a `.field`,
/// `[index]`, or `(call)` continuation? `item.name`, `v[0]`, `x.y()`,
/// `a.b.c` are all yes; prose like `a b`, ` `, `see the {x` are no
/// (there is no path/index/call continuation right after the leading
/// identifier). Used to make the footgun loud instead of silently
/// verbatim while leaving genuine literal-brace prose untouched.
fn fstring_interp_intent(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c0) if c0.is_alphabetic() || c0 == '_' => {}
        _ => return false,
    }
    // Consume the rest of the leading identifier, then inspect the first
    // non-identifier char that follows it.
    for c in chars {
        if c.is_alphanumeric() || c == '_' {
            continue;
        }
        return matches!(c, '.' | '[' | '(');
    }
    // Pure identifier with no continuation — that's a valid slot, handled
    // upstream, never reaches here as a non-ident; treat as not-intent.
    false
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

/// What a `text` node's content lowers to. Both lowerings consume this
/// ONE decision so they cannot drift: `emit_text` renders
/// `text(<expr>)` from it, and the template emitter turns `Literal` into
/// descriptor data and `Expr` into a `text` slot.
pub(crate) enum TextLowering {
    /// A bare string literal with no f-string placeholders — the one
    /// content shape a descriptor can carry as data.
    Literal(String),
    /// The final content expression to hand to `text(...)`.
    Expr(TokenStream2),
    /// The migration guard (a bare `.get()` in text position). Rendered
    /// as-is by both lowerings so the author sees one diagnostic.
    Error(TokenStream2),
}

fn emit_text(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    match text_lowering(props, children) {
        TextLowering::Literal(_) => {
            // The literal path is `Expr` too as far as emission goes;
            // `text_lowering` only splits it out so the descriptor can
            // see it. Re-render from the original tokens.
            let content = literal_content_tokens(props, children);
            quote! { ::runtime_core::text(#content) }
        }
        TextLowering::Expr(e) => quote! { ::runtime_core::text(#e) },
        TextLowering::Error(e) => e,
    }
}

/// The literal content tokens for a `TextLowering::Literal` decision —
/// the author's own literal, so the emitted expression is unchanged from
/// before the split.
fn literal_content_tokens(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    if let Some(kids) = children {
        if let Some(UiNode::Expr(e)) = kids.first() {
            return e.to_token_stream();
        }
    }
    if let Some(p) = props.iter().find(|p| p.name == "content") {
        return p.value.to_token_stream();
    }
    quote! { "" }
}

pub(crate) fn text_lowering(props: &[Prop], children: Option<&[UiNode]>) -> TextLowering {
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
                // Path (1) under `new-core`: the SAME author shape
                // (`text { label(sig) }`) lowers to the equivalent
                // reactive closure — `Derived`'s wire metadata (method
                // name + signal ids) is generator-backend (Roku) data
                // with no runtime behavior on event-driven backends,
                // deferred with the rest of that surface (see
                // runtime-vocabulary's deferred list). Observable
                // reactivity is identical: the closure reads each
                // signal arg via `.get()`, exactly what `Derived`'s
                // `compute` did.
                if let Some(call) = reactive_call_with_gets(expr) {
                    return TextLowering::Expr(quote! {
                        move || ::std::format!("{}", #call)
                    });
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
                    return TextLowering::Expr(quote! {
                        move || ::std::string::ToString::to_string(&{ #body })
                    });
                }
                // Path (2b): F-STRING — a literal child with `{name}`
                // placeholders interpolates, slots live-or-static by
                // TYPE. Placeholder-free literals fall through to the
                // static path untouched (see `parse_fstring`).
                if let Some(src) = try_emit_fstring(expr) {
                    return TextLowering::Expr(src);
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
                return TextLowering::Expr(quote! {
                    move || ::std::string::ToString::to_string(&{ #body })
                });
            }
            if let Some(src) = try_emit_fstring(&p.value) {
                return TextLowering::Expr(src);
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
        return TextLowering::Error(quote! {
            ::std::compile_error!(
                "0.1.0: reactive text must be a closure. Write \
                 `text { move || … }` (e.g. `text { move || format!(\"{}\", sig.get()) }`) \
                 instead of `text { …sig.get()… }` — a bare `.get()` in text is now static. \
                 `rx!(…)` values stay reactive by type."
            )
        });
    }

    // Path (3): static — a literal, a build-time `format!()`, a bare value, or a
    // reactive-BY-TYPE value (`rx!`, a `Reactive<String>` /
    // `Signal<String>` handle) that `IntoTextSource` routes on its own.
    //
    // A bare, placeholder-free STRING LITERAL is split out as `Literal`:
    // it is the one content shape a descriptor can carry as data, so the
    // template lowering has to see it as such. The direct lowering
    // renders it from the author's own tokens, unchanged.
    let content: TokenStream2 = if let Some(kids) = children {
        match kids.len() {
            0 => return TextLowering::Literal(String::new()),
            1 => {
                if let UiNode::Expr(Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(lit),
                    ..
                })) = &kids[0]
                {
                    return TextLowering::Literal(lit.value());
                }
                emit_node(&kids[0], Ctx::Single)
            }
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
        if let Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(lit), .. }) = &p.value {
            return TextLowering::Literal(lit.value());
        }
        p.value.to_token_stream()
    } else {
        return TextLowering::Literal(String::new());
    };

    TextLowering::Expr(content)
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


/// `new-core` counterpart of the structured lowerings: for the same
/// "reactive call" shape (`method(sig_a, sig_b)` — single-segment fn,
/// every arg a bare signal path), return the call rewritten to read each
/// arg (`method((sig_a).get(), (sig_b).get())`). The caller wraps it in
/// whatever closure form the construct needs. Returns `None` for any
/// other shape (the caller falls through to its non-structured paths).
pub(crate) fn reactive_call_with_gets(expr: &Expr) -> Option<TokenStream2> {
    if !is_reactive_call_shape(expr) {
        return None;
    }
    let Expr::Call(call) = expr else { return None };
    let func = &call.func;
    let get_args = call.args.iter().map(|a| quote! { (#a).get() });
    Some(quote! { #func( #(#get_args),* ) })
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

fn emit_button(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let label = button_label(props);
    let on_click = button_on_click(props);
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

/// A `button`'s label expression. Shared with the template lowering.
pub(crate) fn button_label(props: &[Prop]) -> TokenStream2 {
    props
        .iter()
        .find(|p| p.name == "label")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { "" })
}

/// A `button`'s press expression. Shared with the template lowering so
/// the structured-call rewrite below cannot drift between them.
pub(crate) fn button_on_click(props: &[Prop]) -> TokenStream2 {
    // on_click: three shapes, in priority order:
    //   1. `on_click = method(sig) => out_signal` — structured Action
    //   2. `on_click = method(sig)` — structured Action, fire-and-forget
    //   3. `on_click = closure_expression` — opaque coercion via IntoAction
    match props.iter().find(|p| p.name == "on_click") {
        Some(p) => {
            // The structured `Action` (wire metadata: method name,
            // signal ids) was generator-backend surface and died with
            // the old core. The same author shape
            // (`on_click = method(sig)` / `… => out`) lowers to the
            // equivalent fire closure: args read at press time, result
            // written to the output signal — exactly what
            // `Action::fire` did.
            if let Some(call) = reactive_call_with_gets(&p.value) {
                match p.arrow_target.as_ref() {
                    Some(out) => quote! { move || { (#out).set(#call); } },
                    None => quote! { move || { #call; } },
                }
            } else {
                p.value.to_token_stream()
            }
        }
        None => quote! { || {} },
    }
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
    // `animate` takes a StrokeAnimation struct directly. `draw_in` is
    // shorthand for a `(duration, easing)` tuple.
    //
    // The tuple is bound to a local FIRST, and that is a fix, not a
    // style choice: the emission used to splice the author's expression
    // TWICE (`.draw_in((#v).0, (#v).1)`), so `draw_in = next_anim()` ran
    // the call once per element — the duration and the easing came from
    // two different evaluations, and any side effect happened twice.
    let (anim_prelude, anim_call) =
        if let Some(p) = props.iter().find(|p| p.name == "animate") {
            let v = &p.value;
            (quote! {}, quote! { .animate(#v) })
        } else if let Some(p) = props.iter().find(|p| p.name == "draw_in") {
            let v = &p.value;
            (
                quote! { let __ui_draw_in = #v; },
                quote! { .draw_in(__ui_draw_in.0, __ui_draw_in.1) },
            )
        } else {
            (quote! {}, quote! {})
        };
    if anim_prelude.is_empty() {
        quote! { ::runtime_core::icon(#data) #color_call #stroke_call #anim_call }
    } else {
        quote! {
            {
                #anim_prelude
                ::runtime_core::icon(#data) #color_call #stroke_call #anim_call
            }
        }
    }
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
        .unwrap_or_else(|| {
            // `runtime_world::Signal` has no `new`; the glue's
            // `fresh_signal` mints the uncontrolled default.
            quote! { ::runtime_vocabulary::glue::fresh_signal(::std::string::String::new()) }
        });
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
        .unwrap_or_else(|| {
            quote! { ::runtime_vocabulary::glue::fresh_signal(false) }
        });
    let on_change = props
        .iter()
        .find(|p| p.name == "on_change")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| quote! { |_| {} });
    quote! { ::runtime_core::primitives::toggle::toggle(#value, #on_change) }
}

/// Lower each named inline prop to the builder call of the same name,
/// `name = v` → `.name(v)`, skipping any the author did not write.
///
/// The primitive emitters used to hand-roll one `if let Some(p) =
/// props.iter().find(..)` per prop, and every prop that was NOT
/// hand-rolled was dropped in silence — an unknown attribute on a
/// primitive is not an error. That is how `scroll_view(on_end_reached
/// = cb)` compiled and reached nothing, and cost an instrumented device
/// build to diagnose (47d014a2). A table is harder to leave a setter
/// out of than a chain of `if let`s.
fn builder_calls(props: &[Prop], names: &[&str]) -> TokenStream2 {
    let calls = names.iter().filter_map(|name| {
        props.iter().find(|p| p.name == *name).map(|p| {
            let m = syn::Ident::new(name, proc_macro2::Span::call_site());
            let v = &p.value;
            quote! { .#m(#v) }
        })
    });
    quote! { #(#calls)* }
}

/// The inline props `scroll_view` lowers, each to the `GlueScrollView`
/// setter of the same name. Every entry here is a setter on that
/// builder; adding a setter there means adding it here, or the inline
/// spelling is dropped in silence.
const SCROLL_VIEW_BUILDER_PROPS: &[&str] = &[
    "horizontal",
    "on_scroll",
    "on_end_reached",
    "end_reached_threshold",
    "bounces",
    "always_bounce",
    "safe_area",
];

/// `GlueScrollView` setters that are builder-only BY DECISION, so
/// `setter_tables_cover_every_glue_setter` does not flag them. Every
/// name here needs a reason.
///
/// - `bind` takes a `Ref<ScrollViewHandle>`; every primitive spells
///   that as `.bind(r)` after the call, never inline.
const SCROLL_VIEW_BUILDER_ONLY: &[&str] = &["bind"];

/// `scroll_view(horizontal = bool, on_scroll = …, on_end_reached = …,
/// end_reached_threshold = px, bounces = bool, safe_area = …) { children }`.
/// Children list works just like `view`.
fn emit_scroll_view(props: &[Prop], children: Option<&[UiNode]>) -> TokenStream2 {
    let kids = children.unwrap_or(&[]);
    let parts = kids.iter().map(|n| emit_node(n, Ctx::Child));
    let setters = builder_calls(props, SCROLL_VIEW_BUILDER_PROPS);
    quote! {
        ::runtime_core::primitives::scroll_view::scroll_view({
            let mut __c: ::std::vec::Vec<::runtime_core::Element>
                = ::std::vec::Vec::new();
            #( ::runtime_core::ChildList::append_to(#parts, &mut __c); )*
            __c
        }) #setters
    }
}

/// `Slider(value = signal, on_change = closure, min = f32, max = f32, step = f32)`.
fn emit_slider(props: &[Prop], _children: Option<&[UiNode]>) -> TokenStream2 {
    let value = props
        .iter()
        .find(|p| p.name == "value")
        .map(|p| p.value.to_token_stream())
        .unwrap_or_else(|| {
            quote! { ::runtime_vocabulary::glue::fresh_signal(0.0f32) }
        });
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
/// The framework provides a platform-native render target via
/// `OnReadyEvent.target`. On most backends that's a
/// `GraphicsTarget::RawWindow` implementing `raw_window_handle`'s
/// `HasWindowHandle + HasDisplayHandle` — take it with
/// `event.into_surface()` and plug in the GPU library of choice
/// (`wgpu::Instance::create_surface(&surface)`, or anything else that
/// accepts those traits). Backends whose toolkit composites every
/// widget into one native surface (GTK4) instead lend a live GL
/// context as `GraphicsTarget::Gl`, where `into_surface()` is `None`.
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

    // In-app links: SAME emission on both cores (P6 un-deferral). The
    // retarget pass maps `::runtime_core::primitives::link::link` onto
    // `glue::primitives::link::link`, which lowers to the vocabulary
    // link builder's `.route(...)` — the mount handler resolves the
    // ambient `LinkActivator` the P6 navigators now provide
    // (push-vs-select decided by the enclosing navigator, the old
    // `NavigatorControl` link-activator contract).
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
        .unwrap_or_else(|| {
            // `runtime_world::Signal` has no `new`; the glue's
            // `fresh_signal` mints the empty default (same move as
            // the text_input/toggle/slider uncontrolled defaults).
            quote! { ::runtime_vocabulary::glue::fresh_signal(::std::vec::Vec::new()) }
        });
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

    // Every pass-through setter, from one table — see `builder_calls`
    // for why a table and not a chain of `if let`s. `spacing` is the one
    // setter with its own lowering (below), because it is spelled inline
    // as `gap` OR `main_spacing`/`cross_spacing`.
    let setters = builder_calls(props, FLAT_LIST_BUILDER_PROPS);
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
            #setters
            #spacing_call
    }
}

/// The inline props `flat_list` lowers straight to the `GlueFlatList`
/// setter of the same name. `gap` is here because `gap = v` IS
/// `.gap(v)`; `main_spacing`/`cross_spacing` are not, because together
/// they lower to one `.spacing(m, c)` call — see `emit_flat_list`.
const FLAT_LIST_BUILDER_PROPS: &[&str] = &[
    "overscan",
    "axis",
    "lanes",
    // NOTE: `gap` is deliberately ABSENT. `emit_flat_list`'s
    // `spacing_call` already lowers it (it is the `gap = v` spelling of
    // the `main_spacing`/`cross_spacing` pair), so listing it here as
    // well emitted `.gap(v).gap(v)` — the author's expression evaluated
    // twice. Idempotent for a literal, not for a call.
    "safe_area",
    "on_scroll",
    "on_end_reached",
    "end_reached_threshold",
];

/// `GlueFlatList` setters `builder_calls` does not lower BY NAME, with
/// the reason each is exempt from
/// `setter_tables_cover_every_glue_setter`.
///
/// - `on_handle` takes a `FnOnce(VirtualizerHandle)` and is spelled
///   after the call like every `on_handle`.
/// - `spacing(main, cross)` is two inline props, `main_spacing` and
///   `cross_spacing`, not one — `emit_flat_list` lowers those itself.
/// - `gap` IS reachable from `ui!`, just not from the table:
///   `emit_flat_list`'s `spacing_call` owns it (it is the one-value
///   spelling of the `main_spacing`/`cross_spacing` pair). It used to be
///   in BOTH, which emitted `.gap(v).gap(v)` and evaluated the author's
///   expression twice.
const FLAT_LIST_BUILDER_ONLY: &[&str] = &["on_handle", "spacing", "gap"];

/// Emit a user-defined component invocation as a `BuildElement` struct
/// literal (see the function body for the full rationale). A children
/// block (if present) becomes the `children` field, a `Vec<Element>`.
fn emit_user(
    name: &Ident,
    props: &[Prop],
    children: Option<&[UiNode]>,
    node: u32,
) -> TokenStream2 {
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

    let built = quote! {
        #props_ty {
            #(#field_assignments)*
            #children_field
            ..<#props_ty as ::runtime_core::BuildElement>::defaults()
        }
    };
    let built = crate::ui_overlay::component_props(built, name, node);

    quote! {
        ::runtime_core::BuildElement::build(#built)
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
pub(crate) fn empty_view_primitive() -> TokenStream2 {
    quote! { ::runtime_vocabulary::glue::empty_absolute_view() }
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
    // The structured `Derived<bool>` was generator-backend metadata and
    // died with the old core; the same shape lowers to the equivalent
    // reactive closure (see the note in `emit_text`).
    if let Some(call) = reactive_call_with_gets(cond) {
        let then_expr = emit_block_as_primitive(then_body);
        let else_expr =
            else_body.map(emit_block_as_primitive).unwrap_or_else(empty_view_primitive);
        return quote! {
            ::runtime_core::when(move || #call, move || #then_expr, move || #else_expr)
        };
    }

    // 3. Reactive by default (0.4.0 inverted gate) → `when(move || cond, …)`.
    //    ANY condition that might read a signal — a call / method-call / `.get()`
    //    anywhere in the tree (`if sig.get() > 0`, `if a.get() < b.get()`,
    //    `if items.len() > 0`, `if foo(x) < bar(y)`, `if state.is_active()`), or
    //    an unrecognized exotic shape — lowers reactively. The framework's Effect
    //    subscribes to whatever the closure reads while evaluating; a genuinely
    //    static condition yields an inert effect that runs once and never
    //    re-fires (correct, ~free).
    //
    //    Reactivity is the SAFE DEFAULT here: a real signal read is NEVER
    //    silently frozen (the pre-0.4.0 gate assumed static unless it spotted a
    //    top-level call or a literal `.get()`, so `if items.len() > 0` and
    //    `match x.method()` froze). The trade-off is the `'static` move-capture
    //    the closure imposes: a branch rendering a borrowed/non-`Clone` value now
    //    needs a `.clone()` or `'static` capture — a loud, diagnosable compile
    //    error, never a silent freeze. See the `migration-0-3-0-to-0-4-0` guide.
    //    To force a genuinely-static-but-call-containing condition back to a
    //    borrowed-capture plain `if`, read it via `.peek()` (`.get_untracked()`
    //    on a `Reactive<T>` prop) or hoist it to a `let` above the `ui!` block.
    if condition_may_read_signal(cond) {
        let then_expr = emit_block_as_primitive(then_body);
        let else_expr =
            else_body.map(emit_block_as_primitive).unwrap_or_else(empty_view_primitive);
        return quote! {
            ::runtime_core::when(move || #cond, move || #then_expr, move || #else_expr)
        };
    }

    // 4. Type-driven dispatch — a CALL-FREE bare path (`if del_visible`) or field
    //    access (`if state.open`) whose TYPE decides reactivity. Those are the
    //    only remaining shapes that might be a `Signal<bool>`/`Derived<bool>`
    //    rather than a plain `bool`. We emit `(COND).__idealyst_if(then, els)`
    //    with BOTH `StaticCond` and `ReactiveCond` in scope; Rust method
    //    resolution picks the impl from COND's type:
    //      - `bool` → `StaticCond` → the taken branch's flat node list;
    //      - `Signal<bool>` (e.g. `memo(...)`) / `Derived<bool>` →
    //        `ReactiveCond` → one reactive `when`.
    //    Both return `Vec<Element>`, so the branch thunks emit flat node lists
    //    (preserving no-wrapper flat-splat) and `ChildList`/`one_or_view`
    //    normalize per ctx.
    //
    // 5. EVERY OTHER (call-free) condition shape — a literal, `&&`/`||`/`!`, a
    //    comparison of call-free operands (`if kind == Kind::Scope`, `if a && b`)
    //    — is a provably signal-free `bool`, so it falls through to
    //    `emit_plain_if` (a plain Rust `if`, captures BORROWED, flat-splat).
    if !matches!(cond, Expr::Path(_) | Expr::Field(_)) {
        return emit_plain_if(cond, then_body, else_body, ctx);
    }
    let then_thunk = {
        let body = emit_child_scope(then_body);
        quote! { move || #body }
    };
    let else_thunk = match else_body {
        Some(eb) => {
            let body = emit_child_scope(eb);
            quote! { move || #body }
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
            // Each branch is its own template scope; the branch's node
            // list arrives as a `Vec<Element>` and flattens into the
            // shared `__c` exactly as the per-node appends did.
            let then_vec = emit_child_scope(then_body);
            let else_block = match else_body {
                Some(eb) => {
                    let else_vec = emit_child_scope(eb);
                    quote! { ::runtime_core::ChildList::append_to(#else_vec, &mut __c); }
                }
                None => quote! {},
            };
            quote! {
                {
                    let mut __c: ::std::vec::Vec<::runtime_core::Element>
                        = ::std::vec::Vec::new();
                    if #cond {
                        ::runtime_core::ChildList::append_to(#then_vec, &mut __c);
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

/// The 0.4.0 inverted-gate predicate. Returns `true` when a condition /
/// scrutinee is **not provably signal-free** — i.e. it might read a reactive
/// signal, so it must lower reactively (`when` / `switch`). The bias is
/// deliberate and one-directional: any shape we can't prove reads no signal
/// defaults to reactive, so a genuine reactive read is **never silently
/// frozen**. The only cost of a false positive (a static expression treated
/// reactive) is an inert effect that runs once and never re-fires — correct and
/// ~free. This replaces the pre-0.4.0 gate, which assumed *static* unless it
/// spotted a top-level call or a literal `.get()`, and so silently froze
/// reactive reads buried inside a comparison/negation (`if a.get() < b.get()`
/// stayed live only by the `.get()` string check; `if foo(x) < bar(y)` and
/// `match x.method()` froze). See the `migration-0-3-0-to-0-4-0` guide.
///
/// Provably signal-free = a tree built only from literals, paths, field/index
/// access, references, and unary/binary/boolean operators over signal-free
/// operands. **Any** function or method call (`.get()`, `.len()`, `foo(x)`)
/// makes it non-signal-free, because a call can read a signal — directly via
/// `.get()`, or inside the callee. A call-free bare path/field is left for the
/// type-driven `StaticCond` / `ReactiveCond` dispatch in `emit_if` (its *type*
/// decides). Macros are checked for a literal `.get()` (the one seam we can't
/// walk structurally); unrecognized exotic shapes default to reactive.
pub(crate) fn condition_may_read_signal(expr: &Expr) -> bool {
    match expr {
        // A call of any kind may read a signal — directly (`.get()`) or in the
        // callee (`foo(x)`, `items.len()`, a predicate that reads a signal).
        //
        // `.peek()` / `.get_untracked()` are the EXCEPTION: they are the
        // framework's declared intentional-static markers — an explicit
        // non-subscribing read (the same escape the `snapshot-condition` lint
        // honors). `peek()` is the spelling on a `Signal` / `ReadSignal` /
        // `Memo`; `get_untracked()` survives only on the `Reactive<T>` prop
        // wrapper — both must be honored or one of them silently loses the
        // escape. Honoring them here makes a real INLINE escape hatch:
        // `if sig.peek() > 3` lowers to a static plain `if` with borrowed
        // captures, matching author intent, instead of a reactive `when` that
        // never fires. We still recurse into the RECEIVER, which may itself
        // read a signal (`foo().peek()`).
        Expr::MethodCall(m)
            if (m.method == "peek" || m.method == "get_untracked") && m.args.is_empty() =>
        {
            condition_may_read_signal(&m.receiver)
        }
        Expr::Call(_) | Expr::MethodCall(_) => true,
        // Structural operators / wrappers: reactive iff any operand is.
        Expr::Binary(b) => {
            condition_may_read_signal(&b.left) || condition_may_read_signal(&b.right)
        }
        Expr::Unary(u) => condition_may_read_signal(&u.expr),
        Expr::Paren(p) => condition_may_read_signal(&p.expr),
        Expr::Group(g) => condition_may_read_signal(&g.expr),
        Expr::Reference(r) => condition_may_read_signal(&r.expr),
        Expr::Field(f) => condition_may_read_signal(&f.base),
        Expr::Index(i) => {
            condition_may_read_signal(&i.expr) || condition_may_read_signal(&i.index)
        }
        Expr::Cast(c) => condition_may_read_signal(&c.expr),
        Expr::Try(t) => condition_may_read_signal(&t.expr),
        Expr::Tuple(t) => t.elems.iter().any(condition_may_read_signal),
        Expr::Array(a) => a.elems.iter().any(condition_may_read_signal),
        Expr::Range(r) => {
            r.start.as_deref().is_some_and(condition_may_read_signal)
                || r.end.as_deref().is_some_and(condition_may_read_signal)
        }
        // Leaves that cannot read a signal on their own.
        Expr::Lit(_) | Expr::Path(_) => false,
        // `if let PAT = EXPR` bindings are handled by the caller (always static);
        // never treated as a signal read here.
        Expr::Let(_) => false,
        // A macro's tokens can't be walked structurally, so fall back to the
        // visible-`.get()` seam: `matches!(sig.get(), …)` stays reactive,
        // `matches!(kind, Kind::Scope)` stays static.
        Expr::Macro(_) => condition_is_reactive(expr),
        // Exotic shapes (block, closure, async, nested if/match, …) are rare in
        // a condition; default to reactive so nothing is silently frozen.
        _ => true,
    }
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
    // The structured `Element::Switch` (literal-armed Roku metadata)
    // died with the old core: the reactive-call fallback below claims
    // the same shape and rewrites `key(state)` → `key((state).get())`,
    // so observable reactivity is identical.

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
    // 0.4.0 inverted gate: reactive by default. A scrutinee that might read a
    // signal — `match screen.get()`, `match x.method()`, `match key(state)` —
    // lowers to a reactive `switch`; only a provably signal-free scrutinee
    // (`match plain_enum`) stays a static plain `match`. `reactive_call` is kept
    // separately to drive the shape-(b) arg rewrite below (`key(state)` →
    // `key((state).get())`).
    let reactive_call = is_reactive_call_shape(scrutinee);
    if condition_may_read_signal(scrutinee) || reactive_call {
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
                    // Each arm body is its own template scope.
                    let arm_vec = emit_child_scope(&arm.body);
                    let appends = quote! {
                        ::runtime_core::ChildList::append_to(#arm_vec, &mut __c);
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
    // The virtualizer `for i in count_method(sig)` sugar lowered to
    // `Element::Virtualizer` carrying a structured `Derived<usize>`
    // COUNT binding — generator-backend wire metadata that died with the
    // old core and is deferred until generator backends re-land. The
    // closure-form `flat_list(...)` tag DOES lower; rather than silently
    // building a static loop here, fail with the status (repo rule: no
    // silent scope cuts).
    if is_virtualizer_for_shape(pat, iter) {
        return (
            quote! {
                ::std::compile_error!(
                    "`for i in count_method(sig)` (the virtualizer for-sugar) is not \
                     available on runtime v2: it lowered to the structured \
                     generator-backend count binding (`Derived<usize>` wire \
                     metadata), deferred until generator backends re-land. Use the \
                     `flat_list(data = …, key = …, size = …, render = …)` tag, or \
                     iterate a keyed reactive collection: `for item in items_signal, \
                     key = item.id { … }`."
                )
            },
            true,
        );
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
        // The row body is its own template scope (it runs once per row).
        let row_vec = emit_child_scope(body);
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
                    let __build: ::runtime_core::EachRowBuild =
                        ::std::boxed::Box::new(move || #row_vec);
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
    //
    // Under `new-core` the SAME author shape (same recognition
    // conditions — `try_emit_for_repeat` is shared) lowers to
    // `glue::__static_repeat` → `Element::Many(RepeatPrim)`, mounted by
    // the vocabulary's `repeat` multi-node handler: the one-FFI
    // `execute_batch_with_attach` fast path on batching backends (web),
    // per-row mounts + one `insert_many` elsewhere. Both cores make the
    // SAME batching decision for the same tree (scene-parity contract).
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
    // The row body is its own template scope (it runs once per row).
    let row_vec = emit_child_scope(body);
    let dispatch = if let Some(k) = key {
        quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::{StaticForEach as _, ReactiveForEach as _};
                (#iter).__idealyst_for_each_keyed(
                    move |#pat| #k,
                    move |#pat| #row_vec,
                )
            }
        }
    } else {
        quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::{StaticForEach as _, ReactiveForEach as _};
                (#iter).__idealyst_for_each(move |#pat| #row_vec)
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

/// Shape predicate for the virtualizer for-sugar (`for IDENT in
/// method(sig, …)`) — the recognition half of `try_emit_for_virtualizer`,
/// used by the `new-core` deferral gate.
fn is_virtualizer_for_shape(pat: &syn::Pat, iter: &Expr) -> bool {
    matches!(pat, syn::Pat::Ident(p) if p.subpat.is_none() && p.by_ref.is_none())
        && is_reactive_call_shape(iter)
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
    // `__static_repeat` returns a one-element `Vec<Element>` carrying
    // the `Element::Many(RepeatPrim)` payload — the ChildList shape the
    // emission needs. `(end - start)` is evaluated as `usize`: the
    // macro's surface is `usize`-typed loops.
    Some(quote! {
        ::runtime_core::__static_repeat(
            (#end - #start) as usize,
            move |__i: usize| {
                let #ident = (#start) + __i;
                ::runtime_core::IntoElement::into_element(#body_expr)
            },
        )
    })
}

/// Emit a block of UI nodes as a single `Element`-producing expression.
/// Used for if/else/for branches where we need exactly one primitive value.
/// The result is coerced via `IntoElement` so the branch can produce
/// either a `Bound<H>` (from a primitive constructor) or a `Element`
/// (from a user component) and the surrounding `when()` / `if`
/// expression always sees `Element`.
pub(crate) fn emit_block_as_primitive(nodes: &[UiNode]) -> TokenStream2 {
    // A branch / arm / row / presence body is its own template scope —
    // see `emit_child_scope`.
    let scope = ui_split::split(nodes);
    let body = match scope.nodes.len() {
        0 => quote! { ::runtime_core::view(::std::vec::Vec::new()) },
        // Sole node must itself be one Element: single-slot context.
        1 => emit_node(&scope.nodes[0], Ctx::Single),
        // Multiple nodes genuinely need a wrapper to collapse to one
        // value; the wrapper's children are a list, so each is Child.
        _ => {
            let parts = scope.nodes.iter().map(|n| emit_node(n, Ctx::Child));
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
    let with_prelude_body = with_prelude(&scope, body);
    quote! { ::runtime_core::IntoElement::into_element(#with_prelude_body) }
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

// Silence "unused" complaints on items we may need later.
#[allow(dead_code)]
fn _unused(_: Span) {}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    // The recovery shell lives in `runtime-macros-parse` now; its tests
    // stay here, where they exercise it through the same entry point a
    // real expansion takes.
    use runtime_macros_parse::recovery::emit_recovery;

    /// Parse a `ui!` body and emit it, returning the emitted token stream
    /// as a string for substring assertions. NOTE: the output includes the
    /// `__ui_recover` salvage shell (same as a real expansion) — counted
    /// assertions must account for the salvage copy of the input.
    fn parse_and_emit(input: TokenStream2) -> String {
        let ui: Ui = syn::parse2(input.clone()).expect("parse ui");
        emit(ui, &input).to_string()
    }

    /// `link(test_id = …)` must emit a `.test_id(…)` call.
    ///
    /// A link is usually an app's only way to reach another screen, so a dropped
    /// anchor here means automation cannot navigate a route-based app at all.
    /// The macro's generic primitive path carries this (the `link`-specific
    /// emitter deliberately does not duplicate it); the drops that DID bite were
    /// downstream — `LinkPrim` had no identity slot and the glue wrapper was
    /// `test_id_ignored`. Pinned here so the macro half cannot regress silently.
    #[test]
    fn a_route_link_forwards_its_test_id() {
        let out = parse_and_emit(quote::quote! {
            link(route = MY_ROUTE, params = (), test_id = "go") {
                text("Go")
            }
        });
        // NOT `contains("test_id")`: the emitted stream also carries the
        // `__ui_recover` salvage copy of the INPUT, which mentions
        // `test_id = "go"` and would make that assertion vacuous (see
        // `parse_and_emit`). Only a method CALL proves the forwarding.
        assert!(
            out.contains(". test_id ("),
            "the emitted link must carry a `.test_id(…)` CALL; got:\n{out}"
        );
    }

    /// Every setter on the glue builder is either lowered inline by name
    /// or declared builder-only with a reason. Three times in one week a
    /// new `scroll_view` / `flat_list` setter landed without an entry in
    /// the lowering table — `on_end_reached`, `always_bounce`,
    /// `safe_area` — and each time `name = v` inside `ui!` compiled and
    /// reached nothing. The table's own comment asked authors to keep it
    /// in step; this asks the compiler to.
    ///
    /// Reads `glue.rs` as text. That is deliberate: the macro crate
    /// cannot depend on the vocabulary, and a setter is a `pub fn` in a
    /// known `impl` block. If the glue is restructured this fails loudly
    /// with the block it could not find, which is the right failure.
    #[test]
    fn setter_tables_cover_every_glue_setter() {
        let glue = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../vocabulary/src/glue.rs"
        ))
        .expect("read runtime-vocabulary/src/glue.rs");

        fn setters_of(glue: &str, ty: &str) -> Vec<String> {
            let start = glue
                .find(&format!("impl {ty} {{"))
                .unwrap_or_else(|| panic!("no `impl {ty} {{` block in glue.rs"));
            let body = &glue[start..];
            // The impl ends at the first line that is exactly the block's
            // closing brace at its indentation; glue nests these two
            // levels deep.
            let end = body.find("\n        }\n").map(|i| i + 1).unwrap_or(body.len());
            let body = &body[..end];
            let mut out = Vec::new();
            for line in body.lines() {
                let t = line.trim_start();
                if let Some(rest) = t.strip_prefix("pub fn ") {
                    let name: String =
                        rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    // Setters take `mut self`; constructors and accessors do not.
                    if rest.contains("(mut self") {
                        out.push(name);
                    }
                }
            }
            assert!(!out.is_empty(), "found no setters in `impl {ty}` — parser drifted from glue.rs");
            out
        }

        for (ty, inline, builder_only) in [
            ("GlueScrollView", SCROLL_VIEW_BUILDER_PROPS, SCROLL_VIEW_BUILDER_ONLY),
            ("GlueFlatList", FLAT_LIST_BUILDER_PROPS, FLAT_LIST_BUILDER_ONLY),
        ] {
            for setter in setters_of(&glue, ty) {
                let covered = inline.contains(&setter.as_str()) || builder_only.contains(&setter.as_str());
                assert!(
                    covered,
                    "`{ty}::{setter}` is a setter the `ui!` emitter does not know: `{setter} = v` \
                     would compile and be dropped in silence. Add it to the inline table, or to \
                     the BUILDER_ONLY list with a reason."
                );
            }
            // And the tables must not name setters that no longer exist.
            let existing = setters_of(&glue, ty);
            for name in inline.iter().chain(builder_only.iter()) {
                assert!(
                    existing.iter().any(|s| s == name),
                    "the `{ty}` tables name `{name}`, which is not a setter on the glue any more"
                );
            }
        }
    }

    /// Regression: `icon(draw_in = expr)` evaluated `expr` TWICE.
    ///
    /// The emission was `.draw_in((#v).0, (#v).1)`, so a call-valued
    /// `draw_in` ran once per tuple element — the duration and the
    /// easing came from two different evaluations, and any side effect
    /// happened twice. `ui-lowering-parity`'s
    /// `icon_draw_in_evaluates_its_tuple_once` is the behavioural half;
    /// this pins the emission so the shape cannot come back.
    #[test]
    fn regression_icon_draw_in_is_evaluated_once() {
        let raw = parse_and_emit(quote! {
            icon(data = ICON, draw_in = next_anim())
        });
        let out: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
        // The salvage shell carries one copy of the input, so the real
        // chain must add exactly one more.
        assert_eq!(
            out.matches("next_anim()").count(),
            2,
            "`draw_in` must be evaluated once (1 salvage copy + 1 real): {out}"
        );
        // Post-fix shape: the author's expression is hoisted into the
        // scope prelude (it no longer needs `Construct` placement, since
        // the emitter binds the tuple once), and `draw_in` reads that
        // one binding for both arguments.
        assert!(out.contains("__ui_s1=next_anim();"), "{out}");
        assert!(out.contains("let__ui_draw_in=__ui_s1;"), "{out}");
        assert!(out.contains(".draw_in(__ui_draw_in.0,__ui_draw_in.1)"), "{out}");
    }

    /// Regression: `flat_list(gap = expr)` lowered to `.gap(v).gap(v)`.
    ///
    /// `gap` was in `FLAT_LIST_BUILDER_PROPS` *and* claimed by
    /// `emit_flat_list`'s `spacing_call`, so both fired. Idempotent for a
    /// literal, not for a call. `ui-lowering-parity`'s
    /// `flat_list_gap_evaluates_once` is the behavioural half.
    #[test]
    fn regression_flat_list_gap_lowers_to_one_call() {
        let raw = parse_and_emit(quote! {
            flat_list(data = rows, gap = compute_gap())
        });
        let out: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(out.matches(".gap(").count(), 1, "one `.gap(…)` call only: {out}");
        assert_eq!(
            out.matches("compute_gap()").count(),
            2,
            "`gap` must be evaluated once (1 salvage copy + 1 real): {out}"
        );
    }

    /// Regression (47d014a2): `scroll_view(on_end_reached = cb)` compiled
    /// and reached nothing. The emitter lowered only `horizontal`, and an
    /// attribute it did not know was dropped in silence — no warning, no
    /// error, the prop simply stayed `None` and the observer was never
    /// installed. Diagnosing it took an instrumented device build. Every
    /// setter on `GlueScrollView` must now lower from its inline
    /// spelling. Asserted as method CALLS — the salvage copy of the input
    /// would make a bare `contains(name)` vacuous (see `parse_and_emit`).
    #[test]
    fn regression_scroll_view_lowers_every_builder_prop_inline() {
        let out = parse_and_emit(quote::quote! {
            scroll_view(
                horizontal = false,
                on_scroll = |x, y| {},
                on_end_reached = || {},
                end_reached_threshold = 400.0,
                bounces = false,
                always_bounce = false,
                safe_area = SafeAreaSides::all(),
            ) {
                text("row")
            }
        });
        for call in [
            ". horizontal (",
            ". on_scroll (",
            ". on_end_reached (",
            ". end_reached_threshold (",
            ". bounces (",
            ". always_bounce (",
            ". safe_area (",
        ] {
            assert!(out.contains(call), "missing `{call}` in:\n{out}");
        }
    }

    /// A `scroll_view` that sets none of them must emit none of them —
    /// the pass-through lowers only what the author wrote.
    #[test]
    fn scroll_view_emits_only_the_props_it_was_given() {
        let out = parse_and_emit(quote::quote! {
            scroll_view() { text("row") }
        });
        assert!(!out.contains(". on_end_reached ("), "{out}");
        assert!(!out.contains(". horizontal ("), "{out}");
    }

    /// Same trap on the virtualizer, where paging is the whole point.
    #[test]
    fn regression_flat_list_lowers_its_scroll_props_inline() {
        let out = parse_and_emit(quote::quote! {
            flat_list(
                data = items,
                render = |_i, item| text("x"),
                on_scroll = |x, y| {},
                on_end_reached = || {},
                end_reached_threshold = 800.0,
                safe_area = SafeAreaSides::all(),
                overscan = 1.5,
            )
        });
        for call in [
            ". on_scroll (",
            ". on_end_reached (",
            ". end_reached_threshold (",
            ". safe_area (",
            ". overscan (",
        ] {
            assert!(out.contains(call), "missing `{call}` in:\n{out}");
        }
    }

    /// Same for an off-app link, which takes the other branch of the emitter.
    #[test]
    fn an_external_link_forwards_its_test_id() {
        let out = parse_and_emit(quote::quote! {
            link(external = "https://example.com", test_id = "out") {
                text("Out")
            }
        });
        assert!(
            out.contains(". test_id ("),
            "the emitted external link must carry a `.test_id(…)` CALL; got:\n{out}"
        );
    }

    #[test]
    fn empty_ui_emits_empty_view() {
        let out = parse_and_emit(quote! {});
        assert!(out.contains("view"));
        assert!(out.contains("Vec :: new"));
    }

    /// The runtime-v2 lowering deltas. The path RETARGET
    /// (`::runtime_core` → `::runtime_vocabulary::glue`) happens in the
    /// lib.rs entry points and is unit-tested in `new_core.rs`; these
    /// pin the EMISSION decisions at the sites themselves.
    mod new_core_lowering {
        use super::*;

        fn squash(s: String) -> String {
            s.chars().filter(|c| !c.is_whitespace()).collect()
        }

        /// `text { label(count) }` must stay REACTIVE: the structured
        /// `Derived` metadata is dropped, but the closure form reads
        /// each signal arg — a silent static freeze here would be a
        /// behavior change, not a deferral.
        #[test]
        fn structured_text_call_lowers_to_reactive_closure() {
            let out = squash(parse_and_emit(quote! { text { label(count) } }));
            assert!(out.contains("move||"), "{out}");
            assert!(out.contains("label((count).get())"), "{out}");
            assert!(!out.contains("Derived"), "no wire metadata: {out}");
        }

        /// `if is_even(count) { … }` — same rule for the bool shape.
        #[test]
        fn structured_if_call_lowers_to_when_closure() {
            let out = squash(parse_and_emit(quote! {
                if is_even(count) { text { "even" } }
            }));
            assert!(out.contains("when(move||is_even((count).get())"), "{out}");
            assert!(!out.contains("Derived"), "{out}");
        }

        /// `on_click = add(sig) => out` — fire closure reads args at
        /// press time and writes the result, like `Action::fire` did.
        #[test]
        fn structured_action_lowers_to_fire_closure() {
            let out = squash(parse_and_emit(quote! {
                button(label = "go", on_click = add(count) => total)
            }));
            assert!(out.contains("(total).set(add((count).get()))"), "{out}");
            assert!(!out.contains("Action{"), "{out}");
        }

        /// A static range loop lowers to the glue's `__static_repeat`
        /// (→ `Element::Many(RepeatPrim)`, the batched-Repeat port) —
        /// same recognition conditions as the old core's
        /// `Element::Repeat`, so both cores make the same batching
        /// decision for the same author shape.
        #[test]
        fn static_range_for_lowers_to_static_repeat() {
            let out = squash(parse_and_emit(quote! {
                view { for i in 0..3 { text { "row" } } }
            }));
            assert!(!out.contains("Element::Repeat"), "{out}");
            // NB: `__idealyst_for_each` still appears in the emission's
            // dead `__ui_recover` shadow, so assert the POSITIVE form
            // (the live expression is the `__static_repeat` call).
            assert!(out.contains("__static_repeat((3-0)asusize"), "{out}");
        }

        /// The repeat recognition is EXACTLY the old core's: inclusive
        /// ranges, non-ident patterns, and non-range iterables fall
        /// through to the type-driven static path.
        #[test]
        fn non_repeat_static_loops_fall_through_to_type_driven() {
            for body in [
                quote! { view { for i in 0..=3 { text { "row" } } } },
                quote! { view { for item in items { text { "row" } } } },
            ] {
                let out = squash(parse_and_emit(body));
                assert!(!out.contains("__static_repeat"), "{out}");
                assert!(out.contains("__idealyst_for_each"), "{out}");
            }
        }

        /// Regression: `.peek()` is the untracked read on a `Signal`
        /// (`get_untracked` survives only on the `Reactive<T>` prop
        /// wrapper), but the inverted gate only knew `get_untracked` as
        /// the declared-intent escape — so `if sig.peek() > 3`, the
        /// spelling every current guide gives for an intentional
        /// snapshot, still lowered to a reactive `when` (forcing `'static`
        /// move captures) instead of the static plain `if` the author
        /// asked for. Both spellings must lower identically.
        #[test]
        fn regression_peek_condition_lowers_static_like_get_untracked() {
            let peek = squash(parse_and_emit(quote! {
                if count.peek() > 3 { text { "a" } }
            }));
            let untracked = squash(parse_and_emit(quote! {
                if count.get_untracked() > 3 { text { "a" } }
            }));
            for out in [&peek, &untracked] {
                assert!(!out.contains("when("), "declared snapshot must stay static:\n{out}");
            }
            // The control: the tracked read IS reactive.
            let get = squash(parse_and_emit(quote! {
                if count.get() > 3 { text { "a" } }
            }));
            assert!(get.contains("when("), "{get}");
            // A signal read in the RECEIVER still flows, whichever escape wraps it.
            let nested = squash(parse_and_emit(quote! {
                if pick(state).peek() > 3 { text { "a" } }
            }));
            assert!(nested.contains("when("), "{nested}");
        }

        /// Literal-armed reactive `match` must use the closure `switch`
        /// (the structured `Element::Switch` is generator metadata).
        #[test]
        fn structured_match_lowers_to_closure_switch() {
            let out = squash(parse_and_emit(quote! {
                match pick(state) { 0 => { text { "a" } }, _ => { text { "b" } } }
            }));
            assert!(out.contains("switch("), "{out}");
            assert!(out.contains("pick((state).get())"), "{out}");
            assert!(!out.contains("Element::Switch"), "{out}");
        }

        /// Deferred surfaces fail loudly, naming their migration phase.
        /// (The P3-set tags — overlay/anchored_overlay/presence/graphics/
        /// flat_list — are no longer in this list; their un-deferred
        /// lowerings are pinned below. `test_id = …` left with the P5
        /// identity seam, `link(route = …)` with the P6 nav-SDK
        /// retarget — see `link_route_lowers_to_link_constructor`;
        /// `web_view` with the P6 External-SDK wave — see
        /// `web_view_tag_is_plain_component_dispatch_both_cores`.)
        #[test]
        fn deferred_primitives_error_with_migration_status() {
            for (body, needle) in [
                (quote! { for i in count(sig) { text { "r" } } }, "flat_list"),
            ] {
                let out = parse_and_emit(body);
                assert!(out.contains("compile_error"), "{out}");
                assert!(out.contains(needle), "expected `{needle}` in: {out}");
            }
        }

        /// `web_view` is UN-deferred (P6 External-SDK wave) by ceasing to
        /// be macro-special at all: the tag routes through ordinary
        /// component (`BuildElement`) dispatch on BOTH cores — the
        /// WebView SDK ships the `WebView = WebViewProps` tag contract.
        /// Regression: the snake_case spelling previously hit a
        /// new-core-only compile_error while never resolving anywhere on
        /// the old core either (no `web_view` constructor existed).
        #[test]
        fn web_view_tag_is_plain_component_dispatch_both_cores() {
            let out = squash(parse_and_emit(quote! {
                WebView(url = webview::url("https://x"))
            }));
            assert!(!out.contains("compile_error"), "{out}");
            assert!(out.contains("BuildElement"), "{out}");
        }

        /// `link(route = …)` is UN-deferred (P6 nav wave): both cores
        /// emit the same three-positional constructor call; the
        /// retarget maps it onto `glue::primitives::link::link`, whose
        /// mount resolves the ambient `LinkActivator` the vocabulary
        /// navigators provide. Regression: this was a loud
        /// compile_error while the vocabulary had no ambient
        /// link-activator seam.
        #[test]
        fn link_route_lowers_to_link_constructor() {
            let out = squash(parse_and_emit(quote! {
                link(route = HOME) { text { "x" } }
            }));
            assert!(!out.contains("compile_error"), "{out}");
            // `route` is dynamic, so it threads through its hoisted slot.
            assert!(out.contains("__ui_s0=HOME;"), "{out}");
            assert!(out.contains("primitives::link::link(__ui_s0,()"), "{out}");

            // Explicit params thread through as the second positional —
            // as the second hoisted slot, in source order.
            let out = squash(parse_and_emit(quote! {
                link(route = DETAIL, params = DetailParams { id: 3 }) { text { "d" } }
            }));
            assert!(out.contains("__ui_s0=DETAIL;"), "{out}");
            assert!(out.contains("__ui_s1=DetailParams{id:3};"), "{out}");
            assert!(
                out.contains("primitives::link::link(__ui_s0,__ui_s1"),
                "{out}"
            );
        }

        /// `test_id = …` is UN-deferred (P5 identity seam): it lowers to
        /// the same `.test_id(…)` chain as the old core — the retarget
        /// lands it on the glue wrapper's setter, which stores the id on
        /// the vocabulary prim's identity slot for handler-side robot
        /// registration. Regression: this was a loud compile_error while
        /// the prims had no identity slot.
        #[test]
        fn test_id_lowers_to_builder_setter() {
            for body in [
                quote! { view(test_id = "t") { text { "x" } } },
                quote! { text(test_id = "t") { "x" } },
                quote! { button(label = "go", test_id = "t", on_click = move || {}) },
                quote! { scroll_view(test_id = "t") { text { "x" } } },
                quote! { toggle(test_id = "t", value = v, on_change = move |_| {}) },
            ] {
                let out = squash(parse_and_emit(body));
                assert!(!out.contains("compile_error"), "{out}");
                assert!(out.contains(".test_id(\"t\")"), "{out}");
            }
        }

        /// `overlay(...) { … }` lowers to the primitives-path chain the
        /// retarget maps onto `glue::primitives::overlay::overlay` —
        /// same attr → method mapping as the old core.
        #[test]
        fn overlay_tag_lowers_to_overlay_chain() {
            let out = squash(parse_and_emit(quote! {
                overlay(
                    placement = ViewportPlacement::Center,
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = move || open.set(false),
                    trap_focus = true
                ) {
                    text { "modal" }
                }
            }));
            assert!(!out.contains("compile_error"), "{out}");
            assert!(out.contains("::runtime_core::primitives::overlay::overlay("), "{out}");
            for chain in [".placement(", ".backdrop(", ".on_dismiss(", ".trap_focus("] {
                assert!(out.contains(chain), "expected `{chain}` in: {out}");
            }
        }

        /// `anchored_overlay` keeps `target` positional (type-enforced)
        /// and chains the side/align/offset attrs.
        #[test]
        fn anchored_overlay_tag_lowers_with_positional_target() {
            let out = squash(parse_and_emit(quote! {
                anchored_overlay(
                    target = anchor,
                    side = ElementSide::Below,
                    align = ElementAlign::Start,
                    offset = 4.0
                ) {
                    text { "tip" }
                }
            }));
            assert!(!out.contains("compile_error"), "{out}");
            // `target` is a dynamic prop, so it arrives through its
            // hoisted slot local (see `ui_split`): the prelude binds the
            // author's expression and the constructor reads the local.
            assert!(out.contains("__ui_s0=anchor;"), "{out}");
            assert!(
                out.contains("::runtime_core::primitives::overlay::anchored_overlay(__ui_s0,"),
                "{out}"
            );
            for chain in [".side(", ".align(", ".offset("] {
                assert!(out.contains(chain), "expected `{chain}` in: {out}");
            }
            // Missing `target` still fails at the macro level.
            let missing = parse_and_emit(quote! { anchored_overlay { text { "x" } } });
            assert!(missing.contains("compile_error"), "{missing}");
            assert!(missing.contains("target"), "{missing}");
        }

        /// `presence(present = …)` lowers to the child-closure form with
        /// the present/enter/exit chain.
        #[test]
        fn presence_tag_lowers_to_child_closure_chain() {
            let out = squash(parse_and_emit(quote! {
                presence(
                    present = move || open.get(),
                    enter = PresenceAnim::fade(150, Easing::EaseOut),
                    exit = PresenceAnim::fade(100, Easing::EaseIn)
                ) {
                    text { "toast" }
                }
            }));
            assert!(!out.contains("compile_error"), "{out}");
            assert!(
                out.contains("::runtime_core::primitives::presence::presence(move||"),
                "{out}"
            );
            for chain in [".present(", ".enter(", ".exit("] {
                assert!(out.contains(chain), "expected `{chain}` in: {out}");
            }
        }

        /// `graphics(on_ready = …)` lowers with the on_resize/on_lost
        /// chain.
        #[test]
        fn graphics_tag_lowers_with_lifecycle_chain() {
            let out = squash(parse_and_emit(quote! {
                graphics(
                    on_ready = |e| setup(e),
                    on_resize = |e| resized(e),
                    on_lost = || lost()
                )
            }));
            assert!(!out.contains("compile_error"), "{out}");
            assert!(out.contains("::runtime_core::primitives::graphics::graphics("), "{out}");
            for chain in [".on_resize(", ".on_lost("] {
                assert!(out.contains(chain), "expected `{chain}` in: {out}");
            }
        }

        /// `flat_list(...)` lowers to the typed adapter path (the glue
        /// ports the old type-erasure onto the closure-form virtualizer)
        /// with the same `::<_, _, (), _>` turbofish, and layout attrs
        /// chain through.
        #[test]
        fn flat_list_tag_lowers_to_typed_adapter() {
            let out = squash(parse_and_emit(quote! {
                flat_list(
                    data = rows,
                    key = |_i, t: &Row| t.id,
                    size = fixed_size(24.0),
                    render = |_i, t: &Row| row_view(t),
                    overscan = 2.0,
                    gap = 4.0
                )
            }));
            assert!(!out.contains("compile_error"), "{out}");
            // `data` and `size` are dynamic (slots 0 and 2 — the two
            // closures in between are `Construct`-placed and stay where
            // they were written); `overscan` / `gap` are literals and
            // stay inline.
            assert!(out.contains("__ui_s0=rows;"), "{out}");
            assert!(out.contains("__ui_s2=fixed_size(24.0);"), "{out}");
            assert!(
                out.contains(
                    "::runtime_core::primitives::flat_list::flat_list::<_,_,(),_>(__ui_s0,"
                ),
                "{out}"
            );
            for chain in [".overscan(", ".gap("] {
                assert!(out.contains(chain), "expected `{chain}` in: {out}");
            }
        }

        /// A `flat_list` with no `data` mints its empty default through
        /// the glue (`runtime_world::Signal` has no `new`).
        #[test]
        fn flat_list_data_default_uses_fresh_signal() {
            let out = squash(parse_and_emit(quote! {
                flat_list(key = |i, _t: &Row| i as u64)
            }));
            assert!(out.contains("glue::fresh_signal(::std::vec::Vec::new())"), "{out}");
        }

        /// The empty `if` branch stays layout-neutral via the glue
        /// constructor (no old-core StyleSheet construction).
        #[test]
        fn empty_else_uses_glue_absolute_view() {
            let out = squash(parse_and_emit(quote! {
                if sig.get() { text { "on" } }
            }));
            assert!(out.contains("glue::empty_absolute_view()"), "{out}");
            assert!(!out.contains("StyleSheet"), "{out}");
        }

        /// Uncontrolled input defaults mint world signals through the
        /// glue (`runtime_world::Signal` has no `new`).
        #[test]
        fn uncontrolled_input_defaults_use_fresh_signal() {
            let out = squash(parse_and_emit(quote! { toggle(on_change = |_| {}) }));
            assert!(out.contains("glue::fresh_signal(false)"), "{out}");
        }
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
        //
        // A LITERAL is descriptor data and stays inline; a dynamic value
        // is hoisted into the scope prelude and the field reads the
        // local. The `.into()` still sits at the field, which is what
        // pins its target type — and why the prelude uses deferred init
        // (`let x; x = …;`) rather than `let x = …;`.
        assert!(out.contains("(\"x\") . into ()"), "got: {out}");
        assert!(out.contains("let __ui_s0 ; __ui_s0 = score ;"), "got: {out}");
        assert!(out.contains("(__ui_s0) . into ()"), "got: {out}");
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
        //
        // The prop values are dynamic, so each reaches its field through
        // a hoisted slot local, bound in the scope's prelude in SOURCE
        // order (`s` before `t`) — see `ui_split`.
        assert!(out.contains("let __ui_s0 ; __ui_s0 = s ;"), "got: {out}");
        assert!(out.contains("let __ui_s1 ; __ui_s1 = t ;"), "got: {out}");
        assert!(out.contains("(__ui_s0) . into ()"), "got: {out}");
        assert!(out.contains("(__ui_s1) . into ()"), "got: {out}");
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
    fn regression_fstring_field_path_is_error_not_silent_literal() {
        // THE bug: `{item.name}` has no bare-`{ident}` slot, so it used to
        // fall into the prose-tolerance `Ok(None)` path and render the
        // LITERAL text `{item.name}` — a clean-building rendering bug that
        // every list/detail screen hits. A path/index/call on a binding is
        // unambiguous interpolation intent and must now error LOUD.
        for lit in ["{item.name}", "{x.y()}", "{v[0]}", "{a.b.c}", "{foo()}"] {
            assert!(
                parse_fstring(lit).is_err(),
                "clear-interpolation-intent `{lit}` must be a loud error, not silent literal",
            );
        }
        // The error names the fix (reactive closure / local binding).
        let Err(msg) = parse_fstring("{item.name}") else {
            panic!("`{{item.name}}` must be an Err");
        };
        assert!(msg.contains("move ||") && msg.contains("let name"), "guidance msg: {msg}");
        // End-to-end through the macro: emits a `compile_error!`, never the
        // silent `__idealyst_text_from_parts` / verbatim literal.
        let out = parse_and_emit(quote! { text { "{item.name}" } });
        assert!(out.contains("compile_error"), "{out}");
        assert!(!out.contains("__idealyst_text_from_parts"), "{out}");
    }

    #[test]
    fn regression_fstring_prose_braces_still_tolerated() {
        // Positive guards: the fix must NOT hijack legitimate literal-brace
        // prose. Bare `{name}` still interpolates, plain text and prose
        // braces (no path/index/call continuation) stay verbatim `Ok(None)`.
        assert!(matches!(parse_fstring("{name}"), Ok(Some(_))), "bare ident is still a slot");
        assert!(matches!(parse_fstring("plain text"), Ok(None)), "plain text unchanged");
        for prose in ["{a b}", "{ }", "{see the {x} note}", "use {{ and }} freely"] {
            assert!(
                matches!(parse_fstring(prose), Ok(None)),
                "prose `{prose}` must stay verbatim, not error",
            );
        }
        // Unit-level guard on the intent predicate itself.
        assert!(fstring_interp_intent("item.name"));
        assert!(fstring_interp_intent("v[0]"));
        assert!(fstring_interp_intent("x.y()"));
        assert!(!fstring_interp_intent("a b"));
        assert!(!fstring_interp_intent(" "));
        assert!(!fstring_interp_intent("name"));
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