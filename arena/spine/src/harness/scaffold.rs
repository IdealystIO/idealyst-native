//! Build the isolated project the implementation agent works in.
//!
//! Isolation has two requirements that pull against each other:
//!   * the run must test the **current** framework + MCP (not a published
//!     snapshot) → the scaffold's deps must path-point at this working tree;
//!   * the agent's only documentation source must be the MCP → its MCP config
//!     must contain exactly one server.
//!
//! `idealyst new`, given `IDEALYST_FRAMEWORK_PATH`, satisfies the first by
//! emitting `runtime-core = { path = "<repo>/crates/runtime/core" }`. We then
//! overwrite the generated `.mcp.json` to guarantee the second.
//!
//! Known, documented leak: because the deps are absolute paths into the
//! monorepo, an agent *could* `Read` the framework source instead of asking the
//! MCP. We don't sandbox the filesystem (the run needs to build); instead the
//! feedback pass flags out-of-project reads as a doc-bypass signal
//! (`metrics::doc_bypass_reads`).

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Scaffold {
    pub project_dir: PathBuf,
}

/// Scaffold a fresh idealyst app named `name` inside `dest_parent`, with its
/// framework deps path-pointed at `framework_path`, and an MCP config that
/// exposes only the idealyst server.
pub fn create(name: &str, dest_parent: &Path, framework_path: &Path) -> anyhow::Result<Scaffold> {
    std::fs::create_dir_all(dest_parent)?;
    let project_dir = dest_parent.join(name);

    let output = Command::new("idealyst")
        .arg("new")
        .arg(name)
        .current_dir(dest_parent)
        .env("IDEALYST_FRAMEWORK_PATH", framework_path)
        .output()
        .map_err(|e| anyhow::anyhow!("running `idealyst new`: {e} (is `idealyst` on PATH?)"))?;
    if !output.status.success() {
        anyhow::bail!(
            "`idealyst new {name}` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    anyhow::ensure!(
        project_dir.join("Cargo.toml").is_file(),
        "scaffold did not produce {}/Cargo.toml",
        project_dir.display()
    );

    write_isolated_mcp_config(&project_dir)?;
    Ok(Scaffold { project_dir })
}

/// Overwrite the project's `.mcp.json` so the implementation agent sees exactly
/// one MCP server — the idealyst one — and nothing else.
pub fn write_isolated_mcp_config(project_dir: &Path) -> anyhow::Result<PathBuf> {
    let cfg = serde_json::json!({
        "mcpServers": {
            "idealyst": { "command": "idealyst", "args": ["mcp"] }
        }
    });
    let path = project_dir.join(".mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(path)
}

/// Copy a scenario's `assets/` tree over the scaffolded project (recursive,
/// overwriting). This is how "start from a broken/pre-existing app" scenarios
/// (debug-and-fix, perf) get their starting state: the scaffold provides the
/// build plumbing, the assets provide the code under test.
///
/// **Extra dependencies.** A scenario whose starting app needs SDK crates the
/// bare scaffold doesn't carry (e.g. `split-canvas` needs `canvas` /
/// `canvas-vello`) ships a `Cargo.append.toml` at the assets root. It is NOT
/// copied verbatim (that would clobber the scaffold's package name, which the
/// generated `index.html` imports); instead it is MERGED into the scaffold's
/// `Cargo.toml` after substituting the sentinel `__IDEALYST_FRAMEWORK__` with
/// the framework root — so its path-deps point at the same working tree the
/// scaffold path-deps `runtime-core` at. Merging (rather than a text append)
/// keeps a single `[dependencies]` table and lets a fragment add both a normal
/// dep and a `[target.'cfg(...)'.dependencies]` table in one file.
pub fn overlay_assets(
    assets_dir: &Path,
    project_dir: &Path,
    framework_path: &Path,
) -> anyhow::Result<usize> {
    fn walk(src: &Path, dst: &Path, copied: &mut usize) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name();
            // The Cargo dependency fragment is merged, not copied (below).
            if name == "Cargo.append.toml" {
                continue;
            }
            let to = dst.join(&name);
            if entry.file_type()?.is_dir() {
                std::fs::create_dir_all(&to)?;
                walk(&entry.path(), &to, copied)?;
            } else {
                std::fs::copy(entry.path(), &to)?;
                *copied += 1;
            }
        }
        Ok(())
    }
    let mut copied = 0;
    walk(assets_dir, project_dir, &mut copied)?;

    let append = assets_dir.join("Cargo.append.toml");
    if append.is_file() {
        apply_cargo_append(&append, &project_dir.join("Cargo.toml"), framework_path)?;
        copied += 1;
    }
    Ok(copied)
}

/// Merge the (framework-path-substituted) `Cargo.append.toml` fragment into the
/// scaffold's `Cargo.toml`, deep-merging tables so `[dependencies]` and any
/// `[target.'cfg(...)'.dependencies]` accumulate rather than collide.
fn apply_cargo_append(
    append_path: &Path,
    cargo_toml: &Path,
    framework_path: &Path,
) -> anyhow::Result<()> {
    let fw = framework_path.display().to_string();
    let fragment_src = std::fs::read_to_string(append_path)?
        .replace("__IDEALYST_FRAMEWORK__", &fw);
    let fragment: toml::Value = toml::from_str(&fragment_src)
        .map_err(|e| anyhow::anyhow!("parsing {}: {e}", append_path.display()))?;
    let mut base: toml::Value = toml::from_str(&std::fs::read_to_string(cargo_toml)?)
        .map_err(|e| anyhow::anyhow!("parsing {}: {e}", cargo_toml.display()))?;

    merge_toml(&mut base, &fragment);

    let rendered = toml::to_string_pretty(&base)
        .map_err(|e| anyhow::anyhow!("re-serializing {}: {e}", cargo_toml.display()))?;
    std::fs::write(cargo_toml, rendered)?;
    Ok(())
}

/// Recursively deep-merge `overlay` into `base`. Tables merge key-by-key
/// (recursing on nested tables); any non-table value in `overlay` overwrites
/// `base`. New keys are inserted. This is the minimal merge Cargo.toml needs —
/// accumulate dependency tables without dropping the ones the scaffold wrote.
fn merge_toml(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (k, v) in overlay {
                match base.get_mut(k) {
                    Some(existing) => merge_toml(existing, v),
                    None => {
                        base.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

/// Best-effort `idealyst build --web`. Returns the `dist/web` path on success,
/// `Err` with the build-error tail otherwise. Used both as the compile-tier
/// signal and as the prerequisite for the locator pass (which serves the
/// produced `dist/web/`). `robot = true` adds `--robot` so the bundle dials a
/// relay — required when the rubric has `robot`-tier items on web.
pub fn build_web(project_dir: &Path, robot: bool) -> anyhow::Result<PathBuf> {
    let mut args = vec!["build", "--web"];
    if robot {
        args.push("--robot");
    }
    let output = Command::new("idealyst")
        .args(&args)
        .current_dir(project_dir)
        .output()
        .map_err(|e| anyhow::anyhow!("running `idealyst build --web`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
        let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
        anyhow::bail!("web build failed:\n{tail}");
    }
    Ok(project_dir.join("dist").join("web"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A scenario's `Cargo.append.toml` must merge into the scaffold's Cargo.toml
    // (a) preserving the scaffold's package name — the generated index.html
    // imports `/pkg/<name>.js`, so clobbering it breaks the serve — and its
    // existing `runtime-core` dep, and (b) substituting `__IDEALYST_FRAMEWORK__`
    // with the framework root so the added path-deps resolve the same checkout.
    #[test]
    fn cargo_append_merges_deps_and_substitutes_framework_path() {
        let dir = std::env::temp_dir().join(format!("arena_append_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cargo = dir.join("Cargo.toml");
        std::fs::write(
            &cargo,
            "[package]\nname = \"split_canvas_0\"\n\n[dependencies]\n\
             runtime-core = { path = \"/fw/crates/runtime/core\" }\n\n\
             [package.metadata.idealyst.app]\nname = \"Split Canvas\"\ntargets = [\"web\"]\n",
        )
        .unwrap();
        let append = dir.join("Cargo.append.toml");
        std::fs::write(
            &append,
            "[dependencies]\ncanvas = { path = \"__IDEALYST_FRAMEWORK__/crates/sdk/client/canvas\" }\n\n\
             [target.'cfg(target_arch = \"wasm32\")'.dependencies]\n\
             canvas-vello = { path = \"__IDEALYST_FRAMEWORK__/crates/sdk/client/canvas/vello\" }\n",
        )
        .unwrap();

        apply_cargo_append(&append, &cargo, Path::new("/fw")).unwrap();
        let out = std::fs::read_to_string(&cargo).unwrap();
        let parsed: toml::Value = toml::from_str(&out).unwrap();

        // Package name preserved (index.html import target).
        assert_eq!(parsed["package"]["name"].as_str(), Some("split_canvas_0"));
        // Scaffold's runtime-core dep preserved alongside the added canvas dep.
        assert_eq!(
            parsed["dependencies"]["runtime-core"]["path"].as_str(),
            Some("/fw/crates/runtime/core")
        );
        assert_eq!(
            parsed["dependencies"]["canvas"]["path"].as_str(),
            Some("/fw/crates/sdk/client/canvas")
        );
        // Target-specific table merged in with the sentinel substituted.
        let tgt = &parsed["target"]["cfg(target_arch = \"wasm32\")"]["dependencies"];
        assert_eq!(tgt["canvas-vello"]["path"].as_str(), Some("/fw/crates/sdk/client/canvas/vello"));
        // Existing metadata table survives the round-trip.
        assert_eq!(parsed["package"]["metadata"]["idealyst"]["app"]["name"].as_str(), Some("Split Canvas"));
        // No sentinel leaks through.
        assert!(!out.contains("__IDEALYST_FRAMEWORK__"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
