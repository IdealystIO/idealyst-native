//! Building a theme system page — built via the `docs!` macro.
//!
//! The framework doesn't bake a theme concept into its primitives;
//! themes are a user-space pattern of grouping tokens for app-wide
//! swap. This Advanced page is the cookbook for layering one on top
//! of the styling primitives (Tokenized<T>, Stylesheets, the token
//! registry, reactivity).
//!
//! The `idea-ui` crate ships one such convention; this page
//! both documents that convention and shows what's underneath so you
//! can roll your own if you have different needs.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "building-a-theme-system",
    title = "Building a theme system",
    category = Advanced,
    description = "Themes are a user-space pattern, not a framework primitive. This page is the cookbook for layering one on top of tokens + stylesheets.",
    related = ["styles", "reactivity", "writing-a-backend"],
    concepts = [Theme],

    section(heading = "Why this lives in user space") {
        p(code("runtime-core"),
          " ships ", code("Tokenized<T>"),
          " (the primitive) plus ", code("install_tokens(...)"),
          " / ", code("update_tokens(...)"),
          " (the registry API). It does NOT ship a \"theme\" struct \
           or a ", code("Theme"),
          " trait. The reason is taxonomy: tokens are an unambiguous \
           primitive (named values that resolve through a runtime \
           store), but a \"theme\" is whatever YOU want it to be — a \
           flat color palette, a multi-axis design system, a per-\
           tenant config, a per-OS pair. The framework would have to \
           choose, and any choice would be wrong for someone."),
        p("Instead, the framework provides the primitives you need to \
           build whichever theme system fits your app, and ships one \
           opinionated convention (", code("idea-ui"),
          ") that you can use as-is or read as a reference. This page \
           covers both — the convention and the underlying mechanics \
           — so you can pick the level you want to work at."),
    },

    section(heading = "The convention — idea-ui") {
        p(code("idea-ui"),
          " bundles the standard \"install once, swap at runtime\" \
           pattern most apps want. The convention is three things:"),
        list(
            [code("ThemeTokens"),
             " — a trait your theme struct implements to enumerate the \
              tokens it provides."],
            [code("install_theme(theme)"),
             " — call once at app boot. Stores the theme as the active \
              theme and pushes its tokens into the registry."],
            [code("set_theme(theme)"),
             " — call to swap. Replaces the active theme and updates the \
              registry; the framework re-applies styles from there."],
        ),
        code(rust, r##"
            use runtime_core::{Color, Length, Tokenized, TokenEntry, TokenValue};
            use idea_ui::ThemeTokens;

            // Your theme struct. Shape is yours — the framework
            // doesn't care what fields are on it as long as `tokens()`
            // returns the entries you want installed.
            #[derive(Clone)]
            pub struct MyTheme {
                pub background: Tokenized<Color>,
                pub text:       Tokenized<Color>,
                pub primary:    Tokenized<Color>,
                pub spacing_md: Tokenized<Length>,
            }

            impl MyTheme {
                pub fn light() -> Self {
                    Self {
                        background: Tokenized::token("bg",       Color::from("#ffffff")),
                        text:       Tokenized::token("text",     Color::from("#111111")),
                        primary:    Tokenized::token("primary",  Color::from("#3b82f6")),
                        spacing_md: Tokenized::token("space-md", Length::Px(16.0)),
                    }
                }

                pub fn dark() -> Self {
                    Self {
                        background: Tokenized::token("bg",       Color::from("#0b0b0c")),
                        text:       Tokenized::token("text",     Color::from("#f5f5f5")),
                        primary:    Tokenized::token("primary",  Color::from("#60a5fa")),
                        spacing_md: Tokenized::token("space-md", Length::Px(16.0)),
                    }
                }
            }

            impl ThemeTokens for MyTheme {
                fn tokens(&self) -> Vec<TokenEntry> {
                    vec![
                        TokenEntry { name: "bg",       value: TokenValue::Color(self.background.value().clone()) },
                        TokenEntry { name: "text",     value: TokenValue::Color(self.text.value().clone()) },
                        TokenEntry { name: "primary",  value: TokenValue::Color(self.primary.value().clone()) },
                        TokenEntry { name: "space-md", value: TokenValue::Length(*self.spacing_md.value()) },
                    ]
                }
            }
        "##),
        p("Installing + swapping:"),
        code(rust, r##"
            use idea_ui::{install_theme, set_theme};

            #[component]
            fn app() -> Element {
                install_theme(MyTheme::light());
                ui! {
                    // ...
                }
            }

            // Anywhere, anytime:
            set_theme(MyTheme::dark());
        "##),
        p("That's the whole convention. ", code("install_theme(t)"),
          " stores ", code("t"),
          " as the active theme and calls ",
          code("install_tokens(&t.tokens())"), " under the hood. ",
          code("set_theme(t)"),
          " replaces the active theme and calls ",
          code("update_tokens(&t.tokens())"),
          ", which bumps the theme version signal so styled nodes \
           re-apply."),
    },

    section(heading = "Three things to notice about the design") {
        list(
            ["Token names are theme-independent. ",
             code("light()"), " and ", code("dark()"),
             " use the same names (", code("\"bg\""), ", ",
             code("\"text\""), ", ", code("\"primary\""), ", ",
             code("\"space-md\""),
             ") with different fallback values. That's deliberate — \
              it's what lets a single CSS class produced for ",
             code("background: var(--bg, ...)"),
             " be reused across themes on web. The class never has to \
              change; only the variable's value does."],
            ["Fallbacks are the value used when no token value is \
              installed. Once tokens are installed, every backend shows \
              the installed value: web through ",
             code("var(--name, fallback)"),
             ", native backends (iOS, Android, macOS) by resolving the \
              token when they apply a rule (",
             code("Tokenized::resolve()"),
             "). The fallback only shows before install, or for a name \
              the theme never installed."],
            ["The ", code("ThemeTokens"),
             " impl is mechanical. It's a deterministic projection \
              from the theme struct's typed fields to a flat ",
             code("Vec<TokenEntry>"),
             ". This is what the framework needs to install. \
              Stylesheets never see the theme struct; they reference \
              tokens by name (see the next section)."],
        ),
    },

    section(heading = "Using the theme inside stylesheets") {
        p("A stylesheet does not receive the theme. Its ", code("<…>"),
          " slot names a token vocabulary — a type implementing ",
          code("TokenVocabulary"),
          " — and each block's binding is that vocabulary's accessor \
           namespace. An accessor returns a token reference (name + \
           fallback); the value comes from the installed tokens when \
           the rule resolves, so every theme over the same names works \
           with the same sheet. A minimal vocabulary for ",
          code("MyTheme"), "'s tokens:"),
        code(rust, r##"
            use runtime_core::{stylesheet, Color, Length, TokenVocabulary, Tokenized};

            // Zero-sized: it carries token names, not values.
            #[derive(Default, Clone, Copy)]
            pub struct MyTokens;

            impl MyTokens {
                pub fn bg(&self) -> Tokenized<Color> {
                    Tokenized::token("bg", Color::from("#ffffff"))
                }
                pub fn text(&self) -> Tokenized<Color> {
                    Tokenized::token("text", Color::from("#111111"))
                }
                pub fn space_md(&self) -> Tokenized<Length> {
                    Tokenized::token("space-md", Length::Px(16.0))
                }
            }

            impl TokenVocabulary for MyTokens {
                type Tokens = Self;
            }

            stylesheet! {
                pub Card<MyTokens> {
                    base(t) {
                        background: t.bg(),
                        color:      t.text(),
                        padding:    t.space_md(),
                        border_radius: 8.0,
                    }
                }
            }
        "##),
        p("The same stylesheet without a vocabulary — ", code("<()>"),
          ", the binding written ", code("_t"),
          ", tokens referenced by name directly:"),
        code(rust, r##"
            stylesheet! {
                pub Card<()> {
                    base(_t) {
                        background: Tokenized::token("bg",       Color::from("#ffffff")),
                        color:      Tokenized::token("text",     Color::from("#111111")),
                        padding:    Tokenized::token("space-md", Length::Px(16.0)),
                        border_radius: 8.0,
                    }
                }
            }
        "##),
        p("Both forms produce the same token references, so the same \
           content key and the same CSS class on web. The vocabulary \
           form gains compile-time name checking + IDE autocomplete \
           (a misspelled token is a missing method); the string form \
           needs no extra type. idea-ui's sheets use the vocabulary \
           form: ", code("stylesheet! { pub Card<IdeaThemeRef> { base(t) { background: t.color.surface(), … } } }"),
          "."),
    },

    section(heading = "Reading the active theme imperatively") {
        p("Sometimes you need the theme outside a stylesheet — \
           computing a color for a ", code("Graphics"),
          " surface, choosing an icon variant, branching on a token \
           value at runtime:"),
        code(rust, r##"
            use idea_ui::active_theme;

            let theme = active_theme();
            let primary: Color = theme
                .downcast_ref::<MyTheme>()
                .expect("MyTheme not installed")
                .primary
                .value()
                .clone();
        "##),
        p(code("active_theme()"), " returns ", code("Rc<dyn Any>"),
          " — the framework stores the active theme type-erased \
           because it doesn't know what struct you used. Downcasting \
           is on the caller. Reading inside an Effect subscribes you \
           to theme changes; reading outside one is a one-shot fetch."),
    },

    section(heading = "What runs on a swap") {
        p("Walk through what happens when you call ",
          code("set_theme(MyTheme::dark())"), ":"),
        list(
            ["The active theme is replaced. Anything that read ",
             code("active_theme()"), " inside an Effect re-runs."],
            [code("update_tokens(&theme.tokens())"),
             " writes the new values into the registry, clears the \
              style resolution cache, and bumps one theme version \
              signal."],
            ["On the web backend, that's it. The token registry's \
              tokens map 1:1 to CSS custom properties on ",
             code(":root"),
             "; the theme driver effect hands the changed values to the \
              backend, which writes ",
             code("setProperty(\"--name\", new_value)"),
             ". DOM elements never know."],
            ["On iOS / Android / macOS, the theme driver effect then \
              re-applies every styled node (the static-style cohort). \
              Each re-apply resolves its rules against the new token \
              values and the backend updates the native widget's \
              properties. This is not filtered by which token changed: \
              a swap re-applies all styled nodes, through one effect."],
            ["On Roku and other generator backends, the device-side \
              variable store is updated via a wire-protocol command. \
              The device-side runtime re-reads its variables and \
              repaints."],
        ),
        p("The hot path on web is genuinely ", code("O(tokens)"),
          " — touch the variables, the browser handles the rest. The \
           hot path on native is ", code("O(styled nodes)"),
          ": one style re-apply per node, with no component bodies \
           re-running. Both are much smaller than rebuilding the \
           tree."),
    },

    section(heading = "Variations on the convention") {
        p("Once you've seen the convention, the variations are obvious. \
           Some examples:"),
        list(
            ["Multiple themes available at once (light + dark + high-\
              contrast selected from a menu). Make ", code("MyTheme"),
             " an enum (or a configurable struct) whose constructors \
              produce the variant you want. The framework just sees \
              one ", code("MyTheme"), " at a time."],
            ["Per-tenant theming (white-label apps with per-customer \
              palettes). ", code("MyTheme::for_tenant(tenant_id)"),
             " — the same trait, the same install/swap mechanics, the \
              same token store."],
            ["Theme inheritance / composition. Build a base theme, \
              then ", code(".with_overrides(...)"),
              " produces a derived theme that swaps a few fields. The \
              token names stay stable; only some fallback values \
              change."],
            ["Async theme loading. Use ", code("resource(...)"),
             " (see ", link("Reactivity", to = "reactivity"),
             ") to fetch a theme JSON from a server; install it once \
              the resource resolves. The signal-based update \
              propagates everywhere."],
            ["Programmatic OS-tracking. Subscribe to a system dark-\
              mode signal (web's ",
              code("prefers-color-scheme"),
             " media query, iOS's ", code("traitCollection"),
             ") and call ", code("set_theme(...)"),
             " whenever it flips. The user gets dark mode at the OS \
              level without you running an in-app toggle."],
        ),
        p("None of these need framework changes. They're all just \
           variations on \"build the right theme value, install it.\""),
    },

    section(heading = "Rolling your own (without idea-ui)") {
        p("If the ", code("idea-ui"),
          " convention isn't a fit, skip it entirely. ",
          code("runtime-core"),
          " gives you everything you need:"),
        list(
            [code("install_tokens(entries)"),
             " — register a flat list of ", code("(name, value)"),
             " pairs in the token store. Call at app boot."],
            [code("update_tokens(entries)"),
             " — swap the values for already-registered names. The theme \
              version signal bumps; styled nodes re-apply."],
            [code("Tokenized::token(name, fallback)"),
             " — reference a token from a stylesheet (or anywhere \
              else)."],
            [code("Tokenized::<T>::resolve()"),
             " — read a token's current value. Inside an effect, the \
              effect re-runs on the next token swap."],
        ),
        code(rust, r##"
            use runtime_core::{install_tokens, update_tokens, TokenEntry, TokenValue, Color};

            // Boot — no theme struct involved.
            install_tokens(&[
                TokenEntry { name: "bg",      value: TokenValue::Color(Color::from("#fff")) },
                TokenEntry { name: "text",    value: TokenValue::Color(Color::from("#111")) },
                TokenEntry { name: "primary", value: TokenValue::Color(Color::from("#3b82f6")) },
            ]);

            // Later — swap any subset.
            update_tokens(&[
                TokenEntry { name: "bg",   value: TokenValue::Color(Color::from("#000")) },
                TokenEntry { name: "text", value: TokenValue::Color(Color::from("#eee")) },
            ]);
        "##),
        p("This is what ", code("idea-ui"),
          " is doing under the hood, just without the trait + struct \
           ceremony. If your app's token set is fluid or comes from a \
           non-Rust source (config file, server-side theme builder, \
           runtime tenant lookup), going through ",
          code("install_tokens"),
          " directly is sometimes cleaner than coercing it through a \
           typed theme struct."),
    },

    section(heading = "What `idea-ui` does") {
        p(code("idea-ui"),
          "'s theme (the ", code("IdeaTheme"),
          " trait; ", code("light_theme()"), " / ", code("dark_theme()"),
          " return an ", code("IdeaThemeDefaults"),
          ", and sheets name ", code("IdeaThemeRef"),
          " as their vocabulary) is an example of the convention applied to a real \
           component library. Worth reading as a reference even if you \
           don't use it directly:"),
        list(
            ["Organized around intent palettes (", code("Primary"),
             ", ", code("Secondary"), ", ", code("Neutral"),
             ", ", code("Success"), ", ", code("Danger"),
             ", ", code("Warning"), ", ", code("Info"),
             ") instead of role-named colors. Each intent has a full \
              swatch: ", code("solid_bg"), ", ", code("solid_text"), ", ",
             code("soft_bg"), ", ", code("soft_text"), ", ", code("fg"),
             ", ", code("border"), "."],
            ["Each component stylesheet picks the right intent based \
              on its ", code("tone"),
             " axis, so a button can be \"primary danger\" and pick \
              up the right palette without code per intent."],
            ["Ships preset constructors (", code("light_theme()"),
             ", ", code("dark_theme()"),
             ") so most apps install one line and never touch the \
              individual fields."],
        ),
        p("If you're building a component library, copy the structure; \
           the work is in choosing the right tokens and grouping them \
           well, not in the install/swap mechanics."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Styles", to = "styles"),
             " — the primitives this page builds on: stylesheets, \
              tokens, variants, overrides, states, transitions."],
            [link("Reactivity", to = "reactivity"),
             " — the theme version signal and the theme driver effect \
              ride the same effect graph everything else uses."],
            [link("Writing a backend", to = "writing-a-backend"),
             " — the backend-side of token handling: ",
             code("StyleOps::install_tokens"), " / ",
             code("update_tokens"),
             " write CSS custom properties on web; native backends \
              resolve tokens when they apply a rule."],
        ),
    },
}
