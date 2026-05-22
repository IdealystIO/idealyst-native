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
use crate::shell::{codeblock, pageheader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "building-a-theme-system",
    title = "Building a theme system",
    category = Advanced,
    description = "Themes are a user-space pattern, not a framework primitive. This page is the cookbook for layering one on top of tokens + stylesheets.",
    related = ["styles", "reactivity", "writing-a-backend"],
    concepts = [Theme],

    section(heading = "Why this lives in user space") {
        p(code("framework-core"),
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
             " — call once at app boot. Stores the theme in a global \
              slot and pushes its tokens into the registry."],
            [code("set_theme(theme)"),
             " — call to swap. Updates the global slot and refreshes \
              the registry (per-token reactivity does the rest)."],
        ),
        code(rust, r##"
            use framework_core::{Color, Length, Tokenized, TokenEntry, TokenValue};
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
            fn app() -> Primitive {
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
          " in a global slot and calls ",
          code("install_tokens(t.tokens())"), " under the hood. ",
          code("set_theme(t)"),
          " replaces the global slot and calls ",
          code("update_tokens(t.tokens())"),
          ", which the per-token signals pick up automatically."),
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
            ["Fallbacks are the resolved value when no token store is \
              active. On iOS / Android (no runtime variable system), \
              the fallback IS the rendered value. So a theme's ",
             code("light()"),
             " constructor isn't just bootstrapping — it's also the \
              authoritative source of truth for backends that don't \
              run a token registry. The web backend resolves through \
              variables AND uses fallbacks; native backends just use \
              fallbacks."],
            ["The ", code("ThemeTokens"),
             " impl is mechanical. It's a deterministic projection \
              from the theme struct's typed fields to a flat ",
             code("Vec<TokenEntry>"),
             ". This is what the framework needs to install — the \
              theme struct itself stays type-rich so your stylesheets \
              can access fields by name with full IDE completion."],
        ),
    },

    section(heading = "Using the theme inside stylesheets") {
        p("Stylesheets can take ", code("MyTheme"),
          " as their context type. The macro generates code that pulls \
           the active theme out of the global slot at resolution time, \
           hands it to the stylesheet's closures as ",
          code("&MyTheme"),
          ", and your closures access tokens through typed fields:"),
        code(rust, r##"
            use framework_core::stylesheet;

            stylesheet! {
                pub Card<MyTheme> {
                    base(theme) {
                        background: theme.background.clone(),
                        color:      theme.text.clone(),
                        padding:    theme.spacing_md.clone(),
                        border_radius: 8.0,
                    }
                }
            }
        "##),
        p("Equivalent of the same stylesheet without the theme \
           context — referencing tokens by name directly:"),
        code(rust, r##"
            stylesheet! {
                pub Card<()> {
                    base(_) {
                        background: Tokenized::token("bg",       Color::from("#ffffff")),
                        color:      Tokenized::token("text",     Color::from("#111111")),
                        padding:    Tokenized::token("space-md", Length::Px(16.0)),
                        border_radius: 8.0,
                    }
                }
            }
        "##),
        p("Both forms produce the same content key and the same CSS \
           class on web. The typed form gains compile-time field \
           checking + IDE autocomplete; the bare-token form gains \
           zero coupling to a specific theme type. Pick whichever \
           fits — the framework treats them identically."),
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
            ["The global theme slot is replaced. Anything that read \
              the active theme inside an Effect re-runs."],
            [code("update_tokens(theme.tokens())"),
             " walks the registry and updates each token's signal. \
              Per-token reactivity means only effects that read a \
              changed token are notified."],
            ["On the web backend, that's it. The token registry's \
              tokens map 1:1 to CSS custom properties on ",
             code(":root"),
             "; updating a token's signal triggers the backend to ",
             code("setProperty(\"--name\", new_value)"),
             ". DOM elements never know."],
            ["On iOS / Android backends, each styled node's apply-\
              style Effect re-fires if it transitively reads a changed \
              token. The stylesheet closure runs with the new fallback \
              values, the new ", code("StyleRules"),
             " hit the backend, the backend mutates the native \
              widget's properties."],
            ["On Roku and other generator backends, the device-side \
              variable store is updated via a wire-protocol command. \
              The device-side runtime re-reads its variables and \
              repaints."],
        ),
        p("The hot path on web is genuinely ", code("O(tokens)"),
          " — touch the variables, the browser handles the rest. The \
           hot path on native is ", code("O(styled nodes that read a changed token)"),
          ". Both are dramatically smaller than the rebuild-the-tree \
           cost a less granular system would pay."),
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
          code("framework-core"),
          " gives you everything you need:"),
        list(
            [code("install_tokens(entries)"),
             " — register a flat list of ", code("(name, value)"),
             " pairs in the token store. Call at app boot."],
            [code("update_tokens(entries)"),
             " — swap the values for already-registered names. \
              Per-token signals fire; subscribed effects re-run."],
            [code("Tokenized::token(name, fallback)"),
             " — reference a token from a stylesheet (or anywhere \
              else)."],
            [code("Tokenized::<T>::resolve()"),
             " — read a token's current value. Subscribes the calling \
              effect to that specific token."],
        ),
        code(rust, r##"
            use framework_core::{install_tokens, update_tokens, TokenEntry, TokenValue, Color};

            // Boot — no theme struct involved.
            install_tokens(vec![
                TokenEntry { name: "bg",      value: TokenValue::Color(Color::from("#fff")) },
                TokenEntry { name: "text",    value: TokenValue::Color(Color::from("#111")) },
                TokenEntry { name: "primary", value: TokenValue::Color(Color::from("#3b82f6")) },
            ]);

            // Later — swap any subset.
            update_tokens(vec![
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
          "'s ", code("IdeaTheme"),
          " is an example of the convention applied to a real \
           component library. Worth reading as a reference even if you \
           don't use it directly:"),
        list(
            ["Organized around intent palettes (", code("Primary"),
             ", ", code("Secondary"), ", ", code("Neutral"),
             ", ", code("Success"), ", ", code("Danger"),
             ", ", code("Warning"), ", ", code("Info"),
             ") instead of role-named colors. Each intent has a full \
              swatch (base, hover, pressed, on-color)."],
            ["Each component stylesheet picks the right intent based \
              on its ", code("variant intent"),
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
             " — per-token signals are how stylesheets stay subscribed \
              to only the values they actually read."],
            [link("Writing a backend", to = "writing-a-backend"),
             " — the backend-side of token handling: ",
             code("install_theme_variables"),
             " on web, fallback-only on native."],
        ),
    },
}
