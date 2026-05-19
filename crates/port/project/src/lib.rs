//! Project-level port driver.
//!
//! Walks a directory, ports each recognized source file via the
//! appropriate `port-*` frontend, writes the rendered Rust
//! alongside a mirrored directory layout, and aggregates results
//! into a [`report::ProjectReport`].
//!
//! Pipeline:
//!
//! 1. **Walk** the source tree (skip `node_modules` etc.).
//! 2. **Lift** every file to a `Module` IR (no Rust emission yet).
//! 3. **Build cross-file registry**: merge every file's
//!    `Module.local_interfaces` into one project-wide
//!    `HashMap<name → PropsType>`. Last-write-wins on duplicates.
//! 4. **Resolve** each `Module`'s `unresolved_context_aliases`
//!    against the global registry; for each match, append a
//!    `pub struct Alias { … }` to `Module.passthroughs` so the
//!    aliased type is available locally for `inject::<Alias>()`
//!    and `provide(Alias { … })`.
//! 5. **Emit** each `Module` to Rust source and write to disk.
//!
//! This gives the porter a focused approximation of cross-file
//! symbol resolution without needing to follow imports literally.
//! Interfaces are identified by name only — duplicate names
//! across files take the last definition.

pub mod git;
pub mod report;
pub mod route;
pub mod walk;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use port_core::emit;
use port_core::ir::{Module, PortReport, PropsType, SourceSnippet};
use report::{FilePort, ProjectReport, Status};
use route::Frontend;

/// Configuration for one port run.
pub struct PortConfig<'a> {
    pub input_root: &'a Path,
    pub output_root: &'a Path,
}

pub fn port_project(cfg: &PortConfig) -> std::io::Result<ProjectReport> {
    fs::create_dir_all(cfg.output_root)?;
    let sources = walk::find_sources(cfg.input_root);

    // Phase 1+2: read + lift every file. Anything that fails to
    // lift becomes a finalized `FilePort` straight away; the rest
    // carry their `Module` into the cross-file resolution pass.
    let mut lifted: Vec<(PathBuf, String, Frontend, Module, PortReport)> = Vec::new();
    let mut early: Vec<FilePort> = Vec::new();
    for src in sources {
        match lift_one(&src) {
            LiftOutcome::Lifted { content, frontend, module, report } => {
                lifted.push((src, content, frontend, module, report));
            }
            LiftOutcome::Finalized(fp) => early.push(fp),
        }
    }

    // Phase 3a: harvest types from plain `.ts` files. These
    // aren't ported as components — they're scanned for
    // `interface X { … }` / `type X = { … }` shapes only — and
    // contribute to the cross-file registry. Common pattern:
    // `src/types/internal.ts` holds the shared type universe
    // that the component files reference via imports.
    let mut registry: HashMap<String, PropsType> = HashMap::new();
    for type_file in walk::find_type_sources(cfg.input_root) {
        if let Ok(content) = fs::read_to_string(&type_file) {
            if let Ok(types) = port_tsx::extract_types(&content, false) {
                for (name, props) in types {
                    registry.insert(name, props);
                }
            }
        }
    }

    // Phase 3b: merge in interfaces from already-lifted source
    // files (component-bearing `.tsx` / `.vue` / `.svelte`). The
    // in-file definitions naturally shadow `.ts`-harvested ones
    // if names collide.
    for (_, _, _, module, _) in &lifted {
        for (name, props) in &module.local_interfaces {
            registry.insert(name.clone(), props.clone());
        }
    }

    // Phase 4: resolve unresolved context aliases against the
    // registry. Resolved aliases become passthrough struct
    // declarations; unresolved ones stay in the Module and the
    // emitter renders them as visible TODO sentinel structs.
    for (_, _, _, module, _) in &mut lifted {
        let aliases = std::mem::take(&mut module.unresolved_context_aliases);
        for (alias, type_name) in aliases {
            match registry.get(&type_name) {
                Some(props) => {
                    let snippet = emit::render_struct(&alias, props);
                    module.passthroughs.push(SourceSnippet::new(snippet));
                }
                None => {
                    // Still unresolved after the global pass — put
                    // it back so the emit fallback surfaces it.
                    module.unresolved_context_aliases.push((alias, type_name));
                }
            }
        }
    }

    // Phase 5: emit + write each module.
    let mut files = early;
    for (src, content, frontend, module, report) in lifted {
        files.push(finalize(&src, &content, frontend, module, report, cfg.input_root, cfg.output_root));
    }
    files.sort_by(|a, b| a.input.cmp(&b.input));

    Ok(ProjectReport { files })
}

enum LiftOutcome {
    Lifted { content: String, frontend: Frontend, module: Module, report: PortReport },
    Finalized(FilePort),
}

fn lift_one(input: &Path) -> LiftOutcome {
    let content = match fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            return LiftOutcome::Finalized(FilePort {
                input: input.into(),
                output: None,
                frontend: None,
                status: Status::Error(format!("read failed: {}", e)),
                holes: Vec::new(),
                bytes_in: 0,
                bytes_out: 0,
                components: Vec::new(),
            });
        }
    };
    let frontend = match route::detect(input, &content) {
        Some(f) => f,
        None => {
            return LiftOutcome::Finalized(FilePort {
                input: input.into(),
                output: None,
                frontend: None,
                status: Status::Skipped("unrecognized extension".into()),
                holes: Vec::new(),
                bytes_in: content.len(),
                bytes_out: 0,
                components: Vec::new(),
            });
        }
    };

    let result = match frontend {
        Frontend::React => port_react::lift(&content).map_err(|e| e.to_string()),
        Frontend::Solid => port_solid::lift(&content).map_err(|e| e.to_string()),
        Frontend::Vue => port_vue::lift(&content).map_err(|e| e.to_string()),
        Frontend::Svelte => port_svelte::lift(&content).map_err(|e| e.to_string()),
    };

    match result {
        Ok((module, report)) => LiftOutcome::Lifted { content, frontend, module, report },
        Err(e) => LiftOutcome::Finalized(FilePort {
            input: input.into(),
            output: None,
            frontend: Some(frontend),
            status: Status::Error(e),
            holes: Vec::new(),
            bytes_in: content.len(),
            bytes_out: 0,
            components: Vec::new(),
        }),
    }
}

fn finalize(
    input: &Path,
    content: &str,
    frontend: Frontend,
    module: Module,
    report: PortReport,
    input_root: &Path,
    output_root: &Path,
) -> FilePort {
    let components: Vec<String> = module.components.iter().map(|c| c.name.clone()).collect();

    // No exported components → Skipped (preserves the prior
    // behavior of not writing empty files for app bootstraps,
    // type-only modules, re-exports).
    if report.component_count == 0 {
        return FilePort {
            input: input.into(),
            output: None,
            frontend: Some(frontend),
            status: Status::Skipped("no exported components".into()),
            holes: report.holes,
            bytes_in: content.len(),
            bytes_out: 0,
            components,
        };
    }

    let rendered = emit::emit_module(&module);
    let output_path = compute_output_path(input, input_root, output_root);
    let bytes_out = rendered.len();
    let write_result = (|| -> std::io::Result<()> {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&output_path, rendered.as_bytes())
    })();

    match write_result {
        Ok(()) => FilePort {
            input: input.into(),
            output: Some(output_path),
            frontend: Some(frontend),
            status: Status::Ok,
            holes: report.holes,
            bytes_in: content.len(),
            bytes_out,
            components,
        },
        Err(e) => FilePort {
            input: input.into(),
            output: Some(output_path),
            frontend: Some(frontend),
            status: Status::Error(format!("write failed: {}", e)),
            holes: report.holes,
            bytes_in: content.len(),
            bytes_out,
            components,
        },
    }
}

/// Build the output path for `input` relative to `input_root`,
/// mirrored under `output_root`. The filename's stem is
/// snake-cased (Rust convention) and the extension becomes `.rs`.
fn compute_output_path(input: &Path, input_root: &Path, output_root: &Path) -> PathBuf {
    let rel = input.strip_prefix(input_root).unwrap_or(input);
    let parent = rel.parent().unwrap_or(Path::new(""));
    let stem = rel
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ported");
    let snake = snake_case(stem);
    let mut out = output_root.to_path_buf();
    out.push(parent);
    out.push(format!("{}.rs", snake));
    out
}

fn snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_lower = false;
        } else if ch == '-' || ch == ' ' {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            prev_lower = false;
        } else {
            out.push(ch);
            prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_basics() {
        assert_eq!(snake_case("Counter"), "counter");
        assert_eq!(snake_case("TodoItem"), "todo_item");
        assert_eq!(snake_case("useToggle"), "use_toggle");
        assert_eq!(snake_case("my-component"), "my_component");
        assert_eq!(snake_case("ALL_CAPS"), "all_caps");
    }
}
