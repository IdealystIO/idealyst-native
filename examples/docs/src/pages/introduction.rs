//! Introduction page — the architectural overview.
//!
//! Sits ahead of "Getting Started" as the optional why-and-how
//! treatment. Readers who only want to ship an app can skip
//! directly to Quickstart; this page is for the reader who wants
//! to understand the shape of the framework before opening it.
//!
//! Terminology note: the **Painter** layer (per-primitive look)
//! lives under `crates/gpu-backend/painter/` and is exposed through
//! the `Painter` trait in `gpu-backend/engine`.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "introduction",
    title = "Introduction",
    category = Introduction,
    description = "How Idealyst is built and why — the architecture, from the seam outward.",
    related = ["overview", "quickstart", "backends", "writing-a-backend", "third-party-primitives", "dev-tools"],
    concepts = [Runtime, Walker, Host, Painter, Engine],

    section(heading = "Two halves") {
        p("Idealyst is two concepts with a contract between them."),
        p("The ", code("Runtime"), " is the upper half — primitives, the \
           reactive graph, the render walker, the macros you author against. \
           It is platform-agnostic. It knows nothing about UIKit, the DOM, \
           Android views, or wgpu pipelines."),
        p("The ", code("Backend"), " is the lower half — a concrete \
           implementation that turns runtime intent into something you can \
           see on a particular substrate. It is platform-specific by \
           definition."),
        p("The two halves meet at the ", code("Backend"), " trait — the \
           Backend Interface. It is the entire surface across which the \
           Runtime drives a Backend, and the only API a Backend has to \
           implement. Everything Idealyst can render to is something that \
           satisfies this contract."),
        p("That single seam is the design's whole bet: keep the contract \
           narrow, keep the layers below it interchangeable, and the same \
           app code reaches every substrate someone writes a Backend for."),
    },

    section(heading = "The Runtime") {
        p("What the Runtime actually consists of:"),
        list(
            [code("Primitives"), " — the fixed vocabulary of nodes the \
              Backend Interface understands: View, Text, Button, Pressable, \
              TextInput, ScrollView, Icon, and a handful more. See ",
              link("Primitives", to = "primitives"), "."],
            [code("Reactivity"), " — signals, effects, scopes. The \
              fine-grained dependency graph that drives every update. See ",
              link("Reactivity", to = "reactivity"), "."],
            [code("The Walker"), " — the traversal that turns a tree of \
              primitives into Backend Interface calls. Visits each node, \
              calls the matching ", code("create_*"), " method, applies \
              styles, recurses into children, and wraps every reactive \
              expression in an Effect so future signal writes update the \
              right node."],
            [code("Macros"), " — ", code("ui!"), ", ", code("jsx!"), ", ",
              code("#[component]"), ", ", code("stylesheet!"), ". Compile-time \
              sugar that lowers into plain Runtime calls. No runtime cost."],
        ),
        p("All of this lives in ", code("runtime-core"), " (plus its \
           reactive-arena and macro sibling crates). It compiles once. The \
           same compiled Runtime hands its output to whichever Backend the \
           build target picked."),
    },

    section(heading = "The Backend Interface") {
        p("The Backend Interface is a Rust trait — ", code("Backend"),
          " in ", code("runtime_core::backend"), ". It defines the complete \
           set of operations sufficient to materialize every primitive: \
           creating each kind of node, inserting one inside another, removing \
           one, applying a style, updating a text node's contents, attaching \
           an event handler."),
        p("It has no behavior of its own. It is purely a specification of \
           what a Backend must be able to do."),
        p("Above the seam: platform-agnostic. Below the seam: \
           platform-specific. Everything Idealyst swaps to retarget a new \
           substrate is satisfied here. See ",
          link("Writing a Backend", to = "writing-a-backend"),
          " for the method-by-method walkthrough."),
    },

    section(heading = "Native backends") {
        p("The simplest kind of Backend wraps a platform's existing UI \
           toolkit. Each one lives in its own crate and translates Backend \
           Interface calls into operations on the underlying SDK:"),
        list(
            ["iOS — UIKit views, driven via ", code("objc2"), "."],
            ["Android — native ", code("View"), " hierarchy, driven via JNI."],
            ["macOS — AppKit, also via ", code("objc2"), "."],
            ["Web — DOM nodes, compiled to WebAssembly."],
        ),
        p("These Backends inherit a lot for free. The platform already has \
           an event loop, accessibility tree, input method, keyboard \
           handling, scroll physics, hit-testing — the entire substrate. \
           The Backend's job is to translate; it doesn't reimplement the \
           toolkit."),
        p("Other native Backends shipping today: Roku (SceneGraph) and a \
           terminal Backend. The contract is the same; the substrate \
           differs."),
    },

    section(heading = "GPUBackend: Host, Painter, Engine") {
        p("Not every Backend has a native toolkit underneath it. The \
           wgpu-driven GPU Backend draws everything itself — it's a Backend \
           that owns its own pixels. With no platform UI substrate to \
           inherit from, the GPU Backend has to provide the substrate \
           internally."),
        p("Internally, the GPU Backend is composed of three plug-and-play \
           layers:"),
        list(
            [code("Host"), " — the platform integration layer. Owns the \
              window, the drawing surface, and the event source. Translates \
              the platform's vocabulary (touches, keystrokes, gestures, \
              resizes, accessibility events) into the internal event \
              vocabulary the rest of the GPU Backend understands. Different \
              Hosts target different platforms — ", code("host-winit"),
              " for desktop windowing, ", code("host-appkit"), " for macOS, ",
              code("host-web"), " for browser, ", code("host-terminal"),
              " for TTY, with ", code("host-ios-mobile"), " and ",
              code("host-android-mobile"), " planned so GPU-rendered apps \
              can target iOS and Android directly."],
            [code("Painter"), " — the primitive renderer. When the Backend \
              Interface receives ", code("create_button"), ", the Painter \
              knows what a button looks like and emits the geometry. \
              Painters are swappable: the same Engine + Host pair can drive \
              an iOS-styled Painter or an Android-styled Painter, and the \
              app code does not change."],
            [code("Engine"), " — the rendering engine itself. Owns the wgpu \
              surface and pipeline, frame management, text shaping, input \
              dispatch. Knows nothing about primitives — the Painter does \
              that. The Engine just draws what the Painter hands it."],
        ),
        p("Each layer's job is small enough to swap. Swap the Painter — \
           change the look. Swap the Host — change the platform. Swap the \
           Engine — change the rendering substrate (wgpu today; Skia or \
           vello could fit in the same slot)."),
        p("The three layers communicate through a small, explicit contract \
           crate — ", code("render-api"), " — so a new Host can pair with \
           any Engine, and a new Engine can serve any Host, without either \
           side knowing the other's internals."),
    },

    section(heading = "Reactivity threads through everything") {
        p("Reactivity is not a separate system bolted on. It is woven \
           through the runtime."),
        p("The Walker wraps every reactive expression it meets — a ",
          code("Text"), " whose contents read a signal, a style that reads \
           a token, a ", code("for"), " over a signal-backed list — in an \
           Effect. When a signal changes, the framework runs only the \
           Effects that subscribed to it. Each Effect makes the surgical \
           Backend call needed to apply the new value. No diff, no virtual \
           DOM, no reconciliation pass."),
        p("This means cost scales with what changed, not with the size of \
           the tree. A signal write touches the specific node that read it \
           and stops. The Backend gets one update call per actually-changed \
           leaf. See ", link("Reactivity", to = "reactivity"),
          " for the model in full."),
    },

    section(heading = "Runtime locality") {
        p("The Runtime can live where the Backend lives — on the device, \
           in the browser tab — or it can live somewhere else entirely. \
           The seam is fine-grained enough that the Backend Interface calls \
           can be serialized as messages instead of executed locally."),
        p("This is what powers the dev server: in ", code("idealyst dev"),
          " mode, the Runtime runs on the developer's machine and the \
           Backend on the device, with Backend Interface calls streamed \
           over a wire protocol. Hot reload is the same path; the \
           re-evaluated tree produces a diff that is shipped as a sequence \
           of wire commands and replayed against the live Backend."),
        p("The architectural shape is the same as Phoenix LiveView or \
           Blazor Server, but expressed against the Backend Interface \
           rather than HTML/DOM. The wire protocol is ",
          code("wire"), "; the device-side replayer is ",
          code("dev-client"), ". See ",
          link("Dev Tools", to = "dev-tools"),
          " for the full picture."),
    },

    section(heading = "Extension primitives") {
        p("The Runtime ships a fixed list of primitives, but ",
          code("Element::External"), " is an escape hatch: a tagged \
           variant that lets a third-party crate define its own primitive \
           plus per-Backend implementations, then register them through the \
           Backend's external registry."),
        p("WebView and Maps ship today as reference implementations in ",
          code("crates/sdk/"), ". Each defines one primitive plus a per-Backend \
           impl (iOS / Android / web). Nothing in ", code("runtime-core"),
          " has to know about them — the registry slot is enough."),
        p("This is how the framework grows without bloating the core: a \
           team can ", code("cargo new"), " an Idealyst extension crate and \
           ship coverage across every platform their Backend impls reach. \
           See ",
          link("Third-party Primitives", to = "third-party-primitives"),
          " for the pattern in full."),
    },

    section(heading = "Project-aware MCP") {
        p("The macros that emit your component code also emit a structured \
           catalog of what they defined — component names, prop types, \
           method signatures, animation declarations. The catalog is \
           scoped to the project being compiled."),
        p("A sidecar MCP server consumes that catalog and exposes it to \
           agents working in the codebase. They get accurate, current \
           answers about the components and types in this specific project \
           — not training-data approximations, not parsed-from-source \
           guesses. The same source produces both the running UI and the \
           catalog; they cannot drift."),
    },

    section(heading = "Why this shape holds together") {
        p("Each box has a job small enough to describe in one sentence:"),
        list(
            ["The Runtime turns app code into a primitive tree, wires \
              reactivity into it, and drives the Backend Interface."],
            ["The Backend Interface defines what a Backend must do."],
            ["A Backend does that, against a specific substrate."],
            ["For substrates without a native toolkit, Host + Painter + \
              Engine compose into a Backend whose internals are themselves \
              swappable."],
        ),
        p("Each seam is an interface small enough to replace. Swap the \
           Painter and the look changes. Swap the Host and the platform \
           changes. Swap the entire Backend and the substrate changes. \
           Everything above the Backend Interface stays the same."),
        p("That's the whole bet. Keep the contract narrow, keep the layers \
           below it interchangeable, and one app reaches every substrate \
           someone writes a Backend for."),
    },

    section(heading = "Where to go next") {
        p("If you want to start writing an app: ",
          link("Getting Started", to = "quickstart"), "."),
        p("If you want the friendly tour of how the pieces feel in code: ",
          link("Overview", to = "overview"), "."),
        p("If you want to write your own Backend: ",
          link("Writing a Backend", to = "writing-a-backend"), "."),
        p("If you want to add a primitive of your own: ",
          link("Third-party Primitives", to = "third-party-primitives"), "."),
    },
}
