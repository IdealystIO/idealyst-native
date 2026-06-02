//! Quick start: create a project, run it, and get into the live-reload
//! loop. Deliberately overlaps a little with the marketing site's
//! getting-started copy — that's fine; this is the tutorial's own front
//! door, and a reader who lands here shouldn't have to bounce back out
//! to start building.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Typography};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::routes::HOME_ROUTE;
use crate::shell;

pub fn page() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = HOME_ROUTE.name(),
            title = "Quick start".to_string(),
            lead = "Create a project, run it in your browser, and watch it live-reload \u{2014} \
                in under a minute.".to_string(),
        ) {
            Typography(
                content = "Idealyst builds native apps for web, iOS, Android, macOS, and the \
                    terminal from one Rust codebase. This page just gets a project running; the \
                    tracks in the sidebar teach how it works once it's on your screen. You'll \
                    need a Rust toolchain installed \u{2014} everything else the CLI handles.".to_string()
            )

            Typography(content = "Create a project".to_string(), kind = typography_kind::H2)
            Typography(
                content = "new scaffolds a fresh project into its own directory \u{2014} the \
                    Cargo workspace, an app entry point, and the platform glue. Pass a Cargo-style \
                    name (lowercase, hyphens or underscores).".to_string()
            )
            CodePanel(src = r##"idealyst new my-app
cd my-app"##.to_string())
            Typography(
                content = "Add --bundle-id com.example.my_app to set the reverse-DNS identifier \
                    for the native targets (underscores, not hyphens); it defaults to \
                    com.example.<name> if you skip it.".to_string()
            )

            Typography(content = "Run it".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Pick a platform. Web is the fastest loop \u{2014} it builds to wasm, \
                    serves on localhost, and reloads on every save.".to_string()
            )
            CodePanel(src = r##"idealyst dev --web        # http://localhost:8080
idealyst dev --ios        # iOS simulator
idealyst dev --android    # Android emulator
idealyst dev --macos      # native AppKit window
idealyst dev --terminal   # TTY app
idealyst dev --all        # every buildable target at once"##.to_string())
            Typography(
                content = "Edit your app() and the change shows up without a manual rebuild. The \
                    default runtime-server mode hot-patches the running tree and preserves state; \
                    add --local for a lighter cold start that full-reloads on save instead.".to_string()
            )

            Callout(label = "First build is slow, the rest are fast".to_string()) {
                Typography(
                    content = "The initial compile warms the whole dependency graph; subsequent \
                        runs are incremental. If a device target won't build, run idealyst doctor \
                        \u{2014} it checks your Rust targets, Xcode, and Android NDK and tells you \
                        what's missing.".to_string(),
                    muted = true,
                )
            }

            Typography(content = "What's next".to_string(), kind = typography_kind::H2)
            Typography(
                content = "With the app running, work through the sidebar top to bottom. \
                    Foundations is the map \u{2014} how signals, the UI, and the theme are one \
                    engine. Then the tracks drill in: Reactivity, Stylesheets, and Media queries, \
                    each taught against runtime-core directly. Every step ends with a prev/next \
                    bar and links out to the deep-dive docs.".to_string()
            )

            DocsLink(
                summary = "Prefer the verbose reference, or want the full CLI surface? Start with \
                    the docs index.".to_string(),
                link_label = "Docs overview".to_string(),
                doc_file = "README.md".to_string(),
            )
        }
    })
}
