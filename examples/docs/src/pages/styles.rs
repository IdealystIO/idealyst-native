//! Styles & Themes — the `stylesheet!` macro and the theme trait.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, stack, BodyTone, HeadingKind, StackGap};

use crate::shell::{
    codeblock, pageheader, section, CodeBlockProps, PageHeaderProps, SectionProps,
};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Styles & Themes".to_string(),
                description = "Typed, themed styles that compile down to per-platform style bundles.".to_string(),
            )

            Section(
                title = "Stylesheets".to_string(),
                body = "The `stylesheet!` macro declares a typed builder for a single \
                        styleable surface. Each sheet has a `base(theme)` block of default \
                        rules, optional `variant` axes (discrete enums like \
                        `size = sm | md | lg`), state-keyed overlays (`hovered`, `pressed`, \
                        `focused`, `disabled`), and a `transitions` block.".to_string(),
            )

            Card {
                Heading(content = "A typical sheet".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Sheets close over a theme reference, so every rule resolves \
                               against whatever theme is installed at render time. Variants \
                               and overrides flow through `with(...)` calls at the call \
                               site.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "stylesheet! {\n    \
                                pub NavLink<IdeaThemeRef> {\n        \
                                    base(t) {\n            \
                                        padding_horizontal: t.spacing().md,\n            \
                                        color: t.colors().text_muted.clone(),\n            \
                                        border_radius: t.radius().md,\n        \
                                    }\n        \
                                    variant active {\n            \
                                        #[default]\n            \
                                        off(_t) {}\n            \
                                        on(t) {\n                \
                                            background: t.intents().primary.solid_bg.clone(),\n                \
                                            color: t.intents().primary.solid_text.clone(),\n            \
                                        }\n        \
                                    }\n        \
                                    state hovered(t) { color: t.colors().text.clone(), }\n        \
                                    transitions { background: 200ms EaseOut, }\n    \
                                }\n\
                            }".to_string(),
                )
            }

            Section(
                title = "Themes".to_string(),
                body = "A theme is any type that implements the theme trait your stylesheet \
                        expects. The framework stays unopinionated about design tokens — your \
                        app defines a struct, implements the trait, and calls \
                        `install_theme(your_theme)` at startup. The `idea-ui` library ships \
                        `light_theme()` and `dark_theme()` you can use out of the box, swap \
                        between, or extend.".to_string(),
            )

            Section(
                title = "Variant axes vs overrides".to_string(),
                body = "Variant axes are enums — a finite set of named values like \
                        `size: sm | md | lg`. Stylesheets can cache the resolved style per \
                        (theme, variant) tuple, so variants are cheap. Overrides are \
                        continuous values (a custom width, an arbitrary color); they take a \
                        slower path because every value is unique. Prefer variants when the \
                        design has a fixed set of choices.".to_string(),
            )

            Section(
                title = "State overlays".to_string(),
                body = "`state hovered(t) { ... }` rules merge on top of the base when the \
                        native component reports that state. Pointer/touch interaction \
                        triggers `pressed`; focus triggers `focused`; keyboard hover triggers \
                        `hovered`; disabled controls trigger `disabled`. Backends raise these \
                        signals natively, so you don't wire them up in app code.".to_string(),
            )

            Section(
                title = "Reactive styles".to_string(),
                body = "Pass a closure to a style prop and the framework treats it as \
                        reactive — the closure re-runs (and the style re-applies) whenever any \
                        signal it reads changes. This is how the nav link in the sidebar of \
                        these docs flips its active highlight as the route changes.".to_string(),
            )
        }
    }
}
