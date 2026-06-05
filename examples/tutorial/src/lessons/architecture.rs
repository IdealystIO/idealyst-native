//! Architecture track — how the framework is layered, in depth. The
//! overview lays out the whole map; the three following steps zoom into
//! the Backend layer (direct vs hosted runtime), the catalog and what it
//! powers (docs + MCP), and the SDKs.
//!
//! The diagrams are assembled from the native chart primitives in
//! `crate::chart`; the prose is the substance.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::chart::{ChartArrow, ChartBox, ChartLabel, ChartRow};
use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::routes::{ARCH_BACKENDS_ROUTE, ARCH_CATALOG_ROUTE, ARCH_OVERVIEW_ROUTE, ARCH_SDKS_ROUTE};
use crate::shell;

// =============================================================================
// Step 1 — the layered model (the whole map).
// =============================================================================

pub fn overview() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ARCH_OVERVIEW_ROUTE.name(),
            title = "The layered model".to_string(),
            lead = "One reactive core, a single Backend interface to every platform, and the \
                tooling built around it.".to_string(),
        ) {
            Typography(
                content = "Read the diagram top-down. You author your app once against the Core. \
                    The Core never touches a platform directly \u{2014} it only ever speaks to a \
                    Backend. Everything below the Core is a different way of satisfying that one \
                    interface, and everything beside it is tooling built on top.".to_string()
            )

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
                        body = "Local & release. Core → Backend → native platform.".to_string(),
                    )
                    ChartBox(
                        title = "Hosted runtime".to_string(),
                        body = "Dev mode. Core on a server, driving any device over the Wire.".to_string(),
                    )
                }
                ChartArrow()
                ChartLabel(label = "Built on the catalog".to_string())
                ChartRow {
                    ChartBox(
                        title = "Auto-generated docs".to_string(),
                        body = "Every component & type, with live recipe previews.".to_string(),
                    )
                    ChartBox(
                        title = "MCP server".to_string(),
                        body = "Real-time, accurate context for LLMs.".to_string(),
                    )
                }
                ChartArrow()
                ChartBox(
                    eyebrow = "Built on the core".to_string(),
                    title = "SDKs".to_string(),
                    body = "camera · microphone · video · screen-recorder … capabilities you can combine.".to_string(),
                )
            }

            Typography(content = "Why it's shaped this way".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The Core is deliberately minimal: the reactive system and the scene \
                    model, and the Backend interface that makes the whole thing cross-platform. \
                    Anything composable from primitives lives outside it \u{2014} that's what keeps \
                    the heart small and the boundaries clean. The next three steps walk each layer.".to_string()
            )

            Callout(label = "The Backend is the only boundary".to_string()) {
                Typography(
                    content = "Cross-platform ubiquity is the framework's reason to exist: one \
                        author tree, every backend, native output that behaves the same. The \
                        Backend trait absorbs the toolkit differences (UIKit vs AppKit vs DOM vs \
                        wgpu) so the observable behavior is identical.".to_string(),
                    muted = true,
                )
            }
        }
    })
}

// =============================================================================
// Step 2 — direct vs hosted runtime (the Wire).
// =============================================================================

pub fn backends() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ARCH_BACKENDS_ROUTE.name(),
            title = "Direct vs hosted runtime".to_string(),
            lead = "The Backend is one interface satisfied two ways — and the second way is where \
                hot reload and cross-device state come from.".to_string(),
        ) {
            Typography(
                content = "A Backend implements the framework's single point of contact with the platform: it \
                    creates native nodes, updates their properties, mutates the tree, and applies \
                    style. Implement it once and the entire app surface runs on that target. There \
                    are two ways to put a Backend behind the Core.".to_string()
            )

            Typography(content = "The direct path".to_string(), kind = typography_kind::H2)
            Typography(
                content = "This is what a local or release build uses. The Core talks to the \
                    Backend, which talks to the native SDKs and platform. Nothing sits in between \
                    — the shortest path, and the one that ships.".to_string()
            )
            Stack(gap = StackGap::Sm) {
                ChartBox(
                    eyebrow = "Hosted on the device".to_string(),
                    title = "Framework Core".to_string(),
                    body = "Your app's reactive state + scene model".to_string(),
                    accent = true,
                )
                ChartArrow()
                ChartBox(
                    title = "Direct Backend".to_string(),
                    body = "UIKit · AppKit · DOM · wgpu · Android".to_string(),
                )
                ChartArrow()
                ChartBox(
                    title = "Native SDKs / Platform".to_string(),
                    body = "The real widgets on the real device".to_string(),
                )
            }

            Typography(content = "The hosted-runtime path".to_string(), kind = typography_kind::H2)
            Typography(
                content = "In dev mode, three layers slot in between the Core and the device's real \
                    Backend: the Runtime Server Backend, the Wire, and the Dev Client. Together \
                    they host the Core on a server and connect it to any platform over websockets \
                    — possible precisely because the Backend is an abstraction the wire can speak \
                    on the Core's behalf.".to_string()
            )
            Stack(gap = StackGap::Sm) {
                ChartBox(
                    eyebrow = "Hosted on a server".to_string(),
                    title = "Framework Core".to_string(),
                    body = "The session lives here".to_string(),
                    accent = true,
                )
                ChartArrow()
                ChartBox(
                    title = "Runtime Server Backend".to_string(),
                    body = "A Backend impl that serializes calls instead of rendering".to_string(),
                )
                ChartArrow()
                ChartBox(
                    title = "The Wire".to_string(),
                    body = "Backend calls, messaged over websockets".to_string(),
                )
                ChartArrow()
                ChartBox(
                    title = "Dev Client (on the device)".to_string(),
                    body = "Receives the calls and drives the device's real Backend → native platform".to_string(),
                )
            }

            Typography(content = "Why bother".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Two payoffs, both falling out of the same split. First, hot reload \
                    without shipping an updated binary: the device never changes, only the \
                    server-side Core does — and hot-linking native code on a device is often \
                    impossible, so this is the practical way to get fast iteration on hardware.".to_string()
            )
            Typography(
                content = "Second, the session state lives on the runtime server. So you can say \
                    \"I want to see what this looks like on Android instead of iPhone\", connect to \
                    the same server from Android, and get the exact same state and render on the \
                    new device. It isn't a perfect system, but it's a powerful one for many \
                    applications.".to_string()
            )

            Callout(label = "Same interface, transparent proxy".to_string()) {
                Typography(
                    content = "The Wire just messages the Backend API over websockets; the client's \
                        real Backend builds its own native chrome from those calls. There's nothing \
                        iOS- or web-specific in the wire path itself — it's solved generically, once, \
                        at the Backend boundary.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The full Backend trait surface and per-method contract.".to_string(),
                link_label = "Backend reference".to_string(),
                doc_file = "backend.md".to_string(),
            )
        }
    })
}

// =============================================================================
// Step 3 — catalog, docs & MCP.
// =============================================================================

pub fn catalog() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ARCH_CATALOG_ROUTE.name(),
            title = "Catalog, docs & MCP".to_string(),
            lead = "A registry of every component and type — with compile-time safeguards — that \
                feeds two consumers: the docs and the MCP server.".to_string(),
        ) {
            Typography(
                content = "The catalog is peripheral but critical. It's a registry of every \
                    component and type in your app and the libraries it pulls in, with optional \
                    compile-time safeguards that enforce best practices (documentation coverage, \
                    prop contracts, and the like). Two paths build on it.".to_string()
            )

            Stack(gap = StackGap::Sm) {
                ChartBox(
                    eyebrow = "The source of truth".to_string(),
                    title = "Catalog".to_string(),
                    body = "Component & type registry + compile-time safeguards".to_string(),
                    accent = true,
                )
                ChartArrow()
                ChartRow {
                    ChartBox(
                        title = "Auto-generated docs".to_string(),
                        body = "Understand how every component and type fits together — with live recipe previews.".to_string(),
                    )
                    ChartBox(
                        title = "MCP server".to_string(),
                        body = "Real-time context + access to component docs for LLMs, including third-party libraries'.".to_string(),
                    )
                }
            }

            Typography(content = "Recipes make it reliable".to_string(), kind = typography_kind::H2)
            Typography(
                content = "A recipe is a compiled usage example. Because it compiles against the \
                    real component, a change to a component's props that the recipe doesn't follow \
                    triggers a compile error. That's the whole trick: the recipe can't silently \
                    fall out of date, so the docs and the LLM context built from it can't either. \
                    If they're stale, the build is red.".to_string()
            )
            CodePanel(src = r##"// A recipe is a real, compiled call site. Rename or retype a prop and
// THIS stops compiling — the docs and MCP context can't drift unnoticed.
recipe!(Button, fn example() {
    ui! {
        Button(
            label = "Save".to_string(),
            tone = Tone::Primary,
            on_press = || save(),
        )
    }
});"##.to_string())

            Callout(label = "Auto-docs vs. MCP".to_string()) {
                Typography(
                    content = "Same catalog, two audiences. The auto-docs are for a human browsing \
                        how components and types work together; the MCP server is for an LLM that \
                        needs accurate, real-time answers about the same surface while it writes \
                        code with you.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "How the MCP catalog is structured and what it exposes.".to_string(),
                link_label = "Framework MCP spec".to_string(),
                doc_file = "framework-mcp-spec.md".to_string(),
            )
        }
    })
}

// =============================================================================
// Step 4 — SDKs.
// =============================================================================

pub fn sdks() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = ARCH_SDKS_ROUTE.name(),
            title = "SDKs".to_string(),
            lead = "Optimized, cross-platform capabilities built on the Core — and composable into \
                bigger ones.".to_string(),
        ) {
            Typography(
                content = "SDKs are peripheral but first-class. They build on the \
                    Core to bring desirable functionality to every supported platform behind one \
                    author-facing API — video, camera, microphone, screen recording, media \
                    writing, and more. Idealyst ships a set of these, highly optimized per \
                    platform, so you don't re-solve them.".to_string()
            )

            Stack(gap = StackGap::Sm) {
                ChartBox(
                    eyebrow = "Built on the Core".to_string(),
                    title = "SDKs".to_string(),
                    body = "One API, native implementation per platform".to_string(),
                    accent = true,
                )
                ChartArrow()
                ChartRow {
                    ChartBox(title = "camera".to_string(), body = "MediaStream".to_string())
                    ChartBox(title = "microphone".to_string(), body = "AudioStream".to_string())
                    ChartBox(title = "screen-recorder".to_string(), body = "MediaStream".to_string())
                    ChartBox(title = "media-writer".to_string(), body = "→ mp4".to_string())
                }
            }

            Typography(content = "They compose".to_string(), kind = typography_kind::H2)
            Typography(
                content = "The abstractions an SDK exposes are themselves building blocks. The \
                    MediaStream and AudioStream types behind the camera and recorder are the same \
                    pieces you'd reach for to build, say, a cross-platform video compositor that \
                    works everywhere the streams do. Extending the framework means stacking a new \
                    SDK on the existing ones \u{2014} not forking the core.".to_string()
            )

            Callout(label = "Minimal & unopinionated".to_string()) {
                Typography(
                    content = "An SDK establishes the raw capability through a small, unopinionated \
                        surface (a callback or trait). Opinions — state management, file handling — \
                        layer in as a later SDK on top, rather than being baked into the capability \
                        itself.".to_string(),
                    muted = true,
                )
            }
        }
    })
}
