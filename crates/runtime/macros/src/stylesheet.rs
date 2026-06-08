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
//!     pub Card<Theme> {
//!         base(theme) {
//!             background: Color(theme.colors.surface.clone()),
//!             padding: theme.spacing.medium,
//!             border_radius: 8.0,
//!         }
//!         variant size {
//!             small(theme)  { padding: theme.spacing.medium * 0.5 }
//!             #[default]
//!             medium(_theme) {}
//!             large(theme)  { padding: theme.spacing.medium * 2.0 }
//!         }
//!         variant kind {
//!             #[default]
//!             elevated(theme) { background: Color(theme.colors.surface.clone()) }
//!             outlined(theme) {
//!                 background: Color("transparent".into()),
//!                 border: Border::new(2, theme.colors.foreground.clone()),
//!             }
//!         }
//!         override padding: f32
//!     }
//! }
//! ```
//!
//! # What it generates
//!
//! For the above declaration, with builder name `Card` and theme
//! type `Theme`:
//!
//! - `pub fn card_style() -> Rc<StyleSheet>` — convention-name version
//!   of the stylesheet (snake_case + `_style` suffix). Cached in a
//!   thread-local so repeat calls return the same `Rc`.
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
    /// Parsed for syntactic backward-compatibility (`pub Card<Theme>`).
    /// Ignored at emission — stylesheet closures take `&VariantSet`,
    /// not a theme reference. See `check_no_theme_refs`.
    #[allow(dead_code)]
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
}

/// One `state name(theme) { ... }` block. The name must be one of
/// the four well-known interaction states; arbitrary names are
/// rejected so the cross-platform contract is enforced at compile
/// time.
struct StateArm {
    name: Ident,
    #[allow(dead_code)]
    theme_binding: Ident,
    rules: RulesBlock,
}

/// One `breakpoint name(theme) { ... }` block. The name must be one of
/// the overlay breakpoints (`sm`, `md`, `lg`, `xl`); `xs` is the
/// mobile-first base and is rejected so authors don't accidentally
/// write a base-shadowing overlay.
struct BreakpointArm {
    name: Ident,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
                "container" => {
                    // Grammar: `container (min_width: 400px)(theme) { … }`.
                    // The first paren group is the query; v1 supports only
                    // `min_width: <length>` (mobile-first cascade). The
                    // second is the (vestigial) theme binding, kept for
                    // consistency with `base`/`breakpoint`/`state`.
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

pub fn emit(decl: StyleSheetDecl) -> TokenStream2 {
    if let Err(err) = check_no_theme_refs(&decl) {
        return err.to_compile_error();
    }
    let stylesheet_fn = emit_stylesheet_fn(&decl);
    let enums = decl.variants.iter().map(|v| emit_variant_enum(&decl, v)).collect::<Vec<_>>();
    let builder = emit_builder(&decl);

    quote! {
        #stylesheet_fn
        #(#enums)*
        #builder
    }
}

/// Walk every rules-block expression in the declaration and reject
/// references to the theme binding (`base(theme) { theme.colors.fg }`,
/// `base(t) { t.colors().fg }` — whatever name the author chose for
/// the binding). The token primitives now live in `runtime-core`,
/// so author code emits `Tokenized::Token { name, fallback }`
/// directly. The legacy theme-struct pattern (where rules could read
/// `theme.colors.primary` etc) lives in `idea-ui`'s theme runtime but
/// it's no longer wired through the stylesheet macro — emitting a
/// clear error here keeps the migration explicit.
fn check_no_theme_refs(decl: &StyleSheetDecl) -> syn::Result<()> {
    // Collect every theme-binding name the declaration uses so the
    // check is agnostic to whether the author wrote `theme`, `t`, or
    // anything else. Bindings prefixed with `_` (idiomatic "unused")
    // are skipped — those don't reference the theme by name in the
    // body anyway, and rejecting them would flag stylesheets that
    // already opted out by writing `_theme`.
    let mut bindings: Vec<String> = Vec::new();
    let add = |list: &mut Vec<String>, ident: &Ident| {
        let s = ident.to_string();
        if !s.starts_with('_') && !list.contains(&s) {
            list.push(s);
        }
    };
    add(&mut bindings, &decl.base.theme_binding);
    for axis in &decl.variants {
        for arm in &axis.arms {
            add(&mut bindings, &arm.theme_binding);
        }
    }
    for arm in &decl.states {
        add(&mut bindings, &arm.theme_binding);
    }
    for arm in &decl.breakpoints {
        add(&mut bindings, &arm.theme_binding);
    }
    for arm in &decl.containers {
        add(&mut bindings, &arm.theme_binding);
    }

    struct Finder<'a> {
        bindings: &'a [String],
        offender: Option<syn::Ident>,
    }
    impl<'ast, 'a> Visit<'ast> for Finder<'a> {
        fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
            if self.offender.is_some() {
                return;
            }
            // Detect a bare ident path matching one of the
            // theme-binding names (`theme`, `t`, etc.). This catches
            // both `theme.colors.fg` field chains and
            // `theme.colors()` method chains, since the receiver
            // parses as an `ExprPath`.
            if let Some(seg) = node.path.segments.last() {
                let name = seg.ident.to_string();
                if self.bindings.iter().any(|b| b == &name) {
                    self.offender = Some(seg.ident.clone());
                    return;
                }
            }
            visit::visit_expr_path(self, node);
        }
    }
    let mut finder = Finder { bindings: &bindings, offender: None };
    let mut blocks: Vec<&RulesBlock> = vec![&decl.base.rules];
    for axis in &decl.variants {
        for arm in &axis.arms {
            blocks.push(&arm.rules);
        }
    }
    for arm in &decl.states {
        blocks.push(&arm.rules);
    }
    for arm in &decl.breakpoints {
        blocks.push(&arm.rules);
    }
    for arm in &decl.containers {
        blocks.push(&arm.rules);
    }
    for block in blocks {
        for (_, expr) in &block.fields {
            finder.visit_expr(expr);
            if let Some(offender) = finder.offender.take() {
                return Err(syn::Error::new(
                    offender.span(),
                    "theme.* references are no longer supported in stylesheet bodies — \
                     use `Tokenized::Token { name: \"...\", fallback: ... }` directly. \
                     See idea-ui's theme runtime for the legacy theme-struct pattern.",
                ));
            }
        }
    }
    Ok(())
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

/// Emits the `Rc<StyleSheet>` constructor with thread-local caching.
/// Mirrors the hand-rolled `card_style()` / `banner_style()` style
/// the codebase already uses.
///
/// The declared `<Theme>` generic and `base(theme) { ... }` bindings
/// are accepted for syntactic backward-compatibility but ignored at
/// emission: stylesheet closures now take `&VariantSet`, not a theme
/// reference. Authors who relied on `theme.*` field reads will see a
/// compile error from `check_no_theme_refs`.
fn emit_stylesheet_fn(decl: &StyleSheetDecl) -> TokenStream2 {
    let fn_name = format_ident!("{}_style", snake_case(&decl.name));
    let vis = &decl.vis;
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
            quote! {
                .variant(#axis_name, #arm_name, |_vs: &::runtime_core::VariantSet| #rules)
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
        quote! {
            .variant(#axis, "on", |_vs: &::runtime_core::VariantSet| #rules)
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
        quote! {
            .variant(#axis, "on", |_vs: &::runtime_core::VariantSet| #rules)
        }
    });

    // Container overlays: each `container (min_width: N) { … }` block is a
    // variant overlay under a `__cq_minw_<bits>` axis (single "on" value).
    // The threshold is encoded as the lossless 8-char hex of its `f32`
    // bit pattern — the SAME encoding `runtime_core::container_axis_name`
    // produces, so the runtime decodes the px value back via
    // `container_axis_threshold`. The framework realizes the namespace
    // per-backend (CSS `@container` on web, reactive merge on native).
    let container_chain = decl.containers.iter().map(|arm| {
        let axis = format!("__cq_minw_{:08x}", arm.threshold.to_bits());
        let rules = emit_rules_struct(&arm.rules);
        quote! {
            .variant(#axis, "on", |_vs: &::runtime_core::VariantSet| #rules)
        }
    });

    quote! {
        #vis fn #fn_name() -> ::std::rc::Rc<::runtime_core::StyleSheet> {
            ::std::thread_local! {
                static SHEET: ::std::rc::Rc<::runtime_core::StyleSheet> =
                    ::std::rc::Rc::new(
                        ::runtime_core::StyleSheet::new(
                            |_vs: &::runtime_core::VariantSet| #base_rules
                        )
                            #(#variant_chain)*
                            #(#state_chain)*
                            #(#breakpoint_chain)*
                            #(#container_chain)*
                    );
            }
            SHEET.with(|s| ::std::clone::Clone::clone(s))
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
fn emit_builder(decl: &StyleSheetDecl) -> TokenStream2 {
    let name = &decl.name;
    let vis = &decl.vis;
    let entry_fn = name; // `Card()` returns `Card` — see free function below.
    let stylesheet_fn = format_ident!("{}_style", snake_case(name));

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
    let axis_applies = decl.variants.iter().map(|axis| {
        let axis_str = axis.axis.to_string();
        let f = format_ident!("__v_{}", axis.axis);
        quote! {
            if let ::std::option::Option::Some(g) = self.#f.as_ref() {
                __app = __app.with(#axis_str, g());
            }
        }
    });
    let override_applies = decl.overrides.iter().map(|o| {
        let f = format_ident!("__o_{}", o.name);
        let method = format_ident!("override_{}", o.name);
        quote! {
            if let ::std::option::Option::Some(g) = self.#f.as_ref() {
                __app = __app.#method(g());
            }
        }
    });

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
        }

        impl ::std::default::Default for #name {
            fn default() -> Self { Self::new() }
        }

        impl ::runtime_core::IntoStyleSource for #name {
            fn into_style_source(self) -> ::runtime_core::StyleSource {
                // The builder routes to one of two style sources:
                //
                // - All-constant inputs (variant values are plain enums,
                //   overrides are plain values) → `StyleSource::Static`:
                //   resolved once here, no per-node `Effect`, cohort theme
                //   reactivity only. For the common case this is a strict
                //   win — 10k static rows allocate zero per-node effects.
                //
                // - Any setter received a reactive source (`Signal` /
                //   `derived(...)`) → `StyleSource::Reactive`: the build
                //   closure is handed to the framework's apply-style
                //   `Effect`, which re-runs it on every signal change so
                //   the variant / override re-resolves and the style
                //   re-applies. `__reactive` (set by the setters) selects
                //   the path. The boxed closure re-invokes each stored
                //   per-axis closure on every run, so signals read inside
                //   a `derived` become live dependencies.
                let __reactive = self.__reactive;
                let __build = move || {
                    let mut __app = ::runtime_core::StyleApplication::new(#stylesheet_fn());
                    #(#axis_applies)*
                    #(#override_applies)*
                    __app
                };
                if __reactive {
                    ::runtime_core::StyleSource::Reactive(::std::boxed::Box::new(__build))
                } else {
                    ::runtime_core::StyleSource::Static(__build())
                }
            }
        }

        /// Entry point: `Card()` returns a fresh builder. The free
        /// function shadows the struct name for call sites like
        /// `Card().size(...).kind(...)`.
        #[allow(non_snake_case)]
        #vis fn #entry_fn() -> #name { #name::new() }
    }
}
