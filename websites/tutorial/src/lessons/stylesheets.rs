//! Track 2 — Stylesheets. Style tokens, the stylesheet! macro,
//! variants and interaction states.
//!
//! Every snippet is `include_str!`-ed from `crate::samples`, so the
//! compiler checks the sheets, the generated builders, and the generated
//! variant enums the lessons name.

use idea_ui::{typography_kind, Typography};
use runtime_core::{ui, Element};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::routes::{ST_STYLESHEETS_ROUTE, ST_TOKENS_ROUTE, ST_VARIANTS_ROUTE};
use crate::shell;

pub fn tokens() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ST_TOKENS_ROUTE.name(),
            title = "Style tokens".to_string(),
            lead = "A named value you update at runtime; a theme is a collection of them.".to_string(),
        ) {
            Typography(
                content = "A style token is a named value a stylesheet reads by name. Tokens are \
                    the framework's runtime-restyle mechanism: change a token's value and the \
                    new value flows to everything using it, without recomputing any stylesheet. \
                    A theme is a collection of style tokens \u{2014} light, dark, a brand \
                    palette \u{2014} so installing or swapping a theme is installing or updating a \
                    set of tokens in one turn.".to_string()
            )
            CodePanel(src = include_str!("../samples/st_tokens.rs").to_string())

            Callout(label = "TokenValue variants must match".to_string()) {
                Typography(
                    content = "A token is Color, Length, or Number(f32). The variant must match \
                        the property reading it \u{2014} a Color token feeds a color property, a \
                        Length token a size. A mismatch warns in debug and falls back to the \
                        literal.".to_string(),
                    muted = true,
                )
            }
            Typography(
                content = "Each token name owns its own signal under the hood, so a styled effect \
                    subscribes only to the tokens it actually reads. update_tokens stages its \
                    writes like any other, so a swap that changes fifty tokens commits at one \
                    flush and an effect that read five of them re-runs once.".to_string()
            )

            DocsLink(
                summary = "The token model, the resolution cache, and web vs native realization.".to_string(),
                link_label = "Styling reference \u{2014} Style tokens".to_string(),
                doc_file = "styling.md".to_string(),
            )
        }
    })
}

pub fn stylesheets() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ST_STYLESHEETS_ROUTE.name(),
            title = "Defining a stylesheet".to_string(),
            lead = "stylesheet! { base(_t) { ... } } and how it resolves.".to_string(),
        ) {
            Typography(
                content = "stylesheet! declares a style once. The base block is the \
                    unconditional rules; property values are either literals or token references \
                    named through the block binding (t.spacing.md()). The macro generates a builder (Panel()), an associated \
                    Panel::sheet(), and the convention-named panel_style() \u{2014} both \
                    accessors hand back the same cached Rc<StyleSheet>.".to_string()
            )
            CodePanel(src = include_str!("../samples/st_sheet.rs").to_string())

            Callout(label = "The <..> slot names your token vocabulary".to_string()) {
                Typography(
                    content = "The angle-bracket slot names a token vocabulary and the block \
                        binding hands it to the body, so a theme token is a path the compiler \
                        checks: base(t) { padding: t.spacing.md() } references the spacing-md \
                        token. The binding carries names, not values \u{2014} values arrive at \
                        resolve time from the token registry, which is what makes a theme swap \
                        one write per token rather than a re-mint. So theme.colors.primary does \
                        not compile; there is no such field. Declare <()> when the sheet has no \
                        vocabulary and write its binding (_t).".to_string(),
                    muted = true,
                )
            }
            Typography(
                content = "Resolution merges layers property-wise: base, then any active variant \
                    overlays, then compounds, then per-call overrides. Each field is Option<T>, \
                    so a layer only overlays the properties it sets. Results are memoized by \
                    (sheet, variants, theme, overrides).".to_string()
            )

            DocsLink(
                summary = "StyleRules, StyleSheet, the macro grammar, and the resolution pipeline.".to_string(),
                link_label = "Styling reference".to_string(),
                doc_file = "styling.md".to_string(),
            )
        }
    })
}

pub fn variants() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ST_VARIANTS_ROUTE.name(),
            title = "Variants & states".to_string(),
            lead = "Select a named version at the call site; layer in states and one-off overrides.".to_string(),
        ) {
            Typography(
                content = "A variant is a named alternative version of a stylesheet that you \
                    select at the call site. You declare an axis (give it a name like tone) and \
                    list named values under it; each value carries its own rules, and selecting \
                    a value merges those rules over the base. Btn().tone(BtnTone::Primary) \
                    resolves to base plus the primary value's rules. Because every value is \
                    named and known at compile time, the framework resolves each one ahead of \
                    time, so the web backend mints one CSS class per value and picking a variant \
                    costs a lookup instead of a recompute.".to_string()
            )
            CodePanel(src = include_str!("../samples/st_variants.rs").to_string())

            Typography(content = "Interaction states".to_string(), kind = typography_kind::H2)
            Typography(
                content = "An interaction state is a set of rules the framework merges in while \
                    that state is active; there are four of them: hovered, pressed, focused, \
                    disabled. On web they compile to CSS pseudo-classes (:hover, :active) and \
                    the browser handles activation; on native the backend's input listeners flip \
                    a state bit that re-resolves the style. The result looks the same either \
                    way. You declare them with state blocks, like the state hovered(_t) { ... } \
                    block in the snippet above.".to_string()
            )

            Typography(content = "Overrides".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Variants cover values from a fixed set. For a value that can't be \
                    enumerated \u{2014} a user-controlled font size, a color computed at \
                    runtime \u{2014} set it as an override. The override_* methods live on \
                    StyleApplication: build one from the sheet, pick any variants with \
                    .with(axis, value), then layer the per-call value on top. Overrides merge \
                    in last, after base and variants, so they always win:".to_string()
            )
            CodePanel(src = include_str!("../samples/st_overrides.rs").to_string())

            Callout(label = "Why variants cache and overrides don't".to_string()) {
                Typography(
                    content = "A variant's values are fixed, so the framework resolves them once \
                        up front and reuses the result. An override's value is arbitrary, so \
                        each distinct value resolves on its own. Use overrides only when a value \
                        genuinely can't be a variant.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Variants vs overrides, interaction states, and backend caching.".to_string(),
                link_label = "Styling reference".to_string(),
                doc_file = "styling.md".to_string(),
            )
        }
    })
}
