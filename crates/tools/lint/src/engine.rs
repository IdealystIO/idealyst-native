//! The lint driver: discover `.rs` files, parse each, run the rules, then
//! resolve every raw finding into a located [`Diagnostic`] by applying the
//! configured severity and inline suppressions.

use std::path::{Path, PathBuf};

use crate::config::{Config, Level, Suppressions};
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules;
use crate::source_map::SourceMap;

/// Result of linting one or more files.
pub struct LintRun {
    pub diagnostics: Vec<Diagnostic>,
    pub files_scanned: usize,
    /// Files that failed to parse (path + the syn error), reported so a
    /// malformed file doesn't silently vanish from the sweep.
    pub parse_errors: Vec<(PathBuf, String)>,
}

impl LintRun {
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error).count()
    }

    pub fn warn_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Warn).count()
    }

    /// Whether the run should fail a build / CI gate: any error-level
    /// diagnostic, or any file that wouldn't parse.
    pub fn failed(&self) -> bool {
        self.error_count() > 0 || !self.parse_errors.is_empty()
    }
}

/// Lint a single in-memory source string. The pure core — no filesystem,
/// fully deterministic — so rules and suppression are unit-testable.
pub fn lint_source(
    source: &str,
    path: &Path,
    config: &Config,
) -> Result<Vec<Diagnostic>, syn::Error> {
    let file = syn::parse_file(source)?;
    let map = SourceMap::new(source);
    let suppressions = Suppressions::parse(source);

    let mut out = Vec::new();
    for raw in rules::collect(&file) {
        let severity = match config.level_for(raw.rule) {
            Level::Off => continue,
            Level::Warn => Severity::Warn,
            Level::Error => Severity::Error,
        };
        let start = raw.span.start();
        let end = raw.span.end();
        // proc-macro2 columns are 0-based; rustc/editor columns are 1-based.
        let line_start = start.line;
        let col_start = start.column + 1;
        if suppressions.suppresses(raw.rule, line_start) {
            continue;
        }
        out.push(Diagnostic {
            rule: raw.rule,
            severity,
            message: raw.message,
            help: raw.help,
            file: path.to_path_buf(),
            line_start,
            col_start,
            line_end: end.line,
            col_end: end.column + 1,
            byte_start: map.byte_offset(start),
            byte_end: map.byte_offset(end),
            source_line: map.line_text(line_start).to_string(),
        });
    }
    // Stable order: by position, then rule, so output and tests don't
    // depend on AST visit order.
    out.sort_by(|a, b| {
        (a.line_start, a.col_start, a.rule).cmp(&(b.line_start, b.col_start, b.rule))
    });
    Ok(out)
}

/// Lint a single file on disk.
pub fn lint_file(path: &Path, config: &Config) -> std::io::Result<Result<Vec<Diagnostic>, String>> {
    let source = std::fs::read_to_string(path)?;
    Ok(lint_source(&source, path, config).map_err(|e| e.to_string()))
}

/// Lint a path: a single `.rs` file, or a directory walked recursively.
pub fn lint_path(root: &Path, config: &Config) -> LintRun {
    let files = if root.is_file() {
        vec![root.to_path_buf()]
    } else {
        discover_rs_files(root)
    };

    let mut run = LintRun { diagnostics: Vec::new(), files_scanned: 0, parse_errors: Vec::new() };
    for file in files {
        match lint_file(&file, config) {
            Ok(Ok(mut diags)) => {
                run.files_scanned += 1;
                run.diagnostics.append(&mut diags);
            }
            Ok(Err(parse_err)) => {
                run.files_scanned += 1;
                run.parse_errors.push((file, parse_err));
            }
            Err(io_err) => {
                run.parse_errors.push((file, format!("read error: {io_err}")));
            }
        }
    }
    run
}

/// Directories never worth walking: build output, VCS metadata, vendored
/// JS deps. Skipping `target` also keeps us off generated/`OUT_DIR` code.
const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules", ".idea", ".vscode"];

/// Recursively collect `.rs` files under `root`, skipping [`SKIP_DIRS`].
pub fn discover_rs_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(src: &str) -> Vec<Diagnostic> {
        lint_source(src, Path::new("test.rs"), &Config::default()).unwrap()
    }

    #[test]
    fn flags_raw_signal_new() {
        let diags = lint("fn f() { let s = Signal::new(0); }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "prefer-signal-macro");
        assert_eq!(diags[0].severity, Severity::Warn);
    }

    #[test]
    fn flags_qualified_signal_new() {
        let diags = lint("fn f() { let s = runtime_core::Signal::new(0); }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "prefer-signal-macro");
    }

    #[test]
    fn flags_raw_effect_new() {
        let diags = lint("fn f() { Effect::new(|| {}); }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "prefer-effect-macro");
    }

    #[test]
    fn flags_memo_call() {
        let diags = lint("fn f() { let m = memo(|| 1); }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "prefer-memo-macro");
    }

    #[test]
    fn flags_builder_call() {
        let diags = lint("fn f() { builder::view(vec![]); }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "prefer-ui-macro");
    }

    #[test]
    fn flags_element_struct_literal() {
        let diags = lint("fn f() { let e = Element::View { children: vec![] }; }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "prefer-ui-macro");
    }

    #[test]
    fn does_not_flag_element_external() {
        // The blessed third-party extension path is exempt.
        let diags = lint("fn f() { let e = Element::External { name: \"x\" }; }");
        assert!(diags.is_empty(), "got {diags:?}");
    }

    #[test]
    fn flags_non_pascal_component() {
        let diags = lint("#[component]\nfn icon_button() -> Element { todo!() }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "component-pascal-case");
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].help.as_ref().unwrap().contains("IconButton"));
    }

    #[test]
    fn does_not_flag_pascal_component() {
        let diags = lint("#[component]\nfn IconButton() -> Element { todo!() }");
        assert!(diags.is_empty(), "got {diags:?}");
    }

    #[test]
    fn does_not_flag_plain_fn() {
        // No `#[component]` attr → naming is the author's business.
        let diags = lint("fn helper() {}");
        assert!(diags.is_empty());
    }

    #[test]
    fn does_not_descend_into_ui_macro_body() {
        // `Signal::new` / `builder::view` written *inside* a macro body are
        // opaque tokens syn never visits, so they're not flagged.
        let diags = lint("fn f() { ui! { view() { Signal::new(0) } }; }");
        assert!(diags.is_empty(), "got {diags:?}");
    }

    #[test]
    fn off_level_suppresses() {
        let cfg = parse_config("[rules]\nprefer-signal-macro = \"off\"\n");
        let diags = lint_source("fn f() { Signal::new(0); }", Path::new("t.rs"), &cfg).unwrap();
        assert!(diags.is_empty());
    }

    #[test]
    fn inline_disable_suppresses() {
        let src = "fn f() {\n    let s = Signal::new(0); // idealyst-lint-disable-line\n}";
        let diags = lint(src);
        assert!(diags.is_empty(), "got {diags:?}");
    }

    #[test]
    fn position_is_one_based_column() {
        // `Signal` starts at column 18 (1-based) on the single line.
        let diags = lint("fn f() { let s = Signal::new(0); }");
        assert_eq!(diags[0].line_start, 1);
        assert_eq!(diags[0].col_start, 18);
    }

    // Helper: build a Config from inline TOML for tests.
    fn parse_config(toml_src: &str) -> Config {
        use std::io::Write;
        let mut tf = tempfile::NamedTempFile::new().unwrap();
        tf.write_all(toml_src.as_bytes()).unwrap();
        Config::load_file(tf.path()).unwrap().config
    }
}
