//! Extensibility — the two directions the framework opens up:
//! `Element::External` + per-backend registries for adding new native
//! primitives from outside core, and `idealyst export` for embedding
//! idealyst components in React/Vue/vanilla hosts as Web Components.
//! The showcase example is real: this site's own code blocks are an
//! external primitive (`crates/sdk/client/codeblock`).

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection, Section};
use crate::routes::ARCHITECTURE_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let hatch_ref: Ref<ViewHandle> = Ref::new();
    let example_ref: Ref<ViewHandle> = Ref::new();
    let small_ref: Ref<ViewHandle> = Ref::new();
    let export_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: hatch_ref, label: "The External element" },
        TocEntry { handle: example_ref, label: "A real extension: code blocks" },
        TocEntry { handle: small_ref, label: "Core stays small" },
        TocEntry { handle: export_ref, label: "Export to Web Components" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Extensibility",
                blurb = "The framework opens up in both directions. `Element::External` \
                 lets a third-party crate add a new native primitive \u{2014} maps, video, \
                 anything \u{2014} with per-backend renderers, fully typed end to end. And \
                 `idealyst export` packages your components as Web Components that run \
                 inside React, Vue, Svelte, or plain HTML.",
            )
            PageSection(handle = hatch_ref) { external_element() }
            PageSection(handle = example_ref) { codeblock_example() }
            PageSection(handle = small_ref) { core_small() }
            PageSection(handle = export_ref) { export() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Sections — no-param file-local helpers (allowed per CLAUDE.md §9.5).
// =============================================================================

fn external_element() -> Element {
    let example = "// The extension crate defines ONE typed props struct \u{2014} the key\n\
                   // the registry dispatches on \u{2014} plus a constructor for authors:\n\
                   #[derive(Clone, IdealystSchema)]\n\
                   pub struct CodeBlockProps {\n    \
                       pub spans: Vec<(String, Color)>,\n\
                   }\n\
                   \n\
                   pub fn code_block(spans: Vec<(String, Color)>) -> Bound<CodeBlockHandle> { ... }\n\
                   \n\
                   // And one handler per backend it supports \u{2014} fully typed:\n\
                   pub fn register(backend: &mut MacosBackend) {\n    \
                       backend.register_external::<CodeBlockProps, _>(macos::build);\n\
                   }\n\
                   \n\
                   fn build(props: &Rc<CodeBlockProps>, backend: &mut MacosBackend) -> MacosNode {\n    \
                       // one NSScrollView + NSTextField, one color attribute per span\n\
                   }";
    ui! {
        Section(
            title = "The External element".to_string(),
            paragraphs = vec![
                "`Element::External` is the framework's single extension point for \
                 primitives core doesn't ship. An extension crate defines a typed props \
                 struct and a constructor; each backend holds a registry keyed by the \
                 props type's `TypeId` and dispatches to whichever handler the app \
                 registered. The registry is collision-free by construction \u{2014} two \
                 crates can both ship a \"map view\" because their props types are \
                 distinct types.".to_string(),
                "Type erasure is paid at exactly one line, inside `register_external`. \
                 The author-facing constructor, the props struct, and the per-backend \
                 handler all stay fully typed \u{2014} the handler receives \
                 `&Rc<CodeBlockProps>`, never a `dyn Any`.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn codeblock_example() -> Element {
    let example = "// App bootstrap \u{2014} once per target:\n\
                   pub fn register_extensions(backend: &mut MacosBackend) {\n    \
                       codeblock::register(backend);\n\
                   }\n\
                   \n\
                   // Anywhere in the tree, styled like any other primitive:\n\
                   code_block(tokenize(source)).with_style(code_style())";
    ui! {
        Section(
            title = "A real extension: code blocks".to_string(),
            paragraphs = vec![
                "Every syntax-highlighted panel on this site is an external primitive \
                 (`crates/sdk/client/codeblock`). Each `code_block(...)` renders as a \
                 single native node per backend: a `<pre>` with one `<span>` per color \
                 run on the web, a `UIScrollView` + `UILabel` with `NSAttributedString` \
                 ranges on iOS, an `NSScrollView` + `NSTextField` on macOS, a \
                 `HorizontalScrollView` + `TextView` with `SpannableString` spans on \
                 Android.".to_string(),
                "It started life inside core and moved out. Measurement kept the design \
                 honest: the single-node renderer generates orders of magnitude fewer \
                 backend ops per re-render than composing a `view` of per-token `text` \
                 nodes, but the primitive is peripheral \u{2014} so it ships as an \
                 extension, through the same registry any third party would use.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn core_small() -> Element {
    ui! {
        Section(
            title = "Core stays small".to_string(),
            paragraphs = vec![
                "The framework's primitive vocabulary is deliberately fixed and low-level. \
                 Anything peripheral or platform-specific \u{2014} maps, video players, \
                 web views, rich embeds \u{2014} arrives as an extension crate: a typed \
                 facade plus per-backend leaf crates, registered once at app bootstrap. \
                 Routing, theming, styling, refs, and reactivity all work on external \
                 nodes exactly as they do on built-in ones.".to_string(),
                "A backend without a registered handler renders an explicit \
                 \"not registered\" placeholder, so partial platform support degrades \
                 visibly during development instead of failing at compile time in \
                 unrelated crates.".to_string(),
            ],
        )
    }
}

fn export() -> Element {
    let author = "#[derive(Default, IdealystSchema)]\n\
                  pub struct GreeterProps {\n    \
                      /// Who to greet.\n    \
                      pub name: Reactive<String>,\n    \
                      /// Fired when the Greet button is pressed.\n    \
                      pub on_greet: Option<Rc<dyn Fn()>>,\n\
                  }\n\
                  \n\
                  #[component(external)]\n\
                  pub fn Greeter(props: &GreeterProps) -> Element { ... }";
    let usage = "idealyst export                    # \u{2192} dist/external/\n\
                 \n\
                 <!-- any framework that consumes custom elements -->\n\
                 <idl-greeter name=\"World\"></idl-greeter>\n\
                 \n\
                 // React \u{2014} a generated, typed wrapper:\n\
                 import { Greeter } from \"./dist/external/react/Greeter\";\n\
                 <Greeter name=\"World\" onGreet={() => console.log(\"greeted\")} />";
    ui! {
        Stack(gap = StackGap::Md) {
            Section(
                title = "Export to Web Components".to_string(),
                paragraphs = vec![
                    "Extensibility also runs outward. Tag a component \
                     `#[component(external)]`, derive `IdealystSchema` on its props, and \
                     `idealyst export` generates a wasm-backed custom element plus \
                     TypeScript declarations and typed wrappers for React, Vue, Svelte, \
                     and Angular. The component itself is ordinary framework code \u{2014} \
                     the attribute is the only change.".to_string(),
                    "Prop writes are reactive (setting `el.name` re-renders in place), \
                     and callbacks surface as DOM events. That gives an incremental \
                     adoption path: build one component in idealyst, drop it into an \
                     existing React or Vue app, and grow from there.".to_string(),
                ],
                code = Some(author.to_string()),
            )
            CodePanel(src = usage.to_string())
            link(route = &ARCHITECTURE_ROUTE, params = ()) {
                Typography(content = "Where the seams live \u{2192} Architecture".to_string())
            }
        }
    }
}
