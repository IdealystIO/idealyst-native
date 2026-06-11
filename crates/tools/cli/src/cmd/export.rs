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
///
/// The universal `web/` layer (the bare custom element + wasm + vanilla
/// helpers — `cg::Framework::Vanilla`) is **always** emitted: it carries
/// the umbrella barrel, the demo, and the no-framework helpers every
/// consumer can fall back to. `--frameworks` only selects which *typed*
/// framework wrappers to add on top.
fn resolve_frameworks(names: &[String]) -> Vec<cg::Framework> {
    let mut out: Vec<cg::Framework> = if names.is_empty() {
        cg::Framework::ALL.to_vec()
    } else {
        let mut sel: Vec<cg::Framework> = Vec::new();
        for n in names {
            match cg::Framework::parse(n) {
                Some(f) if !sel.contains(&f) => sel.push(f),
                Some(_) => {}
                None => eprintln!(
                    "[idealyst export] unknown framework `{n}` — skipping \
                     (known: web, react, vue, svelte, angular)"
                ),
            }
        }
        if sel.is_empty() {
            cg::Framework::ALL.to_vec()
        } else {
            sel
        }
    };
    // Always ship the universal web layer, first in the list.
    if !out.contains(&cg::Framework::Vanilla) {
        out.insert(0, cg::Framework::Vanilla);
    }
    out
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
    // wasm-bindgen output is staged here once, then copied into each
    // self-contained framework folder (`web/pkg`, `react/pkg`, …).
    let stage_pkg = out_dir.join(".pkg-stage");
    std::fs::create_dir_all(&stage_pkg)
        .with_context(|| format!("create {}", stage_pkg.display()))?;

    // 2. Generate the bridge crate.
    let bridge_dir = generate_bridge_crate(&project, &source, &manifest, &components)?;

    // 3. Build wasm + run wasm-bindgen into the staging dir.
    let wasm = build_wasm(&bridge_dir, &source, &project, args.release)?;
    run_wasm_bindgen(&wasm, &stage_pkg)?;

    // 4 + 5. Generate the JS/TS surface and the demo page.
    let frameworks = resolve_frameworks(&args.frameworks);
    emit_js_surface(&out_dir, &stage_pkg, &manifest.name, &components, &frameworks)?;

    // The staging pkg has been copied into every folder; drop it.
    let _ = std::fs::remove_dir_all(&stage_pkg);

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
///
/// Each framework gets a **self-contained** folder under `out_dir`: its own
/// copy of the wasm `pkg/` (from `stage_pkg`), its own custom-element shells
/// + `.d.ts`, and its typed wrapper(s). Nothing reaches across folders — a
/// wrapper imports the element sitting beside it (`./idl-*.js`), so `web/`
/// (the universal layer) and `react/`/`vue/`/… are each independently
/// consumable. The umbrella `package.json` + `README.md` live at `out_dir`
/// root and point at the `web/` barrel.
fn emit_js_surface(
    out_dir: &Path,
    stage_pkg: &Path,
    pkg_name: &str,
    components: &[cg::ExternalComponent],
    frameworks: &[cg::Framework],
) -> Result<()> {
    // An earlier layout emitted the universal layer loose at the root
    // (`idl-*.js`, `index.js`, `pkg/`) plus a `vanilla/` folder. The
    // current layout moves all of that into `web/`, so re-exporting over an
    // old tree would leave confusing orphans (a stale root `pkg/` beside
    // `web/pkg/`). Sweep the known legacy artifacts first.
    remove_legacy_root_artifacts(out_dir);

    for &fw in frameworks {
        let dir = out_dir.join(fw.slug());
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create {}", dir.display()))?;

        // Each folder carries its own wasm + glue, so it stands alone.
        copy_dir_if_changed(stage_pkg, &dir.join("pkg"))?;

        // The custom elements + declarations, local to this folder, plus
        // the framework's typed wrapper.
        for c in components {
            let stem = c.tag.strip_prefix("idl-").unwrap_or(&c.tag);
            write_if_changed(
                &dir.join(format!("idl-{stem}.js")),
                &cg::gen_element_js(c, "external_bridge"),
            )?;
            write_if_changed(&dir.join(format!("idl-{stem}.d.ts")), &cg::gen_dts(c))?;
            write_if_changed(&dir.join(fw.filename(c)), &fw.generate(c))?;
        }
    }

    // The universal `web/` folder additionally hosts the barrels + demo,
    // which the umbrella `package.json` points at.
    let web = out_dir.join(cg::Framework::Vanilla.slug());
    let mut barrel = String::from("// GENERATED by `idealyst export`.\n");
    for c in components {
        let stem = c.tag.strip_prefix("idl-").unwrap_or(&c.tag);
        barrel.push_str(&format!("export * from \"./idl-{stem}.js\";\n"));
    }
    write_if_changed(&web.join("index.js"), &barrel)?;
    write_if_changed(&web.join("index.d.ts"), &cg::gen_index_dts(components))?;
    write_if_changed(&web.join("index.html"), &gen_demo_html(components))?;

    // Umbrella package files at the root.
    write_if_changed(&out_dir.join("package.json"), &cg::gen_package_json(pkg_name, frameworks))?;
    write_if_changed(&out_dir.join("README.md"), &gen_readme(components, frameworks))?;
    Ok(())
}

/// Remove artifacts the *previous* (pre-`web/` folder) export layout left
/// at `out_dir` root, so an in-place re-export doesn't strand orphans
/// beside the new self-contained folders. Conservative on purpose: only
/// deletes files that carry our own generated markers (never a user file
/// that happens to share a name), and only the `pkg/`/`vanilla/` dirs that
/// are unmistakably our prior output.
fn remove_legacy_root_artifacts(out_dir: &Path) {
    let is_generated = |p: &Path, marker: &str| {
        std::fs::read_to_string(p).map(|s| s.contains(marker)).unwrap_or(false)
    };
    if let Ok(entries) = std::fs::read_dir(out_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let is_element = name.starts_with("idl-")
                && (name.ends_with(".js") || name.ends_with(".d.ts"));
            let is_barrel = name == "index.js" || name == "index.d.ts";
            let is_demo = name == "index.html";
            let stale = ((is_element || is_barrel)
                && is_generated(&p, "GENERATED by `idealyst export`"))
                || (is_demo && is_generated(&p, "Idealyst external export — demo"));
            if stale {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    // The old shared `pkg/` (now copied per-folder) — identified by its
    // wasm-bindgen output — and the renamed `vanilla/` folder.
    let old_pkg = out_dir.join("pkg");
    if old_pkg.join("external_bridge.js").is_file() {
        let _ = std::fs::remove_dir_all(&old_pkg);
    }
    let old_vanilla = out_dir.join("vanilla");
    if old_vanilla.is_dir() {
        let _ = std::fs::remove_dir_all(&old_vanilla);
    }
}

/// Recursively copy `src` into `dst`, skipping any file whose bytes are
/// already identical (mirrors [`write_if_changed`] — keeps re-exports fast
/// and git diffs empty when the wasm hasn't changed).
fn copy_dir_if_changed(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("create {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_if_changed(&from, &to)?;
        } else {
            let bytes = std::fs::read(&from).with_context(|| format!("read {}", from.display()))?;
            let unchanged = std::fs::read(&to).map(|cur| cur == bytes).unwrap_or(false);
            if !unchanged {
                std::fs::write(&to, &bytes).with_context(|| format!("write {}", to.display()))?;
            }
        }
    }
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
         Each folder below (`web/`, `react/`, `vue/`, …) is **self-contained** \
         — it carries its own copy of the wasm `pkg/` and the custom elements, \
         so you can consume one without the others.\n\n\
         ## Universal (any framework / vanilla)\n\n\
         ```html\n\
         <script type=\"module\" src=\"./web/index.js\"></script>\n\
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
                "## Vanilla / web (imperative helpers)\n\n```js\nimport {{ create{name} }} \
                 from \"./web/{name}.js\";\ndocument.body.append(create{name}({{ \
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

#[cfg(test)]
mod tests {
    use super::*;

    fn greeter() -> cg::ExternalComponent {
        cg::ExternalComponent {
            name: "Greeter".into(),
            module_path: "demo".into(),
            tag: "idl-greeter".into(),
            props: Vec::new(),
        }
    }

    /// The fix: react/vue/etc. land in their OWN self-contained folders,
    /// each with its own `pkg/` + element files — nothing is dumped loose
    /// at the root (the old "everything in one folder" behaviour), and the
    /// universal layer lives in `web/`, not mixed in at the top level.
    #[test]
    fn emits_self_contained_per_framework_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("dist/external");
        let stage = tmp.path().join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        // Stand-in wasm artifacts (the real ones come from wasm-bindgen).
        std::fs::write(stage.join("external_bridge.js"), b"// wasm glue").unwrap();
        std::fs::write(stage.join("external_bridge_bg.wasm"), b"\0wasm").unwrap();

        let comps = [greeter()];
        let fws = [cg::Framework::Vanilla, cg::Framework::React];
        emit_js_surface(&out, &stage, "demo", &comps, &fws).unwrap();

        // Each folder carries its own pkg copy + its own element shell.
        for slug in ["web", "react"] {
            assert!(out.join(slug).join("pkg/external_bridge.js").is_file(), "{slug}/pkg");
            assert!(out.join(slug).join("idl-greeter.js").is_file(), "{slug} element");
        }
        // The React wrapper imports the LOCAL element (same folder), so it
        // never reaches back into the universal/web layer.
        let tsx = std::fs::read_to_string(out.join("react/Greeter.tsx")).unwrap();
        assert!(tsx.contains("import \"./idl-greeter.js\""));
        assert!(!tsx.contains("../idl-greeter.js"));

        // The universal `web/` folder owns the barrel, demo, and vanilla
        // helper.
        assert!(out.join("web/index.js").is_file());
        assert!(out.join("web/index.html").is_file());
        assert!(out.join("web/Greeter.js").is_file());

        // Umbrella package.json at the root points into `web/`.
        let pkg = std::fs::read_to_string(out.join("package.json")).unwrap();
        assert!(pkg.contains("\"main\": \"web/index.js\""));

        // Regression: NOTHING is emitted loose at the root anymore.
        assert!(!out.join("idl-greeter.js").exists(), "no loose root element");
        assert!(!out.join("pkg").exists(), "no shared root pkg");
        assert!(!out.join("index.js").exists(), "no root barrel");
    }

    /// Re-exporting over an old-layout tree sweeps the stale root files but
    /// leaves unrelated user files and our own new umbrella files alone.
    #[test]
    fn re_export_clears_legacy_root_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("dist/external");
        std::fs::create_dir_all(out.join("pkg")).unwrap();
        std::fs::create_dir_all(out.join("vanilla")).unwrap();
        // Legacy generated artifacts at the root.
        std::fs::write(
            out.join("idl-greeter.js"),
            "// GENERATED by `idealyst export` — do not edit.\n",
        )
        .unwrap();
        std::fs::write(
            out.join("index.js"),
            "// GENERATED by `idealyst export`.\n",
        )
        .unwrap();
        std::fs::write(out.join("index.html"), "<title>Idealyst external export — demo</title>")
            .unwrap();
        std::fs::write(out.join("pkg/external_bridge.js"), "// wasm").unwrap();
        // A user file the sweep must NOT touch (same dir, not ours).
        std::fs::write(out.join("notes.md"), "keep me").unwrap();
        // A barrel name that ISN'T ours (no generated marker) must survive.
        std::fs::write(out.join("index.d.ts"), "// hand-written").unwrap();

        let stage = tmp.path().join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("external_bridge.js"), b"// wasm").unwrap();
        emit_js_surface(&out, &stage, "demo", &[greeter()], &[cg::Framework::Vanilla]).unwrap();

        // Legacy artifacts swept.
        assert!(!out.join("idl-greeter.js").exists());
        assert!(!out.join("index.js").exists());
        assert!(!out.join("index.html").exists());
        assert!(!out.join("pkg").exists());
        assert!(!out.join("vanilla").exists());
        // Non-ours files preserved.
        assert_eq!(std::fs::read_to_string(out.join("notes.md")).unwrap(), "keep me");
        assert_eq!(std::fs::read_to_string(out.join("index.d.ts")).unwrap(), "// hand-written");
        // New layout in place.
        assert!(out.join("web/index.js").is_file());
        assert!(out.join("web/pkg/external_bridge.js").is_file());
    }

    #[test]
    fn resolve_frameworks_always_includes_web_first() {
        // An explicit subset still gets the universal web layer prepended.
        let sel = resolve_frameworks(&["react".to_string()]);
        assert_eq!(sel.first(), Some(&cg::Framework::Vanilla));
        assert!(sel.contains(&cg::Framework::React));
        // Default (empty) → all, web first.
        assert_eq!(resolve_frameworks(&[]).first(), Some(&cg::Framework::Vanilla));
        // `web` is a recognised token too.
        assert_eq!(resolve_frameworks(&["web".to_string()]), vec![cg::Framework::Vanilla]);
    }

    #[test]
    fn copy_dir_if_changed_recurses_and_skips_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        std::fs::write(src.join("nested/b.txt"), b"b").unwrap();
        let dst = tmp.path().join("dst");
        copy_dir_if_changed(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.join("nested/b.txt")).unwrap(), b"b");
        // A second copy with identical bytes is a no-op and still succeeds.
        copy_dir_if_changed(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("nested/b.txt")).unwrap(), b"b");
    }
}
