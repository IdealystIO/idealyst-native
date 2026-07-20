//! CLI tooling — the `idealyst` binary end to end. Command one-liners
//! mirror the CLI's own clap descriptions (crates/tools/cli/src/main.rs);
//! keep them in sync when commands change. Deep dives live on the pages
//! this one links to (server functions for the full-stack dev loop,
//! Robot & MCP for the AI surface).

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{PageHeader, PageSection, Section};
use crate::routes::{AGENTIC_ROUTE, SERVER_FUNCTIONS_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let run_ref: Ref<ViewHandle> = Ref::new();
    let quality_ref: Ref<ViewHandle> = Ref::new();
    let ship_ref: Ref<ViewHandle> = Ref::new();
    let env_ref: Ref<ViewHandle> = Ref::new();
    let ai_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: run_ref, label: "Build & run" },
        TocEntry { handle: quality_ref, label: "Keeping it correct" },
        TocEntry { handle: ship_ref, label: "Shipping" },
        TocEntry { handle: env_ref, label: "Project environment" },
        TocEntry { handle: ai_ref, label: "The AI surface" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "CLI tooling",
                blurb = "One binary drives the whole lifecycle: scaffold, develop with \
                 hot reload, type-check every platform, test against the live app, and \
                 ship signed builds. Each target's build pipeline and platform wrapper \
                 is the CLI's job \u{2014} your project stays plain Rust.",
            )
            PageSection(handle = run_ref) { build_run() }
            PageSection(handle = quality_ref) { quality() }
            PageSection(handle = ship_ref) { shipping() }
            PageSection(handle = env_ref) { environment() }
            PageSection(handle = ai_ref) { ai_surface() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Sections — no-param file-local helpers (allowed per CLAUDE.md §9.5).
// =============================================================================

fn build_run() -> Element {
    let snippet = "idealyst new my-app             # scaffold a cross-platform project\n\
                   idealyst dev --web              # build + serve + hot reload\n\
                   idealyst dev --macos            # native window, tied to the CLI's lifetime\n\
                   idealyst run ios                # build + boot the iOS simulator\n\
                   idealyst run android            # build + install on emulator or device\n\
                   idealyst build --web --release  # content-hashed production bundle";
    ui! {
        Section(
            title = "Build & run".to_string(),
            paragraphs = vec![
                "`new` / `init` scaffold a project; `dev` runs it with hot reload; `run` \
                 builds and launches on a simulator or device; `build` produces release \
                 artifacts per platform. The web release build content-addresses the \
                 bundle (one hash in every filename), so it deploys behind immutable \
                 caching headers as-is.".to_string(),
                "For a project with a `server_bin`, `dev --web --local` runs the full \
                 stack \u{2014} wasm bundle, API server, file watcher \u{2014} as one \
                 command on one port.".to_string(),
            ],
            code = Some(snippet.to_string()),
        )
    }
}

fn quality() -> Element {
    let snippet = "idealyst check                  # type-check across configured platforms\n\
                   idealyst lint                   # idiom lints; --format json plugs into rust-analyzer\n\
                   idealyst test --macos           # run the Robot suite against the live app";
    ui! {
        Section(
            title = "Keeping it correct".to_string(),
            paragraphs = vec![
                "`check` type-checks every configured platform's cfg surface in one run, \
                 so an iOS-only compile error surfaces while you're developing on the \
                 web. `lint` catches idiom drift \u{2014} raw reactive primitives, \
                 hand-built elements, non-PascalCase components \u{2014} and its JSON \
                 output wires into rust-analyzer as a `check.overrideCommand` for inline \
                 squiggles.".to_string(),
                "`test` runs a Robot suite: it launches the app (or attaches to a \
                 running one), drives it over the robot relay \u{2014} find an element, \
                 act on it, assert a signal or label \u{2014} and reports pass/fail. The \
                 same suite runs against any platform the app targets.".to_string(),
            ],
            code = Some(snippet.to_string()),
        )
    }
}

fn shipping() -> Element {
    let snippet = "idealyst publish ios            # distribution-signed .ipa \u{2192} App Store Connect\n\
                   idealyst publish macos          # Mac App Store .pkg or notarized .dmg\n\
                   idealyst worker                 # run the jobs-queue worker as its own process\n\
                   idealyst export                 # components \u{2192} Web Component suite";
    ui! {
        Stack(gap = StackGap::Md) {
            Section(
                title = "Shipping".to_string(),
                paragraphs = vec![
                    "`publish` builds a distribution-signed app and, optionally, ships \
                     it \u{2014} `.ipa` to App Store Connect for iOS; a Mac App Store \
                     `.pkg` or a Developer ID notarized `.dmg` for macOS. `worker` runs \
                     the project's jobs-queue worker as a dedicated process, the \
                     standalone counterpart to the one `dev` auto-spawns. `export` \
                     packages `#[component(external)]` components as Web Components (see \
                     Extensibility).".to_string(),
                ],
                code = Some(snippet.to_string()),
            )
            link(route = &SERVER_FUNCTIONS_ROUTE, params = ()) {
                Typography(content = "The full-stack dev loop \u{2192} Server functions".to_string())
            }
        }
    }
}

fn environment() -> Element {
    let snippet = "idealyst doctor                 # diagnose the toolchain (Rust, web, iOS, Android)\n\
                   idealyst configure devcontainer # Dev Container + managed Postgres/Redis/MinIO\n\
                   idealyst configure vscode       # editor extensions + rust-analyzer lint wiring\n\
                   idealyst sync                   # regenerate icons, splash, derived assets";
    ui! {
        Section(
            title = "Project environment".to_string(),
            paragraphs = vec![
                "`doctor` diagnoses the local toolchain and tells you what's missing per \
                 target. `configure devcontainer` initializes or updates a Dev Container \
                 with idealyst-managed sidecar services (Postgres/MySQL, Redis, MinIO) \
                 \u{2014} it owns its own compose file and leaves yours alone. \
                 `configure vscode` sets up the workspace: recommended extensions plus \
                 the lint wiring. `sync` regenerates derived assets like icons and \
                 splash screens from their sources.".to_string(),
            ],
            code = Some(snippet.to_string()),
        )
    }
}

fn ai_surface() -> Element {
    let snippet = "idealyst mcp                    # framework MCP server on stdio\n\
                   idealyst catalog-json           # machine-readable component catalog\n\
                   idealyst docs                   # catalog-driven docs site for the project";
    ui! {
        Stack(gap = StackGap::Md) {
            Section(
                title = "The AI surface".to_string(),
                paragraphs = vec![
                    "The same catalog that documents your project feeds AI tooling. \
                     `catalog-json` prints every component, props schema, primitive, and \
                     guide as JSON \u{2014} the machine-facing entry editor tooling reads. \
                     `mcp` serves that catalog (plus the Robot introspection tools) over \
                     the Model Context Protocol on stdio; point Claude Desktop or Claude \
                     Code at it as a `command` server. `docs` builds and serves a \
                     browsable documentation site from the same data.".to_string(),
                ],
                code = Some(snippet.to_string()),
            )
            link(route = &AGENTIC_ROUTE, params = ()) {
                Typography(content = "What the MCP surface exposes \u{2192} Robot & MCP".to_string())
            }
        }
    }
}
