//! `ui-overlay` emission: the `Descriptor` a `ui!` site registers, and
//! the origin tag each node it builds carries.
//!
//! Entirely behind the `ui-overlay` cargo feature. With the feature OFF
//! this module is a handful of `#[inline(always)]` no-ops and the
//! emission is byte-identical to a build without it —
//! `crates/dev/ui-lowering-parity` runs its whole corpus in BOTH feature
//! states against the SAME frozen goldens, which is the strongest form
//! of "identical" available: same scene, same op sequence, same driven
//! deltas.
//!
//! # What ON adds
//!
//! Per site, at the head of its top-level scope:
//!
//! ```ignore
//! static __UI_DESC: TemplateDescriptor = TemplateDescriptor { … };
//! runtime_core::__overlay::register(&__UI_DESC);
//! ```
//!
//! and per NODE, wrapping that node's finished expression:
//!
//! ```ignore
//! runtime_core::__overlay::tag(<expr>, "<site hash>", <node index>)
//! ```
//!
//! That is the whole delta: one `static` and one call per node. No
//! builder trampoline, no per-site generics, no second emission.
//!
//! # Why the index is assigned DURING emission
//!
//! A tag's only job is to map a descriptor node back to the `Element`
//! that node produced. That mapping is trustworthy exactly when both are
//! produced by the SAME walk. A descriptor built by a second pass over
//! the same tree would agree until the two passes disagreed on some edge
//! case — and then mis-address a patch silently, which is the worst
//! failure this system can have.
//!
//! So the accumulator is threaded through the emission itself:
//! `emit_component` calls [`open_node`] as it emits and tags with the
//! index it gets back. One accumulator per expansion, in a thread-local
//! — a proc-macro expansion is single-threaded and never interleaved,
//! the same reason `ui_split`'s slot counter is one.
//!
//! # Why registration is lazy
//!
//! A `static` cannot run code, and this crate will not take a `ctor` /
//! `linkme` dependency for a dev-time feature. [`register`] is called at
//! the head of every build of the site: a map insert behind an
//! already-present check, once per site per process in practice. A site
//! that is never built never registers, which is correct — a patch for
//! it could not apply anyway.

use proc_macro2::TokenStream as TokenStream2;

#[cfg(not(feature = "ui-overlay"))]
pub(crate) use inert::*;
#[cfg(feature = "ui-overlay")]
pub(crate) use live::*;

/// A node's identity within its site, handed back by [`open_node`] and
/// spliced into both the descriptor and the tag.
///
/// `Option`-shaped so the feature-off path has a zero-sized "no index"
/// to return without every caller branching on a `cfg`.
pub(crate) type NodeIndex = Option<u32>;

// ===========================================================================
// Feature OFF
// ===========================================================================

#[cfg(not(feature = "ui-overlay"))]
mod inert {
    use super::{NodeIndex, TokenStream2};
    use crate::ui_split::Scope;

    #[inline(always)]
    pub(crate) fn begin_site(_input: &TokenStream2) {}

    #[inline(always)]
    pub(crate) fn record_scope(_scope: &Scope) {}

    #[inline(always)]
    pub(crate) fn open_node() -> NodeIndex {
        None
    }

    #[inline(always)]
    pub(crate) fn close_node(
        _index: NodeIndex,
        _kind: NodeKind<'_>,
        _props: Vec<(String, PropValue)>,
        _children: Vec<u32>,
    ) {
    }

    #[inline(always)]
    pub(crate) fn tag(body: TokenStream2, _index: NodeIndex) -> TokenStream2 {
        body
    }

    #[inline(always)]
    pub(crate) fn site_prelude() -> TokenStream2 {
        TokenStream2::new()
    }

    /// Shape-only mirrors so call sites compile identically in both
    /// states. Never constructed when the feature is off — the
    /// `#[allow]` is because the compiler cannot see that.
    #[allow(dead_code)]
    pub(crate) enum NodeKind<'a> {
        Prim(&'a str),
        Component(&'a str),
        Opaque(Option<u32>),
    }

    #[allow(dead_code)]
    pub(crate) enum PropValue {
        Str(String),
        Int(i64),
        Float(f64),
        Bool(bool),
        Path(String),
        Slot(u32),
    }
}

// ===========================================================================
// Feature ON
// ===========================================================================

#[cfg(feature = "ui-overlay")]
mod live {
    use std::cell::RefCell;

    use proc_macro2::TokenStream as TokenStream2;
    use quote::quote;
    use sha2::{Digest, Sha256};

    use super::NodeIndex;
    use crate::ui_split::{Scope, StaticValue};

    /// What a descriptor node is.
    pub(crate) enum NodeKind<'a> {
        /// Canonical primitive name, exactly as `canonical_primitive`
        /// spells it.
        Prim(&'a str),
        /// The PascalCase tag as written, which is also the props-type
        /// path (`#[component]` emits the `pub type Tag = TagProps`
        /// alias).
        Component(&'a str),
        /// Addressable, not patchable inside. Carries the slot of the
        /// construct's defining expression where there is one.
        Opaque(Option<u32>),
    }

    /// A prop's recorded value.
    pub(crate) enum PropValue {
        Str(String),
        Int(i64),
        Float(f64),
        Bool(bool),
        Path(String),
        Slot(u32),
    }

    impl PropValue {
        /// Classify an already-split prop value. A `Prelude` slot's
        /// value in the rewritten tree is its `__ui_sN` local, so the
        /// slot index is read back out of the name.
        pub(crate) fn of(value: &syn::Expr) -> PropValue {
            if let Some(index) = crate::ui_split::slot_index_of(value) {
                return PropValue::Slot(index as u32);
            }
            match crate::ui_split::classify_static(value) {
                Some(StaticValue::Str(s)) => PropValue::Str(s),
                Some(StaticValue::Int(i)) => PropValue::Int(i),
                Some(StaticValue::Float(f)) => PropValue::Float(f),
                Some(StaticValue::Bool(b)) => PropValue::Bool(b),
                Some(StaticValue::Path(p)) => PropValue::Path(p),
                // A `Construct`-placed expression (a closure, a macro, a
                // reactive-call shape) is code with no slot local to
                // point at. Recorded as a path-shaped note of its
                // source text so a differ can still see that it
                // CHANGED, without pretending it is patchable.
                None => PropValue::Path(super::source_text(value)),
            }
        }

        fn tokens(&self) -> TokenStream2 {
            let lit = match self {
                PropValue::Slot(i) => {
                    return quote! { ::runtime_core::__template::TemplatePropValue::Slot(#i) }
                }
                PropValue::Str(s) => quote! {
                    ::runtime_core::__template::TemplateLiteral::Str(
                        ::std::borrow::Cow::Borrowed(#s))
                },
                PropValue::Int(i) => {
                    quote! { ::runtime_core::__template::TemplateLiteral::Int(#i) }
                }
                PropValue::Float(f) => {
                    quote! { ::runtime_core::__template::TemplateLiteral::Float(#f) }
                }
                PropValue::Bool(b) => {
                    quote! { ::runtime_core::__template::TemplateLiteral::Bool(#b) }
                }
                PropValue::Path(p) => quote! {
                    ::runtime_core::__template::TemplateLiteral::Path(
                        ::std::borrow::Cow::Borrowed(#p))
                },
            };
            quote! { ::runtime_core::__template::TemplatePropValue::Lit(#lit) }
        }
    }

    /// One accumulating site.
    #[derive(Default)]
    struct Builder {
        /// Emitted node literals, indexed by node id. `None` while a
        /// node is open (its children are still being emitted).
        nodes: Vec<Option<TokenStream2>>,
        slots: Vec<TokenStream2>,
        hash: String,
    }

    thread_local! {
        static SITE: RefCell<Builder> = RefCell::new(Builder::default());
    }

    pub(crate) fn begin_site(input: &TokenStream2) {
        let mut hasher = Sha256::new();
        hasher.update(input.to_string().as_bytes());
        let digest = hasher.finalize();
        let hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        SITE.with(|s| {
            *s.borrow_mut() = Builder { nodes: Vec::new(), slots: Vec::new(), hash }
        });
    }

    /// Record a scope's slots. Called once per scope, before its nodes,
    /// so slot indices stay in source order.
    pub(crate) fn record_scope(scope: &Scope) {
        SITE.with(|s| {
            let mut b = s.borrow_mut();
            for slot in &scope.slots {
                let name = match slot.name {
                    Some(n) => quote! {
                        ::core::option::Option::Some(::std::borrow::Cow::Borrowed(#n))
                    },
                    None => quote! { ::core::option::Option::None },
                };
                let role = slot.role.as_str();
                let kind = slot.kind;
                b.slots.push(quote! {
                    ::runtime_core::__template::TemplateSlotInfo {
                        name: #name,
                        role: ::std::borrow::Cow::Borrowed(#role),
                        kind: ::std::borrow::Cow::Borrowed(#kind),
                    }
                });
            }
        });
    }

    /// Reserve this node's index BEFORE its children are emitted, so a
    /// parent always precedes its children in the array and a child's
    /// index is known by the time the parent closes.
    pub(crate) fn open_node() -> NodeIndex {
        SITE.with(|s| {
            let mut b = s.borrow_mut();
            b.nodes.push(None);
            Some((b.nodes.len() - 1) as u32)
        })
    }

    pub(crate) fn close_node(
        index: NodeIndex,
        kind: NodeKind<'_>,
        props: Vec<(String, PropValue)>,
        children: Vec<u32>,
    ) {
        let Some(index) = index else { return };
        let entries: Vec<TokenStream2> = props
            .iter()
            .map(|(name, value)| {
                let v = value.tokens();
                quote! {
                    ::runtime_core::__template::TemplatePropEntry {
                        name: ::std::borrow::Cow::Borrowed(#name),
                        value: #v,
                    }
                }
            })
            .collect();
        let dynamic: Vec<&str> = props
            .iter()
            .filter(|(_, v)| matches!(v, PropValue::Slot(_) | PropValue::Path(_)))
            .map(|(n, _)| n.as_str())
            .collect();
        let node = match kind {
            NodeKind::Prim(name) => quote! {
                ::runtime_core::__template::TemplateNode::Prim {
                    kind: ::std::borrow::Cow::Borrowed(#name),
                    props: ::std::borrow::Cow::Borrowed(&[#(#entries),*]),
                    children: ::std::borrow::Cow::Borrowed(&[#(#children),*]),
                }
            },
            NodeKind::Component(path) => quote! {
                ::runtime_core::__template::TemplateNode::Component {
                    path: ::std::borrow::Cow::Borrowed(#path),
                    props: ::std::borrow::Cow::Borrowed(&[#(#entries),*]),
                    children: ::std::borrow::Cow::Borrowed(&[#(#children),*]),
                    dynamic: ::std::borrow::Cow::Borrowed(&[
                        #(::std::borrow::Cow::Borrowed(#dynamic)),*
                    ]),
                }
            },
            NodeKind::Opaque(slot) => {
                let slot = match slot {
                    Some(i) => quote! { ::core::option::Option::Some(#i) },
                    None => quote! { ::core::option::Option::None },
                };
                quote! {
                    ::runtime_core::__template::TemplateNode::Opaque {
                        slot: #slot,
                        children: ::std::borrow::Cow::Borrowed(&[#(#children),*]),
                    }
                }
            }
        };
        SITE.with(|s| s.borrow_mut().nodes[index as usize] = Some(node));
    }

    pub(crate) fn tag(body: TokenStream2, index: NodeIndex) -> TokenStream2 {
        let Some(index) = index else { return body };
        let site = SITE.with(|s| s.borrow().hash.clone());
        quote! { ::runtime_core::__overlay::tag(#body, #site, #index) }
    }

    pub(crate) fn site_prelude() -> TokenStream2 {
        // TAKE, never borrow: the accumulator holds `TokenStream2`s, and
        // a `proc_macro2` token in a real proc-macro context wraps a
        // bridge HANDLE that is only valid inside the expansion that
        // created it. Leaving them in the thread-local means the next
        // expansion drops tokens from the previous one and the bridge
        // panics in `handle.rs` ("unexpectedly panicked", with the
        // backtrace pointing at this macro). Emptying the accumulator
        // while still inside the owning expansion is the fix.
        //
        // `begin_site` / `site_prelude` always pair: `emit` calls both,
        // and a parse failure goes to `emit_recovery`, which calls
        // neither.
        SITE.with(|s| {
            let b = std::mem::take(&mut *s.borrow_mut());
            if b.nodes.is_empty() {
                return TokenStream2::new();
            }
            // A node still open here would be an emitter bug — every
            // `open_node` is paired with a `close_node`. Fill with an
            // `Opaque` rather than panicking: a wrong descriptor for one
            // node must not fail an author's build.
            let nodes: Vec<TokenStream2> = b
                .nodes
                .iter()
                .map(|n| {
                    n.clone().unwrap_or_else(|| {
                        quote! {
                            ::runtime_core::__template::TemplateNode::Opaque {
                                slot: ::core::option::Option::None,
                                children: ::std::borrow::Cow::Borrowed(&[]),
                            }
                        }
                    })
                })
                .collect();
            let slots = &b.slots;
            let hash = &b.hash;
            quote! {
                static __UI_DESC: ::runtime_core::__template::TemplateDescriptor =
                    ::runtime_core::__template::TemplateDescriptor {
                        site: ::runtime_core::__template::TemplateSiteId {
                            module: ::std::borrow::Cow::Borrowed(::core::module_path!()),
                            hash: ::std::borrow::Cow::Borrowed(#hash),
                        },
                        slots: ::runtime_core::__template::TemplateSlotSig {
                            slots: ::std::borrow::Cow::Borrowed(&[#(#slots),*]),
                        },
                        nodes: ::std::borrow::Cow::Borrowed(&[#(#nodes),*]),
                        roots: ::std::borrow::Cow::Borrowed(&[0u32]),
                    };
                ::runtime_core::__overlay::register(&__UI_DESC, #hash);
            }
        })
    }
}

/// Whitespace-squashed source text of an expression.
#[cfg(feature = "ui-overlay")]
fn source_text(expr: &syn::Expr) -> String {
    use quote::ToTokens;
    expr.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect()
}
