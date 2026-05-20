//! File templates shared by `idealyst new` and `idealyst init`.
//!
//! Two flavours:
//!
//! - **Project** — a cross-platform app crate. Single `[lib]` with
//!   `cdylib + rlib`, a `#[component] fn app()` entry point, web
//!   bootstrap, and `[package.metadata.idealyst.app]` so the CLI's
//!   `build` / `run` / `dev` commands pick it up.
//! - **Library** — a third-party External-primitive extension. Pure
//!   `rlib`, defines a `*Props` struct + a PascalCase constructor,
//!   per-backend `register` stubs gated on `target_arch` /
//!   `target_os`. Mirrors the in-tree `crates/sdk/maps*/` pattern.
//!
//! Both flavours emit a `framework-core = { git = "...", rev = "..." }`
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
// Project (app)
// =============================================================================

fn write_project(
    dir: &Path,
    name: &str,
    lib_name: &str,
    source: &FrameworkSource,
    bundle_id: Option<&str>,
) -> Result<()> {
    fs::create_dir_all(dir.join("src"))
        .with_context(|| format!("create {}", dir.join("src").display()))?;

    let bundle_id = bundle_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_bundle_id(name));
    let app_title = title_case(name);

    let fcore_dep = source.dep("crates/framework/core", &[]);
    let ftheme_dep = source.dep("crates/framework/theme", &[]);
    let bweb_dep = source.dep("crates/backend/web", &[]);

    let cargo_toml = format!(
        r##"[package]
name = "{name}"
version = "0.0.1"
edition = "2021"
license = "MIT OR Apache-2.0"

# `cdylib` for the web build (wasm-bindgen's `--target web` consumes
# the produced `.wasm`); `rlib` so the CLI's per-platform wrapper
# crates (materialized into `target/idealyst/<platform>/`) can depend
# on this crate as a library.
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
framework-core  = {fcore_dep}
framework-theme = {ftheme_dep}

# Web build of the backend + the wasm-bindgen glue.
[target.'cfg(target_arch = "wasm32")'.dependencies]
backend-web = {bweb_dep}
wasm-bindgen = "0.2"
console_error_panic_hook = "0.1"
# Smaller WASM allocator — slightly higher per-alloc cost in exchange
# for a few KB shaved off the bundle.
lol_alloc = "0.4"

# wasm-opt's bundled binaryen rejects bulk-memory ops emitted by recent
# rustc; pass the enable flags explicitly. `-Oz` prioritizes size like
# `opt-level = "z"` does for rustc.
[package.metadata.wasm-pack.profile.release]
wasm-opt = ["-Oz", "--strip-debug", "--strip-producers", "--enable-bulk-memory", "--enable-nontrapping-float-to-int"]

# Idealyst project config. The CLI reads this on `idealyst build`,
# `idealyst run`, `idealyst dev`, etc.
[package.metadata.idealyst.app]
name      = "{app_title}"
bundle_id = "{bundle_id}"
version   = "0.0.1"
# Platforms `idealyst build` produces when no target list is given on
# the command line. Override per-invocation: `idealyst build ios`.
targets   = ["web", "ios", "android"]

[package.metadata.idealyst.app.splash]
background  = "#0f1115"
title       = "{app_title}"
title_color = "#ffffff"
duration_ms = 1200
"##,
    );

    fs::write(dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(dir.join("src/lib.rs"), project_lib_rs(lib_name))?;
    fs::write(dir.join("src/web.rs"), project_web_rs())?;
    fs::write(dir.join("index.html"), project_index_html(&app_title, lib_name))?;
    fs::write(dir.join(".gitignore"), GITIGNORE)?;
    Ok(())
}

fn project_lib_rs(lib_name: &str) -> String {
    format!(
        r##"//! `{lib_name}` — cross-platform Idealyst app.
//!
//! `app()` returns the root primitive tree. Per-platform glue
//! (wasm-bindgen start fn for web, generated iOS / Android wrappers
//! produced by `idealyst build`) calls it; the same tree renders
//! everywhere.

use framework_core::{{component, signal, ui, Primitive}};
use framework_theme::{{install_theme, ThemeTokens, TokenEntry}};

#[cfg(target_arch = "wasm32")]
mod web;

/// Starter theme — empty token set. Replace with a real one once you
/// start using token-referencing styles (`Tokenized::token(...)`).
/// Framework panics on render without a prior `install_theme(...)`,
/// even when nothing references a token.
struct StarterTheme;

impl ThemeTokens for StarterTheme {{
    fn tokens(&self) -> Vec<TokenEntry> {{ Vec::new() }}
}}

#[component]
pub fn app() -> Primitive {{
    install_theme(StarterTheme);

    let count = signal!(0i32);

    ui! {{
        View {{
            Text {{ "Hello from {lib_name}" }}
            Button(
                label = "Increment",
                on_click = move || count.update(|n| *n += 1),
            )
            Text {{ format!("Count: {{}}", count.get()) }}
        }}
    }}
}}
"##
    )
}

fn project_web_rs() -> String {
    r##"//! Web entry — wasm-bindgen `start()`. Wires the framework's web
//! backend up to `#app` in `index.html`.
//!
//! Built only on `wasm32` so native targets (iOS / Android wrappers)
//! don't pull in `wasm-bindgen`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

// Smaller WASM allocator — trades a few cycles per allocation for a
// few KB shaved off the bundle.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    /// `render` returns an `Owner` that must outlive the page. Stash
    /// it in a thread-local so it survives `start()` returning.
    static OWNER: RefCell<Option<framework_core::Owner>> =
        const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();

    // Register the web scheduler so framework_core::scheduling
    // (microtasks, after_animation_frame, etc.) has a backend. Without
    // this the framework panics on first dispatch.
    backend_web::install_scheduler();

    // `mount` runs `app()` inside the root reactive scope. Use this
    // (not `render`) when `app()` declares any top-level `effect!` /
    // `signal!` / `Ref::new` — those need a scope to adopt them.
    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    let owner = framework_core::mount(backend, super::app);
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}
"##.to_string()
}

fn project_index_html(title: &str, lib_name: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{title}</title>
<style>
  html, body, #app {{ margin: 0; padding: 0; height: 100%; font-family: system-ui, sans-serif; }}
</style>
</head>
<body>
<div id="app"></div>
<script type="module">
import init from './pkg/{lib_name}.js';
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
framework-core = {fcore_dep}

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

use framework_core::{{external, Bound, ExternalHandle}};

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

// =============================================================================
// Shared bits
// =============================================================================

const GITIGNORE: &str = "/target\n/pkg\nCargo.lock\n";

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
