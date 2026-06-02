//! Styling & theming — the `stylesheet!` macro, variants, interaction
//! states, transitions, the token-based theme system, gradients, and
//! the responsive breakpoint helpers. Companion to the bundled
//! `styling.md` guide.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection, Section};
use crate::routes::{CONCEPTS_ROUTE, CROSS_PLATFORM_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let anatomy_ref: Ref<ViewHandle> = Ref::new();
    let variants_ref: Ref<ViewHandle> = Ref::new();
    let states_ref: Ref<ViewHandle> = Ref::new();
    let transitions_ref: Ref<ViewHandle> = Ref::new();
    let tokens_ref: Ref<ViewHandle> = Ref::new();
    let gradients_ref: Ref<ViewHandle> = Ref::new();
    let responsive_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: anatomy_ref, label: "Anatomy of a stylesheet" },
        TocEntry { handle: variants_ref, label: "Variants" },
        TocEntry { handle: states_ref, label: "Interaction states" },
        TocEntry { handle: transitions_ref, label: "Transitions" },
        TocEntry { handle: tokens_ref, label: "Themes are token tables" },
        TocEntry { handle: gradients_ref, label: "Gradients" },
        TocEntry { handle: responsive_ref, label: "Responsive breakpoints" },
        TocEntry { handle: next_ref, label: "Where to go from here" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Styling & theming",
                blurb = "Style is declared with the `stylesheet!` macro: typed, \
                 theme-aware by construction, and resolved by the same token table on \
                 every backend. One stylesheet drives CSS on the web, `UIView` \
                 properties on iOS, and `View` attributes on Android — the values \
                 converge even though the mechanism differs.",
            )
            PageSection(handle = anatomy_ref) { anatomy() }
            PageSection(handle = variants_ref) { variants() }
            PageSection(handle = states_ref) { states() }
            PageSection(handle = transitions_ref) { transitions() }
            PageSection(handle = tokens_ref) { tokens() }
            PageSection(handle = gradients_ref) { gradients() }
            PageSection(handle = responsive_ref) { responsive() }
            PageSection(handle = next_ref) { where_next() }
        }
    };
    layout_with_toc(content, toc)
}

// ============================================================================
// Sections
// ============================================================================

fn anatomy() -> Element {
    let snippet = "stylesheet! {\n    \
                       pub PrimaryButton<()> {\n        \
                           base(_t) {\n            \
                               padding: Length::Px(8.0),\n            \
                               border_radius: Length::Px(6.0),\n            \
                               background: Tokenized::token(\"color-accent\", Color(\"#5b6cff\".into())),\n        \
                           }\n    \
                       }\n\
                   }\n\
                   \n\
                   // Applied to any primitive's `style` slot:\n\
                   ui! { button(label = \"Save\", style = PrimaryButton()) }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Anatomy of a stylesheet".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A `stylesheet!` block declares a named, strongly-typed \
                style. The `base(_t)` arm is the always-applied baseline; every property \
                is a real Rust value (a `Length`, a `Color`, a `Tokenized<T>`), so a typo \
                in a property name or a wrong value type is a compile error, not a \
                silently-ignored CSS line.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Stylesheets never read a theme struct in their bodies — \
                they reference values by token NAME via `Tokenized::token(\"…\", \
                fallback)`. That indirection is what lets a theme swap re-resolve the same \
                stylesheet against a different palette without re-authoring any styles.".to_string())
        }
    }
}

fn variants() -> Element {
    let snippet = "stylesheet! {\n    \
                       pub PrimaryButton<()> {\n        \
                           base(_t) { padding: Length::Px(8.0) }\n        \
                           variant size {\n            \
                               default medium(_t) { padding: Length::Px(8.0) }\n            \
                               small(_t)  { padding: Length::Px(4.0) }\n            \
                               large(_t)  { padding: Length::Px(12.0) }\n        \
                           }\n    \
                       }\n\
                   }\n\
                   \n\
                   // Pick an arm at the call site:\n\
                   ui! { button(label = \"Save\", style = PrimaryButton().size(Size::Large)) }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Variants".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A `variant <axis>` block declares an orthogonal set of \
                options — one arm per value, with `default` marking the implicit choice. \
                Variants are N-way and compose: a button can have a `size` axis and a \
                `tone` axis at once, and the macro generates a typed setter per axis so \
                only declared values are reachable.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Because each axis is its own type, an invalid combination \
                doesn't exist to be written. This is the same mechanism idea-ui's `Button` \
                tone / variant axes ride on.".to_string())
        }
    }
}

fn states() -> Element {
    let snippet = "stylesheet! {\n    \
                       pub PrimaryButton<()> {\n        \
                           base(_t) { background: Tokenized::token(\"color-accent\", Color(\"#5b6cff\".into())) }\n        \
                           state hovered(_t)  { background: Tokenized::token(\"color-accent-hover\", Color(\"#4a5bef\".into())) }\n        \
                           state pressed(_t)  { background: Tokenized::token(\"color-accent-press\", Color(\"#3a4bdf\".into())) }\n        \
                           state disabled(_t) { opacity: 0.5 }\n    \
                       }\n\
                   }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Interaction states".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A `state <name>(_t)` block overlays one of the four \
                interaction states — `hovered`, `pressed`, `focused`, `disabled`. Any other \
                name is rejected at compile time, so a stylesheet can't carry a typo'd \
                state that never fires. The backend tracks the live state and re-applies \
                the overlay; on web that's `:hover` / `:active`, on iOS it's the touch \
                lifecycle, on Android the pressed drawable state — same author code, \
                native delivery.".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn transitions() -> Element {
    let snippet = "stylesheet! {\n    \
                       pub PrimaryButton<()> {\n        \
                           base(_t) { background: Tokenized::token(\"color-accent\", Color(\"#5b6cff\".into())) }\n        \
                           transitions { background: 200ms EaseOut, opacity: 150ms EaseInOut }\n        \
                           state pressed(_t) { background: Tokenized::token(\"color-accent-press\", Color(\"#3a4bdf\".into())) }\n    \
                       }\n\
                   }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Transitions".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A `transitions` block animates per-property changes — when \
                a state overlay or a variant flip changes a property, the backend tweens \
                between the old and new value over the declared duration and easing. This is \
                the declarative path for hover/press feedback; for choreographed, \
                multi-step motion driven from app state, reach for the imperative \
                `AnimatedValue` system on the Reactivity page instead.".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn tokens() -> Element {
    let snippet = "// 1. A theme is whatever struct implements `ThemeTokens` —\n\
                   //    its `tokens()` method returns the flat (name, value) table.\n\
                   impl ThemeTokens for MyTheme {\n    \
                       fn tokens(&self) -> Vec<TokenEntry> {\n        \
                           vec![\n            \
                               TokenEntry { name: \"color-accent\", value: TokenValue::Color(self.accent.clone()) },\n            \
                               TokenEntry { name: \"radius-md\",    value: TokenValue::Length(self.radius) },\n        \
                           ]\n    \
                       }\n\
                   }\n\
                   \n\
                   // 2. Install once at bootstrap (required before render):\n\
                   install_theme(MyTheme::default());\n\
                   \n\
                   // 3. Swap any time — every styled primitive re-resolves reactively:\n\
                   set_theme(MyTheme::dark());";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Themes are token tables".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A theme is a struct that implements `ThemeTokens` — its \
                `tokens()` method produces a flat table of `(name, value)` pairs. \
                Stylesheets reference those values by name, so the theme and the stylesheets \
                are decoupled: the same `PrimaryButton` stylesheet renders against whatever \
                token set is currently installed.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Token reads are reactive. A `set_theme(..)` swap updates \
                the token signals and re-applies exactly the rules that read a changed token \
                — a dark/light toggle re-tints the whole tree with no manual rebuild. \
                `install_theme(..)` is required before the first render, even for a static, \
                never-changing theme.".to_string())
            Typography(content = "The framework core ships only the token plumbing \
                (`install_tokens` / `Tokenized::token`). The named `light_theme()` / \
                `dark_theme()` instances, the `theme.colors.primary`-style ergonomic \
                accessors, and `install_theme` / `set_theme` are an idea-theme convenience \
                layer built on top — exactly the pattern your own theme system would \
                follow.".to_string())
        }
    }
}

fn gradients() -> Element {
    let snippet = "stylesheet! {\n    \
                       pub HeroBanner<()> {\n        \
                           base(_t) {\n            \
                               background_gradient: Gradient::linear(180.0, vec![\n                \
                                   GradientStop { offset: 0.0, color: Color(\"#EFDD74\".into()) },\n                \
                                   GradientStop { offset: 1.0, color: Color(\"#ffffff\".into()) },\n            \
                               ]),\n        \
                               }\n    \
                       }\n\
                   }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Gradients".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "`background_gradient` works on every backend that paints \
                a surface. Linear and radial kinds are both supported, and the mechanism is \
                native per platform: a `CAGradientLayer` sublayer on iOS, a \
                `GradientDrawable` on Android, a `background-image: *-gradient(...)` rule on \
                web. A radial gradient's radius is closest-side scaled — `1.0` reaches the \
                nearest edge midpoint — so the same stops describe the same shape \
                everywhere.".to_string())
            CodePanel(src = snippet)
        }
    }
}

fn responsive() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Section(
                title = "Responsive breakpoints",
                paragraphs = vec![
                    "Layout that adapts to viewport width reads the active breakpoint with \
                     `current_breakpoint()` — a reactive signal the framework keeps in sync \
                     with the real window size on every backend (the web backend wires it \
                     through a resize observer). Because it's a signal, a style closure or a \
                     `when(..)` that reads it re-resolves when the user crosses a \
                     threshold.".to_string(),
                    "This is how the navigator drawer decides modal-vs-pinned and how this \
                     very site collapses its sidebar — keyed off a theme-owned breakpoint, \
                     not a magic pixel scattered at the call site. The same signal is \
                     available to your own components for any width-driven layout \
                     decision.".to_string(),
                ],
                code = Some(
                    "// Reactive: re-runs when the breakpoint changes.\n\
                     let columns = move || match current_breakpoint().get() {\n    \
                         Breakpoint::Xl | Breakpoint::Lg => 3,\n    \
                         Breakpoint::Md => 2,\n    \
                         _ => 1,\n\
                     };".to_string()
                ),
            )
            Typography(content = "Per the framework's native-first stance, layout chrome \
                (titles, tab bars, drawers) is configured through navigator screen options, \
                not the `style` system. The `.layout(...)` builder is a deliberate web-only \
                escape hatch — reach for breakpoints and screen options first.".to_string(),
                muted = true)
        }
    }
}

fn where_next() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Where to go from here".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Transitions cover declarative property animation. For \
                state-driven, choreographed motion — springs, timelines, multi-property \
                entrances — see the Reactivity & animation page.".to_string())
            link(route = &CONCEPTS_ROUTE, params = ()) {
                Typography(content = "Core concepts \u{2192}".to_string())
            }
            Typography(content = "The cross-platform page explains why one stylesheet \
                produces the same look on UIKit, the DOM, and a GPU pipeline without \
                per-platform tweaks.".to_string())
            link(route = &CROSS_PLATFORM_ROUTE, params = ()) {
                Typography(content = "Cross-platform \u{2192}".to_string())
            }
        }
    }
}
