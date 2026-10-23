//! Turns a submitted set of files into a served web bundle, through the
//! framework's own pipeline: `idealyst build --web`, the command a real
//! app ships with. What the preview runs is therefore exactly what that
//! code would be as an app — same entry point, same boot, same backend.
//!
//! The build happens in a working copy of `template/` at `.work/`. The
//! submitted entry file becomes `src/app.rs` and any other file
//! `src/app/<path>`, which is where `mod widgets;` inside `app.rs`
//! resolves — the layout a scaffolded project has. The working copy sits
//! at the template's depth, so the template's relative path deps resolve
//! from it, and inside the framework checkout, so the CLI resolves the
//! framework from the checkout and cargo finds the repo's registry config.
//!
//! Builds are serialized: there is one working copy, and one cargo target
//! directory (the CLI's per-workspace `target/idealyst-web-*`), which is
//! what keeps a rebuild warm — editing one file recompiles only the
//! snippet crate (under a second on an M-series Mac).
//!
//! Outputs are content-addressed under `compiled/<hash>/`, keyed by the
//! files plus a stamp of the framework source (see [`framework_stamp`]),
//! so the same code against the same framework is served without a build.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The file every submission must contain: it defines `pub fn app()`.
pub const ENTRY_FILE: &str = "app.rs";

/// Bounds on a submission. Generous for teaching code; small enough that
/// a bad client cannot fill the disk through the cache.
const MAX_FILES: usize = 32;
const MAX_TOTAL_BYTES: usize = 256 * 1024;

/// How many bundles `compiled/` keeps. The oldest beyond this are removed
/// after each build.
const MAX_CACHED_BUNDLES: usize = 400;

/// The package name in `template/Cargo.toml`; diagnostics from any other
/// package (framework warnings) are not the learner's.
const SNIPPET_PACKAGE: &str = "fiddle-snippet";

/// Inserted into every served `index.html`: reports runtime failures —
/// a panic's message reaches `console.error` through the boot's panic
/// hook — to the embedding page as `{"source":"idealyst-fiddle", ...}`
/// JSON strings, so an editor can show them beside the code.
const ERROR_BRIDGE: &str = r#"<script>
(function () {
  function post(kind, text) {
    try {
      parent.postMessage(JSON.stringify({ source: "idealyst-fiddle", kind: kind, text: String(text) }), "*");
    } catch (_) {}
  }
  var error = console.error.bind(console);
  console.error = function () {
    error.apply(console, arguments);
    post("error", Array.prototype.map.call(arguments, String).join(" "));
  };
  addEventListener("error", function (e) { post("error", e.message); });
  addEventListener("unhandledrejection", function (e) { post("error", e.reason); });
})();
</script>
"#;

/// One compiler message about the submitted files, with its location in
/// the learner's terms (`app.rs`, `widgets.rs`), not the template's.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    /// `error` or `warning`.
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The submitted file the primary span is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based, as rustc reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// 1-based, as rustc reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// rustc's full rendering, with paths rewritten to the submitted ones.
    pub rendered: String,
}

/// What `POST /compile` answers. A compile error is a normal outcome,
/// not a server failure.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Built {
        /// The bundle's directory under `compiled/`.
        hash: String,
        /// Where the preview loads from: `/compiled/<hash>/`.
        url: String,
        /// Build time; 0 when served from the cache.
        ms: u64,
        cached: bool,
        /// Warnings about the submitted files (the build still succeeded).
        diagnostics: Vec<Diagnostic>,
    },
    Failed {
        diagnostics: Vec<Diagnostic>,
        /// The tail of the build's output, for failures with no compiler
        /// diagnostic (a toolchain problem, a link error).
        log: String,
    },
}

pub struct Compiler {
    /// `examples/fiddle/`.
    root: PathBuf,
    /// The `idealyst` executable.
    cli: PathBuf,
    /// The framework checkout, when the fiddle runs inside one.
    workspace: Option<PathBuf>,
    /// Held for the whole write-files-then-build sequence.
    lock: Mutex<()>,
}

impl Compiler {
    pub fn new(root: PathBuf, cli: PathBuf) -> Result<Self> {
        let workspace = root.parent().and_then(Path::parent).map(Path::to_path_buf);
        let compiler = Compiler { root, cli, workspace, lock: Mutex::new(()) };
        compiler.prepare_work_dir()?;
        Ok(compiler)
    }

    pub fn compiled_dir(&self) -> PathBuf {
        self.root.join("compiled")
    }

    fn work_dir(&self) -> PathBuf {
        self.root.join(".work")
    }

    /// Refresh `.work/` from `template/`: the manifest and the fixed
    /// sources. Its `Cargo.lock` and `src/app*` are left alone — the
    /// lock keeps resolution stable across restarts, and the app files
    /// are rewritten by every build.
    fn prepare_work_dir(&self) -> Result<()> {
        let template = self.root.join("template");
        let work = self.work_dir();
        fs::create_dir_all(work.join("src"))
            .with_context(|| format!("creating {}", work.display()))?;
        for rel in ["Cargo.toml", "src/main.rs", "src/lib.rs"] {
            fs::copy(template.join(rel), work.join(rel))
                .with_context(|| format!("copying template/{rel} into .work/"))?;
        }
        let lock = work.join("Cargo.lock");
        if !lock.exists() && template.join("Cargo.lock").exists() {
            fs::copy(template.join("Cargo.lock"), &lock).context("seeding .work/Cargo.lock")?;
        }
        Ok(())
    }

    pub fn compile(&self, files: &BTreeMap<String, String>) -> Result<Outcome> {
        validate(files)?;
        let stamp = format!("{}{}", framework_stamp(self.workspace.as_deref()), cli_stamp(&self.cli));
        let hash = bundle_hash(files, &stamp);
        let bundle = self.compiled_dir().join(&hash);
        let built = |ms, cached, diagnostics| Outcome::Built {
            url: format!("/compiled/{hash}/"),
            hash: hash.clone(),
            ms,
            cached,
            diagnostics,
        };
        if bundle.join("index.html").exists() {
            return Ok(built(0, true, Vec::new()));
        }

        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // A request for the same files may have built it while we waited.
        if bundle.join("index.html").exists() {
            return Ok(built(0, true, Vec::new()));
        }

        self.write_app_files(files)?;
        let staging = self.compiled_dir().join(format!(".staging-{hash}"));
        let _ = fs::remove_dir_all(&staging);
        let events = self.work_dir().join("events.jsonl");
        let started = Instant::now();
        let output = Command::new(&self.cli)
            .args(["build", "--web", "--no-split", "--out-dir"])
            .arg(&staging)
            .arg("--events-file")
            .arg(&events)
            .arg(self.work_dir())
            .output()
            .with_context(|| format!("running {} — is the idealyst CLI installed?", self.cli.display()))?;
        let ms = started.elapsed().as_millis() as u64;

        let diagnostics = fs::read_to_string(&events)
            .map(|text| snippet_diagnostics(&text))
            .unwrap_or_default();
        if !output.status.success() {
            let _ = fs::remove_dir_all(&staging);
            let log = tail(&String::from_utf8_lossy(&output.stderr), 40);
            return Ok(Outcome::Failed { diagnostics, log });
        }

        rewrite_index(&staging.join("index.html"))?;
        let _ = fs::remove_dir_all(&bundle);
        fs::rename(&staging, &bundle)
            .with_context(|| format!("moving the bundle into {}", bundle.display()))?;
        self.evict_old_bundles();
        Ok(built(ms, false, diagnostics))
    }

    /// Replace the working copy's `src/app.rs` + `src/app/` with the
    /// submission. Clearing first matters: a file dropped from the
    /// submission must not linger and satisfy a stale `mod` line.
    fn write_app_files(&self, files: &BTreeMap<String, String>) -> Result<()> {
        let src = self.work_dir().join("src");
        let _ = fs::remove_file(src.join(ENTRY_FILE));
        let _ = fs::remove_dir_all(src.join("app"));
        for (rel, contents) in files {
            let path = if rel == ENTRY_FILE { src.join(ENTRY_FILE) } else { src.join("app").join(rel) };
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
        }
        Ok(())
    }

    fn evict_old_bundles(&self) {
        let Ok(entries) = fs::read_dir(self.compiled_dir()) else { return };
        let mut bundles: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        if bundles.len() <= MAX_CACHED_BUNDLES {
            return;
        }
        bundles.sort();
        let excess = bundles.len() - MAX_CACHED_BUNDLES;
        for (_, path) in bundles.into_iter().take(excess) {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// Reject anything that is not a small set of relative `.rs` paths with
/// the entry file present. Paths are joined under `.work/src/`, so `..`,
/// absolute paths and empty segments are refused outright.
fn validate(files: &BTreeMap<String, String>) -> Result<()> {
    if !files.contains_key(ENTRY_FILE) {
        bail!("the submission needs an `{ENTRY_FILE}` that defines `pub fn app() -> Element`");
    }
    if files.len() > MAX_FILES {
        bail!("at most {MAX_FILES} files per submission");
    }
    let total: usize = files.values().map(String::len).sum();
    if total > MAX_TOTAL_BYTES {
        bail!("the submission is {total} bytes; the limit is {MAX_TOTAL_BYTES}");
    }
    for rel in files.keys() {
        let segments_ok = rel
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != ".." && !seg.contains('\\'));
        if !segments_ok || !rel.ends_with(".rs") || (rel != ENTRY_FILE && rel.starts_with("app.rs/")) {
            bail!("invalid path {rel:?}: use relative `.rs` paths like `widgets.rs` or `ui/card.rs`");
        }
    }
    Ok(())
}

/// What the framework source looks like right now: `HEAD` plus a digest
/// of uncommitted changes under `crates/` and the lockfile. Editing the
/// framework changes the stamp, so a cached bundle built against the old
/// source is never served for it. Outside a git checkout the stamp is
/// empty and the cache lives until it is cleared.
fn framework_stamp(workspace: Option<&Path>) -> String {
    let Some(ws) = workspace else { return String::new() };
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(ws)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
            .unwrap_or_default()
    };
    let mut h = Sha256::new();
    h.update(git(&["rev-parse", "HEAD"]));
    h.update(git(&["diff", "HEAD", "--", "crates", "Cargo.lock"]));
    h.update(git(&["ls-files", "--others", "--exclude-standard", "--", "crates"]));
    hex(&h.finalize()[..8])
}

/// The building CLI's identity: its size and modification time. The CLI
/// writes the served page and drives the pipeline, so a rebuilt binary can
/// change a bundle even when no framework source did (and its source can
/// be edited before the binary is rebuilt). A bare name on `PATH` is
/// resolved first.
fn cli_stamp(cli: &Path) -> String {
    let resolved = if cli.components().count() > 1 {
        Some(cli.to_path_buf())
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).map(|dir| dir.join(cli)).find(|p| p.is_file())
        })
    };
    resolved
        .and_then(|p| fs::metadata(p).ok())
        .map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{}:{modified}", m.len())
        })
        .unwrap_or_default()
}

fn bundle_hash(files: &BTreeMap<String, String>, stamp: &str) -> String {
    let mut h = Sha256::new();
    h.update(stamp.as_bytes());
    for (path, contents) in files {
        h.update(b"\0");
        h.update(path.as_bytes());
        h.update(b"\0");
        h.update(contents.as_bytes());
    }
    hex(&h.finalize()[..10])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The snippet's own diagnostics from the build's events file (one JSON
/// envelope per line, `idealyst build --events-file`), with paths mapped
/// back to the submitted names. Errors come first.
fn snippet_diagnostics(events: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("diagnostic"))
        .filter_map(|v| v.get("diagnostic").cloned())
        .filter(|d| {
            d.get("package")
                .and_then(|p| p.as_str())
                .is_some_and(|p| p.contains(&format!("#{SNIPPET_PACKAGE}@")))
        })
        .filter_map(|d| {
            let level = d.get("level")?.as_str()?.to_string();
            if level != "error" && level != "warning" {
                return None;
            }
            let file = d.get("file").and_then(|f| f.as_str()).map(submitted_path);
            Some(Diagnostic {
                level,
                message: d.get("message")?.as_str()?.to_string(),
                code: d.get("code").and_then(|c| c.as_str()).map(String::from),
                file,
                line: d.get("line").and_then(|l| l.as_u64()).map(|l| l as u32),
                column: d.get("column").and_then(|c| c.as_u64()).map(|c| c as u32),
                rendered: d
                    .get("rendered")
                    .and_then(|r| r.as_str())
                    .map(|r| r.replace("src/app/", "").replace("src/app.rs", ENTRY_FILE))
                    .unwrap_or_default(),
            })
        })
        .collect();
    // A summary line ("aborting due to 2 previous errors") has no span and
    // repeats what the real errors say.
    out.retain(|d| d.file.is_some() || !d.message.starts_with("aborting due to"));
    out.sort_by_key(|d| d.level != "error");
    out
}

/// `src/app.rs` → `app.rs`; `src/app/widgets.rs` → `widgets.rs`.
fn submitted_path(build_path: &str) -> String {
    if build_path == "src/app.rs" {
        ENTRY_FILE.to_string()
    } else {
        build_path.strip_prefix("src/app/").unwrap_or(build_path).to_string()
    }
}

/// The staged `index.html` assumes it is served at the site root
/// (`<base href="/">`, `/pkg/...`). A bundle is served from
/// `/compiled/<hash>/`, so its references become relative, and the error
/// bridge goes in ahead of the app's own script.
fn rewrite_index(path: &Path) -> Result<()> {
    let html = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let html = served_index(&html);
    fs::write(path, html).with_context(|| format!("writing {}", path.display()))
}

fn served_index(html: &str) -> String {
    let html = html
        .lines()
        .filter(|l| !l.trim_start().starts_with("<base href=\"/\""))
        .collect::<Vec<_>>()
        .join("\n");
    // `./`, not bare `pkg/`: an ES module import of a bare path is a
    // package specifier and fails to resolve.
    let html = html.replace("\"/pkg/", "\"./pkg/").replace("'/pkg/", "'./pkg/");
    match html.find("</head>") {
        Some(at) => format!("{}{ERROR_BRIDGE}{}", &html[..at], &html[at..]),
        None => html,
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_submission_needs_the_entry_file_and_safe_relative_paths() {
        assert!(validate(&files(&[("app.rs", "")])).is_ok());
        assert!(validate(&files(&[("app.rs", ""), ("ui/card.rs", "")])).is_ok());
        assert!(validate(&files(&[("lib.rs", "")])).is_err());
        for bad in ["../x.rs", "/etc/x.rs", "a//b.rs", "x.txt", "./x.rs"] {
            assert!(validate(&files(&[("app.rs", ""), (bad, "")])).is_err(), "{bad} refused");
        }
    }

    #[test]
    fn the_hash_follows_the_files_and_the_framework_stamp() {
        let a = files(&[("app.rs", "fn a() {}")]);
        let b = files(&[("app.rs", "fn b() {}")]);
        assert_eq!(bundle_hash(&a, "s1"), bundle_hash(&a, "s1"));
        assert_ne!(bundle_hash(&a, "s1"), bundle_hash(&b, "s1"));
        assert_ne!(bundle_hash(&a, "s1"), bundle_hash(&a, "s2"));
    }

    #[test]
    fn diagnostics_keep_the_snippets_own_and_name_the_submitted_files() {
        let events = [
            r#"{"v":1,"seq":1,"at_ms":0,"type":"diagnostic","target":"web","diagnostic":{"level":"warning","message":"unused import","file":"crates/x/src/lib.rs","line":1,"column":1,"package":"path+file:///w/crates/x#runtime-x@1.0.0","rendered":"w"}}"#,
            r#"{"v":1,"seq":2,"at_ms":0,"type":"diagnostic","target":"web","diagnostic":{"level":"warning","message":"unused variable","file":"src/app/widgets.rs","line":3,"column":9,"package":"path+file:///w/.work#fiddle-snippet@0.0.1","rendered":"--> src/app/widgets.rs:3:9"}}"#,
            r#"{"v":1,"seq":3,"at_ms":0,"type":"diagnostic","target":"web","diagnostic":{"level":"error","message":"cannot find value `x`","code":"E0425","file":"src/app.rs","line":8,"column":75,"package":"path+file:///w/.work#fiddle-snippet@0.0.1","rendered":"--> src/app.rs:8:75"}}"#,
            r#"{"v":1,"seq":4,"at_ms":0,"type":"diagnostic","target":"web","diagnostic":{"level":"error","message":"aborting due to 1 previous error","package":"path+file:///w/.work#fiddle-snippet@0.0.1","rendered":"a"}}"#,
            r#"{"v":1,"seq":5,"at_ms":0,"type":"cargo_progress"}"#,
        ]
        .join("\n");
        let d = snippet_diagnostics(&events);
        assert_eq!(d.len(), 2, "framework warnings and the abort summary are dropped");
        assert_eq!(d[0].level, "error", "errors first");
        assert_eq!(d[0].file.as_deref(), Some("app.rs"));
        assert_eq!((d[0].line, d[0].column), (Some(8), Some(75)));
        assert_eq!(d[0].rendered, "--> app.rs:8:75");
        assert_eq!(d[1].file.as_deref(), Some("widgets.rs"));
    }

    #[test]
    fn the_served_index_is_relative_and_carries_the_error_bridge() {
        let staged = "<html>\n  <head>\n    <base href=\"/\" />\n    <link href=\"/pkg/premint.css\">\n  </head>\n  <body>\n    <script type=\"module\">import init from \"/pkg/app.js\"; init();</script>\n  </body>\n</html>";
        let served = served_index(staged);
        assert!(!served.contains("<base"));
        assert!(served.contains("\"./pkg/premint.css\""));
        assert!(served.contains("from \"./pkg/app.js\""));
        assert!(served.find("idealyst-fiddle").unwrap() < served.find("</head>").unwrap());
    }
}
