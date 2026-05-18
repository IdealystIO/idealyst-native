//! Quickstart — install, scaffold, and your first running app.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, stack, BodyTone, HeadingKind, StackGap};

use crate::shell::{
    codeblock, pageheader, section, sectionwithcode, CodeBlockProps, PageHeaderProps,
    SectionProps, SectionWithCodeProps,
};

pub fn page() -> Primitive {
    ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Quickstart".to_string(),
                description = "Get an Idealyst app running on your machine in a few minutes.".to_string(),
            )

            Section(
                title = "Prerequisites".to_string(),
                body = "Rust 1.70+ (via rustup), plus the toolchain for whichever platform you want \
                        to target. Web targets need `wasm-pack` and the `wasm32-unknown-unknown` \
                        target; iOS needs Xcode; Android needs the NDK. Run `idealyst doctor` to \
                        see what's missing.".to_string(),
            )

            SectionWithCode(
                title = "Install the CLI".to_string(),
                body = "The `idealyst` CLI is the orchestrator — it scaffolds projects, runs the dev \
                        server, materializes per-platform wrappers, and drives builds. Install it from \
                        the repo workspace:".to_string(),
                code = "cargo install --path crates/cli".to_string(),
            )

            SectionWithCode(
                title = "Create a project".to_string(),
                body = "Once installed, scaffold an empty project. The CLI sets up the cross-platform \
                        crate plus the `Cargo.toml` keys it needs to know about your app name and \
                        bundle id.".to_string(),
                code = "idealyst new my-app\ncd my-app".to_string(),
            )

            Card {
                Heading(content = "Your first app".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "Every Idealyst app exposes a single `pub fn app() -> Primitive` \
                               annotated with `#[component]`. Inside, install a theme and \
                               return a UI tree. The whole thing is platform-agnostic — the \
                               same function renders on web, iOS, Android, and beyond.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "use framework_core::{component, signal, ui, Primitive};\n\
                            use idea_ui::{install_idea_theme, light_theme, ButtonKind, IntentTag, StackGap};\n\
                            \n\
                            #[component]\n\
                            pub fn app() -> Primitive {\n    \
                                install_idea_theme(light_theme());\n    \
                                let count = signal!(0);\n    \
                                ui! {\n        \
                                    Stack(gap = StackGap::Lg) {\n            \
                                        Heading(content = \"Hello, Idealyst\".to_string())\n            \
                                        Body(content = format!(\"Count: {}\", count.get()))\n            \
                                        Btn(\n                \
                                            label = \"Increment\".to_string(),\n                \
                                            on_click = std::rc::Rc::new(move || count.update(|n| *n += 1)),\n                \
                                            intent = IntentTag::Primary,\n                \
                                            kind = ButtonKind::Solid,\n            \
                                        )\n        \
                                    }\n    \
                                }\n\
                            }".to_string(),
                )
            }

            SectionWithCode(
                title = "Run it".to_string(),
                body = "Spin up the dev server. It builds the WASM target, hosts a local web server, \
                        and watches your source — saves trigger an incremental rebuild and live patch \
                        with no full page reload.".to_string(),
                code = "idealyst dev".to_string(),
            )

            SectionWithCode(
                title = "Build for production".to_string(),
                body = "When you're ready to ship, build a release artifact. Add a `--platform` flag \
                        to target a specific host (e.g. `--platform ios`).".to_string(),
                code = "idealyst build --release".to_string(),
            )
        }
    }
}
