//! Styles page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{code_block, page_header, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{typography, card, stack};

docs! {
    slug = "styles",
    title = "Styles",
    category = Foundation,
    description = "Stylesheets, tokens, variants, overrides, states, transitions. The styling primitives every UI builds on.",
    related = ["reactivity", "primitives", "components", "backends", "building-a-theme-system"],
    concepts = [Stylesheet, Token, Variant, Override, StyleState, Transition],

    section(heading = "Overview") {
        p("Styling in Idealyst is built from three primitives: ",
          code("Stylesheet"), "s (typed declarations of style rules), ",
          code("Tokenized<T>"),
          " (named values that may resolve through a runtime variable \
           store), and per-primitive ", code("style"),
          " slots that take the resolved rule set. The framework caches \
           resolved rules and applies them through the same reactive \
           substrate everything else uses."),
        p("This page is about those primitives — declaring stylesheets, \
           using tokens, variants, overrides, interaction states, \
           transitions, and how resolution works. A \"theme\" is not a \
           framework concept; it's a user-space pattern of grouping \
           tokens for app-wide swap. See ",
          link("Building a theme system", to = "building-a-theme-system"),
          " for the cookbook on layering one on top."),
    },

    section(heading = "The pieces") {
        p("Three moving parts:"),
        list(
            [code("Stylesheet"), " — declared with the ",
              code("stylesheet!"),
              " macro. A typed builder that produces a ", code("StyleRules"),
              " from a tracked context. Stylesheets can be parameterized \
              over any context type (your own app's config struct, \
              \"theme\" struct, whatever) — the macro doesn't impose a \
              shape."],
            [code("Tokenized<T>"), " — a ", code("(name, fallback)"),
              " pair, or just a literal value. Stylesheets emit ",
              code("Tokenized"), " property values; backends with a \
              runtime variable system (web) resolve through the named \
              store, backends without one (iOS, Android) read the \
              fallback baked into the rule."],
            [code("StyleRules"), " — the concrete output. Every \
              primitive's ", code("style"),
              " slot eventually gets one of these."],
        ),
        p("Application code writes stylesheets. The framework handles \
           caching, resolution, and the backend calls that put the \
           result on screen. Bundling tokens into a swappable \"theme\" \
           is one pattern (a common and useful one), but it lives in \
           user space — see the ", link("Building a theme system",
                                        to = "building-a-theme-system"),
          " page for that."),
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

        p("A stylesheet emitting a ", code("Tokenized<Color>"),
          " produces either a literal color or a token reference like ",
          code("Token { name: \"primary\", fallback: Color(\"#3b82f6\") }"),
          ". The backend decides what to do with it. Tokens are \
           installed (and updated) via ", code("install_tokens(...)"),
          " / ", code("update_tokens(...)"),
          " — both take a flat ", code("Vec<TokenEntry>"),
          " so any code path can register or refresh the token store."),

        p("What the web backend does: tokens are installed as CSS \
           custom properties on the document root:"),

        code(text, r##"
            :root {
                --bg: #ffffff;
                --text: #111111;
                --primary: #3b82f6;
                --space-md: 16px;
            }
        "##),

        p("When the backend emits CSS for a stylesheet rule, every \
           tokenized property turns into a ",
          code("var(--name, fallback)"), " reference:"),

        code(text, r##"
            .idealyst-card-12 {
                background: var(--bg, #ffffff);
                color: var(--text, #111111);
                padding: var(--space-md, 16px);
            }
        "##),

        p("That's the whole trick. When tokens are updated (say a \
           dark-mode swap), the backend writes new values into ",
          code(":root"),
          "'s style block. Every CSS rule that referenced ",
          code("var(--bg)"),
          " now resolves to the new color automatically — by the \
           browser, in one paint pass. The framework doesn't iterate \
           over DOM elements. It doesn't change class names. It \
           doesn't touch any rule body."),

        p("What native backends do: iOS and Android don't have a \
           runtime variable system, so they ignore the token name and \
           read the ", code(".value()"),
          " (the fallback). When tokens change, every styled node has \
           a per-token reactive subscription via the framework's ",
          code("TOKEN_REGISTRY"),
          "; only nodes whose resolved style references the changed \
           token re-apply. The rule-set cache deduplicates: if two \
           token configurations produce identical rules for the same ",
          code("(sheet, variants)"),
          " pair, the second resolution is a refcount bump."),

        p("What generator backends do: Roku and other generator \
           backends can't ship closures to the device. They get a \
           different deal — the ",
          code("Derived<T>"),
          " machinery and the wire protocol install a device-side \
           variable system that mirrors the host's tokens. Token \
           swaps on the device are device-side variable rewrites, \
           conceptually the same as the web's model."),
    },

    section(heading = "Stylesheets") {
        p("The ", code("stylesheet!"), " macro is how you declare a \
           typed stylesheet. The generic parameter is the context type \
           the stylesheet's closures receive — your own app's config \
           struct, your token bundle, a \"theme\" struct (one pattern \
           authors use), or ", code("()"),
          " if you don't need a context. Grammar:"),

        code(rust, r##"
            use runtime_core::{stylesheet, Color, Length, Tokenized};

            // Token-only example. No context type needed; the
            // stylesheet directly references token names.
            stylesheet! {
                pub Card<()> {
                    base(_) {
                        background: Tokenized::token("bg",       Color::from("#ffffff")),
                        color:      Tokenized::token("text",     Color::from("#111111")),
                        padding:    Tokenized::token("space-md", Length::Px(16.0)),
                        border_radius: 8.0,
                    }

                    variant size {
                        small(_)  { padding: Length::Px(8.0) }
                        #[default]
                        medium(_) {}
                        large(_)  { padding: Length::Px(24.0) }
                    }

                    variant kind {
                        #[default]
                        elevated(_) {
                            shadow: Shadow { x: 0.0, y: 2.0, blur: 8.0, color: Color::from("#0001") },
                        }
                        outlined(_) {
                            background: Color::from("transparent"),
                            border: (2.0, Tokenized::token("text", Color::from("#111111"))),
                        }
                    }

                    override padding: Length

                    state hovered(_) { opacity: 0.92 }
                    state pressed(_) { opacity: 0.85 }

                    transitions {
                        background: 200ms EaseOut
                        opacity:    150ms Linear
                    }
                }
            }
        "##),

        p("Pass a context type (anything you want — most apps that go \
           this route define a struct holding their tokens grouped \
           semantically) when stylesheet closures should consume \
           typed values instead of repeating token names everywhere. \
           Both shapes are first-class; see ",
          link("Building a theme system", to = "building-a-theme-system"),
          " for the bundled-context pattern."),

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
          code("(stylesheet, variants, context, overrides)"),
          " to a concrete ",
          code("Rc<StyleRules>"),
          ". The framework caches the result so the same combination \
           of inputs returns the same ", code("Rc"), " instance."),

        p("The cache key:"),
        list(
            ["Stylesheet is identified by ", code("Rc"), " pointer."],
            ["Variants are an ordered set of ", code("(axis, value)"),
             " strings."],
            ["Context (whatever the stylesheet is parameterized over) \
              is identified by ", code("Rc"), " pointer."],
            ["Overrides are serialized into a content key."],
        ),

        p("The cleverness on the web side: the content key that the \
           web backend uses to mint a CSS class hashes tokens by their \
           name, not by their resolved value. So ",
          code("Card().size(Large).kind(Outlined)"),
          " produces the same content key regardless of what the \
           current token values are. The web backend mints one class \
           per ", code("(sheet, variants)"),
          " pair and reuses it forever — token updates turn into a \
           handful of ", code(":root"),
          " variable writes, never new class minting and never \
           className mutations on individual nodes. The swap is \
           O(tokens) rather than O(styled nodes)."),

        compare(from = React) {
            p("Where React-style libraries typically rebuild on Context \
               value change (every consumer re-renders), Idealyst keeps \
               the per-node style Effects subscribed to a small set of \
               tokens through the ", code("TOKEN_REGISTRY"),
              " — only effects that actually read a changed token \
               re-fire. On web the per-node effects are bypassed \
               entirely because CSS variables do the work in the \
               browser."),
            p("From styled-components / Emotion: each themed component \
               usually generates a new class name when context-derived \
               styles change, which forces every instance to re-attach. \
               Idealyst's class names are token-stable — the content \
               key hashes token names, so a stylesheet's class is the \
               same regardless of token values. No className mutations \
               on token swap."),
            p("From Tailwind: closest analog is CSS-variable-based \
               theming in Tailwind (the ",
              code("darkMode: \"class\""),
              " strategy with custom properties). Same payoff: a class \
               flip on ", code(":root"),
              " updates colors downstream without touching anything \
               else. Idealyst's split between literal fallbacks (for \
               non-web backends) and variable references (for web) \
               gives you the same payoff automatically — you don't \
               choose between the two strategies, the backend \
               chooses."),
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
                pub Btn<()> {
                    base(_) {
                        background: Tokenized::token("primary", Color::from("#3b82f6")),
                    }
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

    // The "Building your own theme system" section moved to a
    // dedicated Advanced page so this page stays focused on the
    // styling primitives. See `building-a-theme-system`.

    section(heading = "Building your own stylesheets") {
        p("The ", code("stylesheet!"),
          " macro is part of ", code("runtime-core"),
          ". Nothing in it is idea-ui-specific. To build your own \
           component library or just some app-local styles:"),
        list(
            ["Pick a context type, or use ", code("()"),
             " if your stylesheets don't need one. (The bundled-context \
              \"theme\" pattern is covered separately in ",
             link("Building a theme system", to = "building-a-theme-system"),
             ".)"],
            ["Write ", code("stylesheet!"),
             " blocks for each styled surface (",
             code("Card"), ", ", code("Btn"), ", ", code("Heading"),
             ", ...). Reference token names directly via ",
             code("Tokenized::token(\"name\", fallback)"), "."],
            ["Attach stylesheets to primitives at the call site via the ",
             code("style = ..."), " prop inside ", code("ui!"), "."],
        ),

        p("You can mix styling sources freely. A primitive's ",
          code("style"), " slot takes any ", code("IntoStyleSource"),
          " — a stylesheet builder, a raw ", code("StyleRules"),
          ", a closure that returns one. You can hand-write rule sets \
           for one-off cases and use the macro for the rest."),
    },

    section(heading = "Pre-generation") {
        p("For backends that benefit from up-front rule emission \
           (web), the framework calls ",
          code("Backend::register_stylesheet"), " once per ",
          code("(stylesheet, context)"),
          " pair and hands it the pre-resolved rules for every \
           variant combination. The web backend mints CSS classes \
           eagerly; subsequent ", code("apply_style"),
          " calls just set ", code("className"),
          ". This is what keeps the per-node apply path cheap on web \
           — no string building, no rule-text emission inside the hot \
           path."),

        p("You don't write code that interacts with pre-generation. \
           It's the framework calling backends; mentioned here only \
           so the next section about backend internals is less \
           surprising."),
    },

    section(heading = "Where to read more") {
        list(
            [link("Building a theme system", to = "building-a-theme-system"),
             " — the cookbook for bundling tokens into a typed \"theme\" \
              struct with install/swap-at-runtime mechanics."],
            [link("Reactivity", to = "reactivity"),
             " — how per-token signals plug into the same effect graph \
              everything else uses."],
            ["idea-ui — a complete stylesheet system built on this \
              page's primitives. Use it, fork it, or read it for ideas."],
            [link("Backends", to = "backends"),
             " — what the ", code("apply_style"), ", ",
             code("register_stylesheet"),
             ", and token APIs look like from a backend's side."],
        ),
    },
}
