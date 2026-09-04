//! `stylesheet! { ... }` declaration macro.
//!
//! Generates a typed stylesheet builder with variant enums, override
//! setters, and a `IntoStyleSource` impl that handles both static and
//! reactive (`Signal<T>`) inputs uniformly.
//!
//! Beyond `variant` axes and `override` fields, a declaration may also
//! carry:
//! - `state <hovered|pressed|focused|disabled>(theme) { … }` —
//!   interaction-state overlays (realized as CSS pseudo-classes on web,
//!   event-driven re-resolution on native).
//! - `breakpoint <sm|md|lg|xl>(theme) { … }` — responsive overlays
//!   layered mobile-first on the `base` (Xs) rules. Web realizes them
//!   as `@media (min-width: …)` rules — so a statically rendered / SSR
//!   first paint is already responsive without JS — while native
//!   backends merge the active bucket reactively off
//!   `runtime_core::current_breakpoint`. Both keyed off the installed
//!   [`runtime_core::breakpoints`] thresholds.
//! - `transitions { … }` — per-property animation declarations.
//!
//! # Grammar
//!
//! ```ignore
//! stylesheet! {
//!     pub Card<IdeaThemeRef> {
//!         base(t) {
//!             background: t.color.surface(),
//!             padding: t.spacing.md(),
//!             border_radius: 8.0,
//!         }
//!         variant size {
//!             small(t)  { padding: t.spacing.sm() }
//!             #[default]
//!             medium(_t) {}
//!             large(t)  { padding: t.spacing.xl() }
//!         }
//!         variant kind {
//!             #[default]
//!             elevated(t) { background: t.color.surface() }
//!             outlined(t) {
//!                 background: Color("transparent".into()),
//!                 border_color: t.color.border(),
//!             }
//!         }
//!         override padding: f32
//!     }
//! }
//! ```
//!
//! The `<…>` slot names a **token vocabulary** — a type implementing
//! [`runtime_core::TokenVocabulary`] — and each block's binding (`base(t)`)
//! is that vocabulary, so a theme token is spelled as a path the compiler
//! checks (`t.spacing.md()` → the `spacing-md` token) rather than a string
//! literal. The binding carries names only; values arrive at resolve time
//! from the token registry, which is what keeps a theme swap one write per
//! token. `<()>` declares no vocabulary — such a sheet writes its bindings
//! `_t` and references any tokens with `Tokenized::token("name", fallback)`.
//!
//! # What it generates
//!
//! For the above declaration, with builder name `Card` and theme
//! type `Theme`:
//!
//! - `pub fn card_style() -> Rc<StyleSheet>` — convention-name version
//!   of the stylesheet (snake_case + `_style` suffix). Cached via
//!   `runtime_core::cached_stylesheet` (one shared thread-local registry
//!   keyed by a per-sheet address) so repeat calls return the same `Rc`.
//!   We intentionally avoid a `thread_local!` per sheet — Android bionic
//!   caps pthread TLS keys at 128 and idea-ui alone would blow past it.
//! - `pub enum CardSize { Small, Medium, Large }` + `Default` impl
//!   (picks the `#[default]` arm).
//! - `pub enum CardKind { Elevated, Outlined }` + `Default` impl.
//! - `pub struct Card { ... }` — the builder. Stores closures per
//!   variant axis / override so static and reactive inputs unify.
//! - `pub fn Card() -> Card` — entry point, so call sites read
//!   `Card().size(CardSize::Small)`.
//! - `impl Card { fn size(...), fn kind(...), fn padding(...) }` — one
//!   setter per axis and per override. Setters accept either the
//!   typed value or a `Signal<T>` via the `IntoVariantSource` /
//!   `IntoOverrideSource` traits in runtime-core.
//! - `impl IntoStyleSource for Card` — converts the builder to a
//!   `StyleSource` so `.with_style(Card()...)` works.
//!
//! # Mapping to existing framework
//!
//! Variant enums implement a `to_variant_value` method returning the
//! `&'static str` the framework's `VariantSet` already expects. The
//! macro picks the string from the enum variant's snake_case name.
//! So `CardSize::Small` maps to `"small"`, matching the legacy
//! `with("size", "small")` call shape.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::visit::{self, Visit};
use syn::{braced, parenthesized, Expr, Ident, Token, Type, Visibility};

// =============================================================================
// AST
// =============================================================================

pub struct StyleSheetDecl {
    vis: Visibility,
    name: Ident,
    /// The token vocabulary this sheet references (`pub Card<IdeaThemeRef>`).
    /// Resolved through `runtime_core::TokenVocabulary` at emission: each
    /// block's binding is bound to `<theme_ty as TokenVocabulary>::Tokens`,
    /// so `base(t) { padding: t.spacing.md() }` type-checks the token name
    /// instead of trusting a string literal. `<()>` yields the empty
    /// `NoTokens`, so a sheet that declares no vocabulary can't read one.
    theme_ty: Type,
    base: BaseBlock,
    variants: Vec<VariantAxisDecl>,
    overrides: Vec<OverrideDecl>,
    transitions: Vec<TransitionDecl>,
    /// Interaction states (`hovered`, `pressed`, `focused`,
    /// `disabled`). Stored as overlays under the reserved
    /// `__state` axis — same machinery as variants, so resolution
    /// and pre-generation Just Work. Each backend listens for the
    /// relevant native event and flips the corresponding bit on
    /// the node's active-state set; unsupported states (e.g.
    /// `hovered` on mobile) are silent no-ops.
    states: Vec<StateArm>,
    /// Responsive breakpoint overlays (`breakpoint md { … }`). Stored
    /// as overlays under reserved `__bp_*` axes — same machinery as
    /// states/variants. Web realizes them as `@media (min-width: …)`
    /// CSS (so SSR's first paint is responsive without JS); native
    /// backends merge the active bucket reactively. `Xs` is the
    /// mobile-first base and is therefore not a valid block name.
    breakpoints: Vec<BreakpointArm>,
    /// Container-query overlays (`container (min_width: N) { … }`).
    /// Stored as overlays under reserved `__cq_minw_*` axes (one per
    /// distinct threshold) — same machinery as breakpoints, but keyed on
    /// an arbitrary px length rather than a named bucket. Web realizes
    /// them as `@container (min-width: N) { … }` CSS against the nearest
    /// `container-type` ancestor; native merges the active overlays
    /// reactively against the nearest container's resolved inline-size.
    containers: Vec<ContainerArm>,
    /// Compound overlays (`compound (axis: value, axis: value)(t) { … }`)
    /// — rules that apply only when EVERY named axis holds its named
    /// value. `resolve` layers these AFTER every per-axis overlay, which
    /// is the point: two axes that touch the same property otherwise
    /// resolve by ALPHABETICAL axis name (`StyleSheet::variants` is a
    /// `BTreeMap`), so the axis that wins is decided by spelling. A
    /// compound states the combination outright instead.
    compounds: Vec<CompoundArm>,
}

/// One `compound (axis: value, …)(theme) { ... }` block. The condition
/// is validated against the sheet's own declared axes at macro time: a
/// compound naming an axis or arm that does not exist could never fire,
/// and would do so silently.
struct CompoundArm {
    /// The `(axis, value)` pairs that must all hold.
    when: Vec<(Ident, Ident)>,
    theme_binding: Ident,
    rules: RulesBlock,
}

/// One `state name(theme) { ... }` block. The name must be one of
/// the four well-known interaction states; arbitrary names are
/// rejected so the cross-platform contract is enforced at compile
/// time.
struct StateArm {
    name: Ident,
    theme_binding: Ident,
    rules: RulesBlock,
}

/// One `breakpoint name(theme) { ... }` block. The name must be one of
/// the overlay breakpoints (`sm`, `md`, `lg`, `xl`); `xs` is the
/// mobile-first base and is rejected so authors don't accidentally
/// write a base-shadowing overlay.
struct BreakpointArm {
    name: Ident,
    theme_binding: Ident,
    rules: RulesBlock,
}

/// One `container (min_width: N)(theme) { ... }` block. `min_width` is
/// the only comparison supported in v1 (mobile-first cascade);
/// `threshold` is the px length parsed from the literal. Stored under a
/// `__cq_minw_<bits>` axis whose name encodes the threshold losslessly.
struct ContainerArm {
    /// The `min_width` threshold in px.
    threshold: f32,
    theme_binding: Ident,
    rules: RulesBlock,
}

/// One line inside a `transitions { ... }` block. The `property` name
/// may be a shorthand (`padding`, `margin`, `border_radius`, etc.) that
/// fans out to multiple per-property transition fields during emit.
struct TransitionDecl {
    property: Ident,
    duration_ms: u32,
    easing: Ident,
    /// Optional explicit `cubic-bezier(a, b, c, d)` form. When set,
    /// `easing` is "CubicBezier" and `cubic_bezier` holds the four
    /// control points.
    cubic_bezier: Option<(Expr, Expr, Expr, Expr)>,
}

struct BaseBlock {
    theme_binding: Ident,
    rules: RulesBlock,
}

struct VariantAxisDecl {
    axis: Ident,
    arms: Vec<VariantArm>,
}

struct VariantArm {
    name: Ident,
    is_default: bool,
    theme_binding: Ident,
    rules: RulesBlock,
}

struct OverrideDecl {
    name: Ident,
    ty: Type,
}

/// A `{ field: expr, ... }` block — the contents of a base or variant arm.
struct RulesBlock {
    fields: Vec<(Ident, Expr)>,
}

// =============================================================================
// Parser
// =============================================================================

impl Parse for StyleSheetDecl {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let vis: Visibility = input.parse()?;
        let name: Ident = input.parse()?;
        let _lt: Token![<] = input.parse()?;
        let theme_ty: Type = input.parse()?;
        let _gt: Token![>] = input.parse()?;

        let body;
        braced!(body in input);

        // First section must be `base(...) { ... }`.
        let base_kw: Ident = body.parse()?;
        if base_kw != "base" {
            return Err(syn::Error::new(base_kw.span(), "expected `base(theme) { ... }`"));
        }
        let theme_args;
        parenthesized!(theme_args in body);
        let theme_binding: Ident = theme_args.parse()?;
        let rules = parse_rules_block(&body)?;
        let base = BaseBlock { theme_binding, rules };

        // Then any number of `variant axis { ... }`, `override field: Type`,
        // `transitions { ... }`, and `state name(theme) { ... }` lines, in
        // any order.
        let mut variants = Vec::new();
        let mut overrides = Vec::new();
        let mut transitions = Vec::new();
        let mut states = Vec::new();
        let mut breakpoints = Vec::new();
        let mut containers = Vec::new();
        let mut compounds = Vec::new();
        while !body.is_empty() {
            // `override` is a reserved Rust keyword, so we can't parse
            // it as an Ident. Detect it specifically via Token![override]
            // and treat `variant` / `transitions` / `state` as soft keywords.
            if body.peek(Token![override]) {
                let _: Token![override] = body.parse()?;
                let name: Ident = body.parse()?;
                let _colon: Token![:] = body.parse()?;
                let ty: Type = body.parse()?;
                overrides.push(OverrideDecl { name, ty });
                // Optional trailing comma between override decls.
                let _ = body.parse::<Token![,]>();
                continue;
            }
            let kw: Ident = body.parse()?;
            match kw.to_string().as_str() {
                "variant" => {
                    let axis: Ident = body.parse()?;
                    let arms_body;
                    braced!(arms_body in body);
                    let mut arms = Vec::new();
                    while !arms_body.is_empty() {
                        arms.push(parse_variant_arm(&arms_body)?);
                    }
                    variants.push(VariantAxisDecl { axis, arms });
                }
                "transitions" => {
                    let block;
                    braced!(block in body);
                    while !block.is_empty() {
                        transitions.push(parse_transition_decl(&block)?);
                    }
                }
                "state" => {
                    let name: Ident = body.parse()?;
                    // Whitelist the four well-known states. Arbitrary
                    // names would silently never activate (no backend
                    // listens for them), so we reject at parse time.
                    let allowed = ["hovered", "pressed", "focused", "disabled"];
                    if !allowed.contains(&name.to_string().as_str()) {
                        return Err(syn::Error::new(
                            name.span(),
                            format!(
                                "unknown state `{}`; expected one of: hovered, pressed, focused, disabled",
                                name
                            ),
                        ));
                    }
                    let theme_args;
                    parenthesized!(theme_args in body);
                    let theme_binding: Ident = theme_args.parse()?;
                    let rules = parse_rules_block(&body)?;
                    states.push(StateArm { name, theme_binding, rules });
                }
                "breakpoint" => {
                    let name: Ident = body.parse()?;
                    // Whitelist the overlay breakpoints. `xs` is the
                    // mobile-first base (the `base { … }` block IS the
                    // xs layout), so a `breakpoint xs` block is rejected
                    // — it would otherwise silently shadow the base.
                    let allowed = ["sm", "md", "lg", "xl"];
                    if !allowed.contains(&name.to_string().as_str()) {
                        return Err(syn::Error::new(
                            name.span(),
                            format!(
                                "unknown breakpoint `{}`; expected one of: sm, md, lg, xl \
                                 (`xs` is the mobile-first base — put those rules in `base`)",
                                name
                            ),
                        ));
                    }
                    let theme_args;
                    parenthesized!(theme_args in body);
                    let theme_binding: Ident = theme_args.parse()?;
                    let rules = parse_rules_block(&body)?;
                    breakpoints.push(BreakpointArm { name, theme_binding, rules });
                }
                "compound" => {
                    // Grammar mirrors `container`: a paren group for the
                    // condition, then the token-vocabulary binding, then
                    // the rules.
                    //
                    //   compound (pinned: left, row_hovered: on)(t) {
                    //       background: t.color.surface_alt(),
                    //   }
                    let cond;
                    parenthesized!(cond in body);
                    let mut when: Vec<(Ident, Ident)> = Vec::new();
                    while !cond.is_empty() {
                        let axis: Ident = cond.parse()?;
                        let _colon: Token![:] = cond.parse()?;
                        let value: Ident = cond.parse()?;
                        if when.iter().any(|(a, _)| *a == axis) {
                            return Err(syn::Error::new(
                                axis.span(),
                                format!(
                                    "axis `{axis}` named twice in one compound; an axis \
                                     holds one value at a time, so this can never match"
                                ),
                            ));
                        }
                        when.push((axis, value));
                        if cond.is_empty() {
                            break;
                        }
                        let _: Token![,] = cond.parse()?;
                    }
                    if when.len() < 2 {
                        return Err(syn::Error::new(
                            kw.span(),
                            "a compound needs at least two `axis: value` pairs; with one \
                             it is just that axis's arm — put the rules there instead",
                        ));
                    }
                    let theme_args;
                    parenthesized!(theme_args in body);
                    let theme_binding: Ident = theme_args.parse()?;
                    let rules = parse_rules_block(&body)?;
                    compounds.push(CompoundArm { when, theme_binding, rules });
                }
                "container" => {
                    // Grammar: `container (min_width: 400px)(theme) { … }`.
                    // The first paren group is the query; v1 supports only
                    // `min_width: <length>` (mobile-first cascade). The
                    // second is the token-vocabulary binding, same as
                    // `base`/`breakpoint`/`state`.
                    let query;
                    parenthesized!(query in body);
                    let cmp: Ident = query.parse()?;
                    if cmp != "min_width" {
                        return Err(syn::Error::new(
                            cmp.span(),
                            format!(
                                "unknown container query `{}`; v1 supports only `min_width` \
                                 (max_width / ranges are a planned extension)",
                                cmp
                            ),
                        ));
                    }
                    let _colon: Token![:] = query.parse()?;
                    let threshold = parse_px_length(&query)?;
                    let theme_args;
                    parenthesized!(theme_args in body);
                    let theme_binding: Ident = theme_args.parse()?;
                    let rules = parse_rules_block(&body)?;
                    containers.push(ContainerArm { threshold, theme_binding, rules });
                }
                other => {
                    return Err(syn::Error::new(
                        kw.span(),
                        format!(
                            "expected `variant`, `override`, `transitions`, `state`, \
                             `breakpoint`, or `container`, got `{}`",
                            other
                        ),
                    ));
                }
            }
        }

        // A compound whose condition names an axis or arm the sheet does
        // not declare can never match, and would fail SILENTLY — the
        // resolve simply never layers it. Reject it here, where the span
        // points at the author's typo.
        for c in &compounds {
            for (axis, value) in &c.when {
                let Some(declared) = variants.iter().find(|v| v.axis == *axis) else {
                    return Err(syn::Error::new(
                        axis.span(),
                        format!(
                            "compound names axis `{axis}`, which this sheet does not \
                             declare; a compound on an undeclared axis can never match"
                        ),
                    ));
                };
                if !declared.arms.iter().any(|a| a.name == *value) {
                    let known: Vec<String> =
                        declared.arms.iter().map(|a| a.name.to_string()).collect();
                    return Err(syn::Error::new(
                        value.span(),
                        format!(
                            "axis `{axis}` has no arm `{value}`; expected one of: {}",
                            known.join(", ")
                        ),
                    ));
                }
            }
        }

        Ok(StyleSheetDecl {
            vis,
            name,
            theme_ty,
            base,
            variants,
            overrides,
            transitions,
            states,
            breakpoints,
            containers,
            compounds,
        })
    }
}

/// Parse a px length literal for a container query: an integer (with an
/// optional `px` suffix, e.g. `400px` or `400`) or a float (`400.5`),
/// returned as `f32`. Rejects other unit suffixes so `400rem` is a clear
/// error rather than a silently-wrong threshold.
fn parse_px_length(input: ParseStream) -> syn::Result<f32> {
    let lit: syn::Lit = input.parse()?;
    match lit {
        syn::Lit::Int(i) => {
            let suffix = i.suffix();
            if !suffix.is_empty() && suffix != "px" {
                return Err(syn::Error::new(
                    i.span(),
                    format!("container `min_width` must be a px length; got suffix `{}`", suffix),
                ));
            }
            i.base10_parse::<f32>()
        }
        syn::Lit::Float(f) => {
            let suffix = f.suffix();
            if !suffix.is_empty() && suffix != "px" {
                return Err(syn::Error::new(
                    f.span(),
                    format!("container `min_width` must be a px length; got suffix `{}`", suffix),
                ));
            }
            f.base10_parse::<f32>()
        }
        other => Err(syn::Error::new(
            other.span(),
            "container `min_width` must be a numeric px length, e.g. `400px`",
        )),
    }
}

/// Parse one transition line: `property: 200ms EaseOut` or
/// `property: 200ms cubic_bezier(a, b, c, d)`.
fn parse_transition_decl(input: ParseStream) -> syn::Result<TransitionDecl> {
    let property: Ident = input.parse()?;
    let _: Token![:] = input.parse()?;

    // Duration: integer literal with `ms` suffix, e.g. `200ms`. We
    // accept the suffix as part of the literal token.
    let duration_lit: syn::LitInt = input.parse()?;
    let duration_ms = parse_duration_ms(&duration_lit)?;

    // Easing: a single ident (`EaseOut`, `Linear`, etc.) or a
    // `cubic_bezier(a, b, c, d)` call.
    let easing: Ident = input.parse()?;
    let cubic_bezier = if easing == "cubic_bezier" {
        let args;
        parenthesized!(args in input);
        let a: Expr = args.parse()?;
        let _: Token![,] = args.parse()?;
        let b: Expr = args.parse()?;
        let _: Token![,] = args.parse()?;
        let c: Expr = args.parse()?;
        let _: Token![,] = args.parse()?;
        let d: Expr = args.parse()?;
        Some((a, b, c, d))
    } else {
        None
    };

    // Optional trailing comma between transition decls.
    let _ = input.parse::<Token![,]>();

    Ok(TransitionDecl { property, duration_ms, easing, cubic_bezier })
}

/// Parse a `LitInt` whose suffix is `ms` (the `200ms` literal form).
/// Anything else is an error.
fn parse_duration_ms(lit: &syn::LitInt) -> syn::Result<u32> {
    let suffix = lit.suffix();
    if suffix != "ms" {
        return Err(syn::Error::new(
            lit.span(),
            format!("expected duration like `200ms`, found suffix `{}`", suffix),
        ));
    }
    lit.base10_parse::<u32>()
}

fn parse_variant_arm(input: ParseStream) -> syn::Result<VariantArm> {
    // Optional `#[default]` marker on the arm.
    let is_default = if input.peek(Token![#]) {
        let _: Token![#] = input.parse()?;
        let attr_content;
        syn::bracketed!(attr_content in input);
        let marker: Ident = attr_content.parse()?;
        if marker != "default" {
            return Err(syn::Error::new(
                marker.span(),
                "only `#[default]` is supported on variant arms",
            ));
        }
        true
    } else {
        false
    };
    let name: Ident = input.parse()?;
    let theme_args;
    parenthesized!(theme_args in input);
    let theme_binding: Ident = theme_args.parse()?;
    let rules = parse_rules_block(input)?;
    Ok(VariantArm { name, is_default, theme_binding, rules })
}

fn parse_rules_block(input: ParseStream) -> syn::Result<RulesBlock> {
    let block_content;
    braced!(block_content in input);
    let mut fields = Vec::new();
    while !block_content.is_empty() {
        let field: Ident = block_content.parse()?;
        let _colon: Token![:] = block_content.parse()?;
        let value: Expr = block_content.parse()?;
        fields.push((field, value));
        let _ = block_content.parse::<Token![,]>();
    }
    Ok(RulesBlock { fields })
}

// =============================================================================
// Emitter
// =============================================================================

/// FNV-1a 64 over the macro's raw input text. Feeds the preminted
/// class base (`iy-<12 hex chars>`): stable across builds of the same
/// source, shared by identical sheets, moved by any edit. The dump
/// build and the shipped build hash the same source, which is what
/// lets the `.css` and the runtime agree on names with no manifest.
pub fn content_hash(input: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in input.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub fn emit(decl: StyleSheetDecl, content_hash: u64) -> TokenStream2 {
    let enums = decl.variants.iter().map(|v| emit_variant_enum(&decl, v)).collect::<Vec<_>>();

    // Premint eligibility — one disqualifier, keeping the sheet on the
    // live-minting path everywhere (correct, just not preminted):
    //
    // - a `font_family` whose value is not a string literal: a non-literal
    //   value (`&INTER`, `active_font_family()`) can be a `Typeface`,
    //   whose `@font-face` + face-asset registration rides sheet
    //   registration (`ensure_typefaces_registered_with`) — exactly the
    //   step a preminted class skips. A string literal is always
    //   `FontFamily::System` (plain family names, no registration), so it
    //   stays eligible.
    //
    // (`shadow` used to disqualify too — the old single field lowered
    // per node kind, `text-shadow` on text vs `box-shadow` on boxes, and
    // a class name carries no kind. Since the `shadow`/`text_shadow`
    // split each field lowers to exactly one property, so shadowed
    // sheets premint like any other.)
    let premintable = !sheet_has_dynamic_font(&decl);
    let base_class = format!("iy-{:012x}", content_hash & 0xffff_ffff_ffff);
    let stylesheet_fn =
        emit_stylesheet_fn(&decl, premintable.then_some(base_class.as_str()));
    let builder = emit_builder(&decl, &base_class, premintable);
    let registration = if premintable {
        emit_premint_registration(&decl, &base_class)
    } else {
        TokenStream2::new()
    };

    quote! {
        #stylesheet_fn
        #(#enums)*
        #builder
        #registration
    }
}

/// `true` if any rules block sets `font_family` to a value the premint
/// pipeline can't prove constant — see the premint-eligibility note in
/// [`emit`].
///
/// Premintable font values:
/// - a string literal (`"ui-monospace, …"` → `FontFamily::System`,
///   needs no registration), and
/// - a path or `&`-reference expression (`&INTER`, `theme::MONO`) — a
///   reference to a `static`/`const` `Typeface`, constant by
///   construction. The dump build emits the family's `@font-face`
///   rules (with served-file URLs) into the preminted `.css`, standing
///   in for the runtime `register_typeface` that sheet registration
///   would have performed.
///
/// Everything else (a call like `active_font_family()`, a method
/// chain, a conditional) can vary at runtime, so the sheet stays on
/// the live-minting path.
fn sheet_has_dynamic_font(decl: &StyleSheetDecl) -> bool {
    fn constant_font_expr(expr: &Expr) -> bool {
        match expr {
            Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(_), .. }) => true,
            Expr::Path(_) => true,
            Expr::Reference(r) => constant_font_expr(&r.expr),
            _ => false,
        }
    }
    any_rules_block(decl, |b| {
        b.fields
            .iter()
            .any(|(name, expr)| name == "font_family" && !constant_font_expr(expr))
    })
}

/// Apply `pred` across every rules block on every layer of the sheet.
fn any_rules_block(decl: &StyleSheetDecl, pred: impl Fn(&RulesBlock) -> bool) -> bool {
    pred(&decl.base.rules)
        || decl.variants.iter().any(|axis| axis.arms.iter().any(|arm| pred(&arm.rules)))
        || decl.states.iter().any(|s| pred(&s.rules))
        || decl.breakpoints.iter().any(|b| pred(&b.rules))
        || decl.containers.iter().any(|c| pred(&c.rules))
}

/// The `cfg(idealyst_premint_dump)` linkme registration for one sheet —
/// the collection side the CLI's dump build links in. Never present in
/// shipped builds (the cfg is only set for the ephemeral dump binary,
/// paired with runtime-core's `style-dump` feature which provides
/// `runtime_core::premint`).
fn emit_premint_registration(decl: &StyleSheetDecl, base_class: &str) -> TokenStream2 {
    let stylesheet_fn = format_ident!("{}_style", snake_case(&decl.name));
    quote! {
        // The `#[cfg]` sits on the INNER static, not on this const:
        // when a cfg strips an item, the lint level for
        // `unexpected_cfgs` comes from the item's ANCESTORS — the
        // stripped item's own `#[allow]` is discarded with it. With the
        // allow here on the enclosing const, app crates (which don't
        // declare this build-pipeline cfg in check-cfg) compile
        // warning-free.
        #[allow(unexpected_cfgs)]
        const _: () = {
            #[cfg(idealyst_premint_dump)]
            #[::runtime_core::premint::linkme::distributed_slice(
                ::runtime_core::premint::PREMINT_SHEETS
            )]
            #[linkme(crate = ::runtime_core::premint::linkme)]
            static __PREMINT_SHEET: ::runtime_core::premint::PremintSheet =
                ::runtime_core::premint::PremintSheet {
                    base_class: #base_class,
                    sheet: #stylesheet_fn,
                };
        };
    }
}

/// Does any property expression in `rules` mention `binding`?
///
/// Decides whether the block gets a token-namespace binding emitted (see
/// `bind_tokens` in `emit_stylesheet_fn`). A bare-path match is the right
/// granularity: `t.spacing.md()` and `t.color.surface()` both parse with
/// `t` as an `ExprPath` receiver, and a false positive costs only an
/// unused ZST binding, while a false negative would be a confusing
/// "cannot find value `t`" in author code.
fn block_uses_binding(rules: &RulesBlock, binding: &Ident) -> bool {
    struct Finder<'a> {
        binding: &'a Ident,
        found: bool,
    }
    impl<'ast, 'a> Visit<'ast> for Finder<'a> {
        fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
            if self.found {
                return;
            }
            if node.path.is_ident(self.binding) {
                self.found = true;
                return;
            }
            visit::visit_expr_path(self, node);
        }
    }
    // A binding the author already marked unused (`_t`) never counts,
    // even if something in the block happens to name it — that spelling
    // IS the opt-out, and honouring it keeps `<()>`-typed sheets off the
    // `TokenVocabulary` projection entirely.
    if binding.to_string().starts_with('_') {
        return false;
    }
    let mut finder = Finder { binding, found: false };
    for (_, expr) in &rules.fields {
        finder.visit_expr(expr);
        if finder.found {
            return true;
        }
    }
    false
}

fn snake_case(ident: &Ident) -> Ident {
    let s = ident.to_string();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i != 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    Ident::new(&out, ident.span())
}

/// Emits the `Rc<StyleSheet>` constructor. The built sheet is cached
/// through `runtime_core::cached_stylesheet` — a single shared
/// thread-local registry keyed by a per-sheet `static` address — rather
/// than a `thread_local!` per sheet (see the constructor body for the
/// Android pthread-key-limit rationale).
///
/// Each rules closure takes `&VariantSet` and opens by binding the
/// block's declared ident to the vocabulary's token namespace, so a
/// block can name a token as `t.spacing.md()`. The namespace is a ZST
/// carrying names only — the theme's *values* still arrive at resolve
/// time through the token registry, which is what keeps a theme swap a
/// per-token write. Authors who relied on the old `theme.colors.fg`
/// field reads get a type error naming the vocabulary's namespace.
fn emit_stylesheet_fn(decl: &StyleSheetDecl, premint_class: Option<&str>) -> TokenStream2 {
    let fn_name = format_ident!("{}_style", snake_case(&decl.name));
    let vis = &decl.vis;
    let theme_ty = &decl.theme_ty;

    // Every block header names a binding (`base(t)`, `md(t)`, …). Bind it
    // to the declared vocabulary's token namespace so the block can spell
    // a token as `t.spacing.md()` instead of a string literal. The
    // namespace is zero-sized and carries only names — see
    // `runtime_shared::TokenVocabulary` for why the theme's *values* don't
    // (and must not) flow through here.
    //
    // Emitted ONLY for blocks that actually reference the binding. The
    // `<Theme as TokenVocabulary>` projection would otherwise be a hard
    // requirement on every declared type, and most sheets in the tree
    // name one that has no vocabulary at all (`<()>`, or a local marker
    // struct) purely to satisfy the grammar. Gating on use keeps those
    // compiling untouched and makes the trait an opt-in of exactly the
    // sheets that reference a token.
    //
    // `#[allow(unused_variables)]` guards the reverse case: a reference
    // this walk sees but the compiler doesn't (inside `stringify!`, a
    // skipped `cfg` arm), which would otherwise warn in author crates.
    let bind_tokens = |binding: &Ident, rules: &RulesBlock| {
        if !block_uses_binding(rules, binding) {
            return TokenStream2::new();
        }
        quote! {
            #[allow(unused_variables)]
            let #binding = <
                <#theme_ty as ::runtime_core::TokenVocabulary>::Tokens
                as ::core::default::Default
            >::default();
        }
    };
    // The base rules carry the transition declarations too. Transitions
    // are property values on `StyleRules` — same field layout, just
    // sitting alongside the regular property fields.
    let base_rules = emit_rules_struct_with_transitions(&decl.base.rules, &decl.transitions);

    // Per-axis variants: emit one `.variant(...)` call per arm. Tried
    // collapsing N per-arm closures into one match-dispatched
    // `.variant_axis(...)` closure per axis: bundle GREW ~21 KB raw
    // / ~15 KB gz on the website. Best theory: wasm-opt's
    // function-merging pass was already deduplicating similar per-arm
    // closure bodies ACROSS stylesheets (they share the same
    // `Option`-of-`StyleRules` shell, only the inlined constants
    // differ). One big match defeats those merges. Stick with the
    // per-arm shape — the linker is smarter than collapsing here.
    let variant_chain = decl.variants.iter().flat_map(|axis| {
        let axis_name = axis.axis.to_string();
        let arm_calls = axis.arms.iter().map(|arm| {
            let arm_name = arm.name.to_string();
            let rules = emit_rules_struct(&arm.rules);
            let bind = bind_tokens(&arm.theme_binding, &arm.rules);
            quote! {
                .variant(#axis_name, #arm_name, |_vs: &::runtime_core::VariantSet| {
                    #bind
                    #rules
                })
            }
        }).collect::<Vec<_>>();
        let default_calls = axis.arms.iter().filter(|a| a.is_default).map(|arm| {
            let axis_str = &axis_name;
            let arm_str = arm.name.to_string();
            quote! { .variant_default(#axis_str, #arm_str) }
        }).collect::<Vec<_>>();
        arm_calls.into_iter().chain(default_calls)
    });

    // States: each is its own variant axis with a single "on" value,
    // so multiple states can be active simultaneously (a hovered +
    // focused button gets both overlays merged). The axis names are
    // namespaced with `__state_` to keep them out of the regular
    // variant namespace. Resolution and pre-generation reuse the
    // variant machinery — applying a state change at runtime is a
    // class swap, not a rule mint.
    let state_chain = decl.states.iter().map(|arm| {
        let axis = format!("__state_{}", arm.name);
        let rules = emit_rules_struct(&arm.rules);
        let bind = bind_tokens(&arm.theme_binding, &arm.rules);
        quote! {
            .variant(#axis, "on", |_vs: &::runtime_core::VariantSet| {
                #bind
                #rules
            })
        }
    });

    // Breakpoints: each `breakpoint md { … }` block is a variant
    // overlay under the reserved `__bp_md` axis (single "on" value),
    // exactly like states. The framework recognizes the `__bp_*`
    // namespace and realizes it per-backend (CSS `@media` on web,
    // reactive merge on native).
    let breakpoint_chain = decl.breakpoints.iter().map(|arm| {
        let axis = format!("__bp_{}", arm.name);
        let rules = emit_rules_struct(&arm.rules);
        let bind = bind_tokens(&arm.theme_binding, &arm.rules);
        quote! {
            .variant(#axis, "on", |_vs: &::runtime_core::VariantSet| {
                #bind
                #rules
            })
        }
    });

    // Container overlays: each `container (min_width: N) { … }` block is a
    // variant overlay under a `__cq_minw_<bits>` axis (single "on" value).
    // The threshold is encoded as the lossless 8-char hex of its `f32`
    // bit pattern — the SAME encoding `runtime_core::container_axis_name`
    // produces, so the runtime decodes the px value back via
    // `container_axis_threshold`. The framework realizes the namespace
    // per-backend (CSS `@container` on web, reactive merge on native).
    // Compounds: layered by `resolve` AFTER every per-axis overlay, so
    // this is how a sheet states which of two axes wins on a shared
    // property instead of leaving it to alphabetical axis order.
    let compound_chain = decl.compounds.iter().map(|arm| {
        let pairs = arm.when.iter().map(|(axis, value)| {
            let a = axis.to_string();
            let v = value.to_string();
            quote! { (#a, #v) }
        });
        let rules = emit_rules_struct(&arm.rules);
        let bind = bind_tokens(&arm.theme_binding, &arm.rules);
        quote! {
            .compound(
                ::std::vec![#(#pairs),*],
                |_vs: &::runtime_core::VariantSet| {
                    #bind
                    #rules
                },
            )
        }
    });

    let container_chain = decl.containers.iter().map(|arm| {
        let axis = format!("__cq_minw_{:08x}", arm.threshold.to_bits());
        let rules = emit_rules_struct(&arm.rules);
        let bind = bind_tokens(&arm.theme_binding, &arm.rules);
        quote! {
            .variant(#axis, "on", |_vs: &::runtime_core::VariantSet| {
                #bind
                #rules
            })
        }
    });

    // A premintable sheet carries its class on the sheet OBJECT, not just
    // in the generated builder's fast path. That is what lets the
    // `StyleApplication::new(Foo::sheet())` idiom — which skips the
    // builder entirely, and is how most of idea-ui and the websites style
    // things — resolve to the build-time class instead of falling through
    // to the live engine. The CSS was already being emitted for these
    // sheets (they register in `PREMINT_SHEETS` whenever `premintable`);
    // only the runtime half was missing.
    let bind_base = bind_tokens(&decl.base.theme_binding, &decl.base.rules);
    let sheet_expr = quote! {
        ::runtime_core::StyleSheet::new(
            |_vs: &::runtime_core::VariantSet| {
                #bind_base
                #base_rules
            }
        )
            #(#variant_chain)*
            #(#state_chain)*
            #(#breakpoint_chain)*
            #(#container_chain)*
            #(#compound_chain)*
    };
    let finish_sheet = match premint_class {
        Some(class) => quote! { #sheet_expr.premint_with_class(#class) },
        None => quote! { ::std::rc::Rc::new(#sheet_expr) },
    };

    quote! {
        #vis fn #fn_name() -> ::std::rc::Rc<::runtime_core::StyleSheet> {
            // Process-unique key for this stylesheet: the address of a
            // function-local `static`. We deliberately DO NOT emit a
            // per-sheet `::std::thread_local!` here — on Android, std's
            // TLS is pthread-key-backed and bionic caps total keys at
            // PTHREAD_KEYS_MAX (128). idea-ui declares 70+ stylesheets, so
            // a key apiece exhausted the table and aborted in
            // `LazyKey::lazy_init` at mount. Instead every sheet routes
            // through runtime-core's single shared thread-local registry,
            // keyed by this address — one TLS key for all stylesheets.
            static __SHEET_KEY: u8 = 0;
            ::runtime_core::cached_stylesheet(
                &__SHEET_KEY as *const u8 as usize,
                || #finish_sheet,
            )
        }
    }
}

/// Emits `StyleRules { field: value, ..Default::default() }`.
///
/// Two pieces of sugar:
///
/// 1. Each property value is wrapped in `Into::into(...)` so authors
///    can write `padding: 16` (i32) for a `Length`-typed field, or
///    `background: Color(...)` for a `Color`-typed field, etc. The
///    target type's `From` impls drive the conversion.
///
/// 2. Shorthand property names expand to multiple per-side fields:
///    - `padding: V` → all four `padding_*` fields set to `V.into()`
///    - `padding_horizontal: V` → `padding_left` + `padding_right`
///    - `padding_vertical: V` → `padding_top` + `padding_bottom`
///    Same for `margin`, `border_radius`, `border_width`,
///    `border_color`.
fn emit_rules_struct(rules: &RulesBlock) -> TokenStream2 {
    let mut field_assignments: Vec<TokenStream2> = Vec::new();
    for (name, value) in &rules.fields {
        for (field_name, expr) in expand_field(name, value) {
            field_assignments.push(quote! {
                #field_name: ::std::option::Option::Some(::std::convert::Into::into(#expr))
            });
        }
    }
    quote! {
        ::runtime_core::StyleRules {
            #(#field_assignments,)*
            ..::std::default::Default::default()
        }
    }
}

/// Expand a single declared field into one or more concrete
/// `StyleRules` fields. Shorthand names like `padding` fan out into
/// `padding_top`/`_right`/`_bottom`/`_left`. The value expression is
/// shared (re-used) across all fan-out fields — relying on the
/// value's type being `Copy` or cheap to clone, which is true for
/// `Length`/`f32`/`Color::clone()` etc. We clone the token stream so
/// each output field has its own copy of the expression.
fn expand_field(name: &Ident, value: &Expr) -> Vec<(Ident, TokenStream2)> {
    let v = quote!(#value);
    let span = name.span();
    let mk = |n: &str| Ident::new(n, span);
    match name.to_string().as_str() {
        "padding" => vec![
            (mk("padding_top"), v.clone()),
            (mk("padding_right"), v.clone()),
            (mk("padding_bottom"), v.clone()),
            (mk("padding_left"), v),
        ],
        "padding_horizontal" => vec![
            (mk("padding_left"), v.clone()),
            (mk("padding_right"), v),
        ],
        "padding_vertical" => vec![
            (mk("padding_top"), v.clone()),
            (mk("padding_bottom"), v),
        ],
        "margin" => vec![
            (mk("margin_top"), v.clone()),
            (mk("margin_right"), v.clone()),
            (mk("margin_bottom"), v.clone()),
            (mk("margin_left"), v),
        ],
        "margin_horizontal" => vec![
            (mk("margin_left"), v.clone()),
            (mk("margin_right"), v),
        ],
        "margin_vertical" => vec![
            (mk("margin_top"), v.clone()),
            (mk("margin_bottom"), v),
        ],
        "border_radius" => vec![
            (mk("border_top_left_radius"), v.clone()),
            (mk("border_top_right_radius"), v.clone()),
            (mk("border_bottom_left_radius"), v.clone()),
            (mk("border_bottom_right_radius"), v),
        ],
        "border_width" => vec![
            (mk("border_top_width"), v.clone()),
            (mk("border_right_width"), v.clone()),
            (mk("border_bottom_width"), v.clone()),
            (mk("border_left_width"), v),
        ],
        "border_color" => vec![
            (mk("border_top_color"), v.clone()),
            (mk("border_right_color"), v.clone()),
            (mk("border_bottom_color"), v.clone()),
            (mk("border_left_color"), v),
        ],
        _ => vec![(name.clone(), v)],
    }
}

/// Like `emit_rules_struct` but also injects per-property transition
/// fields. Transitions are declared once at the stylesheet level
/// (in the `transitions { ... }` block) and apply to the base rule
/// set — variants inherit them via the merge logic.
fn emit_rules_struct_with_transitions(
    rules: &RulesBlock,
    transitions: &[TransitionDecl],
) -> TokenStream2 {
    let mut field_assignments: Vec<TokenStream2> = Vec::new();
    for (name, value) in &rules.fields {
        for (field_name, expr) in expand_field(name, value) {
            field_assignments.push(quote! {
                #field_name: ::std::option::Option::Some(::std::convert::Into::into(#expr))
            });
        }
    }
    for t in transitions {
        for field_name in expand_transition_property(&t.property) {
            let value = transition_value_expr(t);
            field_assignments.push(quote! {
                #field_name: ::std::option::Option::Some(#value)
            });
        }
    }
    quote! {
        ::runtime_core::StyleRules {
            #(#field_assignments,)*
            ..::std::default::Default::default()
        }
    }
}

/// Build the `Transition` value expression from a parsed declaration.
fn transition_value_expr(t: &TransitionDecl) -> TokenStream2 {
    let duration = t.duration_ms;
    let easing = if let Some((a, b, c, d)) = &t.cubic_bezier {
        quote! { ::runtime_core::Easing::CubicBezier(#a as f32, #b as f32, #c as f32, #d as f32) }
    } else {
        let variant = &t.easing;
        quote! { ::runtime_core::Easing::#variant }
    };
    quote! {
        ::runtime_core::Transition::new(#duration, #easing)
    }
}

/// Expand a transition property name into the concrete
/// `*_transition` field names on `StyleRules`. Mirrors the
/// `expand_field` fanout for regular properties, so authors can write
/// `padding: 200ms EaseOut` and get all four sides animated.
fn expand_transition_property(name: &Ident) -> Vec<Ident> {
    let span = name.span();
    let mk = |n: &str| Ident::new(n, span);
    match name.to_string().as_str() {
        // Shorthands fanning out to multiple sides.
        "padding" => vec![
            mk("padding_top_transition"),
            mk("padding_right_transition"),
            mk("padding_bottom_transition"),
            mk("padding_left_transition"),
        ],
        "padding_horizontal" => vec![
            mk("padding_left_transition"),
            mk("padding_right_transition"),
        ],
        "padding_vertical" => vec![
            mk("padding_top_transition"),
            mk("padding_bottom_transition"),
        ],
        "margin" => vec![
            mk("margin_top_transition"),
            mk("margin_right_transition"),
            mk("margin_bottom_transition"),
            mk("margin_left_transition"),
        ],
        "margin_horizontal" => vec![
            mk("margin_left_transition"),
            mk("margin_right_transition"),
        ],
        "margin_vertical" => vec![
            mk("margin_top_transition"),
            mk("margin_bottom_transition"),
        ],
        "border_radius" => vec![
            mk("border_top_left_radius_transition"),
            mk("border_top_right_radius_transition"),
            mk("border_bottom_left_radius_transition"),
            mk("border_bottom_right_radius_transition"),
        ],
        "border_width" => vec![
            mk("border_top_width_transition"),
            mk("border_right_width_transition"),
            mk("border_bottom_width_transition"),
            mk("border_left_width_transition"),
        ],
        "border_color" => vec![
            mk("border_top_color_transition"),
            mk("border_right_color_transition"),
            mk("border_bottom_color_transition"),
            mk("border_left_color_transition"),
        ],
        // Single-property: just append `_transition`. Authors write
        // `background: 200ms EaseOut`; we map to `background_transition`.
        other => vec![mk(&format!("{}_transition", other))],
    }
}

/// Emits `pub enum CardSize { Small, Medium, Large } + Default + ToStr`.
fn emit_variant_enum(decl: &StyleSheetDecl, axis: &VariantAxisDecl) -> TokenStream2 {
    let enum_name = format_ident!("{}{}", decl.name, pascal(&axis.axis));
    let vis = &decl.vis;
    let variants = axis.arms.iter().map(|arm| {
        let v = format_ident!("{}", pascal(&arm.name));
        quote! { #v }
    });
    // For Default, pick the arm marked #[default]. If none, no Default impl.
    let default_impl = axis.arms.iter().find(|a| a.is_default).map(|arm| {
        let v = format_ident!("{}", pascal(&arm.name));
        quote! {
            impl ::std::default::Default for #enum_name {
                fn default() -> Self { Self::#v }
            }
        }
    });
    // to_variant_str: snake-case the arm name.
    let arm_arms = axis.arms.iter().map(|arm| {
        let v = format_ident!("{}", pascal(&arm.name));
        let s = arm.name.to_string();
        quote! { Self::#v => #s }
    });
    // all_variants(): every variant in declaration order, as a
    // 'static slice. Used by reflective tooling (notably the docs
    // app's `DocControls` derive) to enumerate variant pickers.
    let all_variants_items = axis.arms.iter().map(|arm| {
        let v = format_ident!("{}", pascal(&arm.name));
        quote! { Self::#v }
    });
    quote! {
        #[derive(::std::clone::Clone, ::std::marker::Copy, ::std::fmt::Debug, ::std::cmp::PartialEq, ::std::cmp::Eq)]
        #vis enum #enum_name {
            #(#variants,)*
        }
        impl ::runtime_core::VariantEnum for #enum_name {
            fn as_variant_str(self) -> &'static str {
                match self {
                    #(#arm_arms,)*
                }
            }
            fn all_variants() -> &'static [Self] {
                &[ #(#all_variants_items,)* ]
            }
        }
        #default_impl
    }
}

/// snake → Pascal. `border_radius` → `BorderRadius`.
fn pascal(ident: &Ident) -> Ident {
    let s = ident.to_string();
    let mut out = String::with_capacity(s.len());
    let mut next_upper = true;
    for c in s.chars() {
        if c == '_' {
            next_upper = true;
        } else if next_upper {
            out.extend(c.to_uppercase());
            next_upper = false;
        } else {
            out.push(c);
        }
    }
    Ident::new(&out, ident.span())
}

/// Emits `pub struct Card { ... }` + `impl` + `IntoStyleSource`.
///
/// The builder stores `Option<Box<dyn Fn() -> X>>` per field. Static
/// callers wrap their value in a constant closure; reactive callers
/// pass a `Signal<T>` or `derived(...)`. A `__reactive` flag records
/// whether any setter received a reactive source. `into_style_source`
/// then emits `StyleSource::Reactive` (signal changes re-apply the
/// style) when the flag is set, and the cheaper `StyleSource::Static`
/// (no per-node Effect) when every input was constant.
fn emit_builder(decl: &StyleSheetDecl, base_class: &str, premintable: bool) -> TokenStream2 {
    let name = &decl.name;
    let vis = &decl.vis;
    let entry_fn = name; // `Card()` returns `Card` — see free function below.
    let stylesheet_fn = format_ident!("{}_style", snake_case(name));
    // Per-axis class-segment assembly for the premint branch. One class
    // per SELECTED axis (`<base>-<axis>-<value>`, space-separated after
    // the base class): the dump emits each arm as a standalone DELTA
    // rule in the resolver's merge order, and the CSS source-order
    // cascade reproduces `StyleSheet::resolve`'s later-wins merge — so
    // CSS size is the sum of arms, not their cartesian product. An
    // unset axis contributes its `#[default]` arm's class (same rules
    // resolution as an explicit default selection), or nothing when the
    // axis declares no default (no arm active ⇒ no delta to apply).
    let premint_axis_pushes: Vec<TokenStream2> = decl
        .variants
        .iter()
        .map(|axis| {
            let f = format_ident!("__v_{}", axis.axis);
            let seg_prefix = format!(" {}-{}-", base_class, axis.axis);
            match axis.arms.iter().find(|a| a.is_default) {
                Some(d) => {
                    let dname = d.name.to_string();
                    quote! {
                        __class.push_str(#seg_prefix);
                        __class.push_str(match self.#f.as_ref() {
                            ::std::option::Option::Some(g) => g(),
                            ::std::option::Option::None => #dname,
                        });
                    }
                }
                None => quote! {
                    if let ::std::option::Option::Some(g) = self.#f.as_ref() {
                        __class.push_str(#seg_prefix);
                        __class.push_str(g());
                    }
                },
            }
        })
        .collect();
    let premint_override_fields: Vec<_> = decl
        .overrides
        .iter()
        .map(|o| format_ident!("__o_{}", o.name))
        .collect();

    // Per-axis fields and setters.
    let axis_fields = decl.variants.iter().map(|axis| {
        let f = format_ident!("__v_{}", axis.axis);
        quote! { #f: ::std::option::Option<::std::boxed::Box<dyn Fn() -> &'static str>> }
    });
    let axis_setters = decl.variants.iter().map(|axis| {
        let setter = &axis.axis;
        let f = format_ident!("__v_{}", axis.axis);
        let enum_name = format_ident!("{}{}", decl.name, pascal(&axis.axis));
        quote! {
            pub fn #setter<V: ::runtime_core::IntoVariantSource<#enum_name>>(mut self, value: V) -> Self {
                // A reactive source (Signal / `derived`) forces the whole
                // builder onto `StyleSource::Reactive` so the style
                // re-applies when the signal changes. Read reactivity
                // BEFORE `into_variant_source` consumes `value`.
                self.__reactive = self.__reactive
                    || <V as ::runtime_core::IntoVariantSource<#enum_name>>::is_reactive(&value);
                self.#f = ::std::option::Option::Some(value.into_variant_source());
                self
            }
        }
    });

    // Per-override fields and setters.
    let override_fields = decl.overrides.iter().map(|o| {
        let f = format_ident!("__o_{}", o.name);
        let ty = &o.ty;
        quote! { #f: ::std::option::Option<::std::boxed::Box<dyn Fn() -> #ty>> }
    });
    let override_setters = decl.overrides.iter().map(|o| {
        let setter = &o.name;
        let f = format_ident!("__o_{}", o.name);
        let ty = &o.ty;
        quote! {
            pub fn #setter<V: ::runtime_core::IntoOverrideSource<#ty>>(mut self, value: V) -> Self {
                self.__reactive = self.__reactive
                    || <V as ::runtime_core::IntoOverrideSource<#ty>>::is_reactive(&value);
                self.#f = ::std::option::Option::Some(value.into_override_source());
                self
            }
        }
    });

    let default_axis_fields = decl.variants.iter().map(|axis| {
        let f = format_ident!("__v_{}", axis.axis);
        quote! { #f: ::std::option::Option::None }
    });
    let default_override_fields = decl.overrides.iter().map(|o| {
        let f = format_ident!("__o_{}", o.name);
        quote! { #f: ::std::option::Option::None }
    });

    // Resolution closure body for IntoStyleSource. Reads each closure
    // (which may subscribe to a Signal) and applies to the
    // StyleApplication.
    let axis_applies: Vec<TokenStream2> = decl.variants.iter().map(|axis| {
        let axis_str = axis.axis.to_string();
        let f = format_ident!("__v_{}", axis.axis);
        quote! {
            if let ::std::option::Option::Some(g) = self.#f.as_ref() {
                __app = __app.with(#axis_str, g());
            }
        }
    }).collect();
    let override_applies: Vec<TokenStream2> = decl.overrides.iter().map(|o| {
        let f = format_ident!("__o_{}", o.name);
        let method = format_ident!("override_{}", o.name);
        quote! {
            if let ::std::option::Option::Some(g) = self.#f.as_ref() {
                __app = __app.#method(g());
            }
        }
    }).collect();

    // Emitted only for premint-eligible sheets (see `sheet_has_shadow`);
    // ineligible sheets compile to the live path with no cfg block at all.
    //
    // Two variants of the preminted return: the old core's
    // `StyleSource::Preminted` vs the new core's `StyleProp::Preminted`
    // (same class-string assembly either way). Selected by this CRATE's
    // `new-core` feature — the same switch that retargets the whole
    // expansion (`finish`), so the `::runtime_core::…` path below lands
    // on `::runtime_vocabulary::glue::StyleProp` post-retarget.
    let premint_return = quote! {
        return ::runtime_core::StyleProp::Preminted {
            // The generated builder has no inline-layer surface yet; an
            // author reaching for one uses `StyleApplication::with_inline`
            // directly. (This fast path already bails on any override.)
            inline: ::core::option::Option::None,
            class: ::std::borrow::Cow::Owned(__class),
            overrides: ::std::option::Option::None,
        };
    };
    // The REACTIVE preminted path. Every arm of every axis already has a
    // rule in the shipped `.css` (the dump emits `-active-on` AND
    // `-active-off`), so an axis driven by a signal is a CLASS SWAP, not a
    // rule mint — the closure below re-reads the axis sources, and the
    // per-node effect behind `PremintedDynamic` re-stamps. This is what
    // makes selection UI premintable: 46 of 68 fall-throughs measured on
    // the component catalog were one nav-item sheet whose only reactivity
    // was `active`, and each of them dragged in the whole style engine.
    //
    // Note the axis reads happen INSIDE the closure, so the effect
    // subscribes to exactly the signals the author's sources touch —
    // including a `derived(...)` reading several at once, which
    // `SignalClass` (one signal id) cannot express.
    let premint_dynamic_return = quote! {
        return ::runtime_core::StyleProp::PremintedDynamic {
            class_of: ::std::boxed::Box::new(move || {
                let mut __class = ::std::string::String::from(#base_class);
                #(#premint_axis_pushes)*
                __class
            }),
            overrides: ::std::option::Option::None,
        };
    };
    let premint_branch = if premintable {
        quote! {
            // Preminted fast path (web builds with build-time CSS):
            // a builder with no runtime slot overrides resolves to class
            // names the CLI's style-dump pass already wrote into the
            // shipped `.css` — no StyleRules work at runtime, constant or
            // reactive. Only an override falls through to the live engine.
            //
            // With no `override` slots declared — the overwhelming case —
            // `__any_override` folds to a literal `false`, both branches
            // return, and the live path below becomes provably dead. That
            // is what lets LLVM drop this sheet's entire arm tree, not
            // just skip it at runtime.
            #[cfg(idealyst_premint)]
            {
                let __any_override = false #(|| self.#premint_override_fields.is_some())*;
                if !__any_override {
                    if self.__reactive {
                        #premint_dynamic_return
                    }
                    let mut __class = ::std::string::String::from(#base_class);
                    #(#premint_axis_pushes)*
                    #premint_return
                }
            }
        }
    } else {
        TokenStream2::new()
    };

    // The builder → style-value conversion impl. Old core:
    // `IntoStyleSource` → `StyleSource::{Static,Reactive}`. New core:
    // `IntoStyleProp` → `StyleProp::{Sheet,SheetDynamic}` — the same
    // static-vs-reactive split (`__reactive` gates it), lowered onto the
    // vocabulary's sheet paths (static → cohort enrollment, reactive →
    // per-node binding effect). Paths are spelled `::runtime_core::…`
    // in BOTH arms on purpose: the shared `finish()` retarget rewrites
    // them to `::runtime_vocabulary::glue::…` under `new-core`, where
    // the glue re-exports resolve them (`IntoStyleProp`/`StyleProp` are
    // glue-only names — they do not exist in runtime-core, which is
    // fine: this arm is only emitted when the retarget runs).
    let conversion_impl = quote! {
        #[allow(unexpected_cfgs)]
        impl ::runtime_core::IntoStyleProp for #name {
            fn into_style_prop(self) -> ::runtime_core::StyleProp {
                #premint_branch
                // Same static-vs-reactive routing as the old core's
                // `IntoStyleSource` (see the sibling emission): constant
                // builders take the cohort path (`Sheet`), any reactive
                // input (`Signal<E>` isn't available under new-core —
                // use `derived(move || sig.get())`, whose closure reads
                // subscribe the binding effect) takes the per-node
                // effect path (`SheetDynamic`).
                let __reactive = self.__reactive;
                let __build = move || {
                    let mut __app = ::runtime_core::StyleApplication::new(#stylesheet_fn());
                    #(#axis_applies)*
                    #(#override_applies)*
                    __app
                };
                if __reactive {
                    ::runtime_core::StyleProp::SheetDynamic(::std::boxed::Box::new(__build))
                } else {
                    ::runtime_core::StyleProp::Sheet(::std::boxed::Box::new(__build()))
                }
            }
        }
    };

    quote! {
        #vis struct #name {
            #(#axis_fields,)*
            #(#override_fields,)*
            /// `true` once any setter received a reactive source
            /// (`Signal` / `derived`). Gates the `Static` vs `Reactive`
            /// emission in `into_style_source`.
            __reactive: ::std::primitive::bool,
        }

        impl #name {
            pub fn new() -> Self {
                Self {
                    #(#default_axis_fields,)*
                    #(#default_override_fields,)*
                    __reactive: false,
                }
            }

            /// Convenience accessor for the underlying `Rc<StyleSheet>`
            /// in case authors want the raw sheet (e.g. to pass to APIs
            /// that don't take a builder).
            pub fn sheet() -> ::std::rc::Rc<::runtime_core::StyleSheet> {
                #stylesheet_fn()
            }

            #(#axis_setters)*
            #(#override_setters)*

            /// The live-engine `StyleApplication` this builder describes —
            /// for components that COMPOSE or INTROSPECT resolved styles
            /// (merge an inherited color onto a label sheet, layer a
            /// reactive hover onto a cell) rather than hand the style
            /// straight to a node. Deliberately bypasses the premint fast
            /// path: composition requires the resolution engine, so
            /// anything derived from this stays live-minted even in
            /// `--premint` builds. Reactive setter inputs are read ONCE
            /// here (no subscription) — reactive callers stay on
            /// `into_style_source`.
            pub fn into_style_application(self) -> ::runtime_core::StyleApplication {
                let mut __app = ::runtime_core::StyleApplication::new(#stylesheet_fn());
                #(#axis_applies)*
                #(#override_applies)*
                __app
            }
        }

        impl ::std::default::Default for #name {
            fn default() -> Self { Self::new() }
        }

        #conversion_impl

        /// Entry point: `Card()` returns a fresh builder. The free
        /// function shadows the struct name for call sites like
        /// `Card().size(...).kind(...)`.
        #[allow(non_snake_case)]
        #vis fn #entry_fn() -> #name { #name::new() }
    }
}
