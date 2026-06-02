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
//! - **Library** — a third-party External-primitive extension. Pure
//!   `rlib`, defines a `*Props` struct + a PascalCase constructor,
//!   per-backend `register` stubs gated on `target_arch` /
//!   `target_os`. Mirrors the in-tree `crates/sdk/maps*/` pattern.
//!
//! Both flavours emit a `runtime-core = { git = "...", rev = "..." }`
//! dep using the source the CLI resolved (workspace path-deps in-tree,
//! git deps outside). Same dep specs the build crates write into
//! their generated wrapper Cargo.tomls — keeps the convention single.

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

    // MCP catalog emitter binary — built with the `mcp` feature
    // (`cargo run --bin catalog --features mcp -- --emit-catalog`).
    fs::create_dir_all(dir.join("src/bin"))
        .with_context(|| format!("create {}", dir.join("src/bin").display()))?;
    fs::write(dir.join("src/bin/catalog.rs"), catalog_bin_rs(lib_name))?;

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
    let fcore_dep = source.dep("crates/framework/core", &[]);

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

# Catalog emitter — `cargo run --bin catalog --features mcp -- --emit-catalog`
# prints this project's MCP catalog (every component, method,
# animation, type, plus all the framework's built-in primitives /
# utilities / guides) as JSON to stdout. The MCP server spawns this
# binary on file changes to refresh its in-memory catalog without
# restarting. `required-features = ["mcp"]` keeps the bin out of
# normal builds — only present when the consumer opts in.
[[bin]]
name = "catalog"
path = "src/bin/catalog.rs"
required-features = ["mcp"]

[features]
default = []
# Turns on `mcp-catalog` registration in the `#[component]` /
# `#[derive(IdealystSchema)]` / `#[idealyst_tool]` macros. The
# catalog binary calls `runtime_core::__mcp::catalog_json()` (a
# hidden re-export of `mcp-catalog`) so no direct dep is needed.
mcp = ["runtime-core/mcp"]

[dependencies]
runtime-core = {fcore_dep}

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
    )
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
    let fcore_dep = source.dep("crates/framework/core", &[]);
    let bweb_dep = source.dep("crates/backend/web", &[]);
    let bios_dep = source.dep("crates/backend/ios/mobile", &[]);
    let bandroid_dep = source.dep("crates/backend/android/mobile", &[]);

    let cargo_toml = format!(
        r##"[package]
name = "{name}"
version = "0.0.1"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Third-party Idealyst External-primitive extension."

[lib]
crate-type = ["rlib"]

[dependencies]
runtime-core = {fcore_dep}

# Web leaf. `web-sys` is pulled in for the DOM-construction path in
# `src/web.rs`. Add bindings as you need them
# (`features = ["HtmlElement", ...]`).
[target.'cfg(target_arch = "wasm32")'.dependencies]
backend-web = {bweb_dep}
web-sys = {{ version = "0.3", features = ["Document", "Element", "Window"] }}

# iOS leaf. `backend-ios-mobile`'s `register_external` is the entry
# point your `src/ios.rs` will hook into once it lands.
[target.'cfg(target_os = "ios")'.dependencies]
backend-ios-mobile = {bios_dep}

# Android leaf. Same shape as iOS — wire up in `src/android.rs`.
[target.'cfg(target_os = "android")'.dependencies]
backend-android-mobile = {bandroid_dep}
"##,
    );

    fs::write(dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(dir.join("src/lib.rs"), library_lib_rs(lib_name, &pascal, &props_type))?;
    fs::write(dir.join("src/web.rs"), library_web_rs(&props_type))?;
    fs::write(dir.join("src/ios.rs"), library_ios_rs(&props_type))?;
    fs::write(dir.join("src/android.rs"), library_android_rs(&props_type))?;
    fs::write(dir.join(".gitignore"), GITIGNORE)?;
    Ok(())
}

fn library_lib_rs(lib_name: &str, pascal: &str, props_type: &str) -> String {
    format!(
        r##"//! `{lib_name}` — third-party Idealyst External-primitive extension.
//!
//! Edit [`{props_type}`] to match the data your primitive needs, then
//! implement the per-platform handlers in `web.rs` / `ios.rs` /
//! `android.rs`. App code uses your primitive via the PascalCase
//! constructor [`{pascal}`].
//!
//! ## Usage from an app crate
//!
//! ```ignore
//! // Bootstrap: register once per backend (one line per third-party SDK).
//! let mut backend = WebBackend::new("#app");
//! {lib_name}::register(&mut backend);
//!
//! // In a `ui!` block. Third-party primitives interpolate as expressions:
//! ui! {{
//!     View {{
//!         {{ {pascal}({props_type} {{ example: "hi".into() }}) }}
//!     }}
//! }}
//! ```
//!
//! On platforms with no matching leaf, `register` is a no-op and the
//! framework renders its "External not supported" placeholder when
//! the primitive mounts.

use runtime_core::{{external, Bound, ExternalHandle}};

/// Props for the [`{pascal}`] external primitive. Backends downcast
/// the type-erased payload back to this concrete type via the
/// `TypeId` captured at construction.
#[derive(Clone, Debug)]
pub struct {props_type} {{
    /// Replace with your own fields.
    pub example: String,
}}

/// Construct an instance of the primitive. Interpolate inside a
/// `ui!` block: `{{ {pascal}({props_type} {{ example: "...".into() }}) }}`.
///
/// PascalCase intentionally — matches the visual cadence of
/// first-party primitives like `View` / `Overlay` / `Button` inside
/// `ui!`.
#[allow(non_snake_case)]
pub fn {pascal}(props: {props_type}) -> Bound<ExternalHandle<{props_type}>> {{
    external(props)
}}

// =============================================================================
// Platform-routed `register` re-export.
//
// Exactly one of the cfg-gated re-exports is active per build. Each
// per-platform leaf takes the platform-specific backend type by
// `&mut` and calls `backend.register_external::<{props_type}>(...)`.
// The fallback `register<B>` keeps user code uniform across targets
// the SDK hasn't grown a leaf for.
// =============================================================================

#[cfg(target_arch = "wasm32")] mod web;
#[cfg(target_arch = "wasm32")] pub use web::register;

#[cfg(target_os = "ios")] mod ios;
#[cfg(target_os = "ios")] pub use ios::register;

#[cfg(target_os = "android")] mod android;
#[cfg(target_os = "android")] pub use android::register;

/// No-op fallback for targets without a registered leaf. The
/// framework renders its `External not supported` placeholder at
/// mount time.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register<B>(_backend: &mut B) {{}}
"##
    )
}

fn library_web_rs(props_type: &str) -> String {
    format!(
        r##"//! Web leaf — registers a handler against `WebBackend` that
//! produces a `web_sys::Element` from {props_type}.

use backend_web::WebBackend;

use crate::{props_type};

/// Install the handler. Call once at app bootstrap:
///
/// ```ignore
/// let mut backend = WebBackend::new("#app");
/// crate_name::register(&mut backend);
/// ```
pub fn register(backend: &mut WebBackend) {{
    backend.register_external::<{props_type}, _>(|props, _backend| {{
        build_element(props)
    }});
}}

fn build_element(props: &std::rc::Rc<{props_type}>) -> web_sys::Element {{
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    // Replace with your real DOM construction.
    let el = document
        .create_element("div")
        .expect("create_element(div)");
    let _ = el.set_text_content(Some(&props.example));
    let _ = el.set_attribute("data-external-kind", "{props_type}");
    el
}}
"##
    )
}

fn library_ios_rs(props_type: &str) -> String {
    format!(
        r##"//! iOS leaf — placeholder until `backend-ios-mobile` exposes
//! `register_external`. The `register` fn is intentionally a
//! generic no-op so the app's bootstrap call site compiles on
//! every target uniformly.
//!
//! Once `IosBackend::register_external::<{props_type}, _>(...)` is
//! available, replace the body with a real registration that
//! returns a `UIView` (or your preferred UIKit type) from
//! [`{props_type}`].

#[allow(unused_imports)]
use crate::{props_type};

/// Install the handler. No-op on iOS until the backend grows
/// `register_external` support.
pub fn register<B>(_backend: &mut B) {{
    // TODO: when backend-ios-mobile gains `register_external`,
    // downcast `_backend` to `IosBackend` and register a closure
    // that produces a UIView from `{props_type}`.
}}
"##
    )
}

fn library_android_rs(props_type: &str) -> String {
    format!(
        r##"//! Android leaf — placeholder until `backend-android-mobile`
//! exposes `register_external`. Same pattern as the iOS leaf.

#[allow(unused_imports)]
use crate::{props_type};

/// Install the handler. No-op on Android until the backend grows
/// `register_external` support.
pub fn register<B>(_backend: &mut B) {{
    // TODO: when backend-android-mobile gains `register_external`,
    // downcast `_backend` to `AndroidBackend` and register a closure
    // that produces a `View` from `{props_type}`.
}}
"##
    )
}

fn catalog_bin_rs(lib_name: &str) -> String {
    format!(
        r##"//! MCP catalog emitter.
//!
//! Run with `cargo run --bin catalog --features mcp -- --emit-catalog`
//! to print this project's MCP catalog as JSON on stdout. The
//! Idealyst MCP server (`idealyst mcp`) spawns this binary on file
//! changes to refresh its in-memory catalog without process restart.
//!
//! Linking the user's library here is what populates the
//! distributed `inventory` slices for components/methods/animations
//! /types. Without the `use {lib_name} as _;` below, those entries
//! would not appear in the catalog (the macro emission only lands
//! in the binary if the linker pulls the user crate in).

use {lib_name} as _;

fn main() {{
    // `dump_catalog_json` does the serialize + println — keeps this
    // binary free of a direct `serde_json` dep. The default
    // (no-arg) path is also the catalog dump, so the MCP server can
    // spawn it without arguments.
    runtime_core::__mcp::dump_catalog_json();
}}
"##,
    )
}

// =============================================================================
// Shared bits
// =============================================================================

const GITIGNORE: &str = "/target\n/pkg\nCargo.lock\n/.idealyst/\n";

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
/// launches the server. The server finds (or `cargo run`-builds) this
/// project's `catalog` bin and lists every `#[component]`. When an app
/// is also running (`idealyst dev`), the live catalog flows over its
/// Robot bridge and takes precedence.
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
