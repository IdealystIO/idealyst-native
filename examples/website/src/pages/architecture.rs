//! Architecture — the high-level "how Idealyst fits together" page.
//!
//! One reactive core, a single Backend interface to every platform, and the
//! documentation + SDK tooling built around it. The diagram is built from
//! native primitives — labeled cards (`ChartBox`) stacked into layers with
//! `ChartArrow` connectors and `ChartRow` splits — so it renders identically
//! on every backend (no SVG, no absolute positioning). This page is the
//! overview; the in-depth walkthroughs live in the tutorial site.

use runtime_core::{
    component, stylesheet, ui, AlignItems, Color, Element, FlexDirection, FontWeight, Length, Ref,
    StyleApplication, TextAlign, TextTransform, Tokenized, ViewHandle,
};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::{PageHeader, PageSection};
use crate::shell::{layout_with_toc, TocEntry};

// =============================================================================
// Chart styles — cards, connectors, the row split, and the text tiers.
// =============================================================================

stylesheet! {
    pub ChartCard<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 6.0,
            padding: 18.0,
            border_radius: 14.0,
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e7e2d3".into())),
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            width: Length::pct(100.0),
            min_width: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// The Core card — the heart. Tinted with the primary intent so the eye
// lands on it first.
stylesheet! {
    pub ChartCardAccent<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            gap: 6.0,
            padding: 18.0,
            border_radius: 14.0,
            border_width: 1.0,
            border_color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            background: Tokenized::token("intent-primary-soft-bg", Color("rgba(91, 108, 255, 0.12)".into())),
            width: Length::pct(100.0),
            min_width: 0.0,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// Row that holds two (or more) cards side by side. `ChartRow` wraps each
// child in a flex-1 cell so the cards share the width evenly and stretch
// to equal height.
stylesheet! {
    pub ChartSplit<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            gap: 14.0,
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    pub ChartCell<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: 0.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub ChartEyebrow<()> {
        base(_t) {
            font_size: 11.0,
            font_weight: FontWeight::SemiBold,
            letter_spacing: 0.8,
            text_transform: TextTransform::Uppercase,
            color: Tokenized::token("color-text-muted", Color("#8a8270".into())),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub ChartTitle<()> {
        base(_t) {
            font_size: 17.0,
            font_weight: FontWeight::Bold,
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub ChartBody<()> {
        base(_t) {
            font_size: 14.0,
            line_height: 20.0,
            color: Tokenized::token("color-text-muted", Color("#6b7280".into())),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

stylesheet! {
    pub ChartArrowText<()> {
        base(_t) {
            font_size: 20.0,
            color: Tokenized::token("color-text-muted", Color("#8a8270".into())),
            text_align: TextAlign::Center,
        }
        transitions { color: 250ms EaseInOut, }
    }
}

// =============================================================================
// Chart components.
// =============================================================================

#[derive(Default)]
pub struct ChartBoxProps {
    /// Small uppercase kicker above the title (e.g. "The heart"). Empty = omit.
    pub eyebrow: String,
    pub title: String,
    pub body: String,
    /// Tint with the primary intent — reserved for the Framework Core card.
    pub accent: bool,
}

/// A single labeled card in the diagram: optional eyebrow, a bold title,
/// and a muted body line, all centered.
#[component]
pub fn ChartBox(props: ChartBoxProps) -> Element {
    // Both arms coerce to the same `fn` pointer so the `if`/`else` unifies
    // on one type; the closure form also re-resolves on theme change.
    let card: fn() -> StyleApplication = if props.accent {
        || StyleApplication::new(ChartCardAccent::sheet())
    } else {
        || StyleApplication::new(ChartCard::sheet())
    };
    let eyebrow = props.eyebrow;
    let title = props.title;
    let body = props.body;
    let eyebrow_style = move || StyleApplication::new(ChartEyebrow::sheet());
    let title_style = move || StyleApplication::new(ChartTitle::sheet());
    let body_style = move || StyleApplication::new(ChartBody::sheet());
    ui! {
        view(style = card) {
            if !eyebrow.is_empty() {
                text(style = eyebrow_style) { eyebrow }
            }
            text(style = title_style) { title }
            if !body.is_empty() {
                text(style = body_style) { body }
            }
        }
    }
}

#[derive(Default)]
pub struct ChartArrowProps {}

/// A centered downward connector between two stacked layers.
#[component]
pub fn ChartArrow(_props: &ChartArrowProps) -> Element {
    let style = move || StyleApplication::new(ChartArrowText::sheet());
    ui! { text(style = style) { "\u{2193}".to_string() } }
}

#[derive(Default)]
pub struct ChartLabelProps {
    pub label: String,
}

/// A centered overline that titles a band of the diagram (e.g. the
/// "Backend" layer) without a card around it.
#[component]
pub fn ChartLabel(props: ChartLabelProps) -> Element {
    let style = move || StyleApplication::new(ChartEyebrow::sheet());
    let label = props.label;
    ui! { text(style = style) { label } }
}

#[derive(Default)]
pub struct ChartRowProps {
    pub children: Vec<Element>,
}

/// Lay children out in a row, each in an equal-width, equal-height cell.
#[component]
pub fn ChartRow(props: ChartRowProps) -> Element {
    let row = ChartSplit();
    ui! {
        view(style = row) {
            for child in props.children {
                view(style = ChartCell()) { child }
            }
        }
    }
}

// =============================================================================
// The diagram itself.
// =============================================================================

fn diagram() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            ChartBox(
                title = "Your app".to_string(),
                body = "Components + reactive state — authored once".to_string(),
            )
            ChartArrow()
            ChartBox(
                eyebrow = "The heart".to_string(),
                title = "Framework Core".to_string(),
                body = "Reactivity · scene model · defines the Backend interface".to_string(),
                accent = true,
            )
            ChartArrow()
            ChartLabel(label = "Backend — the framework's one connection to the platform".to_string())
            ChartRow {
                ChartBox(
                    title = "Direct backend".to_string(),
                    body = "Local & release builds. Core → Backend → native SDKs / platform.".to_string(),
                )
                ChartBox(
                    title = "Hosted runtime".to_string(),
                    body = "Dev mode. Runtime Server Backend → the Wire (websockets) → Dev Client on the device.".to_string(),
                )
            }
            ChartArrow()
            ChartLabel(label = "Built on the catalog".to_string())
            ChartRow {
                ChartBox(
                    title = "Auto-generated docs".to_string(),
                    body = "Browse every component & type, with live recipe previews.".to_string(),
                )
                ChartBox(
                    title = "MCP server".to_string(),
                    body = "Real-time, accurate context for LLMs — yours and third-party libraries'.".to_string(),
                )
            }
            ChartArrow()
            ChartBox(
                eyebrow = "Built on the core".to_string(),
                title = "SDKs".to_string(),
                body = "camera · microphone · video · screen-recorder · media-writer … cross-platform capabilities you can combine into bigger tools.".to_string(),
            )
        }
    }
}

// =============================================================================
// Page.
// =============================================================================

pub fn page() -> Element {
    let model_ref: Ref<ViewHandle> = Ref::new();
    let core_ref: Ref<ViewHandle> = Ref::new();
    let catalog_ref: Ref<ViewHandle> = Ref::new();
    let sdk_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: model_ref, label: "The layered model" },
        TocEntry { handle: core_ref, label: "Core & the Backend interface" },
        TocEntry { handle: catalog_ref, label: "Catalog → docs & MCP" },
        TocEntry { handle: sdk_ref, label: "SDKs" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Architecture",
                blurb = "How Idealyst fits together: one reactive core, a single Backend interface \
                    to every platform, and the tooling built around it. This page is the overview \
                    — the tutorial site walks each layer in depth.",
            )

            PageSection(handle = model_ref) {
                Stack(gap = StackGap::Lg) {
                    Typography(content = "The layered model".to_string(), kind = typography_kind::H2)
                    Typography(
                        content = "Read it top-down. Your app is authored once against the Core; \
                            the Core never talks to a platform directly — it only ever speaks to a \
                            Backend. Everything below the Core is a different way of satisfying that \
                            one interface.".to_string(),
                    )
                    diagram()
                }
            }

            PageSection(handle = core_ref) {
                Stack(gap = StackGap::Lg) {
                    Typography(content = "The core & the Backend interface".to_string(), kind = typography_kind::H2)
                    Typography(
                        content = "Framework Core is the small module at the center: it owns the \
                            reactive system (signals, effects, the token registry) and the scene \
                            model, and it defines the Backend interface that makes the framework \
                            cross-platform. The Core is deliberately minimal — anything composable \
                            from primitives lives outside it.".to_string(),
                    )
                    Typography(
                        content = "A Backend can be satisfied two ways. The direct backend is what \
                            a local or release build uses: the Core talks to the Backend, which \
                            talks to the native SDKs and platform. In dev mode, three layers slot \
                            in between — the Runtime Server Backend, the Wire, and the Dev Client \
                            — so the Core can run on a server and drive any connected device over \
                            websockets.".to_string(),
                    )
                    Typography(
                        content = "That hosted-runtime split buys two things. Hot reload without \
                            re-shipping a binary (hot-linking native code on a device is often \
                            impossible), and a session whose state lives on the server — so \
                            \"show me this exact screen on Android instead of iPhone\" is just a \
                            second device attaching to the same runtime. It isn't a perfect \
                            system, but it's a powerful one.".to_string(),
                        muted = true,
                    )
                    learn_more("See the Backends page for per-platform status of each target.", &crate::routes::BACKENDS_ROUTE, "Backends")
                }
            }

            PageSection(handle = catalog_ref) {
                Stack(gap = StackGap::Lg) {
                    Typography(content = "Catalog → docs & MCP".to_string(), kind = typography_kind::H2)
                    Typography(
                        content = "The catalog is a registry of every component and type, with \
                            optional compile-time safeguards that enforce best practices. Two \
                            things consume it: auto-generated documentation, and an MCP server \
                            that gives LLMs real-time access to component docs — including those \
                            shipped by third-party libraries.".to_string(),
                    )
                    Typography(
                        content = "Recipes are the reliability trick. A recipe is a compiled usage \
                            example, so a change to a component's props that breaks the recipe \
                            fails the build. The docs and the LLM context can't silently drift out \
                            of date — if they're stale, the code doesn't compile.".to_string(),
                    )
                    learn_more("The Robot & MCP page covers the introspection surface.", &crate::routes::AGENTIC_ROUTE, "Robot & MCP")
                }
            }

            PageSection(handle = sdk_ref) {
                Stack(gap = StackGap::Lg) {
                    Typography(content = "SDKs".to_string(), kind = typography_kind::H2)
                    Typography(
                        content = "SDKs are peripheral but first-class: optimized, cross-platform \
                            capabilities built on the Core — camera, microphone, video, screen \
                            recording, media writing, and more. They bring desirable functionality \
                            to every supported platform behind one author-facing API.".to_string(),
                    )
                    Typography(
                        content = "And they compose. The MediaStream / AudioStream abstractions \
                            that back the camera and recorder SDKs are the same building blocks \
                            you'd reach for to build, say, a cross-platform video compositor — \
                            that's the intended way to extend the framework, not fork it.".to_string(),
                        muted = true,
                    )
                }
            }
        }
    };
    layout_with_toc(content, toc)
}

/// A small "learn more" line with an inline route link. The website's
/// `Section` component is prose-only, so sections that cross-link build
/// the link themselves with the framework's `link` primitive.
fn learn_more(summary: &str, route: &'static runtime_core::Route<()>, link_label: &str) -> Element {
    let link_style = move || StyleApplication::new(LearnMoreLink::sheet());
    let summary = summary.to_string();
    let link_label = format!("{} \u{2192}", link_label);
    ui! {
        view(style = LearnMoreRow()) {
            Typography(content = summary, muted = true)
            link(route = route, params = ()) {
                text(style = link_style) { link_label }
            }
        }
    }
}

stylesheet! {
    pub LearnMoreRow<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 8.0,
        }
    }
}

stylesheet! {
    pub LearnMoreLink<()> {
        base(_t) {
            color: Tokenized::token("intent-primary-fg", Color("#3947d6".into())),
            font_size: 15.0,
            font_weight: FontWeight::SemiBold,
        }
        state hovered(_t) {
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions { color: 150ms EaseOut, }
    }
}
