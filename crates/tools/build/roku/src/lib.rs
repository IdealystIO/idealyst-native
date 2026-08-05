//! Roku build orchestration.
//!
//! # Pipeline overview
//!
//! 1. Walk the user's `src/` and transpile every `#[method]` fn
//!    into a single `methods.brs` blob (same logic as the
//!    `idealyst brs` subcommand — they share `backend-roku-transpile`).
//! 2. Take a `ui.json` (a serialized `Vec<RokuCommand>`) the user
//!    has produced via `backend-roku::snapshot`. For now this file
//!    is hand-authored or produced by a user-owned binary; later
//!    a generated snapshot binary can automate it.
//! 3. Lay out the `.pkg` directory at `<project>/dist/roku/`:
//!
//! ```text
//! dist/roku/
//! ├── manifest                   # filled from project Cargo.toml
//! ├── data/ui.json                # copy of the command stream
//! ├── source/main.brs             # entry-point (from runtime/)
//! ├── source/methods.brs          # transpiled #[method] bundle
//! └── components/
//!     ├── IdealystScene.xml       # scene declaration (from runtime/)
//!     └── IdealystScene.brs       # JSON → SceneGraph runtime
//! ```
//!
//! The result is a directory you can zip and side-load via the
//! Roku web UI (`http://<roku-ip>/`). The output isn't a `.zip`
//! yet — Roku's tooling expects directory uploads via the dev UI;
//! adding a `--zip` flag is straightforward when needed.
//!
//! # What this crate does NOT do
//!
//! - Run the user's Rust app to produce `ui.json`. That's a
//!   separate path; the user's app crate must own a small binary
//!   that calls `backend-roku::snapshot(...)` and writes the file.
//!   Wiring an auto-generated snapshot bin (parallel to the
//!   `ios-wrapper` pattern in `build-ios`) is a future TODO.
//! - Wire button callbacks. The runtime sees `CreateButton` and
//!   stores the handler id on the SGNode, but nothing dispatches
//!   the event back yet. Comes in a follow-up.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use syn::{File, Item};

// ---------------------------------------------------------------------------
// Embedded runtime — ships with the build-roku crate.
// ---------------------------------------------------------------------------
//
// We `include_str!` the BrightScript runtime so the crate is a
// self-contained artifact: no `runtime/` directory has to exist on
// disk at runtime, and no path discovery is needed when the CLI is
// installed via `cargo install`. Tradeoff: edits to the runtime
// require a `cargo build` to pick up.

const MANIFEST_TEMPLATE: &str = include_str!("../runtime/manifest");
const MAIN_BRS: &str = include_str!("../runtime/source/main.brs");
const SCENE_XML: &str = include_str!("../runtime/components/IdealystScene.xml");
const SCENE_BRS: &str = include_str!("../runtime/components/IdealystScene.brs");
const LAYOUT_BRS: &str = include_str!("../runtime/components/Layout.brs");
const REACTIVITY_BRS: &str = include_str!("../runtime/components/Reactivity.brs");

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Where to write the package. Defaults to `<project>/dist/roku/`
    /// inside [`build`].
    pub output_dir: Option<PathBuf>,
    /// Optional override of the `ui.json` path. Defaults to
    /// `<project>/dist/ui.json` — the convention is that the user's
    /// own snapshot binary writes the command stream there.
    pub ui_json: Option<PathBuf>,
    /// App title baked into the `manifest`. Defaults to the
    /// project's Cargo package name with the first letter
    /// capitalized.
    pub title: Option<String>,
    /// Where the snapshot wrapper Cargo.toml sources framework crates
    /// from. Only consulted when `ui_json` is `None` (i.e. the
    /// auto-snapshot path).
    pub source: build_ios::FrameworkSource,
}

// No `Default` impl — `source` has no sensible default; the CLI
// constructs it via `FrameworkSource::detect`.

#[derive(Debug)]
pub struct BuildArtifact {
    /// Path to the populated `.pkg` directory.
    pub package_dir: PathBuf,
    /// Path to the zip of `package_dir` — ready to upload to a Roku
    /// in developer mode (drag onto the device's web UI or POST to
    /// `/plugin_install`). Sibling to `package_dir` with `.zip`
    /// suffix: `dist/roku/` + `dist/roku.zip`.
    pub zip_path: PathBuf,
    /// Path to the wrapper crate generated under
    /// `<workspace>/target/idealyst/<project>/roku/snapshot/`. `None`
    /// when the user opted into a manual `dist/ui.json` instead of
    /// the auto-snapshot wrapper.
    pub wrapper_dir: Option<PathBuf>,
    /// Number of `#[method]`-tagged functions transpiled into
    /// `methods.brs`. 0 is fine (no methods → the bundle just
    /// has a header comment).
    pub method_count: usize,
    /// How many `RokuCommand`s were in `ui.json`. 0 means the user's
    /// `app()` produced an empty tree.
    pub command_count: usize,
}

pub fn build(project_dir: &Path, opts: BuildOptions) -> Result<BuildArtifact> {
    let project_dir = project_dir.canonicalize().with_context(|| {
        format!("resolving project dir {}", project_dir.display())
    })?;

    let src_dir = project_dir.join("src");
    if !src_dir.is_dir() {
        return Err(anyhow!(
            "no `src/` under {} — run `idealyst build roku` from a Rust crate root",
            project_dir.display()
        ));
    }

    // --- 1. Collect & transpile #[method] fns ---
    let methods = collect_methods(&src_dir)?;
    let methods_brs = render_methods_brs(&methods);

    // --- 2. Produce the UI command stream. Two paths:
    //   a. Manual: caller passed `opts.ui_json` — use that file directly.
    //      (Useful for hand-authored test fixtures.)
    //   b. Automatic (default): generate a tiny wrapper binary that
    //      depends on the user's crate, `cargo run` it to invoke the
    //      user's `app()`, snapshot via `backend_roku::snapshot_to_pretty_json`,
    //      and read back the JSON it writes.
    let (ui_bytes, command_count, wrapper_dir) = match &opts.ui_json {
        Some(path) => {
            eprintln!("[build-roku] using manual UI snapshot at {}", path.display());
            let bytes = fs::read(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let count = count_commands(&bytes, path)?;
            (bytes, count, None)
        }
        None => {
            let manifest = read_manifest(&project_dir)?;
            let wrapper_dir = opts
                .source
                .wrapper_root(&project_dir)
                .join(&manifest.name)
                .join("roku/snapshot");
            generate_snapshot_wrapper(
                &wrapper_dir,
                &project_dir,
                &opts.source,
                &manifest,
            )?;
            let json_out = wrapper_dir.join("ui.json");
            run_snapshot_wrapper(&wrapper_dir, &json_out)?;
            let bytes = fs::read(&json_out)
                .with_context(|| format!("reading {}", json_out.display()))?;
            let count = count_commands(&bytes, &json_out)?;
            (bytes, count, Some(wrapper_dir))
        }
    };

    // --- 3. Resolve app title ---
    let title = match opts.title {
        Some(t) => t,
        None => infer_title(&project_dir)?,
    };

    // --- 4. Lay out the package ---
    let package_dir = opts
        .output_dir
        .unwrap_or_else(|| project_dir.join("dist/roku"));
    write_package(&package_dir, &title, &methods_brs, &ui_bytes)?;

    // --- 4b. Generate per-virtualizer item components ---
    // Walk the wire stream for `CreateMarkupList` ops; for each,
    // emit a sibling Roku SceneGraph component (`.xml` + `.brs`)
    // under `components/`. The wire op carries the row template's
    // slot so we can bake initial styling into the component's
    // `init()` without needing to re-emit ApplyStyle commands at
    // device boot.
    let virtualizer_components = generate_virtualizer_components(&ui_bytes)?;
    for vc in &virtualizer_components {
        fs::write(
            package_dir.join("components").join(format!("{}.xml", vc.name)),
            &vc.xml,
        )?;
        fs::write(
            package_dir.join("components").join(format!("{}.brs", vc.name)),
            &vc.brs,
        )?;
    }
    if !virtualizer_components.is_empty() {
        eprintln!(
            "[build-roku] generated {} virtualizer item component(s)",
            virtualizer_components.len()
        );
    }

    // --- 5. Zip it ---
    let zip_path = package_dir.with_extension("zip");
    create_channel_zip(&package_dir, &zip_path)
        .with_context(|| format!("zipping {}", package_dir.display()))?;

    Ok(BuildArtifact {
        package_dir,
        zip_path,
        wrapper_dir,
        method_count: methods.len(),
        command_count,
    })
}

/// Zip a Roku package directory into an archive ready for the
/// device's `/plugin_install` endpoint. The contents of `src_dir`
/// land at the zip root — Roku rejects archives that wrap their
/// payload in an enclosing folder.
pub fn create_channel_zip(src_dir: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::create(dest)
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut zw = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    add_dir_to_zip(&mut zw, src_dir, src_dir, options)?;
    zw.finish().context("finalizing zip")?;
    Ok(())
}

fn add_dir_to_zip<W: Write + Seek>(
    zw: &mut zip::ZipWriter<W>,
    root: &Path,
    current: &Path,
    options: zip::write::SimpleFileOptions,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            zw.add_directory(format!("{}/", rel_str), options)?;
            add_dir_to_zip(zw, root, &path, options)?;
        } else {
            zw.start_file(rel_str, options)?;
            let mut f = fs::File::open(&path)?;
            std::io::copy(&mut f, zw)?;
        }
    }
    Ok(())
}

fn count_commands(bytes: &[u8], path: &Path) -> Result<usize> {
    let parsed: serde_json::Value = serde_json::from_slice(bytes)
        .with_context(|| format!("parsing {} as JSON", path.display()))?;
    Ok(parsed.as_array().map(|a| a.len()).unwrap_or(0))
}

#[derive(Debug)]
struct VirtualizerComponent {
    /// Component name without extension (matches the wire op's
    /// `item_component`).
    name: String,
    xml: String,
    brs: String,
}

/// Scan the wire stream for `CreateMarkupList` ops and synthesize
/// a Roku SceneGraph component (`.xml` + `.brs`) for each. The
/// component owns the per-row subtree; the parent scene populates
/// each row's `itemContent` ContentNode and the component watches
/// its fields, mapping them onto the bundled SGNode tree.
///
/// V1 only emits components for the single-text-row shape (one
/// `CreateText` + one `BindText`). Any other shape never lowers
/// to `CreateMarkupList` (the backend's `inspect_simple_text_row`
/// returns `None`) so we don't have to handle it here yet.
fn generate_virtualizer_components(ui_bytes: &[u8]) -> Result<Vec<VirtualizerComponent>> {
    let parsed: serde_json::Value =
        serde_json::from_slice(ui_bytes).context("parsing ui.json for virtualizer codegen")?;
    let mut out = Vec::new();
    let Some(cmds) = parsed.as_array() else {
        return Ok(out);
    };
    for cmd in cmds {
        let Some(obj) = cmd.as_object() else { continue };
        if obj.get("op").and_then(|o| o.as_str()) != Some("CreateMarkupList") {
            continue;
        }
        let name = obj
            .get("item_component")
            .and_then(|v| v.as_str())
            .context("CreateMarkupList missing item_component")?
            .to_string();
        let slot = obj
            .get("row_template")
            .context("CreateMarkupList missing row_template")?;
        let style_hints = extract_style_hints(slot);
        out.push(VirtualizerComponent {
            name: name.clone(),
            xml: render_item_xml(&name, style_hints.background.is_some()),
            brs: render_item_brs(&style_hints),
        });
    }
    Ok(out)
}

#[derive(Default, Debug)]
struct StyleHints {
    /// Hex string with leading `#`, e.g. `#34D399`.
    color: Option<String>,
    /// Pixel size for the row's font.
    font_size: Option<f32>,
    /// Pixel font weight; on Roku we don't differentiate beyond
    /// bold/normal but record for future use.
    font_weight: Option<String>,
    /// Hex string for the row's background fill, e.g. `#1F2937`.
    /// When set, the generated item component inserts a sized
    /// `Rectangle` behind the label so each cell looks like a card
    /// rather than floating text.
    background: Option<String>,
}

/// Pull a coarse `StyleHints` snapshot out of the slot's
/// `ApplyStyleStates.base` field. The item component bakes those
/// values into its `init()` so the row visuals match the
/// stylesheet without round-tripping through ApplyStyle commands.
fn extract_style_hints(slot: &serde_json::Value) -> StyleHints {
    let mut hints = StyleHints::default();
    let Some(cmds) = slot.get("commands").and_then(|c| c.as_array()) else {
        return hints;
    };
    for cmd in cmds {
        let obj = match cmd.as_object() {
            Some(o) => o,
            None => continue,
        };
        let op = obj.get("op").and_then(|s| s.as_str()).unwrap_or("");
        if op != "ApplyStyleStates" && op != "ApplyStyle" {
            continue;
        }
        let base = match op {
            "ApplyStyleStates" => obj.get("base"),
            "ApplyStyle" => obj.get("style"),
            _ => None,
        };
        let Some(base) = base else { continue };
        if let Some(bg) = base.get("background") {
            if let Some(literal) = bg
                .get("kind")
                .and_then(|k| k.as_str())
                .filter(|k| *k == "Literal")
                .and_then(|_| bg.get("value"))
                .and_then(|v| v.as_str())
            {
                hints.background = Some(literal.to_string());
            } else if let Some(tok_fallback) = bg
                .get("kind")
                .and_then(|k| k.as_str())
                .filter(|k| *k == "Token")
                .and_then(|_| bg.get("fallback"))
                .and_then(|v| v.as_str())
            {
                hints.background = Some(tok_fallback.to_string());
            }
        }
        if let Some(color) = base.get("color") {
            // WireColor enum: { kind: "Literal" | "Token", value/name }.
            if let Some(literal) = color
                .get("kind")
                .and_then(|k| k.as_str())
                .filter(|k| *k == "Literal")
                .and_then(|_| color.get("value"))
                .and_then(|v| v.as_str())
            {
                hints.color = Some(literal.to_string());
            } else if let Some(token_fallback) = color
                .get("kind")
                .and_then(|k| k.as_str())
                .filter(|k| *k == "Token")
                .and_then(|_| color.get("fallback"))
                .and_then(|v| v.as_str())
            {
                // Tokenized color — bake the fallback. Theme
                // re-application across MarkupList rows is a
                // follow-up.
                hints.color = Some(token_fallback.to_string());
            }
        }
        if let Some(fs) = base.get("font_size").and_then(|v| v.as_f64()) {
            hints.font_size = Some(fs as f32);
        }
        if let Some(fw) = base.get("font_weight").and_then(|v| v.as_str()) {
            hints.font_weight = Some(fw.to_string());
        }
    }
    hints
}

/// Translate a CSS-ish `#RRGGBB[AA]` to Roku's `0xRRGGBBAA`. Roku
/// requires alpha; default to fully opaque if missing.
fn css_color_to_roku(css: &str) -> String {
    let stripped = css.trim_start_matches('#');
    match stripped.len() {
        6 => format!("0x{}FF", stripped.to_uppercase()),
        8 => format!("0x{}", stripped.to_uppercase()),
        _ => "0xFFFFFFFF".to_string(),
    }
}

fn render_item_xml(name: &str, has_background: bool) -> String {
    let bg_child = if has_background {
        // Rectangle slot for the row's card background. The BS
        // init() sizes it to (cell_w, cell_h) and colors it from
        // the baked `background` hint. Drawn before the Label
        // node so the text renders on top.
        "    <Rectangle id=\"rowBg\" />\n"
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<component name="{name}" extends="Group">
  <interface>
    <field id="itemContent" type="node" onChange="onContentChange" />
    <field id="width" type="float" onChange="onSizeChange" />
    <field id="height" type="float" onChange="onSizeChange" />
  </interface>
  <script type="text/brightscript" uri="pkg:/components/{name}.brs" />
  <children>
{bg_child}    <Label id="rowLabel" horizAlign="center" vertAlign="center" />
  </children>
</component>
"#
    )
}

fn render_item_brs(hints: &StyleHints) -> String {
    let color_line = hints
        .color
        .as_deref()
        .map(|c| format!("    m.label.color = \"{}\"\n", css_color_to_roku(c)))
        .unwrap_or_default();
    let font_size_line = hints
        .font_size
        .map(|fs| {
            // Roku Font is a node with `size` field. Build one in init().
            format!(
                "    font = createObject(\"roSGNode\", \"Font\")\n    font.size = {:.1}\n    m.label.font = font\n",
                fs
            )
        })
        .unwrap_or_default();
    let bg_init = hints
        .background
        .as_deref()
        .map(|bg| {
            format!(
                "    m.bg = m.top.findNode(\"rowBg\")\n    if m.bg <> invalid then m.bg.color = \"{}\"\n",
                css_color_to_roku(bg)
            )
        })
        .unwrap_or_default();
    let bg_size = if hints.background.is_some() {
        "    if m.bg <> invalid then\n        if w > 0 then m.bg.width = w\n        if h > 0 then m.bg.height = h\n    end if\n"
    } else {
        ""
    };
    format!(
        r#"' Auto-generated per-virtualizer item component. The parent
' scene populates this row's `itemContent` ContentNode; we map
' its fields onto the SGNode subtree.

sub init()
    m.label = m.top.findNode("rowLabel")
{bg_init}{color_line}{font_size_line}    ' Each row's ContentNode carries `_cell_w` / `_cell_h` set by
    ' the parent scene's `populateRowFields`. Roku's MarkupList /
    ' RowList don't automatically pass cell dimensions to the
    ' item component's interface — without these the Label
    ' defaults to width = 0 and the text collapses to its natural
    ' bounding rect.
    applyRowSize()
end sub

sub onContentChange()
    content = m.top.itemContent
    if content = invalid then return
    if m.label <> invalid then
        if content.hasField("title") then
            m.label.text = content.title
        end if
    end if
    applyRowSize()
end sub

sub onSizeChange()
    applyRowSize()
end sub

sub applyRowSize()
    if m.label = invalid then return
    w = m.top.width
    h = m.top.height
    content = m.top.itemContent
    if content <> invalid then
        if content.hasField("_cell_w") then
            cw = content._cell_w
            if cw > w then w = cw
        end if
        if content.hasField("_cell_h") then
            ch = content._cell_h
            if ch > h then h = ch
        end if
    end if
    if w > 0 then m.label.width = w
    if h > 0 then m.label.height = h
{bg_size}end sub
"#
    )
}

// ---------------------------------------------------------------------------
// Snapshot wrapper generation + execution
// ---------------------------------------------------------------------------
//
// Pattern mirrors build-ios's `generate_wrapper`: write a tiny Cargo
// crate under `<workspace>/target/idealyst/<project>/roku/snapshot/`
// that depends on the user's crate + `backend-roku`. Its `main`
// invokes the user's `app()`, snapshots the result, and writes the
// JSON to the path passed in argv[1]. Then we `cargo run` it.
//
// The wrapper is regenerated on every build (cheap; just two
// `fs::write` calls); the workspace cache makes the actual cargo
// build incremental.

#[derive(Debug)]
struct UserManifest {
    /// Package name (`[package].name`).
    name: String,
    /// Lib name — what to put after `extern crate` and before `::app()`.
    /// Defaults to `package.name.replace('-', '_')`; overridden by
    /// `[lib].name` if set.
    lib_name: String,
}

fn read_manifest(project_dir: &Path) -> Result<UserManifest> {
    #[derive(Deserialize)]
    struct Raw {
        package: RawPackage,
        #[serde(default)]
        lib: Option<RawLib>,
    }
    #[derive(Deserialize)]
    struct RawPackage {
        name: String,
    }
    #[derive(Deserialize)]
    struct RawLib {
        name: Option<String>,
    }

    let toml_path = project_dir.join("Cargo.toml");
    let raw = fs::read_to_string(&toml_path)
        .with_context(|| format!("reading {}", toml_path.display()))?;
    let parsed: Raw = toml::from_str(&raw)
        .with_context(|| format!("parsing {}", toml_path.display()))?;
    let lib_name = parsed
        .lib
        .as_ref()
        .and_then(|l| l.name.clone())
        .unwrap_or_else(|| parsed.package.name.replace('-', "_"));
    Ok(UserManifest {
        name: parsed.package.name,
        lib_name,
    })
}

// `find_workspace_root` was the legacy lax probe — replaced by
// `build_ios::FrameworkSource` (in-tree path-dep or git fallback).

fn generate_snapshot_wrapper(
    wrapper_dir: &Path,
    project_dir: &Path,
    source: &build_ios::FrameworkSource,
    manifest: &UserManifest,
) -> Result<()> {
    fs::create_dir_all(wrapper_dir.join("src"))
        .with_context(|| format!("create {}", wrapper_dir.display()))?;

    let wrapper_name = format!("{}-roku-snapshot", manifest.name);
    // `new-core` compiles `backend_roku::newcore` (Host + all 30 caps
    // impls, `start`, and `settle` — the embedder contract the snapshot
    // main below implements). The feature is vacuous once backend-roku
    // makes its contents unconditional; drop it at that point.
    let broku_dep = source.dep("crates/backend/roku", &[]);

    let cargo_toml = format!(
        r#"# GENERATED by `idealyst build roku`. Do not edit — rewritten
# every build. This crate's only purpose is to invoke the user's
# `app()` function, snapshot the resulting UI tree to JSON, and exit.

# Empty `[workspace]` declares this wrapper as a standalone project
# even though it physically lives under the main workspace's
# `target/idealyst/...`. Without it, cargo refuses to build because
# the parent Cargo.toml has `[workspace]` and would normally claim
# this directory as a member.
[workspace]

[package]
name = "{wrapper_name}"
version = "0.0.1"
edition = "2021"

[[bin]]
name = "snapshot"
path = "src/main.rs"

[dependencies]
# No framework-root dep: the snapshot main only names `backend_roku`,
# `serde_json`, and the user crate. (A framework-root line used to sit
# here pointing at a pre-move path, which made every generated wrapper
# unresolvable.)
backend-roku = {broku_dep}
# The command stream is serialized here (not by backend-roku) so the
# snapshot can be pretty-printed for the baked `data/ui.json`.
serde_json = "1"
{user_name} = {user_dep}
"#,
        wrapper_name = wrapper_name,
        broku_dep = broku_dep,
        user_name = manifest.name,
        // Plain path dep on the user crate: the app's own defaults
        // select its feature set, and there is one core.
        user_dep = format!("{{ path = \"{}\" }}", project_dir.display()),
    );

    let main_rs = format!(
        r#"//! GENERATED by `idealyst build roku`. Mounts the user's `app()`
//! on a fresh `RokuBackend`, drains the resulting command stream, and
//! writes it as JSON to the path in argv[1] — the `data/ui.json` the
//! `.pkg` bakes in and the BrightScript client replays.

use std::cell::RefCell;
use std::rc::Rc;

fn main() {{
    let out = std::env::args().nth(1)
        .expect("usage: snapshot <output-path>");

    let backend = Rc::new(RefCell::new(backend_roku::RokuBackend::new()));
    // `newcore::start` installs the time source, builds the registry
    // (`register_builtins` + the app's `register_scene_extensions`
    // seam — an unregistered payload panics at realize), realizes the
    // tree inside a fresh `World`, and emits `Finish {{ root }}`.
    let app = backend_roku::newcore::start(
        backend.clone(),
        {lib}::register_scene_extensions,
        || {lib}::app(),
    );
    // EMBEDDER CONTRACT (backend_roku::newcore module docs): `settle()`
    // — drain buffered microtasks + flush the world — must run BEFORE
    // draining the command queue. Signal writes staged during mount are
    // not observable until the flush, so draining first would bake an
    // incomplete initial scene into `ui.json`.
    backend_roku::newcore::settle();
    let commands = backend.borrow_mut().drain();

    let json = serde_json::to_string_pretty(&commands)
        .expect("serialize the Roku command stream");
    std::fs::write(&out, json)
        .expect("write ui.json");

    // Explicit teardown so scope cleanups run before the process exits
    // (the old snapshot path dropped its `Owner` at the same point).
    app.stop();
}}
"#,
        lib = manifest.lib_name,
    );

    fs::write(wrapper_dir.join("Cargo.toml"), cargo_toml)?;
    fs::write(wrapper_dir.join("src/main.rs"), main_rs)?;
    Ok(())
}

fn run_snapshot_wrapper(wrapper_dir: &Path, out_path: &Path) -> Result<()> {
    eprintln!("[build-roku] snapshotting UI via wrapper at {}", wrapper_dir.display());
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--release", "--bin", "snapshot", "--"])
        .arg(out_path)
        .current_dir(wrapper_dir);
    let status = cmd.status().with_context(|| {
        format!("invoking `cargo run` in {}", wrapper_dir.display())
    })?;
    if !status.success() {
        return Err(anyhow!(
            "snapshot wrapper exited with status {} — check that your crate exports \
             `pub fn app() -> runtime_scene::Element` and a registry-generic \
             `pub fn register_scene_extensions<H: runtime_scene::Host>(&mut \
             runtime_scene::Registry<H>)`",
            status
        ));
    }
    if !out_path.is_file() {
        return Err(anyhow!(
            "snapshot wrapper finished but produced no JSON at {}",
            out_path.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Method collection — same shape as the `idealyst brs` subcommand.
// ---------------------------------------------------------------------------

/// Per-method record. `arity` is the number of parameters the
/// `#[method]` function declares — used by the generated
/// `dispatch_method(name, args)` helper so it knows how many slots
/// to index out of `args` when calling each function.
struct MethodInfo {
    arity: usize,
    body: String,
}

fn collect_methods(src_dir: &Path) -> Result<BTreeMap<String, MethodInfo>> {
    let mut out = BTreeMap::new();
    visit_dir(src_dir, &mut |path| scan_file(path, &mut out))?;
    Ok(out)
}

fn visit_dir(dir: &Path, visit: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.')
            || name_str == "target"
            || name_str == "dist"
            || name_str == "node_modules"
        {
            continue;
        }
        if path.is_dir() {
            visit_dir(&path, visit)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            visit(&path)?;
        }
    }
    Ok(())
}

fn scan_file(path: &Path, out: &mut BTreeMap<String, MethodInfo>) -> Result<()> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let parsed: File = match syn::parse_file(&source) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "[build-roku] WARN: skipping {} (parse error: {})",
                path.display(),
                e
            );
            return Ok(());
        }
    };
    walk_items(&parsed.items, path, out)
}

fn walk_items(
    items: &[Item],
    path: &Path,
    out: &mut BTreeMap<String, MethodInfo>,
) -> Result<()> {
    for item in items {
        match item {
            Item::Fn(f) => {
                if has_method_attr(&f.attrs) {
                    let name = f.sig.ident.to_string();
                    let arity = f.sig.inputs.len();
                    let body = backend_roku_transpile::transpile_fn(f).map_err(|e| {
                        anyhow!("{}: `#[method] fn {}`: {}", path.display(), name, e)
                    })?;
                    out.insert(name, MethodInfo { arity, body });
                }
            }
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    walk_items(items, path, out)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn has_method_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .map(|s| s.ident == "method")
            .unwrap_or(false)
    })
}

fn render_methods_brs(methods: &BTreeMap<String, MethodInfo>) -> String {
    let mut s = String::new();
    s.push_str(
        "' Generated by `idealyst build roku` — do not edit by hand.\n\
         ' One BrightScript function per `#[method]` fn in the Rust crate,\n\
         ' plus a `dispatch_method(name, args)` helper the runtime calls when\n\
         ' a signal binding or button action fires.\n\n",
    );
    for (name, info) in methods {
        s.push_str(&format!("' --- {} (arity {}) ---\n", name, info.arity));
        s.push_str(&info.body);
        s.push('\n');
    }
    // Dispatch helper: BrightScript has no built-in "call function by
    // name" facility, so we emit a switch over the names we know. The
    // runtime hands us an roArray of pre-collected argument values
    // (one per parameter), and we index into it according to each
    // method's recorded arity.
    s.push_str(
        "' --- dispatch_method ---\n\
         ' Called by Reactivity.brs whenever a binding fires.\n\
         ' `args` is an roArray sized to the target method's arity.\n\
         function dispatch_method(name as string, args as object) as dynamic\n",
    );
    if methods.is_empty() {
        s.push_str("    ' (no #[method] fns in this project)\n");
    } else {
        // BS doesn't allow single-line `if then return` mixed with
        // `else if`/`end if`. Emit explicit block form: `if/then\n
        // <body>\nelse if/then\n<body>\nend if`.
        for (i, (name, info)) in methods.iter().enumerate() {
            let keyword = if i == 0 { "if" } else { "else if" };
            let call_args: Vec<String> =
                (0..info.arity).map(|i| format!("args[{}]", i)).collect();
            s.push_str(&format!(
                "    {} name = \"{}\" then\n        return {}({})\n",
                keyword,
                name,
                name,
                call_args.join(", "),
            ));
        }
        s.push_str("    end if\n");
    }
    s.push_str(
        "    ? \"[idealyst] dispatch_method: unknown name '\"; name; \"'\"\n\
         \x20   return invalid\n\
         end function\n",
    );
    s
}

// ---------------------------------------------------------------------------
// Title inference
// ---------------------------------------------------------------------------

fn infer_title(project_dir: &Path) -> Result<String> {
    // Tiny Cargo.toml read — we only need `package.name`. Roku
    // apps don't need any of the iOS metadata, so we don't share
    // build-ios's parser (it requires bundle_id, which isn't a
    // thing here).
    #[derive(Deserialize)]
    struct Minimal {
        package: MinimalPackage,
    }
    #[derive(Deserialize)]
    struct MinimalPackage {
        name: String,
    }

    let toml_path = project_dir.join("Cargo.toml");
    let raw = match fs::read_to_string(&toml_path) {
        Ok(s) => s,
        Err(_) => return Ok("Idealyst App".to_string()),
    };
    let parsed: Minimal = match toml::from_str(&raw) {
        Ok(p) => p,
        Err(_) => return Ok("Idealyst App".to_string()),
    };
    // Replace dashes/underscores with spaces; title-case each word
    // so "hello-roku" → "Hello Roku".
    let pretty: String = parsed
        .package
        .name
        .split(|c: char| c == '-' || c == '_')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(if pretty.is_empty() {
        "Idealyst App".to_string()
    } else {
        pretty
    })
}

// ---------------------------------------------------------------------------
// Package layout writer
// ---------------------------------------------------------------------------

fn write_package(
    pkg_dir: &Path,
    title: &str,
    methods_brs: &str,
    ui_json: &[u8],
) -> Result<()> {
    // Clear out any previous build so removed files don't linger.
    if pkg_dir.exists() {
        fs::remove_dir_all(pkg_dir).with_context(|| {
            format!("removing previous package at {}", pkg_dir.display())
        })?;
    }
    fs::create_dir_all(pkg_dir)?;
    fs::create_dir_all(pkg_dir.join("data"))?;
    fs::create_dir_all(pkg_dir.join("source"))?;
    fs::create_dir_all(pkg_dir.join("components"))?;

    // manifest with title substituted
    let manifest = MANIFEST_TEMPLATE
        .replace("__APP_TITLE__", title)
        .replace("__APP_MAJOR__", "1")
        .replace("__APP_MINOR__", "0")
        .replace("__APP_BUILD__", "0");
    fs::write(pkg_dir.join("manifest"), manifest)?;

    fs::write(pkg_dir.join("data/ui.json"), ui_json)?;
    fs::write(pkg_dir.join("source/main.brs"), MAIN_BRS)?;
    fs::write(pkg_dir.join("source/methods.brs"), methods_brs)?;
    fs::write(pkg_dir.join("components/IdealystScene.xml"), SCENE_XML)?;
    fs::write(pkg_dir.join("components/IdealystScene.brs"), SCENE_BRS)?;
    fs::write(pkg_dir.join("components/Layout.brs"), LAYOUT_BRS)?;
    fs::write(pkg_dir.join("components/Reactivity.brs"), REACTIVITY_BRS)?;
    Ok(())
}

#[cfg(test)]
mod wrapper_template_tests {
    //! Shape regression for the generated snapshot wrapper. The wrapper
    //! is ephemeral generated source, so drift is invisible until a user
    //! runs `idealyst build roku` — these pin the load-bearing lines.
    //! They only run the generation step (sub-millisecond); no cargo.

    use super::*;

    fn generated() -> (String, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("project");
        let wrapper_dir = tmp.path().join("wrapper");
        fs::create_dir_all(&project_dir).unwrap();
        let manifest = UserManifest {
            name: "demo-app".to_string(),
            lib_name: "demo_app".to_string(),
        };
        let source = build_ios::FrameworkSource::Workspace {
            root: tmp.path().join("workspace"),
        };
        generate_snapshot_wrapper(&wrapper_dir, &project_dir, &source, &manifest)
            .expect("generate snapshot wrapper");
        (
            fs::read_to_string(wrapper_dir.join("src/main.rs")).unwrap(),
            fs::read_to_string(wrapper_dir.join("Cargo.toml")).unwrap(),
        )
    }

    /// The snapshot wrapper mounts through `backend_roku::newcore::start`
    /// with the app's registry-generic `register_scene_extensions` seam,
    /// and pins no core feature on the user crate (there is one core).
    #[test]
    fn wrapper_mounts_through_newcore_start_with_scene_seam() {
        let (main_rs, cargo) = generated();
        assert!(
            main_rs.contains("backend_roku::newcore::start("),
            "snapshot must mount through newcore::start:\n{main_rs}",
        );
        assert!(
            main_rs.contains("demo_app::register_scene_extensions"),
            "snapshot must register through the scene seam:\n{main_rs}",
        );
        assert!(
            main_rs.contains("demo_app::app()"),
            "snapshot must build the app's tree:\n{main_rs}",
        );
        assert!(
            !main_rs.contains("snapshot_to_pretty_json"),
            "the walker-based snapshot helper is gone:\n{main_rs}",
        );
        assert!(
            !cargo.contains("old-core") && !cargo.contains("default-features = false"),
            "user-crate dep must be a plain path dep:\n{cargo}",
        );
        // backend-roku's scene module is unconditional. The wrapper must
        // depend on the crate and must NOT name the deleted `new-core`
        // feature (a generated manifest naming it fails cargo resolution
        // before anything compiles — baseline §7.7).
        assert!(
            cargo.contains("crates/backend/roku"),
            "wrapper must depend on backend-roku:\n{cargo}",
        );
        assert!(
            !cargo.contains("new-core"),
            "the deleted `new-core` feature must not appear:\n{cargo}",
        );
    }

    /// The embedder contract from `backend_roku::newcore`'s module docs:
    /// `settle()` (drain buffered microtasks + flush the world) must run
    /// BEFORE `drain()`. Signal writes staged during mount are not
    /// observable until the flush, so draining first bakes an incomplete
    /// initial scene into `ui.json` — a silent wrong-output bug with no
    /// build error. This test pins the ORDER, not just the presence.
    #[test]
    fn wrapper_settles_before_draining_the_command_queue() {
        let (main_rs, _cargo) = generated();
        let settle = main_rs
            .find("newcore::settle()")
            .expect("snapshot must call the embedder-contract settle()");
        let drain = main_rs
            .find(".drain()")
            .expect("snapshot must drain the command queue");
        assert!(
            settle < drain,
            "settle() must precede drain() (see backend_roku::newcore \
             module docs) — otherwise mount-staged writes are missing \
             from ui.json:\n{main_rs}",
        );
    }

    /// The wrapper carried a `runtime-core = { path =
    /// "crates/framework/core" }` line — a pre-move path pointing at a
    /// directory that does not exist, so every generated wrapper was
    /// unresolvable. Nothing in the snapshot main names a framework root,
    /// so the dep is gone rather than repaired.
    #[test]
    fn wrapper_has_no_stale_framework_core_dep() {
        let (_main, cargo) = generated();
        assert!(
            !cargo.contains("crates/framework/core"),
            "stale pre-move path would make the wrapper unresolvable:\n{cargo}",
        );
        assert!(
            !cargo.contains("runtime-core"),
            "the snapshot main names no framework root — the dep should be \
             absent, not repointed:\n{cargo}",
        );
    }
}
