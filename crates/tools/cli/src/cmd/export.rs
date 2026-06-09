//! `idealyst export` — build a project's `#[component(external)]`s into a
//! framework-agnostic Web Component suite (wasm-backed custom elements +
//! `.d.ts` + React/Vue wrappers), emitted to `dist/external/`.
//!
//! Pipeline:
//! 1. **Discover** — generate the ephemeral external-manifest wrapper
//!    (shares [`catalog_wrapper`]'s force-link machinery) and run it to get
//!    the list of external components + their prop schemas as JSON.
//! 2. **Generate the bridge crate** — an ephemeral `cdylib` that path-deps
//!    the project and emits one `#[wasm_bindgen]` class per component
//!    ([`export_codegen::gen_bridge_lib`]).
//! 3. **Build wasm** — `cargo build --target wasm32-unknown-unknown` then
//!    `wasm-bindgen --target web`.
//! 4. **Generate the JS/TS surface** — custom-element shells, `.d.ts`,
//!    React + Vue wrappers, a barrel, and a vanilla demo page.
//! 5. **Emit** everything to `<project>/dist/external/`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::export_codegen as cg;
use crate::framework_source;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project whose `#[component(external)]`s to export. Defaults to the
    /// current directory.
    #[arg(default_value = ".")]
    pub project: PathBuf,

    /// Output directory. Defaults to `<project>/dist/external`.
    #[arg(long)]
    pub out_dir: Option<PathBuf>,

    /// Front-end wrappers to generate (comma-separated): `vanilla`,
    /// `react`, `vue`, `svelte`, `angular`. Defaults to all. The bare
    /// custom element works in every framework regardless — these are
    /// typed convenience wrappers on top.
    #[arg(long, value_delimiter = ',')]
    pub frameworks: Vec<String>,

    /// Build the wasm in release mode (smaller, slower to build).
    #[arg(long)]
    pub release: bool,
}

/// Resolve the `--frameworks` list to [`cg::Framework`]s. Empty → all.
/// Unknown names warn and are skipped.
fn resolve_frameworks(names: &[String]) -> Vec<cg::Framework> {
    if names.is_empty() {
        return cg::Framework::ALL.to_vec();
    }
    let mut out: Vec<cg::Framework> = Vec::new();
    for n in names {
        match cg::Framework::parse(n) {
            Some(f) if !out.contains(&f) => out.push(f),
            Some(_) => {}
            None => eprintln!(
                "[idealyst export] unknown framework `{n}` — skipping \
                 (known: vanilla, react, vue, svelte, angular)"
            ),
        }
    }
    if out.is_empty() {
        cg::Framework::ALL.to_vec()
    } else {
        out
    }
}

pub fn run(args: Args) -> Result<()> {
    let project = std::fs::canonicalize(&args.project)
        .with_context(|| format!("resolve project dir {}", args.project.display()))?;
    let source = framework_source::resolve(&project)?;
    let manifest = build_ios::parse_manifest(&project)
        .context("read the project's Cargo.toml")?;

    // 1. Discover external components.
    let components = discover(&project)?;
    if components.is_empty() {
        println!(
            "[idealyst export] no `#[component(external)]` found in {}. \
             Tag a component with `#[component(external)]` (and derive \
             `IdealystSchema` on its props) to export it.",
            project.display()
        );
        return Ok(());
    }
    println!(
        "[idealyst export] exporting {} component(s): {}",
        components.len(),
        components.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
    );
    warn_skipped(&components);

    let out_dir = args.out_dir.clone().unwrap_or_else(|| project.join("dist/external"));
    let pkg_dir = out_dir.join("pkg");
    std::fs::create_dir_all(&pkg_dir)
        .with_context(|| format!("create {}", pkg_dir.display()))?;

    // 2. Generate the bridge crate.
    let bridge_dir = generate_bridge_crate(&project, &source, &manifest, &components)?;

    // 3. Build wasm + run wasm-bindgen into dist/external/pkg.
    let wasm = build_wasm(&bridge_dir, &source, &project, args.release)?;
    run_wasm_bindgen(&wasm, &pkg_dir)?;

    // 4 + 5. Generate the JS/TS surface and the demo page.
    let frameworks = resolve_frameworks(&args.frameworks);
    emit_js_surface(&out_dir, &manifest.name, &components, &frameworks)?;

    println!(
        "[idealyst export] done → {}\n  • custom elements: {}\n  • wrappers: {}\n  \
         • open the demo: idealyst serve {} --port 8080",
        out_dir.display(),
        components.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>().join(", "),
        frameworks.iter().map(|f| f.slug()).collect::<Vec<_>>().join(", "),
        out_dir.display(),
    );
    Ok(())
}

/// Build + run the ephemeral external-manifest extractor and parse its JSON.
fn discover(project: &Path) -> Result<Vec<cg::ExternalComponent>> {
    let wrapper = super::catalog_wrapper::generate_with(
        project,
        "external-manifest",
        "external-manifest",
        "dump_external_components_json",
    )
    .context("prepare the external-component manifest extractor")?;

    let output = Command::new("cargo")
        .current_dir(&wrapper)
        .args(["run", "-q", "--bin", "external-manifest"])
        .output()
        .context("run the external-manifest extractor")?;
    if !output.status.success() {
        bail!(
            "external-manifest extraction failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        external_components: Vec<cg::ExternalComponent>,
    }
    let parsed: Manifest = serde_json::from_slice(&output.stdout)
        .context("parse external-component manifest JSON")?;
    Ok(parsed.external_components)
}

/// Log a one-line warning for every prop the codegen had to skip, so the
/// drop is visible rather than silent.
fn warn_skipped(components: &[cg::ExternalComponent]) {
    for c in components {
        for p in cg::classify_props(&c.props) {
            if let cg::PropKind::Skip { reason } = &p.kind {
                eprintln!(
                    "[idealyst export] {}: prop `{}` not exported — {reason}",
                    c.name, p.name
                );
            }
        }
    }
}

/// Materialize the ephemeral bridge cdylib crate; return its directory.
fn generate_bridge_crate(
    project: &Path,
    source: &build_ios::FrameworkSource,
    manifest: &build_ios::Manifest,
    components: &[cg::ExternalComponent],
) -> Result<PathBuf> {
    let bridge_dir = source.wrapper_root(project).join(&manifest.name).join("external/bridge");
    std::fs::create_dir_all(bridge_dir.join("src"))
        .with_context(|| format!("create {}", bridge_dir.join("src").display()))?;
    std::fs::create_dir_all(bridge_dir.join(".cargo"))
        .with_context(|| format!("create {}", bridge_dir.join(".cargo").display()))?;

    let rc_dep = source.dep("crates/runtime/core", &[]);
    let bw_dep = source.dep("crates/backend/web", &[]);
    let patch_block = source.patch_block();

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst export`. Do not edit — rewritten on demand.
#
# Ephemeral wasm bridge: links the project and exposes one
# `#[wasm_bindgen]` class per `#[component(external)]`. crate-type cdylib
# so wasm-bindgen can process the produced wasm.
[workspace]

[package]
name = "{name}-external-bridge"
version = "0.0.1"
edition = "2021"
publish = false

[lib]
name = "external_bridge"
crate-type = ["cdylib"]

[dependencies]
runtime-core = {rc_dep}
backend-web = {bw_dep}
{user_name} = {{ path = "{user_path}" }}
wasm-bindgen = "0.2"
js-sys = "0.3"
web-sys = {{ version = "0.3", features = ["Element"] }}
{patch_block}"#,
        name = manifest.name,
        rc_dep = rc_dep,
        bw_dep = bw_dep,
        user_name = manifest.name,
        user_path = project.display(),
        patch_block = patch_block,
    );

    let lib_rs = cg::gen_bridge_lib(components);

    // Share the project's target dir so deps stay warm and the wasm lands
    // at a predictable path (mirrors catalog_wrapper).
    let cargo_config = format!(
        "# GENERATED. Share the project target dir.\n[build]\ntarget-dir = \"{}\"\n",
        source.cargo_target_dir(project).display(),
    );

    write_if_changed(&bridge_dir.join("Cargo.toml"), &cargo_toml)?;
    write_if_changed(&bridge_dir.join("src/lib.rs"), &lib_rs)?;
    write_if_changed(&bridge_dir.join(".cargo/config.toml"), &cargo_config)?;

    // Seed the wrapper's lockfile from the project's resolved one on first
    // generation. The bridge is a standalone crate, so without this cargo
    // re-resolves to the newest semver-compatible versions on crates.io —
    // which differ from the workspace's pins and so can't reuse the wasm
    // artifacts an earlier `idealyst build --web` / dev already compiled
    // into the shared target dir. Seeding the same pins makes the first
    // build reuse them. After that, cargo maintains the wrapper's own lock
    // (it carries the bridge package entry the project lock lacks), so we
    // only seed when absent.
    let bridge_lock = bridge_dir.join("Cargo.lock");
    if !bridge_lock.exists() {
        if let Some(src) = find_cargo_lock(project) {
            let _ = std::fs::copy(&src, &bridge_lock);
        }
    }
    Ok(bridge_dir)
}

/// Find the resolved `Cargo.lock` governing `project` — its own, or the
/// nearest ancestor's (a workspace member's lock lives at the workspace
/// root). `None` if the project was never resolved.
fn find_cargo_lock(project: &Path) -> Option<PathBuf> {
    let mut dir = Some(project);
    while let Some(d) = dir {
        let candidate = d.join("Cargo.lock");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Build the bridge crate to wasm; return the path to the produced `.wasm`.
fn build_wasm(
    bridge_dir: &Path,
    source: &build_ios::FrameworkSource,
    project: &Path,
    release: bool,
) -> Result<PathBuf> {
    println!("[idealyst export] building wasm bridge…");
    let mut cmd = Command::new("cargo");
    cmd.current_dir(bridge_dir)
        .args(["build", "--target", "wasm32-unknown-unknown"]);
    if release {
        cmd.arg("--release");
    }
    let status = cmd.status().context("cargo build the wasm bridge")?;
    if !status.success() {
        bail!("wasm bridge build failed");
    }
    let profile = if release { "release" } else { "debug" };
    let wasm = source
        .cargo_target_dir(project)
        .join("wasm32-unknown-unknown")
        .join(profile)
        .join("external_bridge.wasm");
    if !wasm.exists() {
        bail!("expected wasm artifact not found at {}", wasm.display());
    }
    Ok(wasm)
}

/// Run `wasm-bindgen --target web` into `pkg_dir`.
fn run_wasm_bindgen(wasm: &Path, pkg_dir: &Path) -> Result<()> {
    println!("[idealyst export] running wasm-bindgen…");
    let status = Command::new("wasm-bindgen")
        .arg(wasm)
        .arg("--out-dir")
        .arg(pkg_dir)
        .args(["--target", "web"])
        .status()
        .context(
            "run wasm-bindgen — install it with `cargo install wasm-bindgen-cli` \
             (or run `idealyst doctor`)",
        )?;
    if !status.success() {
        bail!("wasm-bindgen failed");
    }
    Ok(())
}

/// Generate every JS/TS artifact + the demo page into `out_dir`.
fn emit_js_surface(
    out_dir: &Path,
    pkg_name: &str,
    components: &[cg::ExternalComponent],
    frameworks: &[cg::Framework],
) -> Result<()> {
    // The universal layer: one custom element + `.d.ts` per component.
    let mut barrel = String::from("// GENERATED by `idealyst export`.\n");
    for c in components {
        let stem = c.tag.strip_prefix("idl-").unwrap_or(&c.tag);
        let elem_file = format!("idl-{stem}.js");
        write_if_changed(&out_dir.join(&elem_file), &cg::gen_element_js(c, "external_bridge"))?;
        write_if_changed(&out_dir.join(format!("idl-{stem}.d.ts")), &cg::gen_dts(c))?;
        barrel.push_str(&format!("export * from \"./{elem_file}\";\n"));
    }

    // Per-framework typed wrappers.
    for &fw in frameworks {
        let dir = out_dir.join(fw.slug());
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create {}", dir.display()))?;
        for c in components {
            write_if_changed(&dir.join(fw.filename(c)), &fw.generate(c))?;
        }
    }

    write_if_changed(&out_dir.join("index.js"), &barrel)?;
    write_if_changed(&out_dir.join("index.d.ts"), &cg::gen_index_dts(components))?;
    write_if_changed(&out_dir.join("package.json"), &cg::gen_package_json(pkg_name, frameworks))?;
    write_if_changed(&out_dir.join("index.html"), &gen_demo_html(components))?;
    write_if_changed(&out_dir.join("README.md"), &gen_readme(components, frameworks))?;
    Ok(())
}

/// A usage README covering the bare custom element (works everywhere) plus
/// each generated framework wrapper.
fn gen_readme(components: &[cg::ExternalComponent], frameworks: &[cg::Framework]) -> String {
    let first = &components[0];
    let tag = &first.tag;
    let name = &first.name;
    let list = components
        .iter()
        .map(|c| format!("- `<{}>` (`{}`)", c.tag, c.name))
        .collect::<Vec<_>>()
        .join("\n");

    let mut out = format!(
        "# Exported Idealyst components\n\n\
         Generated by `idealyst export`. Each component is a **wasm-backed Web \
         Component** (custom element) — it works in **any** framework that renders \
         DOM (React, Vue, Svelte, Angular, Solid, Lit, Qwik, or no framework at \
         all). The per-framework folders are typed convenience wrappers on top.\n\n\
         ## Components\n\n{list}\n\n\
         ## Universal (any framework / vanilla)\n\n\
         ```html\n\
         <script type=\"module\" src=\"./index.js\"></script>\n\
         <{tag} name=\"World\"></{tag}>\n\
         <script type=\"module\">\n\
         \x20 const el = document.querySelector(\"{tag}\");\n\
         \x20 el.name = \"Idealyst\";              // reactive property\n\
         \x20 el.addEventListener(\"greet\", () => {{}}); // callbacks are DOM events\n\
         </script>\n\
         ```\n\n",
    );

    for &fw in frameworks {
        out.push_str(&match fw {
            cg::Framework::Vanilla => format!(
                "## Vanilla (imperative helpers)\n\n```js\nimport {{ create{name} }} \
                 from \"./vanilla/{name}.js\";\ndocument.body.append(create{name}({{ \
                 name: \"World\" }}));\n```\n\n"
            ),
            cg::Framework::React => format!(
                "## React\n\n```tsx\nimport {{ {name} }} from \"./react/{name}\";\n\
                 <{name} name=\"World\" onGreet={{() => {{}}}} />\n```\n\n"
            ),
            cg::Framework::Vue => format!(
                "## Vue\n\n```js\nimport {{ {name} }} from \"./vue/{name}\";\n```\n\n"
            ),
            cg::Framework::Svelte => format!(
                "## Svelte\n\n```svelte\n<script>import {name} from \"./svelte/{name}.svelte\";</script>\n\
                 <{name} name=\"World\" onGreet={{() => {{}}}} />\n```\n\n"
            ),
            cg::Framework::Angular => format!(
                "## Angular\n\n```ts\nimport {{ {name}Component }} from \"./angular/{stem}.component\";\n\
                 // standalone — add to a component's `imports`, then:\n\
                 // <{tag}-ng [name]=\"'World'\" (greet)=\"onGreet()\"></{tag}-ng>\n```\n\n",
                stem = tag.strip_prefix("idl-").unwrap_or(tag),
            ),
        });
    }
    out
}

/// A vanilla HTML page that imports the barrel and drops every exported
/// element on the page — the "load in a plain HTML page" proof.
fn gen_demo_html(components: &[cg::ExternalComponent]) -> String {
    let tags: String = components
        .iter()
        .map(|c| format!("    <h2>&lt;{tag}&gt;</h2>\n    <{tag}></{tag}>\n", tag = c.tag))
        .collect();
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\" />\n  \
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n  \
         <title>Idealyst external export — demo</title>\n  \
         <style>body {{ font-family: system-ui, sans-serif; margin: 2rem; }}</style>\n\
         </head>\n<body>\n  <h1>Exported components (vanilla HTML host)</h1>\n{tags}  \
         <script type=\"module\">import \"./index.js\";</script>\n</body>\n</html>\n",
    )
}

fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == contents {
            return Ok(());
        }
    }
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}
