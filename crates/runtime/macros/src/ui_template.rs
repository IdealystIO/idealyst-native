//! The **template** lowering of a `ui!` body.
//!
//! Where the direct lowering emits builder calls inline, this one emits,
//! per template scope:
//!
//! ```ignore
//! {
//!     let __ui_s0; __ui_s0 = <author expr>;      // the shared prelude
//!     static __UI_DESC: Descriptor = …;          // the STATIC half: data
//!     runtime_core::__template::build(&__UI_DESC, &mut [
//!         SlotValue::style(__ui_s0),             // the DYNAMIC half
//!     ])
//! }
//! ```
//!
//! The front half — the parser, the [`UiNode`](crate::ui::UiNode) tree,
//! and [`crate::ui_split`]'s static/slot classification and prelude — is
//! shared with the direct lowering verbatim. Only the construction
//! differs, which is what makes the two comparable: see
//! `crates/dev/ui-lowering-parity`.
//!
//! # Descriptor-native vs escaped
//!
//! A node is **descriptor-native** when the builder
//! (`runtime_vocabulary::template`) can construct it from data:
//! `view` / `text` / `button` / `image` / `activity_indicator` /
//! `scroll_view` with props the builder models, a `#[component]` whose
//! props are all descriptor literals, and a reactive `if`.
//!
//! Everything else is **escaped**: the node is built by the DIRECT
//! emitter and its finished `Element`(s) land in a slot
//! ([`runtime_template::Node::Escape`]). That is what makes the template
//! lowering complete on day one — every `ui!` construct is expressible
//! this way — while the descriptor-native set grows without ever being a
//! correctness precondition. What an escape costs is the thing
//! descriptors exist for: its literals are compiled in, so a static edit
//! inside one needs a rebuild.
//!
//! An escaped node's BODIES are still nested templates: the ambient
//! lowering (`ui::ambient_lowering`) stays `Template`, so a `for`'s row
//! builder and a `match`'s arms lower through [`emit_child_scope`] /
//! [`emit_single_scope`] here, not through the direct scope emitters.
//! Without that, one escape would sink its whole subtree into the direct
//! lowering and the parity suite would stop testing below it.
//!
//! # Evaluation order
//!
//! Identical to the direct lowering by construction, because the
//! PRELUDE is the same code: `ui_split` hoists the same slots into the
//! same `let`s in the same order, and both lowerings construct
//! afterwards. The slot ARRAY only moves already-bound locals (and
//! `Construct`-placed expressions, which are closure literals and
//! therefore order-free — see `ui_split`'s module docs).
//!
//! # Site identity
//!
//! `module_path!()` plus a digest of the site's body tokens
//! ([`runtime_template::SiteId`]). Not the source line: adding a blank
//! line above a site would otherwise re-key it.

use std::collections::HashMap;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, ToTokens};
use sha2::{Digest, Sha256};
use syn::{Expr, Ident};

use crate::ui::{self, Ctx, Prop, TextLowering, UiNode};
use crate::ui_split::{self, Scope, SlotRole, StaticValue};

// ===========================================================================
// Entry points
// ===========================================================================

/// A whole `ui!` body under the template lowering.
pub(crate) fn emit(nodes: &[UiNode], input: &TokenStream2) -> TokenStream2 {
    ui_split::reset_slot_counter();
    ui::set_ambient_lowering(crate::ui::Lowering::Template);
    let site = begin_site(input);
    let body = emit_scope(nodes, Shape::Root, &site);
    ui::emit_shell(input, body)
}

/// A nested body scope that must yield a `Vec<Element>` (an `if`/`match`
/// branch or arm in children position, a `for` row builder).
pub(crate) fn emit_child_scope(nodes: &[UiNode]) -> TokenStream2 {
    let site = nested_site();
    emit_scope(nodes, Shape::List, &site)
}

/// A nested body scope that must yield exactly one `Element` (a reactive
/// branch, a `presence` child, a virtualizer row).
pub(crate) fn emit_single_scope(nodes: &[UiNode]) -> TokenStream2 {
    let site = nested_site();
    let built = emit_scope(nodes, Shape::Single, &site);
    quote! { ::runtime_core::IntoElement::into_element(#built) }
}

/// What the scope's builder call must produce.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The site's root: one `Element`, `view`-wrapped when there is not
    /// exactly one root (mirroring `ui::emit_root_scope`).
    Root,
    /// Exactly one `Element` (same wrapping rule as `Root`).
    Single,
    /// A flat `Vec<Element>`.
    List,
}

// ===========================================================================
// Scope emission
// ===========================================================================

fn emit_scope(nodes: &[UiNode], shape: Shape, site: &SiteTokens) -> TokenStream2 {
    let scope = ui_split::split(nodes);
    let mut desc = DescBuilder::new(&scope);
    let ctx = if shape == Shape::List { Ctx::Child } else { Ctx::Single };
    // A single-element scope's sole node is emitted in single-slot
    // context, exactly as the direct lowering does.
    let single_ctx = scope.nodes.len() == 1 && ctx == Ctx::Single;
    let roots: Vec<u32> = scope
        .nodes
        .iter()
        .map(|n| desc.lower(n, if single_ctx { Ctx::Single } else { Ctx::Child }))
        .collect();

    let descriptor = desc.finish(site, &roots);
    let slot_values = desc.slot_values();
    let call = match shape {
        Shape::Root | Shape::Single => quote! {
            ::runtime_core::__template::build(&__UI_DESC, &mut [#(#slot_values),*])
        },
        Shape::List => quote! {
            ::runtime_core::__template::build_list(&__UI_DESC, &mut [#(#slot_values),*])
        },
    };
    let body = quote! {
        {
            static __UI_DESC: ::runtime_core::__template::TemplateDescriptor = #descriptor;
            #call
        }
    };
    ui::with_prelude(&scope, body)
}

// ===========================================================================
// Descriptor construction
// ===========================================================================

/// Accumulates one scope's descriptor: the flat node array, the slot
/// value expressions, and the `SlotSig` entries that describe them.
struct DescBuilder<'a> {
    /// `TemplateNode` literals, in allocation order.
    nodes: Vec<TokenStream2>,
    /// `(SlotValue expression, SlotInfo literal)`, in allocation order.
    ///
    /// Allocation order is SOURCE order: a node's props are allocated
    /// before its children, and nodes in a list in order. The slot array
    /// is therefore constructed in source order, matching the prelude.
    slots: Vec<(TokenStream2, TokenStream2)>,
    /// `__ui_sN` → the split's record for slot N, so a hoisted prop's
    /// `SlotInfo` reports the ORIGINAL expression's syntactic kind
    /// rather than `path` (the kind of the substituted local).
    split_by_local: HashMap<String, (Option<&'static str>, SlotRole, &'static str)>,
    /// Held for the lifetime parameter; the split's data is read through
    /// `split_by_local`.
    _scope: &'a Scope,
}

impl<'a> DescBuilder<'a> {
    fn new(scope: &'a Scope) -> DescBuilder<'a> {
        let mut split_by_local = HashMap::new();
        for slot in &scope.slots {
            split_by_local.insert(
                ui_split::slot_ident(slot.index).to_string(),
                (slot.name, slot.role, slot.kind),
            );
        }
        DescBuilder { nodes: Vec::new(), slots: Vec::new(), split_by_local, _scope: scope }
    }

    fn push_node(&mut self, node: TokenStream2) -> u32 {
        self.nodes.push(node);
        (self.nodes.len() - 1) as u32
    }

    /// Allocate a descriptor slot. `value` is the `SlotValue::…(…)`
    /// expression; the `SlotInfo` records what the split pass knew about
    /// it.
    fn push_slot(
        &mut self,
        value: TokenStream2,
        name: Option<&str>,
        role: &str,
        kind: &str,
    ) -> u32 {
        let name_tokens = match name {
            Some(n) => quote! { ::core::option::Option::Some(::std::borrow::Cow::Borrowed(#n)) },
            None => quote! { ::core::option::Option::None },
        };
        let info = quote! {
            ::runtime_core::__template::TemplateSlotInfo {
                name: #name_tokens,
                role: ::std::borrow::Cow::Borrowed(#role),
                kind: ::std::borrow::Cow::Borrowed(#kind),
            }
        };
        self.slots.push((value, info));
        (self.slots.len() - 1) as u32
    }

    /// The split pass's record for a prop value, looked up through the
    /// substituted local when the value was hoisted.
    fn kind_of(&self, value: &Expr) -> &'static str {
        if let Expr::Path(p) = value {
            if p.qself.is_none() && p.path.segments.len() == 1 {
                let ident = p.path.segments[0].ident.to_string();
                if let Some((_, _, kind)) = self.split_by_local.get(&ident) {
                    return kind;
                }
            }
        }
        // Not a hoisted local: a `Construct`-placed expression, recorded
        // verbatim. Its kind is whatever the split pass would have said.
        ui_split::expr_kind_of(value)
    }

    fn slot_values(&self) -> Vec<TokenStream2> {
        self.slots.iter().map(|(v, _)| v.clone()).collect()
    }

    fn finish(&self, site: &SiteTokens, roots: &[u32]) -> TokenStream2 {
        let nodes = &self.nodes;
        let infos: Vec<&TokenStream2> = self.slots.iter().map(|(_, i)| i).collect();
        let module = &site.module;
        let hash = &site.hash;
        quote! {
            ::runtime_core::__template::TemplateDescriptor {
                site: ::runtime_core::__template::TemplateSiteId {
                    module: ::std::borrow::Cow::Borrowed(#module),
                    hash: ::std::borrow::Cow::Borrowed(#hash),
                },
                slots: ::runtime_core::__template::TemplateSlotSig {
                    slots: ::std::borrow::Cow::Borrowed(&[#(#infos),*]),
                },
                nodes: ::std::borrow::Cow::Borrowed(&[#(#nodes),*]),
                roots: ::std::borrow::Cow::Borrowed(&[#(#roots),*]),
            }
        }
    }

    // -----------------------------------------------------------------
    // Node lowering
    // -----------------------------------------------------------------

    /// Lower one node, returning its index in the node array.
    fn lower(&mut self, node: &UiNode, ctx: Ctx) -> u32 {
        match node {
            UiNode::Component { name, props, children, chain } => {
                self.lower_component(node, name, props, children.as_deref(), chain, ctx)
            }
            // Only a reactive `if` is descriptor-native; the rest of
            // control flow escapes (its bodies are still nested
            // templates — see the module docs).
            UiNode::If { cond, then_body, else_body } => {
                match self.lower_reactive_if(cond, then_body, else_body.as_deref()) {
                    Some(index) => index,
                    None => self.escape(node, ctx),
                }
            }
            UiNode::For { .. } | UiNode::Match { .. } => self.escape(node, ctx),
            UiNode::Expr(_) => self.escape(node, ctx),
        }
    }

    /// The completeness escape hatch: build the node with the DIRECT
    /// emitter and hand the finished `Element`(s) to a slot.
    fn escape(&mut self, node: &UiNode, ctx: Ctx) -> u32 {
        let built = ui::emit_node(node, ctx);
        let (value, role) = match ctx {
            Ctx::Single => (
                quote! { ::runtime_core::__template::SlotValue::element(#built) },
                "child",
            ),
            Ctx::Child => (
                quote! { ::runtime_core::__template::SlotValue::children(#built) },
                "child",
            ),
        };
        let slot = self.push_slot(value, None, role, "escape");
        self.push_node(quote! {
            ::runtime_core::__template::TemplateNode::Escape { slot: #slot }
        })
    }

    fn lower_component(
        &mut self,
        node: &UiNode,
        name: &Ident,
        props: &[Prop],
        children: Option<&[UiNode]>,
        chain: &[TokenStream2],
        ctx: Ctx,
    ) -> u32 {
        // A trailing `.method(args)` chain is raw tokens, not a parsed
        // expression — the split pass cannot classify it and the
        // descriptor cannot carry it.
        if !chain.is_empty() {
            return self.escape(node, ctx);
        }
        let name_str = name.to_string();
        match crate::primitives::canonical_primitive(&name_str) {
            Some(canonical) => self.lower_prim(node, canonical, props, children, ctx),
            None => self.lower_user_component(node, name, props, children, ctx),
        }
    }

    // -----------------------------------------------------------------
    // Primitives
    // -----------------------------------------------------------------

    fn lower_prim(
        &mut self,
        node: &UiNode,
        canonical: &'static str,
        props: &[Prop],
        children: Option<&[UiNode]>,
        ctx: Ctx,
    ) -> u32 {
        let Some(kind) = native_prim(canonical) else { return self.escape(node, ctx) };
        // Every prop must be modellable, and `text`'s content must be a
        // literal or one expression. A single unmodellable prop escapes
        // the whole node: a partially-applied descriptor would silently
        // drop it.
        if !props.iter().all(|p| prim_prop_is_native(canonical, p)) {
            return self.escape(node, ctx);
        }
        if canonical == "text" && !text_is_native(props, children) {
            return self.escape(node, ctx);
        }

        // Props before children — source order, which the slot array
        // must preserve.
        let mut entries: Vec<TokenStream2> = Vec::new();
        if canonical == "text" {
            match ui::text_lowering(props, children) {
                TextLowering::Literal(s) => {
                    entries.push(prop_entry("content", lit_str(&s)));
                }
                TextLowering::Expr(expr) => {
                    let slot = self.push_slot(
                        quote! { ::runtime_core::__template::SlotValue::text(#expr) },
                        Some("content"),
                        SlotRole::TextContent.as_str(),
                        "text",
                    );
                    entries.push(prop_entry("content", slot_ref(slot)));
                }
                // The migration guard: emit it where the node was.
                TextLowering::Error(err) => {
                    let slot = self.push_slot(
                        quote! { ::runtime_core::__template::SlotValue::element(#err) },
                        None,
                        "child",
                        "escape",
                    );
                    return self.push_node(quote! {
                        ::runtime_core::__template::TemplateNode::Escape { slot: #slot }
                    });
                }
            }
        }
        if canonical == "button" {
            let label = ui::button_label(props);
            let label_entry = match props.iter().find(|p| p.name == "label") {
                Some(p) => match ui_split::classify_static(&p.value) {
                    Some(StaticValue::Str(s)) => prop_entry("label", lit_str(&s)),
                    _ => {
                        let kind = self.kind_of(&p.value);
                        let slot = self.push_slot(
                            quote! { ::runtime_core::__template::SlotValue::text(#label) },
                            Some("label"),
                            SlotRole::PropValue.as_str(),
                            kind,
                        );
                        prop_entry("label", slot_ref(slot))
                    }
                },
                None => prop_entry("label", lit_str("")),
            };
            entries.push(label_entry);

            let on_click = ui::button_on_click(props);
            let kind = props
                .iter()
                .find(|p| p.name == "on_click")
                .map(|p| self.kind_of(&p.value))
                .unwrap_or("closure");
            let slot = self.push_slot(
                quote! { ::runtime_core::__template::SlotValue::press(#on_click) },
                Some("on_click"),
                SlotRole::PropValue.as_str(),
                kind,
            );
            entries.push(prop_entry("on_click", slot_ref(slot)));
        }

        for p in props {
            let name = p.name.to_string();
            // `text`'s content and `button`'s label/on_click were
            // handled positionally above.
            if canonical == "text" && name == "content" {
                continue;
            }
            if canonical == "button" && (name == "label" || name == "on_click") {
                continue;
            }
            entries.push(self.lower_prim_prop(&name, p));
        }

        let child_indices: Vec<u32> = match (children, prim_takes_children(canonical)) {
            (Some(kids), true) => kids.iter().map(|k| self.lower(k, Ctx::Child)).collect(),
            _ => Vec::new(),
        };

        let kind_tokens = prim_kind_tokens(kind);
        self.push_node(quote! {
            ::runtime_core::__template::TemplateNode::Prim {
                kind: #kind_tokens,
                props: ::std::borrow::Cow::Borrowed(&[#(#entries),*]),
                children: ::std::borrow::Cow::Borrowed(&[#(#child_indices),*]),
            }
        })
    }

    fn lower_prim_prop(&mut self, name: &str, p: &Prop) -> TokenStream2 {
        let value = &p.value;
        // Literal props the builder reads as data.
        if let Some(stat) = ui_split::classify_static(value) {
            if let Some(lit) = literal_tokens(&stat) {
                if literal_fits(name, &stat) {
                    return prop_entry(name, lit);
                }
            }
        }
        let ctor = Ident::new(
            prim_prop_slot_ctor(name).unwrap_or_else(|| {
                unreachable!("`{name}` was declared native with no slot constructor")
            }),
            Span::call_site(),
        );
        let kind = self.kind_of(value);
        let slot = self.push_slot(
            quote! { ::runtime_core::__template::SlotValue::#ctor(#value) },
            Some(name),
            SlotRole::PropValue.as_str(),
            kind,
        );
        prop_entry(name, slot_ref(slot))
    }

    // -----------------------------------------------------------------
    // Components
    // -----------------------------------------------------------------

    fn lower_user_component(
        &mut self,
        node: &UiNode,
        name: &Ident,
        props: &[Prop],
        children: Option<&[UiNode]>,
        ctx: Ctx,
    ) -> u32 {
        // Every prop must be a descriptor literal. A dynamic prop would
        // need the builder to assign an arbitrarily-typed field, which
        // it cannot do; the whole invocation escapes instead. (That is
        // also the fallback for a component with no `#[component]`:
        // slot-only, never an error.)
        let mut literals: Vec<(String, StaticValue)> = Vec::with_capacity(props.len());
        for p in props {
            match ui_split::classify_static(&p.value) {
                Some(stat) => literals.push((p.name.to_string(), stat)),
                None => return self.escape(node, ctx),
            }
        }

        let entries: Vec<TokenStream2> = literals
            .iter()
            .filter_map(|(n, stat)| literal_tokens(stat).map(|lit| prop_entry(n, lit)))
            .collect();
        if entries.len() != literals.len() {
            return self.escape(node, ctx);
        }

        // The site-local constructor: the builder cannot name the props
        // type, so the emission mints defaults, applies the descriptor's
        // literals through the generated `__apply_literal`, resolves the
        // ones it refuses, and builds.
        let ctor = self.component_ctor(name, props, children.is_some());
        let ctor_slot = self.push_slot(ctor, Some("__ctor"), "prop", "ctor");

        let child_indices: Vec<u32> = match children {
            Some(kids) => kids.iter().map(|k| self.lower(k, Ctx::Child)).collect(),
            None => Vec::new(),
        };

        let tag = name.to_string();
        self.push_node(quote! {
            ::runtime_core::__template::TemplateNode::Component {
                tag: ::std::borrow::Cow::Borrowed(#tag),
                ctor: #ctor_slot,
                literals: ::std::borrow::Cow::Borrowed(&[#(#entries),*]),
                children: ::std::borrow::Cow::Borrowed(&[#(#child_indices),*]),
            }
        })
    }

    /// The component constructor slot's expression.
    ///
    /// Two application paths, in this order:
    ///
    /// 1. the props type's generated `__apply_literal`, which handles
    ///    string / integer / float / bool props from the DESCRIPTOR —
    ///    the path that makes a literal edit a data change;
    /// 2. a site-local fallback that assigns the author's own
    ///    expression, for every literal (1) refuses.
    ///
    /// (2) is not belt-and-braces, it is required twice over. An
    /// enum-like path (`tone::Danger`) can never come from (1): a
    /// generated method cannot construct an arbitrary variant of an
    /// arbitrary type from source text without a `FromStr`-shaped bound
    /// on every prop type, which would be an API change across the whole
    /// component tree. And a props type WITHOUT the generated method
    /// (hand-rolled, or behind a macro path that does not emit it)
    /// refuses everything — for which the contract is "slot-only, never
    /// an error", and (2) is how that holds without silently dropping
    /// the prop.
    ///
    /// The cost of (2) is precision, not correctness: for a props type
    /// that has no generated applier, a future descriptor patch of a
    /// literal would be overridden by the compiled value. A
    /// `#[component]` / `#[props]` struct — every component the
    /// framework generates — takes path (1) and is patchable.
    fn component_ctor(
        &mut self,
        name: &Ident,
        props: &[Prop],
        has_children: bool,
    ) -> TokenStream2 {
        let fallback_arms = props.iter().map(|p| {
            let key = p.name.to_string();
            let field = &p.name;
            let value = &p.value;
            quote! {
                #key => { __props.#field = (#value).into(); }
            }
        });
        let children_field = if has_children {
            quote! { children: __children, }
        } else {
            quote! {}
        };
        quote! {
            ::runtime_core::__template::SlotValue::ctor(
                move |__lits: &[::runtime_core::__template::TemplatePropEntry],
                      __children: ::std::vec::Vec<::runtime_core::Element>| {
                    #[allow(unused_imports)]
                    use ::runtime_core::__template::ApplyLiteralFallback as _;
                    #[allow(unused_mut)]
                    let mut __props = <#name as ::runtime_core::BuildElement>::defaults();
                    for __entry in __lits {
                        let ::runtime_core::__template::TemplatePropValue::Lit(__value) =
                            &__entry.value
                        else {
                            continue;
                        };
                        if __props.__apply_literal(&__entry.name, __value) {
                            continue;
                        }
                        #[allow(unreachable_patterns)]
                        match __entry.name.as_ref() {
                            #(#fallback_arms)*
                            _ => {}
                        }
                    }
                    ::runtime_core::BuildElement::build(#name {
                        #children_field
                        ..__props
                    })
                },
            )
        }
    }

    // -----------------------------------------------------------------
    // Reactive `if`
    // -----------------------------------------------------------------

    /// A reactive `if` as [`runtime_template::Node::Dyn`]: the condition
    /// and the two branch thunks are slots, and each branch is its own
    /// NESTED template (the thunk builds from its own descriptor).
    ///
    /// Returns `None` for the shapes `emit_if` lowers statically — an
    /// `if let`, or a provably signal-free condition. Those escape, so
    /// the static-branch flattening semantics stay exactly the direct
    /// lowering's.
    fn lower_reactive_if(
        &mut self,
        cond: &Expr,
        then_body: &[UiNode],
        else_body: Option<&[UiNode]>,
    ) -> Option<u32> {
        if matches!(cond, Expr::Let(_)) {
            return None;
        }
        // Mirror `emit_if`'s two reactive paths, in its order.
        let cond_expr = match ui::reactive_call_with_gets(cond) {
            Some(call) => call,
            None if ui::condition_may_read_signal(cond) => cond.to_token_stream(),
            None => return None,
        };
        let cond_kind = self.kind_of(cond);
        let cond_slot = self.push_slot(
            quote! { ::runtime_core::__template::SlotValue::cond(move || #cond_expr) },
            None,
            SlotRole::Condition.as_str(),
            cond_kind,
        );
        let then_expr = ui::emit_block_as_primitive(then_body);
        let then_slot = self.push_slot(
            quote! { ::runtime_core::__template::SlotValue::branch(move || #then_expr) },
            None,
            "child",
            "branch",
        );
        let else_expr = else_body
            .map(ui::emit_block_as_primitive)
            .unwrap_or_else(ui::empty_view_primitive);
        let else_slot = self.push_slot(
            quote! { ::runtime_core::__template::SlotValue::branch(move || #else_expr) },
            None,
            "child",
            "branch",
        );
        Some(self.push_node(quote! {
            ::runtime_core::__template::TemplateNode::Dyn {
                cond: #cond_slot,
                then: #then_slot,
                otherwise: #else_slot,
            }
        }))
    }
}

// ===========================================================================
// Native-shape tables
// ===========================================================================

/// The primitives the template builder constructs from data. Mirrors
/// `runtime_template::PrimKind` — a tag absent here escapes.
fn native_prim(canonical: &str) -> Option<&'static str> {
    Some(match canonical {
        "view" => "View",
        "text" => "Text",
        "button" => "Button",
        "image" => "Image",
        "activity_indicator" => "ActivityIndicator",
        "scroll_view" => "ScrollView",
        _ => return None,
    })
}

fn prim_kind_tokens(variant: &str) -> TokenStream2 {
    let ident = Ident::new(variant, Span::call_site());
    quote! { ::runtime_core::__template::TemplatePrimKind::#ident }
}

/// Which primitives build their `{ … }` block into a child list. Matches
/// `ui_split`'s `children_kind`: everything else either treats the block
/// as content (`text`) or ignores it.
fn prim_takes_children(canonical: &str) -> bool {
    matches!(canonical, "view" | "scroll_view")
}

/// The `SlotValue` constructor for a dynamic prop — the same (kind,
/// prop) → type mapping the builder matches on. Choosing it HERE is
/// what gives the author's expression an expected type at the call site,
/// which is why an un-annotated closure still compiles in a slot.
///
/// `None` means the prop has no slot form, so it is native only as a
/// literal. [`prim_prop_is_native`] consults this, which is what stops a
/// prop from being declared native without a way to carry its dynamic
/// value — that would drop it in silence.
fn prim_prop_slot_ctor(name: &str) -> Option<&'static str> {
    Some(match name {
        "style" => "style",
        "a11y_label" | "a11y_hint" | "src" | "alt" => "string",
        "disabled" => "boolean",
        _ => return None,
    })
}

/// Is this prop one the builder models for `canonical`?
///
/// Deliberately conservative: a prop the builder would not apply must
/// make the node ESCAPE rather than be silently dropped. A `test_id` /
/// `horizontal` / `bounces` / `a11y_hidden` is native only as a
/// LITERAL — the builder's setters take `&'static str` / `bool` by
/// value, which a runtime slot cannot supply without leaking or
/// flattening.
fn prim_prop_is_native(canonical: &str, p: &Prop) -> bool {
    if p.arrow_target.is_some() {
        // `on_click = f(sig) => out` is handled by `button_on_click`,
        // which the button arm always routes through a slot.
        return canonical == "button" && p.name == "on_click";
    }
    let name = p.name.to_string();
    // A literal is native when the builder reads that literal shape for
    // this prop; a dynamic value is native when the prop has a slot
    // constructor. `content`/`label`/`on_click` are positional on the
    // constructor and handled by `lower_prim` itself.
    let as_literal = ui_split::classify_static(&p.value)
        .is_some_and(|stat| literal_fits(&name, &stat));
    let as_slot = prim_prop_slot_ctor(&name).is_some();
    match (canonical, name.as_str()) {
        ("text", "content") => true,
        ("button", "label") | ("button", "on_click") => true,
        (_, "test_id") | (_, "a11y_hidden") => as_literal,
        ("scroll_view", "horizontal") | ("scroll_view", "bounces") => as_literal,
        (_, "style") | (_, "a11y_label") | (_, "a11y_hint") => as_literal || as_slot,
        ("button", "disabled") => as_literal || as_slot,
        ("image", "src") | ("image", "alt") => as_literal || as_slot,
        _ => false,
    }
}

/// Does a literal of this shape fit the prop's setter?
fn literal_fits(name: &str, stat: &StaticValue) -> bool {
    match name {
        "test_id" | "a11y_label" | "a11y_hint" | "src" | "alt" => {
            matches!(stat, StaticValue::Str(_))
        }
        "a11y_hidden" | "disabled" | "horizontal" | "bounces" => {
            matches!(stat, StaticValue::Bool(_))
        }
        // A style has no literal spelling.
        "style" => false,
        _ => false,
    }
}

/// `text`'s content must be a literal or ONE expression. A multi-node
/// content block is stringified by `emit_text` into a `String`-building
/// block, which the descriptor has no shape for — so it escapes.
fn text_is_native(_props: &[Prop], children: Option<&[UiNode]>) -> bool {
    children.map(|kids| kids.len() <= 1).unwrap_or(true)
}

// ===========================================================================
// Token helpers
// ===========================================================================

fn prop_entry(name: &str, value: TokenStream2) -> TokenStream2 {
    quote! {
        ::runtime_core::__template::TemplatePropEntry {
            name: ::std::borrow::Cow::Borrowed(#name),
            value: #value,
        }
    }
}

fn slot_ref(index: u32) -> TokenStream2 {
    quote! { ::runtime_core::__template::TemplatePropValue::Slot(#index) }
}

fn lit_str(s: &str) -> TokenStream2 {
    quote! {
        ::runtime_core::__template::TemplatePropValue::Lit(
            ::runtime_core::__template::TemplateLiteral::Str(::std::borrow::Cow::Borrowed(#s))
        )
    }
}

/// A [`StaticValue`] as a `TemplatePropValue::Lit(…)`.
fn literal_tokens(stat: &StaticValue) -> Option<TokenStream2> {
    let inner = match stat {
        StaticValue::Str(s) => {
            quote! { ::runtime_core::__template::TemplateLiteral::Str(::std::borrow::Cow::Borrowed(#s)) }
        }
        StaticValue::Int(i) => quote! { ::runtime_core::__template::TemplateLiteral::Int(#i) },
        StaticValue::Float(f) => quote! { ::runtime_core::__template::TemplateLiteral::Float(#f) },
        StaticValue::Bool(b) => quote! { ::runtime_core::__template::TemplateLiteral::Bool(#b) },
        StaticValue::Path(p) => {
            quote! { ::runtime_core::__template::TemplateLiteral::Path(::std::borrow::Cow::Borrowed(#p)) }
        }
    };
    Some(quote! { ::runtime_core::__template::TemplatePropValue::Lit(#inner) })
}

// ===========================================================================
// Site identity
// ===========================================================================

struct SiteTokens {
    module: TokenStream2,
    hash: String,
}

thread_local! {
    /// The current expansion's base site hash plus a counter for its
    /// nested scopes.
    ///
    /// A nested scope needs its own [`runtime_template::SiteId`], and
    /// there is no `ToTokens` for `UiNode` to hash — so a nested scope
    /// is addressed as `<base>-<n>` where `n` counts nested scopes in
    /// EMISSION order. Emission order is deterministic for a given
    /// input, so the id is stable across rebuilds of unchanged source,
    /// which is the property `SiteId` exists for. An edit anywhere in
    /// the site moves the base hash and therefore every nested id,
    /// which is the conservative direction: a stale descriptor is
    /// rejected rather than mis-applied.
    static SITE: std::cell::RefCell<(String, u32)> =
        const { std::cell::RefCell::new((String::new(), 0)) };
}

/// `module_path!()` + a digest of the whole site's tokens.
/// `module_path!()` is spliced as a MACRO CALL so it expands in the
/// consumer crate, naming the module the `ui!` actually sits in.
fn begin_site(tokens: &TokenStream2) -> SiteTokens {
    let mut hasher = Sha256::new();
    hasher.update(tokens.to_string().as_bytes());
    let digest = hasher.finalize();
    let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    SITE.with(|s| *s.borrow_mut() = (hash.clone(), 0));
    SiteTokens { module: quote! { ::core::module_path!() }, hash }
}

fn nested_site() -> SiteTokens {
    let hash = SITE.with(|s| {
        let mut s = s.borrow_mut();
        s.1 += 1;
        format!("{}-{}", s.0, s.1)
    });
    SiteTokens { module: quote! { ::core::module_path!() }, hash }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Ui;

    /// Expand a `ui!` body under the TEMPLATE lowering, whitespace
    /// squashed, with the `__ui_recover` salvage closure stripped —
    /// otherwise every substring assertion would also match the copy of
    /// the INPUT the shell carries (see `ui::emit_shell`).
    fn emitted(input: TokenStream2) -> String {
        let parsed: Ui = syn::parse2(input.clone()).expect("parse ui");
        let out = crate::ui::emit_with(parsed, &input, crate::ui::Lowering::Template);
        let all: String = out.to_string().chars().filter(|c| !c.is_whitespace()).collect();
        match all.find("};") {
            Some(i) => all[i + 2..].to_string(),
            None => all,
        }
    }

    #[test]
    fn a_static_tree_is_all_descriptor_data() {
        let out = emitted(quote::quote! {
            view(test_id = "root") {
                text { "hello" }
            }
        });
        // One `static` descriptor, two native nodes, no escapes, and no
        // slots at all: nothing in this tree is code.
        assert!(out.contains("static__UI_DESC"), "{out}");
        assert_eq!(out.matches("TemplatePrimKind::View").count(), 1, "{out}");
        assert_eq!(out.matches("TemplatePrimKind::Text").count(), 1, "{out}");
        assert!(!out.contains("TemplateNode::Escape"), "{out}");
        assert!(out.contains("build(&__UI_DESC,&mut[])"), "{out}");
        // The literals are DATA, not code.
        assert!(out.contains(r#"TemplateLiteral::Str(::std::borrow::Cow::Borrowed("root"))"#), "{out}");
        assert!(out.contains(r#"TemplateLiteral::Str(::std::borrow::Cow::Borrowed("hello"))"#), "{out}");
    }

    #[test]
    fn a_dynamic_prop_becomes_a_typed_slot_reading_its_hoisted_local() {
        let out = emitted(quote::quote! { view(style = sheet()) { text { "x" } } });
        assert!(out.contains("let__ui_s0;__ui_s0=sheet();"), "{out}");
        assert!(out.contains("SlotValue::style(__ui_s0)"), "{out}");
        assert!(out.contains("TemplatePropValue::Slot(0u32)"), "{out}");
    }

    /// The slot CONSTRUCTOR is what gives the author's expression an
    /// expected type — which is how an un-annotated closure survives a
    /// slot at all.
    #[test]
    fn a_press_handler_is_typed_by_its_slot_constructor() {
        let out = emitted(quote::quote! { button(label = "go", on_click = move || tick()) });
        assert!(out.contains("SlotValue::press(move||tick())"), "{out}");
        // A literal label is descriptor DATA; only a dynamic one becomes
        // a `text` slot.
        assert!(!out.contains("SlotValue::text"), "{out}");
        let dynamic = emitted(quote::quote! { button(label = title, on_click = move || tick()) });
        assert!(dynamic.contains("SlotValue::text(__ui_s0)"), "{dynamic}");
    }

    #[test]
    fn a_reactive_if_is_a_dyn_node_with_nested_branch_templates() {
        let out = emitted(quote::quote! {
            view {
                if flag.get() {
                    text { "on" }
                } else {
                    text { "off" }
                }
            }
        });
        assert!(out.contains("TemplateNode::Dyn{"), "{out}");
        assert!(out.contains("SlotValue::cond(move||flag.get())"), "{out}");
        assert_eq!(out.matches("SlotValue::branch(").count(), 2, "{out}");
        // Each branch body is its own descriptor — a NESTED template,
        // not a direct emission.
        assert_eq!(out.matches("static__UI_DESC").count(), 3, "{out}");
    }

    /// A `for` is not descriptor-native, so the node escapes — but its
    /// ROW BODY is still a nested template, which is what keeps the
    /// template lowering under test below the first escape.
    #[test]
    fn a_for_escapes_but_its_row_body_is_a_nested_template() {
        let out = emitted(quote::quote! {
            view {
                for row in rows, key = row.id {
                    text { "r" }
                }
            }
        });
        assert!(out.contains("TemplateNode::Escape{"), "{out}");
        assert!(out.contains("__idealyst_for_each_keyed"), "{out}");
        // outer descriptor + the row body's
        assert_eq!(out.matches("static__UI_DESC").count(), 2, "{out}");
        assert!(out.contains("build_list(&__UI_DESC"), "{out}");
    }

    #[test]
    fn a_static_match_escapes_and_each_arm_is_a_nested_template() {
        let out = emitted(quote::quote! {
            view {
                match mode {
                    Mode::A => { text { "a" } }
                    Mode::B => { text { "b" } }
                }
            }
        });
        assert!(out.contains("TemplateNode::Escape{"), "{out}");
        // outer + one per arm
        assert_eq!(out.matches("static__UI_DESC").count(), 3, "{out}");
    }

    #[test]
    fn a_literal_only_component_is_a_component_node_with_its_literals_as_data() {
        let out = emitted(quote::quote! { Badge(label = "alpha", count = 5, loud = true) });
        assert!(out.contains("TemplateNode::Component{"), "{out}");
        assert!(out.contains("TemplateLiteral::Int(5i64)"), "{out}");
        assert!(out.contains("TemplateLiteral::Bool(true)"), "{out}");
        // The generated applier gets first refusal…
        assert!(out.contains("__props.__apply_literal(&__entry.name,__value)"), "{out}");
        // …and the site-local fallback covers what it refuses.
        assert!(out.contains(r#""count"=>{__props.count=(5).into();}"#), "{out}");
    }

    /// A component with a DYNAMIC prop escapes whole: the builder cannot
    /// assign an arbitrarily-typed field, and a partially-applied
    /// descriptor would silently drop it.
    #[test]
    fn a_component_with_a_dynamic_prop_escapes() {
        let out = emitted(quote::quote! { Counter(value = count) });
        assert!(out.contains("TemplateNode::Escape{"), "{out}");
        assert!(!out.contains("TemplateNode::Component{"), "{out}");
        assert!(out.contains("BuildElement::build"), "{out}");
    }

    /// A trailing `.method(args)` chain is raw tokens, not a parsed
    /// expression, so the descriptor cannot carry it.
    #[test]
    fn a_trailing_method_chain_escapes_its_node() {
        let out = emitted(quote::quote! { view { text { "x" } }.bind(handle) });
        assert!(out.contains("TemplateNode::Escape{"), "{out}");
        assert!(out.contains(".bind(handle)"), "{out}");
    }

    /// A primitive the builder does not model must ESCAPE, never be
    /// half-applied — a dropped prop would be a silent behavior change.
    #[test]
    fn an_unmodelled_primitive_escapes() {
        for body in [
            quote::quote! { toggle(value = on, on_change = move |v: bool| on.set(v)) },
            quote::quote! { icon(data = ICON) },
            quote::quote! { slider(value = amount, on_change = move |v: f32| amount.set(v)) },
        ] {
            let out = emitted(body);
            assert!(out.contains("TemplateNode::Escape{"), "{out}");
            assert!(!out.contains("TemplatePrimKind::"), "{out}");
        }
    }

    /// A prop a modelled primitive does not model escapes the WHOLE
    /// node, for the same reason.
    #[test]
    fn an_unmodelled_prop_escapes_a_modelled_primitive() {
        let out = emitted(quote::quote! { view(a11y_role = Role::Button) { text { "x" } } });
        assert!(out.contains("TemplateNode::Escape{"), "{out}");
        assert!(!out.contains("TemplatePrimKind::View"), "{out}");
    }

    #[test]
    fn the_slot_signature_records_one_entry_per_slot_with_its_split_kind() {
        let out = emitted(quote::quote! {
            button(label = "go", on_click = move || tick(), style = sheet())
        });
        // label is a literal (no slot); on_click and style are slots.
        assert_eq!(out.matches("TemplateSlotInfo{").count(), 2, "{out}");
        assert!(out.contains(r#"role:::std::borrow::Cow::Borrowed("prop")"#), "{out}");
        assert!(out.contains(r#"kind:::std::borrow::Cow::Borrowed("closure")"#), "{out}");
        // `style = sheet()` was HOISTED, so its recorded kind is the
        // original expression's (`call`), not the substituted local's
        // (`path`).
        assert!(out.contains(r#"kind:::std::borrow::Cow::Borrowed("call")"#), "{out}");
    }

    #[test]
    fn the_site_id_is_module_path_plus_a_body_digest() {
        let out = emitted(quote::quote! { text { "x" } });
        assert!(out.contains("module:::std::borrow::Cow::Borrowed(::core::module_path!())"), "{out}");
        // A different body must produce a different hash.
        let a = emitted(quote::quote! { text { "a" } });
        let b = emitted(quote::quote! { text { "b" } });
        assert_ne!(hash_of(&a), hash_of(&b));
    }

    fn hash_of(emission: &str) -> String {
        let needle = "hash:::std::borrow::Cow::Borrowed(\"";
        let start = emission.find(needle).expect("hash in emission") + needle.len();
        let rest = &emission[start..];
        rest[..rest.find('"').expect("closing quote")].to_string()
    }

    /// `ui_lowered!` rejects an unknown mode rather than silently
    /// picking one.
    #[test]
    fn an_unknown_lowering_name_is_an_error() {
        let err = crate::ui::split_lowered_invocation(quote::quote! { sideways { text { "x" } } })
            .expect_err("unknown mode must fail");
        assert!(err.to_string().contains("sideways"), "{err}");
    }
}
