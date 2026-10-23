//! Styling — `stylesheet!`, layout, themes and tokens. Each step centers
//! on a live fiddle whose starting code lives in `crate::fiddles::styling`.

use idea_ui::{typography_kind, Typography};
use runtime_core::{ui, Element};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage, workspace};
use crate::fiddle::Fiddle;
use crate::routes::{STY_LAYOUT_ROUTE, STY_SHEETS_ROUTE, STY_THEMES_ROUTE};

pub fn stylesheets() -> Element {
    ui! {
        LessonPage(
            current = STY_SHEETS_ROUTE.name(),
            title = "Your first stylesheet".to_string(),
            lead = "Declare a look once with stylesheet!, then hand it to any primitive's style prop.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/styling/sheets.rs")) }),
            learn = vec![
                "Declare a look with stylesheet! and apply it with style",
                "What the macro generates for you",
                "The property names, units and shorthands",
            ],
        ) {
            Typography(
                content = "A stylesheet is a named set of style rules: colors, spacing, borders, \
                    type. You declare it once, at the top level of a file, with the stylesheet! \
                    macro, and apply it by passing its name to a primitive's style prop. The same \
                    declaration styles your app on every platform: the web backend turns it into a \
                    CSS class, iOS and Android set the native view's properties directly.".to_string()
            )

            Typography(content = "Reading the example".to_string(), kind = typography_kind::H2)
            Typography(
                content = "stylesheet! { pub Card<()> { base(_t) { ... } } } declares a sheet named \
                    Card. The base block holds the rules that always apply. <()> says the sheet \
                    reads no theme \u{2014} every value is written out \u{2014} and _t is the \
                    unused name for that theme; you'll fill both in when you get to themes.".to_string()
            )
            Typography(
                content = "The macro generates a function with the sheet's name, so style = Card() \
                    applies it. It also generates card_style(), which returns the shared, cached \
                    sheet itself; the two are interchangeable at a call site.".to_string()
            )

            Typography(content = "Property names".to_string(), kind = typography_kind::H2)
            Typography(
                content = "A rule is property: value. The fill is background and the text color is \
                    color (there is no background_color). Bare numbers are pixels; write \
                    Length::pct(50.0) for a percentage. padding, margin, border_radius, border_width \
                    and border_color set all four sides at once, and padding_horizontal / \
                    padding_vertical set two \u{2014} each also has per-side forms like padding_top.".to_string()
            )
            CodePanel(src = r##"padding: 16.0,                     // all four sides, in px
padding_horizontal: 24.0,          // left and right
width: Length::pct(100.0),         // a percentage of the parent
background: Color("#4f46e5".into()),
font_weight: FontWeight::Bold,"##.to_string())
            Typography(
                content = "Try it: give the card a border_radius of 0, make the title's font_size 28, \
                    or change Page's background. A property name that doesn't exist is a compile \
                    error, underlined where you typed it.".to_string()
            )

            Callout(label = "Style text where it is".to_string()) {
                Typography(
                    content = "A text's color and font come from the text's own style. Setting \
                        color on a view does not reach the text inside it on every platform, so the \
                        example gives Title and Body their own sheets.".to_string(),
                    muted = true,
                )
            }

            Typography(
                content = "A stylesheet can do more than one fixed look: named variants picked at \
                    the call site, and hovered/pressed/focused states. Those are the Component \
                    stylesheets track. First, layout.".to_string()
            )

            DocsLink(
                summary = "StyleRules, the stylesheet! grammar, and how a style is resolved.".to_string(),
                link_label = "Styling reference".to_string(),
                doc_file = "styling.md".to_string(),
            )
        }
    }
}

pub fn layout() -> Element {
    ui! {
        LessonPage(
            current = STY_LAYOUT_ROUTE.name(),
            title = "Layout".to_string(),
            lead = "Every view is a flexbox. Columns by default, rows when you ask.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/styling/layout.rs")) }),
            learn = vec![
                "Why every view is a flex column by default",
                "Rows, gaps, alignment, growth and wrapping",
                "How sizes, padding and percentages work",
            ],
        ) {
            Typography(
                content = "Idealyst lays out with flexbox on every platform \u{2014} one layout engine \
                    computes the same frames for the web, iOS and Android. Every view is a flex \
                    container, and like React Native (unlike the web's default) its children stack \
                    in a column. The layout properties are ordinary style rules, so they go in a \
                    stylesheet next to the colors.".to_string()
            )

            Typography(content = "The properties that do the work".to_string(), kind = typography_kind::H2)
            Typography(
                content = "flex_direction picks the main axis: FlexDirection::Column (the default) or \
                    Row. justify_content distributes children along that axis \u{2014} FlexStart, \
                    Center, SpaceBetween and so on \u{2014} and align_items places them across it \
                    (Stretch by default, which is why a column's children are full width). gap puts \
                    space between children without margins.".to_string()
            )
            Typography(
                content = "On a child, flex_grow claims a share of the leftover space and flex_basis \
                    sets the size it starts from. flex_wrap: FlexWrap::Wrap lets a row continue onto \
                    the next line instead of squeezing. The tiles in the example use all three: each \
                    starts at 120px, grows to fill its line, and wraps when the row is too narrow.".to_string()
            )
            Typography(
                content = "Sizes are width, height, their min_ and max_ forms, and aspect_ratio; \
                    padding and margin space things inside and outside a box. A percentage \
                    (Length::pct(50.0)) is relative to the parent.".to_string()
            )
            Typography(
                content = "Try it: change the Grid to FlexDirection::Column, remove flex_grow from \
                    Tile, or set the Header's justify_content to Center. Then drag the divider of \
                    your browser window narrower and watch the tiles wrap.".to_string()
            )

            Callout(label = "No overflow: scroll".to_string()) {
                Typography(
                    content = "overflow takes Visible or Hidden only. Scrolling is a primitive, not a \
                        property: wrap the content in a scroll_view, which every platform backs with \
                        its native scroller.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Every layout property and its default.".to_string(),
                link_label = "Styling reference".to_string(),
                doc_file = "styling.md".to_string(),
            )
        }
    }
}

pub fn themes() -> Element {
    ui! {
        LessonPage(
            current = STY_THEMES_ROUTE.name(),
            title = "Themes & tokens".to_string(),
            lead = "Name your values once; swap the whole look by swapping the values.".to_string(),
            workspace = workspace(|| ui! { Fiddle(source = include_str!("../fiddles/styling/themes.rs")) }),
            learn = vec![
                "Declare a theme with #[derive(Tokens)]",
                "Read tokens in a stylesheet and swap themes at runtime",
                "How token names are formed, and why they're global",
                "Where idea-ui's theme fits",
            ],
        ) {
            Typography(
                content = "Hard-coded colors are fine for one screen, but an app wants one place that \
                    says what \"surface\" or \"accent\" is \u{2014} and a way to change it for dark \
                    mode. That place is a theme: a struct whose fields are your design's named \
                    values. Each field is a style token. A stylesheet refers to a token by name, so \
                    when the theme changes, every style using that token repaints, and nothing else \
                    is recomputed.".to_string()
            )

            Typography(content = "Declaring a theme".to_string(), kind = typography_kind::H2)
            Typography(
                content = "#[derive(Tokens)] turns the struct into a token set. #[tokens(default = \
                    light)] names the function that returns its default values \u{2014} the values \
                    used before any theme is installed, and in a page rendered ahead of time. Each \
                    token's name is its field path in kebab case (surface, accent), with the prefix \
                    in front: app-surface. A nested struct of tokens becomes a namespace \
                    (t.accent.soft_bg(), named accent-soft-bg).".to_string()
            )
            Typography(
                content = "A second theme is usually the first with a few changes, which is what \
                    light().with(|t| ...) writes. Leaving a field out of a theme function is a \
                    compile error, so every theme sets every token.".to_string()
            )

            Typography(content = "Using it".to_string(), kind = typography_kind::H2)
            Typography(
                content = "In stylesheet! { pub Panel<AppTheme> { base(t) { ... } } }, the <AppTheme> \
                    slot is the vocabulary and t gives you its tokens: t.surface() refers to the \
                    surface token. It is a reference, not a value \u{2014} that is what lets a swap \
                    repaint without re-running anything \u{2014} and a misspelled name fails to \
                    compile. Inside app(), AppTheme::install(light()) installs the theme before \
                    anything renders, and AppTheme::set(dark()) swaps it from a handler.".to_string()
            )
            Typography(
                content = "Try it: add a border_color token, use it in Panel, and give dark() its own \
                    value. Or replace the toggle with AppTheme::follow_system(light(), dark()) to \
                    pick the platform's light or dark preference.".to_string()
            )

            Callout(label = "Token names are global".to_string()) {
                Typography(
                    content = "Every token set in an app shares one table of names, and two sets may \
                        not claim the same name. idea-ui's theme uses color-, spacing-, radius- and a \
                        few other prefixes, so an app theme beside it takes #[tokens(prefix = \
                        \"app\")], as this one does.".to_string(),
                    muted = true,
                )
            }

            Typography(content = "idea-ui's theme".to_string(), kind = typography_kind::H2)
            Typography(
                content = "idea-ui, the framework's component library, ships a complete theme built \
                    this same way. Install it with install_idea_theme(light_theme()) inside app() \
                    and write stylesheets against its vocabulary, IdeaThemeRef: t.color.surface(), \
                    t.color.text(), t.spacing.md(), t.radius.lg(). The next track uses it.".to_string()
            )
            CodePanel(src = r##"stylesheet! {
    pub Panel<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            padding: t.spacing.md(),
            border_radius: t.radius.lg(),
        }
    }
}"##.to_string())

            DocsLink(
                summary = "Token sets, fallbacks, flatten/rename/prefix, and static rendering of every palette.".to_string(),
                link_label = "Styling reference \u{2014} Style tokens".to_string(),
                doc_file = "styling.md".to_string(),
            )
        }
    }
}
