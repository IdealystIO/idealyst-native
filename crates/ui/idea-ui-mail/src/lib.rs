//! `idea-ui-mail` — an email-safe component set for the idealyst framework.
//!
//! This is the **opinion layer** that sits on top of the un-opinionated
//! [`backend-email`](https://docs.rs/backend-email) renderer. The email
//! backend maps primitives to HTML faithfully and takes no stance on layout;
//! these components supply email-sensible defaults — a centered fixed-width
//! content column, padded sections, a call-to-action button, dividers, and
//! typography — so a template reads well across mail clients without the
//! author hand-tuning inline styles.
//!
//! ```ignore
//! use runtime_core::{component, ui, Element};
//! use idea_ui_mail::{Button, EmailBody, EmailContainer, Heading, Section, Text};
//!
//! #[component]
//! pub fn Welcome(props: &WelcomeProps) -> Element {
//!     ui! {
//!         EmailBody() {
//!             EmailContainer() {
//!                 Section(padding = 32.0) {
//!                     Heading(content = "Welcome aboard 🎉")
//!                     Text(content = format!("Hi {}, thanks for signing up.", props.name.get()))
//!                     Button(label = "Get started", href = "https://app.dev/start")
//!                 }
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! Every component builds its `StyleRules` directly (no theme lifecycle), so
//! it renders identically whether or not a theme is installed — which is what
//! a headless email render needs. Colors are plain CSS-color strings so an
//! author can pass any brand color inline.

use std::rc::Rc;

use runtime_core::{
    component, external_link, text, ui, view, AlignItems, ChildList, Color, Element, FlexDirection,
    FontWeight, IdealystSchema, IntoElement, JustifyContent, Length, Reactive, StyleApplication,
    StyleRules, StyleSheet, TextAlign, Tokenized,
};

/// Wrap a hand-built `StyleRules` as a static style application for a
/// primitive's `style =` / `.with_style(...)`.
fn styled(rules: StyleRules) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(rules)))
}

/// A literal CSS color.
fn color(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.to_string()))
}

/// A literal pixel length.
fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

/// A literal 100% length.
fn full() -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(100.0))
}

/// Set all four padding sides to `v` px.
fn pad_all(rules: &mut StyleRules, v: f32) {
    rules.padding_top = Some(px(v));
    rules.padding_bottom = Some(px(v));
    rules.padding_left = Some(px(v));
    rules.padding_right = Some(px(v));
}

/// Set all four border-radius corners to `v` px.
fn radius_all(rules: &mut StyleRules, v: f32) {
    rules.border_top_left_radius = Some(px(v));
    rules.border_top_right_radius = Some(px(v));
    rules.border_bottom_left_radius = Some(px(v));
    rules.border_bottom_right_radius = Some(px(v));
}

// ===========================================================================
// EmailBody — the outer canvas
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct EmailBodyProps {
    /// The full-bleed canvas background behind the content column.
    pub background: String,
    /// Content — typically a single [`EmailContainer`].
    pub children: Vec<Element>,
}

impl Default for EmailBodyProps {
    fn default() -> Self {
        Self { background: Reactive::Static("#f4f4f7".into()), children: Vec::new() }
    }
}

/// The outermost email wrapper: a full-width canvas that horizontally centers
/// its content column and adds a little breathing room around it.
#[component(children)]
pub fn EmailBody(props: EmailBodyProps) -> Element {
    let mut rules = StyleRules {
        background: Some(color(&props.background.get())),
        width: Some(full()),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        ..Default::default()
    };
    pad_all(&mut rules, 24.0);
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = styled(rules)) { children } }
}

// ===========================================================================
// EmailContainer — the constrained content column
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct EmailContainerProps {
    /// Column background (the card the content sits on).
    pub background: String,
    /// Max content width in px (the classic email column is ~600px).
    pub width: f32,
    /// Corner radius in px.
    pub radius: f32,
    /// Content sections.
    pub children: Vec<Element>,
}

impl Default for EmailContainerProps {
    fn default() -> Self {
        Self {
            background: Reactive::Static("#ffffff".into()),
            width: Reactive::Static(600.0),
            radius: Reactive::Static(12.0),
            children: Vec::new(),
        }
    }
}

/// The constrained, centered content column — a `width: 100%` box capped at
/// `width` px, so it fills a phone and caps on desktop.
#[component(children)]
pub fn EmailContainer(props: EmailContainerProps) -> Element {
    let mut rules = StyleRules {
        background: Some(color(&props.background.get())),
        width: Some(full()),
        max_width: Some(px(props.width.get())),
        flex_direction: Some(FlexDirection::Column),
        ..Default::default()
    };
    radius_all(&mut rules, props.radius.get());
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = styled(rules)) { children } }
}

// ===========================================================================
// Section — a padded vertical block
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SectionProps {
    /// Uniform inner padding in px.
    pub padding: f32,
    /// Vertical gap between children in px.
    pub gap: f32,
    /// Optional block background (empty string = transparent).
    pub background: String,
    /// Section content.
    pub children: Vec<Element>,
}

impl Default for SectionProps {
    fn default() -> Self {
        Self {
            padding: Reactive::Static(24.0),
            gap: Reactive::Static(16.0),
            background: Reactive::Static(String::new()),
            children: Vec::new(),
        }
    }
}

/// A padded vertical block that stacks its children with a consistent gap.
#[component(children)]
pub fn Section(props: SectionProps) -> Element {
    let mut rules = StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(px(props.gap.get())),
        ..Default::default()
    };
    pad_all(&mut rules, props.padding.get());
    let bg = props.background.get();
    if !bg.is_empty() {
        rules.background = Some(color(&bg));
    }
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = styled(rules)) { children } }
}

// ===========================================================================
// Heading — a prominent title
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct HeadingProps {
    pub content: Reactive<String>,
    /// Text color.
    pub color: String,
    /// Font size in px.
    pub size: f32,
    /// Center the heading (else left-aligned).
    pub center: bool,
}

impl Default for HeadingProps {
    fn default() -> Self {
        Self {
            content: Reactive::Static(String::new()),
            color: Reactive::Static("#101828".into()),
            size: Reactive::Static(24.0),
            center: Reactive::Static(false),
        }
    }
}

/// A bold section title.
#[component]
pub fn Heading(props: &HeadingProps) -> Element {
    let size = props.size.get();
    let rules = StyleRules {
        color: Some(color(&props.color.get())),
        font_size: Some(px(size)),
        font_weight: Some(FontWeight::Bold),
        line_height: Some(Tokenized::Literal(size * 1.25)),
        text_align: Some(if props.center.get() { TextAlign::Center } else { TextAlign::Left }),
        ..Default::default()
    };
    text(props.content.clone()).with_style(styled(rules)).into_element()
}

// ===========================================================================
// Text — body copy
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct TextProps {
    pub content: Reactive<String>,
    /// Text color.
    pub color: String,
    /// Font size in px.
    pub size: f32,
    /// Center the text (else left-aligned).
    pub center: bool,
}

impl Default for TextProps {
    fn default() -> Self {
        Self {
            content: Reactive::Static(String::new()),
            color: Reactive::Static("#475467".into()),
            size: Reactive::Static(16.0),
            center: Reactive::Static(false),
        }
    }
}

/// A paragraph of body copy.
#[component]
pub fn Text(props: &TextProps) -> Element {
    let size = props.size.get();
    let rules = StyleRules {
        color: Some(color(&props.color.get())),
        font_size: Some(px(size)),
        line_height: Some(Tokenized::Literal(size * 1.5)),
        text_align: Some(if props.center.get() { TextAlign::Center } else { TextAlign::Left }),
        ..Default::default()
    };
    text(props.content.clone()).with_style(styled(rules)).into_element()
}

// ===========================================================================
// Button — a call-to-action
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ButtonProps {
    pub label: Reactive<String>,
    /// Destination URL (`https:`, `mailto:`, …).
    pub href: Reactive<String>,
    /// Button fill color.
    pub background: String,
    /// Label color.
    pub color: String,
    /// Corner radius in px.
    pub radius: f32,
}

impl Default for ButtonProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            href: Reactive::Static(String::new()),
            background: Reactive::Static("#6d28d9".into()),
            color: Reactive::Static("#ffffff".into()),
            radius: Reactive::Static(8.0),
        }
    }
}

/// A call-to-action button: a styled `<a>` link (block-level, centered), so it
/// works with no JavaScript. Full-width within its container — wrap it in a
/// narrower [`Section`] to constrain it.
#[component]
pub fn Button(props: &ButtonProps) -> Element {
    let label = text(props.label.clone())
        .with_style(styled(StyleRules {
            color: Some(color(&props.color.get())),
            font_weight: Some(FontWeight::SemiBold),
            font_size: Some(px(16.0)),
            ..Default::default()
        }))
        .into_element();

    let mut box_rules = StyleRules {
        background: Some(color(&props.background.get())),
        justify_content: Some(JustifyContent::Center),
        align_items: Some(AlignItems::Center),
        ..Default::default()
    };
    box_rules.padding_top = Some(px(12.0));
    box_rules.padding_bottom = Some(px(12.0));
    box_rules.padding_left = Some(px(20.0));
    box_rules.padding_right = Some(px(20.0));
    radius_all(&mut box_rules, props.radius.get());

    external_link(props.href.get(), vec![label])
        .with_style(styled(box_rules))
        .into_element()
}

// ===========================================================================
// Divider — a horizontal rule
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DividerProps {
    /// Line color.
    pub color: String,
    /// Vertical margin above and below in px.
    pub spacing: f32,
}

impl Default for DividerProps {
    fn default() -> Self {
        Self {
            color: Reactive::Static("#e4e6ef".into()),
            spacing: Reactive::Static(16.0),
        }
    }
}

/// A thin horizontal rule (a 1px top border on a zero-height box).
#[component]
pub fn Divider(props: &DividerProps) -> Element {
    let spacing = props.spacing.get();
    let rules = StyleRules {
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_top_color: Some(color(&props.color.get())),
        height: Some(px(0.0)),
        margin_top: Some(px(spacing)),
        margin_bottom: Some(px(spacing)),
        ..Default::default()
    };
    view(vec![]).with_style(styled(rules)).into_element()
}

// ===========================================================================
// Spacer — vertical whitespace
// ===========================================================================

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SpacerProps {
    /// Height in px.
    pub size: f32,
}

impl Default for SpacerProps {
    fn default() -> Self {
        Self { size: Reactive::Static(16.0) }
    }
}

/// Fixed vertical whitespace.
#[component]
pub fn Spacer(props: &SpacerProps) -> Element {
    let rules = StyleRules {
        height: Some(px(props.size.get())),
        ..Default::default()
    };
    view(vec![]).with_style(styled(rules)).into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full stack: a mail template composed from these components, rendered
    /// through the email backend, produces a self-contained document with the
    /// content and inline styles baked in (no classes, no `var(--…)`).
    #[test]
    fn mail_components_render_through_email_backend() {
        let out = backend_email::render_email(|| {
            ui! {
                EmailBody(background = "#f0f0f5") {
                    EmailContainer() {
                        Section(padding = 32.0) {
                            Heading(content = "Welcome aboard")
                            Text(content = "Thanks for signing up — you're all set.")
                            Button(label = "Get started", href = "https://app.dev/start")
                            Divider()
                            Text(content = "Questions? Just reply to this email.")
                        }
                    }
                }
            }
        });

        // Content present.
        assert!(out.html.contains("Welcome aboard"), "heading: {}", out.html);
        assert!(out.html.contains("Get started"), "button label present");
        assert!(out.html.contains(r#"href="https://app.dev/start""#), "button href present");
        // Email-safe styling: inline styles, no classes, no CSS variables.
        assert!(out.html.contains("style=\""), "inline styles present");
        assert!(!out.html.contains("class="), "no classes: {}", out.html);
        assert!(!out.html.contains("var("), "no CSS variables: {}", out.html);
        // The canvas background made it onto the body wrapper.
        assert!(out.html.contains("#f0f0f5"), "canvas background present");
        // Plaintext alternative carries the copy.
        assert!(out.text.contains("Welcome aboard"), "plaintext: {:?}", out.text);
    }

    /// A `Heading` is a single styled text node with the size/weight baked in.
    #[test]
    fn heading_bakes_font_styles_inline() {
        let out = backend_email::render_email(|| {
            ui! { Heading(content = "Big", size = 30.0) }
        });
        assert!(out.html.contains("font-size: 30px"), "got: {}", out.html);
        assert!(out.html.contains("font-weight: 700"), "bold heading: {}", out.html);
        assert!(out.html.contains("Big"));
    }
}
