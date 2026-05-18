//! Styles page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{codeblock, pageheader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "styles",
    title = "Styles and Themes",
    category = Foundation,
    description = "A layered theme + stylesheet system that piggybacks on reactivity, with backend-specific swap strategies.",
    related = ["reactivity", "primitives", "components", "backends"],
    concepts = [Stylesheet, Theme, Token, Variant, Override, StyleState, Transition],

    section(heading = "Overview") {
        p("Styling in Idealyst is a layered system: a ", code("theme"), " holds named \
           values; ", code("stylesheets"), " are functions that take the active theme \
           and produce concrete rule sets; the framework caches the rule sets and \
           applies them through the same reactive substrate everything else uses."),
        p("What makes the system interesting is what happens on a theme change. \
           On web, the framework updates a handful of CSS custom properties. \
           DOM elements aren't touched. Class names don't change. No node \
           re-renders. On native, the same change re-fires only the per-node \
           style Effects whose values actually depended on the changed tokens. \
           This page explains how that works, then shows you how to write your \
           own theme and your own stylesheets."),
    },

    section(heading = "The pieces") {
        p("There are four moving parts:"),
        list(
            [code("Theme"), " — a Rust struct you define. Holds whatever values your \
              app needs: colors, spacing, typography, breakpoints, anything else."],
            [code("Tokens"), " — named values inside the theme. A token is a ",
              code("(name, fallback)"), " pair. Stylesheets reference tokens by name; \
              the fallback is what backends use when no runtime variable system is available."],
            [code("Stylesheets"), " — declared with the ", code("stylesheet!"),
              " macro. A stylesheet is a typed builder that takes the active theme and produces a ",
              code("StyleRules"), " (a flat bag of optional property values)."],
            [code("StyleRules"), " — the concrete output. Every primitive's ",
              code("style"), " slot eventually gets one of these."],
        ),
        p("Application code writes themes and stylesheets. The framework \
           handles caching, theme installation, resolution, and the backend \
           calls that put the result on screen."),
    },

    section(heading = "Themes") {
        p("A theme is a struct you write. The framework doesn't care about its \
           shape; it just has to implement the ", code("ThemeTokens"), " trait so the \
           framework knows what to install as runtime variables."),

        code(rust, r##"
            use framework_core::{Color, Length, Tokenized, ThemeTokens, TokenEntry, TokenValue};

            #[derive(Clone)]
            pub struct MyTheme {
                pub background: Tokenized<Color>,
                pub text: Tokenized<Color>,
                pub primary: Tokenized<Color>,
                pub spacing_md: Tokenized<Length>,
            }

            impl MyTheme {
                pub fn light() -> Self {
                    Self {
                        background: Tokenized::token("bg",      Color::from("#ffffff")),
                        text:       Tokenized::token("text",    Color::from("#111111")),
                        primary:    Tokenized::token("primary", Color::from("#3b82f6")),
                        spacing_md: Tokenized::token("space-md", Length::Px(16.0)),
                    }
                }

                pub fn dark() -> Self {
                    Self {
                        background: Tokenized::token("bg",      Color::from("#0b0b0c")),
                        text:       Tokenized::token("text",    Color::from("#f5f5f5")),
                        primary:    Tokenized::token("primary", Color::from("#60a5fa")),
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

        p("Three things to notice:"),
        list(
            ["Token names are theme-independent. ", code("light()"), " and ", code("dark()"),
              " use the same names (", code("\"bg\""), ", ", code("\"text\""), ", ",
              code("\"primary\""), ", ", code("\"space-md\""), ") with different fallback \
              values. That's deliberate — see \"Why classes don't change on theme swap\" below."],
            ["Tokens declare a fallback. The fallback is the value the token resolves to \
              when there's no runtime variable system (iOS, Android, server-rendered HTML). \
              On web, the fallback also fills in if the CSS variable hasn't been written yet."],
            ["The ", code("ThemeTokens"), " impl is mechanical. It just lists the tokens \
              that should be installed as variables. For backends without runtime variables, \
              this impl is effectively unused."],
        ),

        p("Installing a theme:"),

        code(rust, r##"
            use framework_core::{install_theme, set_theme};

            #[component]
            fn app() -> Primitive {
                install_theme(MyTheme::light());

                ui! {
                    // ...
                }
            }

            // Later, from anywhere:
            set_theme(MyTheme::dark());
        "##),

        p(code("install_theme(t)"), " registers the theme at app boot and is what \
           stylesheet closures see when they run. ", code("set_theme(t)"),
          " swaps it later. Both write to the same arena slot — internally, the active \
           theme is stored as a ", code("Signal<Rc<dyn Any>>"),
          ", so anything that reads it participates in reactivity."),

        p("You can swap themes at any time, from any code path that runs in \
           the main thread. A dark-mode toggle is a one-liner:"),

        code(rust, r##"
            Button(
                label = "Toggle theme",
                on_click = move || set_theme(
                    if is_dark.get() { MyTheme::light() } else { MyTheme::dark() }
                ),
            )
        "##),
    },

    section(heading = "Tokens") {
        p("A token is a value that may resolve through a named runtime variable \
           instead of being baked into rules. The type is ", code("Tokenized<T>"), ":"),

        code(rust, r##"
            pub enum Tokenized<T> {
                Literal(T),
                Token { name: &'static str, fallback: T },
            }
        "##),

        p("A stylesheet receiving ", code("theme.primary"), " reads a ",
          code("Tokenized<Color>"), " — either a literal color, or a token reference like ",
          code("Token { name: \"primary\", fallback: Color(\"#3b82f6\") }"),
          ". The backend decides what to do with it."),

        p("What the web backend does: the web backend installs tokens as CSS custom \
           properties on the document root:"),

        code(text, r##"
            :root {
                --bg: #ffffff;
                --text: #111111;
                --primary: #3b82f6;
                --space-md: 16px;
            }
        "##),

        p("When the backend emits CSS for a stylesheet rule, every tokenized \
           property turns into a ", code("var(--name, fallback)"), " reference:"),

        code(text, r##"
            .idealyst-card-12 {
                background: var(--bg, #ffffff);
                color: var(--text, #111111);
                padding: var(--space-md, 16px);
            }
        "##),

        p("That's the whole trick. When the theme swaps to dark, the backend \
           writes four new values to ", code(":root"), "'s style block. Every CSS rule \
           that referenced ", code("var(--bg)"), " now resolves to the dark color \
           automatically — by the browser, in one paint pass. The framework \
           doesn't iterate over DOM elements. It doesn't change class names. \
           It doesn't touch any rule body."),

        p("What native backends do: iOS and Android don't have a runtime variable system, \
           so they ignore the token name and read the ", code(".value()"),
          " (the fallback). When the theme swaps, every styled node has an Effect \
           wrapping its apply-style call; that Effect re-fires with the new theme, \
           the stylesheet closure re-runs, the new rules go to the backend, the \
           backend mutates the native widget's properties."),

        p("This is more work per swap than the web's variable-update model, \
           but it's still proportional to the number of styled nodes — not \
           the size of the tree. And the rule-set cache deduplicates: if two \
           themes produce identical rules for the same ", code("(sheet, variants)"),
          ", the second swap is a refcount bump."),

        p("What generator backends do: Roku and other generator backends can't \
           ship closures to the device. They get a different deal: the ",
          code("Derived<T>"), " machinery and the wire protocol install a device-side \
           variable system that mirrors the host's tokens. Theme swaps on the device \
           are device-side variable rewrites, conceptually the same as the web's model."),
    },

    section(heading = "Stylesheets") {
        p("The ", code("stylesheet!"), " macro is how you declare a typed, themed \
           stylesheet. Grammar:"),

        code(rust, r##"
            use framework_core::{stylesheet, Color, Length};

            stylesheet! {
                pub Card<MyTheme> {
                    base(theme) {
                        background: theme.background.clone(),
                        color: theme.text.clone(),
                        padding: theme.spacing_md,
                        border_radius: 8.0,
                    }

                    variant size {
                        small(theme)  { padding: theme.spacing_md.value().clone() }
                        #[default]
                        medium(_)     {}
                        large(theme)  { padding: 24.0 }
                    }

                    variant kind {
                        #[default]
                        elevated(theme) {
                            background: theme.background.clone(),
                            shadow: Shadow { x: 0.0, y: 2.0, blur: 8.0, color: Color::from("#0001") },
                        }
                        outlined(theme) {
                            background: Color::from("transparent"),
                            border: (2.0, theme.text.clone()),
                        }
                    }

                    override padding: Length

                    state hovered(theme) { opacity: 0.92 }
                    state pressed(_)     { opacity: 0.85 }

                    transitions {
                        background: 200ms EaseOut
                        opacity:    150ms Linear
                    }
                }
            }
        "##),

        p("What the macro generates from this declaration:"),
        list(
            [code("pub fn card_style() -> Rc<StyleSheet>"), " — the registered \
              stylesheet. Cached in a thread-local; repeat calls return the same ",
              code("Rc"), "."],
            [code("pub fn Card() -> CardBuilder"), " — the entry point you call at use sites."],
            [code("pub enum CardSize { Small, Medium, Large }"), " — one enum per \
              variant axis, with a ", code("Default"), " impl that picks the ",
              code("#[default]"), " arm."],
            [code("pub enum CardKind { Elevated, Outlined }"), " — same."],
            ["A ", code("CardBuilder"), " struct with one setter per variant axis and \
              per override. Each setter accepts either a typed value or a ",
              code("Signal<T>"), " — pass a signal and the variant axis becomes reactive."],
            [code("impl IntoStyleSource for CardBuilder"), " — so the builder can be \
              passed to ", code(".with_style(...)"), " or as ", code("style = ..."),
              " inside ", code("ui!"), "."],
        ),

        code(rust, r##"
            Card()
                .size(CardSize::Large)
                .kind(CardKind::Outlined)
                .padding(20.0)
        "##),

        p("Using a stylesheet:"),

        code(rust, r##"
            ui! {
                View(style = Card().size(CardSize::Large).kind(CardKind::Outlined)) {
                    Text { "..." }
                }
            }
        "##),

        p("With a reactive variant axis:"),

        code(rust, r##"
            let size = signal!(CardSize::Medium);

            ui! {
                View(style = Card().size(size)) {
                    Text { "..." }
                }

                Button(label = "Grow", on_click = move || size.set(CardSize::Large))
            }
        "##),

        p("When ", code("size"), " changes, the framework looks up the cached rule \
           set for the new variant tuple and calls ", code("apply_style"),
          " on the backend for the View's node only."),
    },

    section(heading = "How resolution works") {
        p("Resolution is a function from ",
          code("(stylesheet, variants, theme, overrides)"), " to a concrete ",
          code("Rc<StyleRules>"), ". The framework caches the result so the same \
           combination of inputs returns the same ", code("Rc"), " instance."),

        p("The cache key is interesting:"),
        list(
            ["Stylesheet is identified by ", code("Rc"), " pointer."],
            ["Variants are an ordered set of ", code("(axis, value)"), " strings."],
            ["Theme is identified by ", code("Rc"), " pointer."],
            ["Overrides are serialized into a content key."],
        ),

        p("So ", code("Card().size(Large).kind(Outlined)"), " under ",
          code("MyTheme::light()"), " maps to one cached ", code("Rc<StyleRules>"),
          ". The same builder under ", code("MyTheme::dark()"),
          " maps to a different one."),

        p("But — and this is what makes the web backend's swap cheap — the \
           content key that the web backend uses to mint a CSS class hashes \
           tokens by their name, not by their resolved value. So ",
          code("Card().size(Large).kind(Outlined)"),
          " produces the same content key under ", code("light"), " and ",
          code("dark"), ". The web backend mints one class and reuses it across \
           themes. Theme swap turns into a refcount bump on the existing class \
           registration plus four CSS variable writes, which is what makes the swap \
           O(tokens) rather than O(styled nodes)."),

        compare(from = React) {
            p("Theming in React typically goes through Context; a context value \
               change re-renders every consumer. Even with ", code("React.memo"),
              " and selectors, every styled component participates in the dependency \
               graph and pays some per-swap cost. Here the graph is one signal (the \
               active theme); only the per-node style Effects subscribe to it, and on \
               web the Effects are bypassed entirely because CSS variables do the work."),
            p("From styled-components / Emotion: each themed component usually \
               generates a new class name when the theme changes, which forces every \
               instance to re-attach. Idealyst's class names are theme-stable — the \
               content key hashes token names, so a stylesheet's class is the same \
               under any theme. Theme swap doesn't change className on a single element."),
            p("From Tailwind: closest analog is CSS-variable-based theming in Tailwind \
               (the ", code("darkMode: \"class\""), " strategy with custom properties). \
               Same payoff: a class flip on ", code(":root"),
              " updates colors downstream without touching anything else. Idealyst's \
               split between literal fallbacks (for non-web backends) and variable \
               references (for web) gives you the same payoff automatically — you don't \
               choose between the two strategies, the backend chooses."),
        },
    },

    section(heading = "Variants vs overrides vs states") {
        p("Three different ways to vary a stylesheet at the point of use:"),

        p("Variants are discrete enum-shaped axes. Each variant arm has a \
           fixed rule overlay. Use variants when there's a finite set of named \
           modes — ", code("size: Small | Medium | Large"), ", ",
          code("kind: Solid | Soft | Outlined"), ". The macro generates enum types \
           and ", code("Default"), " impls."),

        p("Overrides are per-instance continuous values. The author passes \
           a specific value at the use site (", code("Card().padding(20.0)"),
          "), and that value lands in the override slot of the resolved rule set. \
           Use overrides when the value isn't from a finite menu — a specific \
           size, color, or duration."),

        p("States are interaction states the backend flips automatically: ",
          code("hovered"), ", ", code("pressed"), ", ", code("focused"), ", ",
          code("disabled"), ". Each state's rule overlay applies when the \
           backend's input layer says the relevant state is active. You don't \
           switch states from app code; the backend listens for the native event \
           and updates the state bits, and the framework applies the overlay."),

        code(rust, r##"
            stylesheet! {
                pub Btn<MyTheme> {
                    base(theme) { background: theme.primary.clone() }
                    state hovered(_)  { opacity: 0.9 }
                    state pressed(_)  { opacity: 0.8 }
                    state disabled(_) { opacity: 0.5 }
                }
            }
        "##),

        p("State overlays land in a reserved ", code("__state"),
          " axis under the hood — same machinery as variants, so resolution \
           caching and pre-generation work without special cases."),
    },

    section(heading = "Transitions") {
        p("A ", code("transitions { ... }"), " block declares which property changes \
           should animate, and how. The framework doesn't drive the animation \
           itself — backends use their native interpolators:"),
        list(
            ["Web: emits ", code("transition: background 200ms ease-out"), "."],
            ["iOS: wraps the property write in a ", code("UIView.animate"), " block."],
            ["Android: uses ", code("ObjectAnimator"), "."],
        ),

        code(rust, r##"
            transitions {
                background: 200ms EaseOut
                opacity:    150ms Linear
                padding:    250ms CubicBezier(0.2, 0.0, 0.0, 1.0)
            }
        "##),

        p("Shorthand property names like ", code("padding"),
          " fan out to all four sides during macro expansion. Properties without a \
           transition spec change instantly."),
    },

    section(heading = "Property reference (short version)") {
        p(code("StyleRules"), " is a flat bag of optional properties. The shape is \
           mobile-first — what React Native's StyleSheet supports, plus a few \
           additions. The categories:"),
        list(
            ["Color + text: ", code("background"), ", ", code("color"), ", ",
              code("font_size"), ", ", code("font_family"), ", ", code("font_weight"),
              ", ", code("font_style"), ", ", code("line_height"), ", ",
              code("letter_spacing"), ", ", code("text_align"), ", ", code("underline"),
              ", ", code("strikethrough"), ", ", code("text_transform"), "."],
            ["Flex container: ", code("flex_direction"), ", ", code("flex_wrap"),
              ", ", code("justify_content"), ", ", code("align_items"), ", ",
              code("align_content"), ", ", code("gap"), ", ", code("row_gap"),
              ", ", code("column_gap"), "."],
            ["Flex item: ", code("flex_grow"), ", ", code("flex_shrink"),
              ", ", code("flex_basis"), ", ", code("align_self"), "."],
            ["Sizing: ", code("width"), ", ", code("height"), ", ", code("min_width"),
              ", ", code("min_height"), ", ", code("max_width"), ", ",
              code("max_height"), "."],
            ["Padding / margin / border radius / border width / border color — per-side \
              fields, with shorthand expansion in the macro."],
            ["Position: ", code("position"), ", ", code("top"), ", ", code("right"),
              ", ", code("bottom"), ", ", code("left"), "."],
            ["Visual: ", code("opacity"), ", ", code("overflow"), ", ", code("shadow"),
              ", ", code("transform"), "."],
            ["Per-property transitions for every animatable property above."],
        ),
        p("There is no display/grid/float. Every node uses flexbox; the \
           framework relies on the browser (web) or Taffy (native) to do the layout."),
    },

    section(heading = "Building your own theme system") {
        p("The ", code("MyTheme"), " walk-through above is the whole thing. To recap \
           the steps:"),
        list(
            ["Define a struct with whatever fields you want. Use ", code("Tokenized<T>"),
              " for fields that should resolve through a runtime variable on web; use \
              plain ", code("T"), " for fields that don't need to."],
            ["Build instance constructors (", code("light()"), ", ", code("dark()"),
              ", ", code("high_contrast()"), "). Each constructor returns a fully-populated instance."],
            ["Implement ", code("ThemeTokens"), " to list the ", code("(name, value)"),
              " pairs the web backend will install as CSS variables."],
            ["Install at boot via ", code("install_theme(MyTheme::light())"), "."],
            ["Swap at runtime via ", code("set_theme(MyTheme::dark())"), "."],
        ),

        p("The theme type is generic over the stylesheets that consume it. ",
          code("stylesheet! { pub Foo<MyTheme> { ... } }"),
          " ties a stylesheet to a specific theme type; the stylesheet's ",
          code("base(theme)"), " closure receives a ", code("&MyTheme"),
          ", so you have full IDE completion and type checking on theme access."),

        p("If you want multiple themes at once (light + dark + high-contrast \
           selectable from a menu), the pattern is to make ", code("MyTheme"),
          " an enum or a configurable struct whose constructors produce the variant \
           you want. The framework just sees one type."),

        p("idea-ui's ", code("IdeaTheme"), " is one example of this pattern, organized \
           around intent palettes (Primary, Secondary, Neutral, Success, Danger, \
           Warning, Info) instead of role-named colors. You can read its source as a \
           reference, copy it, or ignore it entirely and roll your own."),
    },

    section(heading = "Building your own stylesheets") {
        p("The ", code("stylesheet!"), " macro is part of ", code("framework-core"),
          ". Nothing in it is idea-ui-specific. To build your own component library \
           or just some app-local styles:"),
        list(
            ["Declare your theme as above."],
            ["Write ", code("stylesheet!"), " blocks for each styled surface (",
              code("Card"), ", ", code("Btn"), ", ", code("Heading"),
              ", ...). Each one ties to your theme type via the ", code("<MyTheme>"),
              " generic."],
            ["Attach stylesheets to primitives at the call site via the ",
              code("style = ..."), " prop inside ", code("ui!"), "."],
        ),

        p("You can mix styling sources freely. A primitive's ", code("style"),
          " slot takes any ", code("IntoStyleSource"),
          " — a stylesheet builder, a raw ", code("StyleRules"),
          ", a closure that returns one. You can hand-write rule sets for one-off \
           cases and use the macro for the rest."),
    },

    section(heading = "Pre-generation") {
        p("For backends that benefit from up-front rule emission (web), the \
           framework calls ", code("Backend::register_stylesheet"), " once per ",
          code("(stylesheet, theme)"), " pair and hands it the pre-resolved rules \
           for every variant combination. The web backend mints CSS classes eagerly; \
           subsequent ", code("apply_style"), " calls just set ", code("className"),
          ". This is what keeps the per-node apply path cheap on web — no string \
           building, no rule-text emission inside the hot path."),

        p("You don't write code that interacts with pre-generation. It's the \
           framework calling backends; mentioned here only so the next section \
           about backend internals is less surprising."),
    },

    section(heading = "Where to read more") {
        list(
            ["Reactivity — how the active theme being a signal makes the reactive \
              substrate do most of the work for free."],
            ["idea-ui — a complete theme + stylesheet system built on this page's \
              primitives. Use it, fork it, or read it for ideas."],
            ["Backends — what the ", code("apply_style"), ", ",
              code("register_stylesheet"), ", and token APIs look like from a \
              backend's side."],
            ["Animations — how transitions fold into the broader animation story \
              (Presence, GPU effects, gesture-driven motion)."],
        ),
    },
}
