//! idea-ui — a short overview of the first-party component library.
//! Deliberately brief: this page orients and links out; the library
//! will get its own catalog-driven docs site (idealyst docs), which
//! this page will link to when it's published.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{PageHeader, PageSection, Section};
use crate::routes::{PRIMITIVES_ROUTE, STYLING_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let what_ref: Ref<ViewHandle> = Ref::new();
    let ships_ref: Ref<ViewHandle> = Ref::new();
    let theme_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: what_ref, label: "Built on public primitives" },
        TocEntry { handle: ships_ref, label: "What ships" },
        TocEntry { handle: theme_ref, label: "Theming" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "idea-ui",
                blurb = "The first-party component library: forty-plus themed components \
                 \u{2014} buttons, forms, tables, overlays, navigation chrome \u{2014} \
                 built entirely on the same public primitives and stylesheet system \
                 available to any crate. The page you're reading is composed from it.",
            )
            PageSection(handle = what_ref) { built_on() }
            PageSection(handle = ships_ref) { what_ships() }
            PageSection(handle = theme_ref) { theming() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Sections — no-param file-local helpers (allowed per CLAUDE.md §9.5).
// =============================================================================

fn built_on() -> Element {
    let example = "use idea_ui::{Button, Card, Field, Stack, StackGap, Typography};\n\
                   \n\
                   let email = signal(String::new());\n\
                   \n\
                   ui! {\n    \
                       Card() {\n        \
                           Stack(gap = StackGap::Md) {\n            \
                               Typography(content = \"Sign in\".to_string(), kind = typography_kind::H3)\n            \
                               Field(\n                \
                                   label = Some(\"Email\".to_string()),\n                \
                                   value = email,\n                \
                                   on_change = move |v| email.set(v),\n            \
                               )\n            \
                               Button(label = \"Continue\".to_string(), on_click = submit)\n        \
                           }\n    \
                       }\n\
                   }";
    ui! {
        Section(
            title = "Built on public primitives".to_string(),
            paragraphs = vec![
                "idea-ui components are ordinary `#[component]` functions composing \
                 `view` / `text` / `text_input` and the rest through `ui!`, styled with \
                 `stylesheet!` against the theme's tokens. There is no private API \
                 underneath them \u{2014} everything the library does, your components \
                 and your own component library can do the same way.".to_string(),
                "Because they lower to primitives, every component renders natively on \
                 every backend: the same `Card` is real views on iOS, AppKit on macOS, \
                 DOM on the web, and draws through the GPU renderer on a bare \
                 surface.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn what_ships() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Section(
                title = "What ships".to_string(),
                paragraphs = vec![
                    "Layout & surfaces: `Stack`, `Grid`, `Center`, `Card`, `Surface`, \
                     `Divider`, `Spacer`. Forms: `Field`, `Textarea`, `Select`, \
                     `Autocomplete`, `Checkbox`, `Radio`, `Switch`, `Slider`, \
                     `SegmentedControl`. Overlays: `Modal`, `Popover`, `Tooltip`, \
                     `Toast`, `Menu`. Data & feedback: `Table`, `List`, `Tabs`, \
                     `Pagination`, `Breadcrumbs`, `Badge`, `Chip`, `Tag`, `Avatar`, \
                     `Alert`, `Progress`, `Skeleton`, `Spinner`, `Collapsible`. Plus \
                     `Typography`, `Icon` (with the Lucide set), `Image`, and `Link`.".to_string(),
                    "Interactive components carry the details you'd otherwise build \
                     twice: keyboard focus rings drawn from the theme's focus token, \
                     hover/press states on every backend, and per-slot style override \
                     props when a design needs to reach inside.".to_string(),
                ],
            )
            link(route = &PRIMITIVES_ROUTE, params = ()) {
                Typography(content = "The layer underneath \u{2192} Primitives".to_string())
            }
        }
    }
}

fn theming() -> Element {
    let example = "// Install a theme at startup \u{2014} components resolve tokens from it:\n\
                   install_idea_theme(light_theme());\n\
                   \n\
                   // Swap at runtime; every styled node re-resolves reactively:\n\
                   set_theme(dark_theme());\n\
                   \n\
                   // Your own sheets can reference the same tokens by name:\n\
                   stylesheet! {\n    \
                       Panel {\n        \
                           background: theme_color!(\"color-surface\"),\n        \
                           border_color: theme_color!(\"color-border\"),\n    \
                       }\n\
                   }";
    ui! {
        Stack(gap = StackGap::Md) {
            Section(
                title = "Theming".to_string(),
                paragraphs = vec![
                    "Every color, radius, spacing step, and type size in the library \
                     resolves through named theme tokens. Install a theme once and the \
                     whole library follows it; swap themes at runtime and every mounted \
                     component re-resolves \u{2014} including on native backends, where \
                     there is no CSS cascade doing the work.".to_string(),
                    "App styles join the same system: reference a token by name in your \
                     own `stylesheet!` and your custom components track theme swaps \
                     exactly like the library's. This site's dark-mode toggle is that \
                     mechanism.".to_string(),
                ],
                code = Some(example.to_string()),
            )
            link(route = &STYLING_ROUTE, params = ()) {
                Typography(content = "The style system in depth \u{2192} Styling & theming".to_string())
            }
        }
    }
}
