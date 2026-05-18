//! CLI — the `idealyst` command-line tool.

use framework_core::{ui, Primitive};
use idea_ui::{body, card, heading, BodyTone, HeadingKind};

use crate::shell::{
    codeblock, pagebody, pageheader, section, sectionwithcode, CodeBlockProps, PageBodyProps,
    PageHeaderProps, SectionProps, SectionWithCodeProps,
};

pub fn page() -> Primitive {
    ui! {
        PageBody {
            PageHeader(
                title = "CLI".to_string(),
                description = "The `idealyst` binary orchestrates scaffolding, dev, build, and deploy.".to_string(),
            )

            Section(
                title = "Install".to_string(),
                body = "Install the CLI from the workspace crate. The installed binary is \
                        self-contained — it doesn't need the repo to keep working.".to_string(),
            )

            SectionWithCode(
                title = "Scaffold".to_string(),
                body = "Create a new project, or initialize Idealyst inside an existing crate.".to_string(),
                code = "idealyst new my-app          # fresh directory\n\
                        idealyst init                # existing crate".to_string(),
            )

            SectionWithCode(
                title = "Dev server".to_string(),
                body = "`dev` watches your source, rebuilds incrementally, and patches the \
                        running app over a WebSocket — no full reload, state preserved across \
                        edits.".to_string(),
                code = "idealyst dev".to_string(),
            )

            SectionWithCode(
                title = "Build".to_string(),
                body = "Build production artifacts for one or more platforms. Pass `--release` \
                        for a release profile; pass `--platform <name>` to target a specific \
                        host.".to_string(),
                code = "idealyst build --release\n\
                        idealyst build --release --platform ios".to_string(),
            )

            SectionWithCode(
                title = "Run".to_string(),
                body = "Build and launch on a connected device or simulator.".to_string(),
                code = "idealyst run --platform ios\n\
                        idealyst run --platform android".to_string(),
            )

            SectionWithCode(
                title = "Doctor".to_string(),
                body = "Diagnose your local toolchain — what's installed, what's missing, \
                        what's the wrong version. Run this first whenever a build fails for \
                        tooling reasons.".to_string(),
                code = "idealyst doctor".to_string(),
            )

            Card {
                Heading(content = "Other commands".to_string(), kind = HeadingKind::H2)
                Body(
                    content = "`check` runs `cargo check` across configured platforms. `clean` \
                               removes the build cache. `sync` regenerates derived assets \
                               (icons, splash screens) from the project config. `scaffold` \
                               materializes a per-platform wrapper project from the ephemeral \
                               build cache into the repo so you can hand-edit it. `brs` \
                               collects and emits BrightScript methods for the Roku backend.".to_string(),
                    tone = BodyTone::Muted,
                )
                CodeBlock(
                    code = "idealyst check\n\
                            idealyst clean\n\
                            idealyst sync\n\
                            idealyst scaffold ios\n\
                            idealyst brs".to_string(),
                )
            }
        }
    }
}
