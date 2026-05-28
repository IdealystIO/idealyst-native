//! Locate the root component to render in the scratch `main.rs`.
//!
//! Resolution order:
//!
//! 1. If `--root-file <path>` is set, only consider files whose
//!    input path equals (or ends in) that path. Otherwise consider
//!    every successfully-ported file.
//! 2. Among those, look for a component named `<--root NAME>`
//!    (default `App`).
//! 3. The first match wins. Returns the source-relative output
//!    path + the component's PascalCase name, which the caller
//!    converts into a `ported::<…>` Rust module path.
//!
//! Returns `None` if the user didn't specify a root and no
//! component named `App` exists — in that case the scratch crate
//! falls back to compile-check-only mode (no rendering main).

use std::path::{Path, PathBuf};

use port_project::report::{FilePort, Status};

#[derive(Debug)]
pub struct RootMatch {
    /// PascalCase component name as written in source.
    pub component_name: String,
    /// PascalCase Rust function name — equal to `component_name`
    /// under the framework's transform-free dispatch (fn name ==
    /// macro name == call name). Retained as a distinct field for
    /// call sites that read it by role.
    pub fn_name: String,
    /// Output path of the file containing the component (the
    /// generated `.rs` file). Used to compute the `ported::…`
    /// module path.
    pub output_path: PathBuf,
}

pub fn find(
    files: &[FilePort],
    name: &str,
    only_file: Option<&Path>,
) -> Option<RootMatch> {
    for f in files {
        if !matches!(f.status, Status::Ok) {
            continue;
        }
        if let Some(restrict) = only_file {
            if !path_matches(&f.input, restrict) {
                continue;
            }
        }
        if let Some(found) = f.components.iter().find(|c| c.as_str() == name) {
            let output = match &f.output {
                Some(o) => o.clone(),
                None => continue,
            };
            return Some(RootMatch {
                component_name: found.clone(),
                fn_name: found.clone(),
                output_path: output,
            });
        }
    }
    None
}

/// `--root-file path/to/App.tsx` is considered a match for an
/// input that *ends* in the given suffix. This is forgiving by
/// design: users can pass either the absolute path under the
/// temp clone dir, the project-relative path, or just the
/// filename.
fn path_matches(input: &Path, restrict: &Path) -> bool {
    if input == restrict {
        return true;
    }
    let input_str = input.to_string_lossy();
    let restrict_str = restrict.to_string_lossy();
    input_str.ends_with(restrict_str.as_ref())
}

/// Convert a file's output path (under `<scratch>/src/ported/`)
/// into a Rust module path like `ported::src::components::app`.
/// The first segment is always `ported` (per the scratch
/// layout); subsequent segments come from the directories +
/// snake-cased filename stem.
pub fn module_path_for(output_path: &Path, scratch_dir: &Path) -> Option<String> {
    let src_root = scratch_dir.join("src").join("ported");
    let rel = output_path.strip_prefix(&src_root).ok()?;
    let mut segments = vec!["ported".to_string()];
    let parent = rel.parent()?;
    for c in parent.components() {
        let s = c.as_os_str().to_str()?;
        segments.push(sanitize_segment(s));
    }
    let stem = rel.file_stem()?.to_str()?;
    segments.push(sanitize_segment(stem));
    Some(segments.join("::"))
}

/// Mirror of `scaffold::sanitize_module_name` so module path
/// segments line up with the `mod.rs` declarations the scaffold
/// writes. Kept in sync by tests.
fn sanitize_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    let mut first = true;
    for ch in s.chars() {
        if first {
            if ch.is_ascii_digit() {
                out.push_str("n_");
            }
            first = false;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_path_basic() {
        let p = module_path_for(
            Path::new("/tmp/out/src/ported/src/app.rs"),
            Path::new("/tmp/out"),
        );
        assert_eq!(p.as_deref(), Some("ported::src::app"));
    }

    #[test]
    fn module_path_nested() {
        let p = module_path_for(
            Path::new("/tmp/out/src/ported/src/components/widget.rs"),
            Path::new("/tmp/out"),
        );
        assert_eq!(p.as_deref(), Some("ported::src::components::widget"));
    }
}
