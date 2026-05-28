//! Further reading — curated outbound links.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let source_ref: Ref<ViewHandle> = Ref::new();
    let docs_ref: Ref<ViewHandle> = Ref::new();
    let crates_ref: Ref<ViewHandle> = Ref::new();
    let ack_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: source_ref, label: "Source" },
        TocEntry { handle: docs_ref, label: "Design documents" },
        TocEntry { handle: crates_ref, label: "Per-crate READMEs" },
        TocEntry { handle: ack_ref, label: "Acknowledgements" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Further reading",
                "Where to go for the long answers \u{2014} design docs, per-crate \
                 READMEs, the GitHub repository, and the projects that inspired \
                 this one."
            ) }
            { page_section(source_ref, vec![source_section()]) }
            { page_section(docs_ref, vec![docs_section()]) }
            { page_section(crates_ref, vec![crate_readmes()]) }
            { page_section(ack_ref, vec![acknowledgements()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn source_section() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Source".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "github.com/IdealystIO/idealyst-native".to_string())
        },
        ui! {
            Typography(content = "The whole framework, the CLI, the example apps, and \
                this website. Open issues here; the issue tracker is the canonical place \
                for bug reports and feature discussion.".to_string(),
                muted = true)
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn docs_section() -> Element {
    let entries: [(&str, &str); 6] = [
        ("docs/ui-layer.md", "The authoring surface: ui! / jsx! / #[component] / stylesheet! / Ref<H>. Read this for the day-to-day API."),
        ("docs/reactivity.md", "Signals, effects, derived signals, batched writes. The reactive layer end-to-end."),
        ("docs/styling.md", "Stylesheets, variants, tokens, transitions. How the framework's styling system actually works."),
        ("docs/animation.md", "AnimatedValue<T>, spring + decay drivers, the per-frame write path. Declarative vs imperative motion."),
        ("docs/backend.md", "The Backend trait contract \u{2014} render walker rules, per-primitive lifecycle, what a backend must guarantee."),
        ("docs/fonts.md", "Typeface registration, fallback chains, per-platform font loading."),
    ];
    let mut rows: Vec<Element> = Vec::with_capacity(entries.len() * 2);
    for (path, desc) in entries {
        rows.push(ui! { Typography(content = path.to_string(), kind = idea_ui::typography_kind::H3) });
        rows.push(ui! { Typography(content = desc.to_string(), muted = true) });
    }
    let mut children: Vec<Element> = vec![
        ui! { Typography(content = "Design documents".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Long-form design rationale and reference material. \
                These live in the `docs/` directory of the repo.".to_string())
        },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn crate_readmes() -> Element {
    let entries: [(&str, &str); 6] = [
        ("crates/runtime/core/README.md", "Backend trait, primitive vocabulary, render walker, reactivity internals."),
        ("crates/runtime/macros/README.md", "#[component], ui!, jsx!, stylesheet! \u{2014} the author-facing macros and what they expand to."),
        ("crates/backend/web/README.md", "Scheduler / time-source bootstrap requirements, animated-value capabilities."),
        ("crates/backend/ios/mobile/README.md", "UIKit quirks the iOS backend works around (scroll bounds, intrinsic sizing, corner-radius clamping)."),
        ("crates/backend/android/mobile/README.md", "Kotlin runtime requirements, JNI integration, Android Views translation."),
        ("crates/sdk/README.md", "How third-party SDKs (Maps, WebView, navigators) plug in via Element::External."),
    ];
    let mut rows: Vec<Element> = Vec::with_capacity(entries.len() * 2);
    for (path, desc) in entries {
        rows.push(ui! { Typography(content = path.to_string(), kind = idea_ui::typography_kind::H3) });
        rows.push(ui! { Typography(content = desc.to_string(), muted = true) });
    }
    let mut children: Vec<Element> = vec![
        ui! { Typography(content = "Per-crate READMEs".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "When a crate has non-obvious wiring or behavioural \
                quirks, it has its own README. The most useful entry points:".to_string())
        },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn acknowledgements() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Acknowledgements".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Dioxus is another Rust cross-platform UI initiative. \
                Idealyst's iOS and Android backends use Taffy, one of their tools, as \
                the flex-layout engine. Idealyst's rendering approach is different from \
                Dioxus's, but the work and the community over there is worth your \
                attention either way.".to_string())
        },
        ui! {
            Typography(content = "github.com/DioxusLabs/dioxus".to_string())
        },
        ui! {
            Typography(content = "Idealyst's earlier React Native incarnation \u{2014} the \
                project's original form before this Rust rewrite \u{2014} lives at \
                github.com/IdealystIO/idealyst-framework.".to_string(),
                muted = true)
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
