//! File templates shared by `idealyst new` and `idealyst init`.
//!
//! Two flavours:
//!
//! - **Project** — a full Idealyst app. The default scaffold is a
//!   verbatim copy of the in-tree `examples/welcome` project: a
//!   three-act cinematic intro driven by springs / tweens / a raf-
//!   pulse, complete with bundled Inter typeface. The source is
//!   embedded into the CLI binary via `include_str!` /
//!   `include_bytes!`, so the scaffold is always identical to the
//!   reference welcome example — no separate template to drift.
//! - **Library** — a third-party primitive extension. Pure `rlib`,
//!   defines a `*Props` payload + a PascalCase constructor + a scene
//!   `Registry` handler per backend, gated on `target_arch` /
//!   `target_os`. Mirrors the in-tree `crates/sdk/client/svg` pattern.
//!
//! Both flavours emit framework deps using the source the CLI resolved
//! (workspace path-deps in-tree, git deps outside). Same dep specs the
//! build crates write into their generated wrapper Cargo.tomls — keeps
//! the convention single.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use build_ios::FrameworkSource;

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Project,
    Library,
}

/// Materialize the chosen scaffold under `dir`.
///
/// `dir` must already exist and be empty (or the caller is happy to
/// have it stomped — `new` enforces emptiness, `init` does not).
pub fn write(
    dir: &Path,
    name: &str,
    kind: Kind,
    source: &FrameworkSource,
    bundle_id: Option<&str>,
) -> Result<()> {
    let lib_name = name.replace('-', "_");
    match kind {
        Kind::Project => write_project(dir, name, &lib_name, source, bundle_id),
        Kind::Library => write_library(dir, name, &lib_name, source),
    }
}

// =============================================================================
// Project (app) — verbatim copy of the welcome example
// =============================================================================
//
// Each source file is pulled from `examples/welcome/` at compile time
// so any change to the reference welcome propagates here on the next
// CLI rebuild. Cargo.toml + index.html are reformatted with the
// caller's name / bundle id / framework source; everything else is
// dropped through unchanged. The welcome source is intentionally
// name-agnostic (no `welcome::*` self-references, no `mod welcome`)
// so the verbatim copy compiles under any crate name.

const WELCOME_LIB_RS: &str = include_str!("../../../../../examples/welcome/src/lib.rs");
const WELCOME_APP_RS: &str = include_str!("../../../../../examples/welcome/src/app.rs");
const WELCOME_COORDINATOR_RS: &str =
    include_str!("../../../../../examples/welcome/src/coordinator.rs");
const WELCOME_CONSTANTS_RS: &str =
    include_str!("../../../../../examples/welcome/src/constants.rs");
const WELCOME_TYPEFACE_RS: &str =
    include_str!("../../../../../examples/welcome/src/typeface.rs");
const WELCOME_COLOR_RS: &str = include_str!("../../../../../examples/welcome/src/color.rs");
const WELCOME_STYLE_HELPERS_RS: &str =
    include_str!("../../../../../examples/welcome/src/style_helpers.rs");
const WELCOME_COMPONENTS_RS: &str =
    include_str!("../../../../../examples/welcome/src/components.rs");
const WELCOME_COMPONENT_PAGE: &str =
    include_str!("../../../../../examples/welcome/src/components/page.rs");
const WELCOME_COMPONENT_VIGNETTE: &str =
    include_str!("../../../../../examples/welcome/src/components/vignette.rs");
const WELCOME_COMPONENT_SUN_GLARE: &str =
    include_str!("../../../../../examples/welcome/src/components/sun_glare.rs");
const WELCOME_COMPONENT_PLANET: &str =
    include_str!("../../../../../examples/welcome/src/components/planet.rs");
const WELCOME_COMPONENT_WELCOME_PHRASE: &str =
    include_str!("../../../../../examples/welcome/src/components/welcome_phrase.rs");
const WELCOME_COMPONENT_SUBTITLE: &str =
    include_str!("../../../../../examples/welcome/src/components/subtitle.rs");
const WELCOME_COMPONENT_CONTENT_LAYER: &str =
    include_str!("../../../../../examples/welcome/src/components/content_layer.rs");

// Inter typeface — full upright family bundled with every new project
// so the headline / subtitle render at real weight rather than
// platform fake-bold. ~3.6 MB total; embedded into the CLI binary so
// the scaffold has no out-of-tree dependencies.
const INTER_FONTS: &[(&str, &[u8])] = &[
    (
        "fonts/Inter-Thin.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-Thin.ttf"),
    ),
    (
        "fonts/Inter-ExtraLight.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-ExtraLight.ttf"),
    ),
    (
        "fonts/Inter-Light.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-Light.ttf"),
    ),
    (
        "fonts/Inter-Regular.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-Regular.ttf"),
    ),
    (
        "fonts/Inter-Medium.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-Medium.ttf"),
    ),
    (
        "fonts/Inter-SemiBold.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-SemiBold.ttf"),
    ),
    (
        "fonts/Inter-Bold.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-Bold.ttf"),
    ),
    (
        "fonts/Inter-ExtraBold.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-ExtraBold.ttf"),
    ),
    (
        "fonts/Inter-Black.ttf",
        include_bytes!("../../../../../examples/welcome/fonts/Inter-Black.ttf"),
    ),
];

fn write_project(
    dir: &Path,
    name: &str,
    lib_name: &str,
    source: &FrameworkSource,
    bundle_id: Option<&str>,
) -> Result<()> {
    fs::create_dir_all(dir.join("src/components"))
        .with_context(|| format!("create {}", dir.join("src/components").display()))?;
    fs::create_dir_all(dir.join("fonts"))
        .with_context(|| format!("create {}", dir.join("fonts").display()))?;

    let bundle_id = bundle_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_bundle_id(name));
    let app_title = title_case(name);

    fs::write(dir.join("Cargo.toml"), project_cargo_toml(name, &app_title, &bundle_id, source))?;
    fs::write(dir.join("index.html"), project_index_html(&app_title, lib_name))?;
    fs::write(dir.join(".gitignore"), GITIGNORE)?;
    fs::write(dir.join(".mcp.json"), MCP_JSON)?;
    fs::write(dir.join("dev.toml"), DEV_TOML)?;

    // Source tree — copied verbatim from `examples/welcome/src/`.
    // The welcome source has no self-name references so it compiles
    // under any crate name.
    fs::write(dir.join("src/lib.rs"), WELCOME_LIB_RS)?;
    fs::write(dir.join("src/app.rs"), WELCOME_APP_RS)?;
    fs::write(dir.join("src/coordinator.rs"), WELCOME_COORDINATOR_RS)?;
    fs::write(dir.join("src/constants.rs"), WELCOME_CONSTANTS_RS)?;
    fs::write(dir.join("src/typeface.rs"), WELCOME_TYPEFACE_RS)?;
    fs::write(dir.join("src/color.rs"), WELCOME_COLOR_RS)?;
    fs::write(dir.join("src/style_helpers.rs"), WELCOME_STYLE_HELPERS_RS)?;
    fs::write(dir.join("src/components.rs"), WELCOME_COMPONENTS_RS)?;
    fs::write(dir.join("src/components/page.rs"), WELCOME_COMPONENT_PAGE)?;
    fs::write(dir.join("src/components/vignette.rs"), WELCOME_COMPONENT_VIGNETTE)?;
    fs::write(dir.join("src/components/sun_glare.rs"), WELCOME_COMPONENT_SUN_GLARE)?;
    fs::write(dir.join("src/components/planet.rs"), WELCOME_COMPONENT_PLANET)?;
    fs::write(
        dir.join("src/components/welcome_phrase.rs"),
        WELCOME_COMPONENT_WELCOME_PHRASE,
    )?;
    fs::write(dir.join("src/components/subtitle.rs"), WELCOME_COMPONENT_SUBTITLE)?;
    fs::write(
        dir.join("src/components/content_layer.rs"),
        WELCOME_COMPONENT_CONTENT_LAYER,
    )?;

    // No project-level catalog binary or `catalog` feature is scaffolded:
    // `idealyst mcp` generates its own ephemeral wrapper crate that turns
    // the `catalog` feature on (and each component-library dep's own
    // `catalog` feature) for the whole graph — so a project carrying its
    // own `catalog` feature + emitter bin is redundant, and worse, can't
    // enable dependency-only catalog features. See
    // `super::catalog_wrapper`.

    for (rel_path, bytes) in INTER_FONTS {
        fs::write(dir.join(rel_path), bytes)
            .with_context(|| format!("write {}", dir.join(rel_path).display()))?;
    }

    Ok(())
}

fn project_cargo_toml(
    name: &str,
    app_title: &str,
    bundle_id: &str,
    source: &FrameworkSource,
) -> String {
    // The author surface: `runtime-core` is what the project aliases
    // to `runtime_core` at its crate root, `runtime-vocabulary` is where
    // the `ui!` emission's absolute `::runtime_vocabulary::glue::…`
    // paths land, and `runtime-scene` carries the `Registry` the
    // registration seam is generic over. Mirrors
    // `examples/welcome/Cargo.toml` — the scaffold's source of truth.
    let runtime_core_dep = source.dep("crates/runtime/core", &[]);
    let vocab_dep = source.dep("crates/runtime/vocabulary", &[]);
    let scene_dep = source.dep("crates/runtime/scene", &[]);
    let dev_server_dep = source.dep("crates/dev/server", &[]);

    format!(
        r##"[package]
name = "{name}"
version = "0.0.1"
edition = "2021"
license = "MIT OR Apache-2.0"

# Pure `rlib`. The per-platform wrappers the CLI generates at build
# time (`target/idealyst/{name}/{{web,ios,android}}/wrapper/`) carry
# the platform-specific crate-type (cdylib for web/Android, staticlib
# for iOS) and the platform entry-point boilerplate
# (`#[wasm_bindgen(start)]`, `ios_main`, `Java_..._attach`). This
# crate stays platform-agnostic — no `web.rs` / `ios.rs` /
# `android.rs`, no `#[cfg(target_os = "...")]` blocks, no
# `wasm-bindgen` / `backend-*` direct deps. Same source ships to
# every backend.
[lib]
crate-type = ["rlib"]

# No `catalog` feature or `[[bin]] catalog` is needed for the MCP server:
# `idealyst mcp` generates an ephemeral wrapper crate on demand that turns
# on the `catalog` feature (and each component-library dependency's own
# `catalog` feature) across the whole graph and force-links those deps, so
# every `#[component]` plus dependency-provided catalog entries (icon sets,
# …) surface. See the `catalog_wrapper` module in the CLI.

[features]
# `sidecar`: enabled ONLY by the CLI-generated runtime-server sidecar
# wrapper (`idealyst dev`). Pulls `dev-server` so the recorder-side
# registration seams compile.
sidecar = ["dep:dev-server"]

[dependencies]
# The author surface. `runtime-core` is the crate this project aliases
# to `runtime_core` at its root (see src/lib.rs). `runtime-vocabulary` is
# needed because the `ui!` emission spells absolute
# `::runtime_vocabulary::glue::…` paths, and `runtime-scene` because
# `register_scene_extensions` is generic over the scene `Registry` host.
runtime-core = {runtime_core_dep}
runtime-vocabulary = {vocab_dep}
runtime-scene = {scene_dep}
# Recorder backend for the runtime-server sidecar wrapper. Optional —
# pulled in only by the `sidecar` feature (host-side sidecar build);
# never compiled for device/web targets.
dev-server = {dev_server_dep_opt}

# No backend deps: `register_scene_extensions` is generic over the scene
# `Host` (this scaffold registers no third-party SDKs), so the app crate
# is fully platform-agnostic — the CLI-generated wrappers bring their own
# concrete backend. A project that adds an SDK with a backend-CONCRETE
# scene handler specializes `register_scene_extensions` to that backend's
# registry type and adds the matching backend dep then.

# Idealyst project config. The CLI reads this on `idealyst build`,
# `idealyst run`, `idealyst dev`, etc.
[package.metadata.idealyst.app]
name      = "{app_title}"
bundle_id = "{bundle_id}"
version   = "0.0.1"
# Platforms `idealyst dev` and `idealyst build` fan out across when
# no `--web` / `--ios` / `--android` flag is passed on the command
# line. We default to web-only because it's the broadest target with
# zero per-platform toolchain setup (no Xcode, no NDK) — a fresh
# clone of this project will `idealyst dev` straight into a hot-
# reloading browser preview. Add the mobile targets when you're
# ready: `targets = ["web", "ios", "android"]`. The `run ios` /
# `run android` subcommands work regardless of what's listed here
# (they take an explicit platform argument), so you can also just
# leave this alone and invoke them directly.
targets   = ["web"]
"##,
        runtime_core_dep = runtime_core_dep,
        vocab_dep = vocab_dep,
        scene_dep = scene_dep,
        dev_server_dep_opt = optional_dep(&dev_server_dep),
    )
}

/// Splice `optional = true` into a `source.dep(..)`-produced dep spec
/// (`{ path = "…" }` / `{ git = "…", rev = "…" }`).
fn optional_dep(dep: &str) -> String {
    let inner = dep.trim().trim_start_matches('{').trim_end_matches('}').trim();
    format!("{{ {inner}, optional = true }}")
}

fn project_index_html(title: &str, lib_name: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1, user-scalable=no" />
    <base href="/" />
    <title>{title}</title>
    <style>
      html, body, #app {{ height: 100%; margin: 0; }}
      body {{ background: #f7f8fb; }}
      /* Mount is a flex column so the app's root view fills the viewport
         height; without it the root sizes to content and short screens stop
         short of full height on tall windows. */
      #app {{ display: flex; flex-direction: column; }}
      #app > * {{ flex: 1 1 auto; min-height: 0; }}
    </style>
  </head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/{lib_name}.js";
      init();
    </script>
  </body>
</html>
"##
    )
}

// =============================================================================
// Library (External primitive extension)
// =============================================================================

fn write_library(
    dir: &Path,
    name: &str,
    lib_name: &str,
    source: &FrameworkSource,
) -> Result<()> {
    fs::create_dir_all(dir.join("src"))
        .with_context(|| format!("create {}", dir.join("src").display()))?;

    let pascal = pascal_case(name);
    let props_type = format!("{pascal}Props");
    // The author surface (`runtime_core::…` via the crate-root alias) +
    // the two crates a third-party primitive needs directly: the scene
    // `Registry`/`item` contract and the vocabulary capability traits its
    // handlers are generic over.
    let runtime_core_dep = source.dep("crates/runtime/core", &[]);
    let vocab_dep = source.dep("crates/runtime/vocabulary", &[]);
    let scene_dep = source.dep("crates/runtime/scene", &[]);
    let world_dep = source.dep("crates/runtime/world", &[]);
    let bweb_dep = source.dep("crates/backend/web", &[]);
    let bios_dep = source.dep("crates/backend/ios/mobile", &[]);
    let bandroid_dep = source.dep("crates/backend/android/mobile", &[]);

    let cargo_toml = format!(
        r##"[package]
name = "{name}"
version = "0.0.1"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Third-party Idealyst primitive extension."

[lib]
crate-type = ["rlib"]

[dependencies]
# `runtime-core` is aliased to `runtime_core` at the crate root (see
# src/lib.rs) so this crate spells the author surface the same way app
# code and the framework docs do.
runtime-core = {runtime_core_dep}
# The scene registry: `Registry` (where handlers install), `item` (the
# payload-carrying element), `MountCx` (what a handler receives).
runtime-scene = {scene_dep}
# The capability traits a handler is generic over (`ExternalOps`,
# `StyleServices`, …) plus `style_attach` for the author-style channel.
runtime-vocabulary = {vocab_dep}
# `runtime_world::effect` — for reactive props, a handler subscribes with
# a world effect created during mount; it is collected into the enclosing
# subtree and dies at unmount.
runtime-world = {world_dep}

# Web leaf. `web-sys` is pulled in for the DOM-construction path in
# `src/web.rs`. Add bindings as you need them
# (`features = ["HtmlElement", ...]`).
[target.'cfg(target_arch = "wasm32")'.dependencies]
backend-web = {bweb_dep}
web-sys = {{ version = "0.3", features = ["Document", "Element", "Window"] }}

# iOS leaf — a `Registry<IosBackend>`-concrete handler (src/ios.rs).
[target.'cfg(target_os = "ios")'.dependencies]
backend-ios-mobile = {bios_dep}

# Android leaf — a `Registry<AndroidBackend>`-concrete handler
# (src/android.rs).
[target.'cfg(target_os = "android")'.dependencies]
backend-android-mobile = {bandroid_dep}
"##,
    );

    fs::write(dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(dir.join("src/lib.rs"), library_lib_rs(lib_name, &pascal, &props_type))?;
    fs::write(dir.join("src/web.rs"), library_web_rs(&pascal, &props_type))?;
    fs::write(dir.join("src/ios.rs"), library_ios_rs(&pascal, &props_type))?;
    fs::write(dir.join("src/android.rs"), library_android_rs(&pascal, &props_type))?;
    fs::write(dir.join(".gitignore"), GITIGNORE)?;
    Ok(())
}

fn library_lib_rs(lib_name: &str, pascal: &str, props_type: &str) -> String {
    format!(
        r##"//! `{lib_name}` — third-party Idealyst primitive extension.
//!
//! Edit [`{props_type}`] to match the data your primitive needs, then
//! implement the per-platform handlers in `web.rs` / `ios.rs` /
//! `android.rs`. App code uses your primitive via the PascalCase
//! constructor [`{pascal}`].
//!
//! ## How a third-party primitive works
//!
//! The framework has no separate "external" concept: the scene
//! [`Registry`] dispatches first-party primitives and third-party ones
//! through the same contract. You define a payload type, hand it to
//! [`item`] inside an [`Element`], and register a handler that turns the
//! payload into a native node. A payload with no registered handler
//! **panics at realize**, so a missed `register` fails loudly rather
//! than rendering a silent placeholder box.
//!
//! ## Usage from an app crate
//!
//! ```ignore
//! // Bootstrap: compose your registration into the boot entry's
//! // `register` seam (one line per third-party SDK).
//! backend_web::newcore::start_in("#app", {lib_name}::register, app);
//!
//! // In a `ui!` block. Third-party primitives interpolate as
//! // expressions:
//! ui! {{
//!     view {{
//!         {{ {pascal}({props_type} {{ example: "hi".into() }}) }}
//!     }}
//! }}
//! ```

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::AccessibilityProps;
use runtime_scene::{{item, Element, MountCx, Registry}};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
}};

/// Props for the [`{pascal}`] primitive. Handlers receive the payload as
/// `&Rc<{pascal}Prim>` and read these fields back out.
#[derive(Clone, Debug, Default)]
pub struct {props_type} {{
    /// Replace with your own fields.
    pub example: String,
}}

/// The scene payload: the author's props plus the two single-take slots
/// every primitive carries — the author style, and the mount-time work
/// the builder recorded.
pub(crate) struct {pascal}Prim {{
    pub(crate) props: Rc<{props_type}>,
    style: RefCell<Option<StyleProp>>,
}}

/// Author-side builder returned by [`{pascal}`].
pub struct {pascal}Bound {{
    props: Rc<{props_type}>,
    style: Option<StyleProp>,
}}

/// Construct an instance of the primitive. Interpolate inside a `ui!`
/// block: `{{ {pascal}({props_type} {{ example: "...".into() }}) }}`.
///
/// PascalCase intentionally — inside `ui!`, lowercase tags are the
/// framework's own leaf primitives and PascalCase tags route to
/// component / third-party dispatch.
#[allow(non_snake_case)]
pub fn {pascal}(props: {props_type}) -> {pascal}Bound {{
    {pascal}Bound {{ props: Rc::new(props), style: None }}
}}

impl {pascal}Bound {{
    /// Attach the author style — lands on the node your handler returns.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {{
        self.style = Some(style.into_style_prop());
        self
    }}
}}

impl IntoElement for {pascal}Bound {{
    fn into_element(self) -> Element {{
        item(
            {pascal}Prim {{ props: self.props, style: RefCell::new(self.style) }},
            Vec::new(),
        )
    }}
}}

impl From<{pascal}Bound> for Element {{
    fn from(b: {pascal}Bound) -> Element {{
        b.into_element()
    }}
}}

/// Shared mount tail: attach the author style and register teardown.
/// Call it from every platform handler after the node exists.
pub(crate) fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &{pascal}Prim)
where
    H: ExternalOps + StyleServices,
{{
    if let Some(style) = prim.style.borrow_mut().take() {{
        attach_style(backend, node, style);
    }}
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {{
        backend.borrow_mut().release_external(&node);
    }});
}}

// =============================================================================
// Platform-routed `register`.
//
// Exactly one of the cfg-gated re-exports is active per build. Each
// per-platform leaf takes that backend's concrete `Registry` and
// installs a handler that builds a real native node. The fallback
// `register<H>` is generic over the capability traits, so a host without
// a leaf still mounts (as the backend's "not supported" placeholder)
// instead of panicking.
// =============================================================================

#[cfg(target_arch = "wasm32")] mod web;
#[cfg(target_arch = "wasm32")] pub use web::register;

#[cfg(target_os = "ios")] mod ios;
#[cfg(target_os = "ios")] pub use ios::register;

#[cfg(target_os = "android")] mod android;
#[cfg(target_os = "android")] pub use android::register;

/// Fallback registration for targets without a native leaf: installs the
/// framework's "primitive not supported on this backend" degradation
/// path, so the app still mounts and the app's bootstrap call site is
/// uniform across every target.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{{
    registry.register::<{pascal}Prim, _>(mount_placeholder::<H>);
}}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<{pascal}Prim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<{props_type}>(),
        std::any::type_name::<{props_type}>(),
        &payload,
        &AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}}
"##
    )
}

fn library_web_rs(pascal: &str, props_type: &str) -> String {
    format!(
        r##"//! Web leaf — a `Registry<WebBackend>`-concrete handler that builds a
//! DOM node from [`{props_type}`].

use std::rc::Rc;

use backend_web::WebBackend;
use runtime_scene::{{Element, MountCx, Registry}};

use crate::{{finish_mount, {pascal}Prim, {props_type}}};

/// Install the handler. Compose it into the boot entry's register seam:
///
/// ```ignore
/// backend_web::newcore::start_in("#app", crate_name::register, app);
/// ```
pub fn register(registry: &mut Registry<WebBackend>) {{
    registry.register::<{pascal}Prim, _>(mount);
}}

fn mount(
    cx: &mut MountCx<'_, WebBackend>,
    prim: &Rc<{pascal}Prim>,
    _children: Vec<Element>,
) -> web_sys::Node {{
    let backend = cx.backend().clone();
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    // Replace with your real DOM construction.
    let el = document.create_element("div").expect("create_element(div)");
    let _ = el.set_text_content(Some(&prim.props.example));
    let _ = el.set_attribute("data-primitive", "{props_type}");

    let node: web_sys::Node = el.into();
    finish_mount(&backend, &node, prim);
    node
}}
"##
    )
}

fn library_ios_rs(pascal: &str, props_type: &str) -> String {
    format!(
        r##"//! iOS leaf — a `Registry<IosBackend>`-concrete handler.
//!
//! Build your `UIView` (or any UIKit type) from [`{props_type}`] and
//! return it as the backend's node type. Until you do, this mounts the
//! framework's "not supported" degradation path so the app still runs on
//! iOS while you develop the web leaf.

use std::any::Any;
use std::rc::Rc;

use backend_ios::IosBackend;
use runtime_core::AccessibilityProps;
use runtime_scene::{{Element, MountCx, Registry}};
use runtime_vocabulary::caps::ExternalOps;

use crate::{{finish_mount, {pascal}Prim, {props_type}}};

/// Install the handler.
pub fn register(registry: &mut Registry<IosBackend>) {{
    registry.register::<{pascal}Prim, _>(mount);
}}

fn mount(
    cx: &mut MountCx<'_, IosBackend>,
    prim: &Rc<{pascal}Prim>,
    _children: Vec<Element>,
) -> <IosBackend as runtime_scene::Host>::Node {{
    let backend = cx.backend().clone();
    // TODO: construct your UIView here and return it instead.
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<{props_type}>(),
        std::any::type_name::<{props_type}>(),
        &payload,
        &AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}}
"##
    )
}

fn library_android_rs(pascal: &str, props_type: &str) -> String {
    format!(
        r##"//! Android leaf — a `Registry<AndroidBackend>`-concrete handler.
//! Same shape as the iOS leaf: build an `android.view.View` from
//! [`{props_type}`] and return it as the backend's node type.

use std::any::Any;
use std::rc::Rc;

use backend_android::AndroidBackend;
use runtime_core::AccessibilityProps;
use runtime_scene::{{Element, MountCx, Registry}};
use runtime_vocabulary::caps::ExternalOps;

use crate::{{finish_mount, {pascal}Prim, {props_type}}};

/// Install the handler.
pub fn register(registry: &mut Registry<AndroidBackend>) {{
    registry.register::<{pascal}Prim, _>(mount);
}}

fn mount(
    cx: &mut MountCx<'_, AndroidBackend>,
    prim: &Rc<{pascal}Prim>,
    _children: Vec<Element>,
) -> <AndroidBackend as runtime_scene::Host>::Node {{
    let backend = cx.backend().clone();
    // TODO: construct your View here and return it instead.
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<{props_type}>(),
        std::any::type_name::<{props_type}>(),
        &payload,
        &AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}}
"##
    )
}

// =============================================================================
// Shared bits
// =============================================================================

// `.env` / `.env.local` are auto-loaded by the CLI (signing team + App Store
// Connect API credentials) — they hold secrets and must never be committed.
const GITIGNORE: &str =
    "/target\n/pkg\nCargo.lock\n/.idealyst/\n/dist/\n.env\n.env.local\n";

/// Project-local MCP server config — Claude Code auto-loads this when
/// the user opens the scaffolded project. Points at the system-
/// installed `idealyst` binary (assumed on PATH after
/// `cargo install idealyst-cli` or similar). The server defaults to
/// Robot tools on, lazy-connecting to the local app's bridge on
/// 127.0.0.1:9718 — works the moment the user runs `idealyst dev`.
///
/// The bare `["mcp"]` args are enough: with no `--project-root` /
/// `--from-bin`, `idealyst mcp` extracts the catalog from its current
/// directory, which Claude Code sets to this project root when it
/// launches the server. The server generates + `cargo run`-builds a
/// managed wrapper crate (force-linking component-library deps and
/// enabling their catalog features) and lists every `#[component]` plus
/// dependency-provided entries (icon sets, …). When an app is also
/// running (`idealyst dev`), the live catalog flows over its Robot
/// bridge and takes precedence.
const MCP_JSON: &str = r#"{
  "mcpServers": {
    "idealyst": {
      "command": "idealyst",
      "args": ["mcp"]
    }
  }
}
"#;

/// Per-project dev-mode config. Optional — every field has a
/// default; absence is fine. The CLI's `idealyst dev` reads this
/// file at startup and falls back to defaults for unset fields.
const DEV_TOML: &str = r#"# Per-project dev-mode configuration. Read by `idealyst dev`.
# All fields are optional — delete this file or any individual field
# to fall back to defaults.

# Pin the Robot bridge to a specific port. Default: ephemeral (the
# bridge picks an unused port and writes it to
# `.idealyst/bridge.port` for the MCP server to discover).
#
# Pin a port here only if an external tool needs a stable target —
# normal Claude workflows use the discovery file. CLI flag
# `--bridge-port <PORT>` overrides this setting per-run.
# bridge_port = 9718
"#;

fn default_bundle_id(name: &str) -> String {
    // Underscores not hyphens — Android JNI symbol mangler doesn't
    // handle hyphens, and this is the bundle id used to derive JNI
    // symbol prefixes.
    format!("com.example.{}", name.replace('-', "_"))
}

fn title_case(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn pascal_case(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<String>()
}
