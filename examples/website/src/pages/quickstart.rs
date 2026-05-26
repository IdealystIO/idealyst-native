//! Quickstart — scaffold a project and run it.

use runtime_core::{ui, Primitive, Ref, ViewHandle};
use idea_ui::{stack, typography, StackGap, TypographyKind, TypographyTone};

use crate::pages::common::{code_panel, page_header, page_section};
use crate::routes::CONCEPTS_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    let scaffold_ref: Ref<ViewHandle> = Ref::new();
    let layout_ref: Ref<ViewHandle> = Ref::new();
    let run_web_ref: Ref<ViewHandle> = Ref::new();
    let run_native_ref: Ref<ViewHandle> = Ref::new();
    let edit_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: scaffold_ref, label: "Scaffold a project" },
        TocEntry { handle: layout_ref, label: "Project layout" },
        TocEntry { handle: run_web_ref, label: "Run on web" },
        TocEntry { handle: run_native_ref, label: "Run on iOS / Android" },
        TocEntry { handle: edit_ref, label: "Make a change" },
        TocEntry { handle: next_ref, label: "Next: understand the model" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Quickstart",
                "Scaffold a new project, edit one file, and watch it run on web, \
                 iOS, and Android with hot-reload."
            ) }
            { page_section(scaffold_ref, vec![scaffold()]) }
            { page_section(layout_ref, vec![layout_section()]) }
            { page_section(run_web_ref, vec![run_web()]) }
            { page_section(run_native_ref, vec![run_native()]) }
            { page_section(edit_ref, vec![edit_and_reload()]) }
            { page_section(next_ref, vec![next()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn scaffold() -> Primitive {
    let snippet = "idealyst new my-app\ncd my-app";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Scaffold a project".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "`idealyst new` creates a fresh Rust crate seeded \
                with the welcome example \u{2014} a complete reactive app the CLI knows \
                how to build for every supported target. You get a single, \
                platform-agnostic Rust crate; there are no iOS or Android project \
                files in your directory.".to_string())
        },
        code_panel(snippet),
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn layout_section() -> Primitive {
    let snippet = "my-app/\n  Cargo.toml          # crate-type: cdylib + rlib\n\
                   \x20 index.html          # web entry, loads /pkg/my_app.js\n\
                   \x20 fonts/              # bundled typeface assets\n\
                   \x20 src/\n    \
                       lib.rs            # app() + register_extensions()\n    \
                       app.rs            # the root component\n    \
                       components/       # one file per ui! element";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Project layout".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Your crate is platform-agnostic Rust. There's no \
                web.rs / ios.rs / android.rs split, and there are no Xcode or Gradle \
                wrapper projects committed alongside your code. When you run \
                `idealyst dev` or `idealyst run <platform>`, the CLI generates the \
                per-target wrapper under `target/idealyst/<platform>/`, builds it, and \
                launches it \u{2014} the wrapper is ephemeral and you don't edit it.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(
                content = "Exporting the per-target wrapper as an editable Xcode / \
                    Gradle project (for App Store releases, custom native code, etc.) \
                    is a planned follow-up. Today the CLI is the build pipeline; \
                    tomorrow you'll be able to eject.".to_string(),
                tone = TypographyTone::Muted,
            )
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn run_web() -> Primitive {
    let snippet = "idealyst dev          # hot-reload at http://localhost:8080";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Run on web".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "`idealyst dev` is the hot-reload dev server. It \
                builds the wasm bundle, starts a static file server, and opens your \
                browser \u{2014} all in one step. Edit a source file and the running \
                app reflects the change without losing state.".to_string())
        },
        code_panel(snippet),
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn run_native() -> Primitive {
    let snippet = "idealyst run ios       # boot in iOS simulator\n\
                   idealyst run android   # install on emulator or USB device";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Run on iOS / Android".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "iOS and Android use the same source tree. The CLI \
                produces the platform binary, generates the Xcode / Gradle wrapper as \
                needed, and launches the app on a simulator (or a connected device).".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "Same hot-reload behavior \u{2014} edits to `src/` show \
                up live on the device while the app keeps running.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn edit_and_reload() -> Primitive {
    let snippet = "use runtime_core::{bind, component, signal, text_fmt, ui, Primitive};\n\
                   \n\
                   #[component]\n\
                   pub fn app() -> Primitive {\n    \
                       let count = signal!(0);\n    \
                       ui! {\n        \
                           Text { text_fmt!(\"Count: {}\", bind!(count)) }\n        \
                           Button(\n            \
                               label = \"Increment\",\n            \
                               on_click = move || count.update(|n| *n += 1),\n        \
                           )\n    \
                       }\n\
                   }";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Make a change".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Open `src/app.rs` and replace it with the canonical \
                counter. Save and the running app updates in place \u{2014} on web, in \
                the iOS simulator, and on the Android device, all at the same time.".to_string())
        },
        code_panel(snippet),
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn next() -> Primitive {
    let title = ui! {
        Typography(content = "Next: understand the model".to_string(), kind = TypographyKind::H2)
    };
    let para = ui! {
        Typography(content = "If you want to know why the app crate compiles for every \
            platform unchanged, what `Primitive` actually is, and how the reactive layer \
            works, the Core concepts page is the next step.".to_string())
    };
    let cta = ui! {
        Link(route = &CONCEPTS_ROUTE, params = ()) {
            Typography(content = "Read Core concepts \u{2192}".to_string())
        }
    };
    let children: Vec<Primitive> = vec![title, para, cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
