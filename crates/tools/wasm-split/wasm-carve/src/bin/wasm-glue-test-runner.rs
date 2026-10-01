//! `wasm-glue-test-runner` — run `wasm-bindgen-test-runner` over a wasm32
//! test binary that links web-glue.
//!
//! A crate on web-glue (backend-web, in hybrid mode) imports its bindings
//! from `./__idealyst_glue.js`, and the snippets ride in the import names
//! (see `wasm_carve::glue`). wasm-bindgen-test-runner knows nothing of
//! that: left alone it would hand wasm-bindgen a module whose glue imports
//! have no JS behind them. So this wrapper does to the test binary what the
//! build does to an app (`build_web::own_glue::hybrid_pass`):
//!
//! 1. `glue::extract` — rename the glue imports, strip the records;
//! 2. write the stripped module and `__idealyst_glue.js` (the hybrid
//!    namespace, importing `initSync` from `./wasm-bindgen-test` — the
//!    runner's own specifier for its bindgen output, extension-less, which
//!    its server resolves to `.js`) into a directory next to the test
//!    binary. The specifier must be byte-identical to the runner's: ES
//!    modules are keyed by URL, so `./wasm-bindgen-test.js` would load a
//!    SECOND, never-initialized copy of the bindgen module, and every
//!    lazy attach would instantiate from `undefined` ("WebAssembly.Module():
//!    Argument 0 must be a buffer source" — the failure that found this);
//! 3. exec `wasm-bindgen-test-runner` on the stripped module with that
//!    directory as its working directory.
//!
//! Step 3's working directory is the whole trick: the runner serves its
//! generated files from a fresh tempdir and falls back to serving `.`
//! (wasm-bindgen-cli 0.2.128, `server.rs`, `try_asset(request, ".")`), so
//! the page's `import … from "./__idealyst_glue.js"` resolves to the file
//! written in step 2. Browser-mode tests only (`run_in_browser`): the
//! node runner resolves imports from the generated file's own directory.
//!
//! Moving the working directory would also hide a crate's `webdriver.json`
//! (browser capabilities — e.g. Chrome's fake media devices or its
//! autoplay policy — which wasm-bindgen-test-runner reads from its working
//! directory, cargo's package root when it runs a test), so step 2 copies it
//! into the directory too.
//!
//! A test binary with no glue at all is passed through untouched, so this
//! is safe as the workspace-wide wasm32 runner (`.cargo/config.toml`).
//! `WASM_BINDGEN_TEST_RUNNER` names a different underlying runner.
//!
//! cargo hands a wasm32 runner the libtest harness flags it would give a
//! native test binary, and wasm-bindgen-test-runner rejects the ones it
//! does not know. `cargo test -q` passes `--quiet`, which it answers with
//! `Error: unexpected argument '--quiet'` and an exit before any test runs —
//! cargo then reports only "test failed" with no result line, which read
//! as a flaky browser start-up through two release sweeps. `harness_args`
//! translates it to the runner's own spelling of terse output.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use wasm_carve::{glue, glue_js};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let wasm = PathBuf::from(args.next().context("usage: wasm-glue-test-runner <test.wasm> [args…]")?);
    let rest = harness_args(args);
    let runner = std::env::var_os("WASM_BINDGEN_TEST_RUNNER").unwrap_or_else(|| "wasm-bindgen-test-runner".into());

    let bytes = std::fs::read(&wasm).with_context(|| format!("read {}", wasm.display()))?;
    let mut extracted = glue::extract(&bytes).context("extract web-glue from the test binary")?;
    // Same as the build's hybrid pass (`build_web::own_glue::hybrid_extract`):
    // web-glue's own exports must not re-run constructors per call.
    // `IDEALYST_GLUE_KEEP_WRAPPERS=1` skips it — only to reproduce the
    // per-event constructor sweep, i.e. to see backend-web's
    // `regression_glue_dispatch_does_not_rerun_static_constructors` fail.
    let keep = std::env::var_os("IDEALYST_GLUE_KEEP_WRAPPERS").is_some();
    if !keep {
        if let Some((unwrapped, _)) = wasm_carve::command_exports::unwrap_command_exports_where(
            &extracted.wasm,
            |name| name.starts_with("__glue_"),
        )? {
            extracted.wasm = unwrapped;
        }
    }
    let mut cmd = Command::new(&runner);
    if glue_js::needs_glue_file(&extracted) {
        let stem = wasm.file_stem().context("test binary has no file name")?.to_owned();
        let dir = wasm.with_extension("glue");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let stripped = dir.join(&stem).with_extension("wasm");
        std::fs::write(&stripped, &extracted.wasm).with_context(|| format!("write {}", stripped.display()))?;
        // Minified — the release shape — so every browser test of a
        // web-glue crate also runs the minified runtime.
        let js = glue_js::hybrid_glue_js_with(&extracted, "wasm-bindgen-test", glue_js::JsLayout::Minified);
        std::fs::write(dir.join("__idealyst_glue.js"), js).context("write __idealyst_glue.js")?;
        // The package's browser capabilities, if it has any (see the module
        // docs). A stale copy from an earlier run goes when the file does.
        let caps = dir.join("webdriver.json");
        match std::env::current_dir().map(|d| d.join("webdriver.json")) {
            Ok(src) if src.is_file() => {
                std::fs::copy(&src, &caps).with_context(|| format!("copy {}", src.display()))?;
            }
            _ => {
                let _ = std::fs::remove_file(&caps);
            }
        }
        cmd.arg(&stripped).current_dir(&dir);
    } else {
        cmd.arg(&wasm);
    }
    let status = cmd
        .args(&rest)
        .status()
        .with_context(|| format!("run {} — is wasm-bindgen-cli installed?", runner.to_string_lossy()))?;
    // The runner's own exit code (test failures) is the result cargo reports.
    std::process::exit(status.code().unwrap_or(1));
}

/// The harness flags after the test binary, in wasm-bindgen-test-runner's
/// spelling: libtest's `--quiet`/`-q` is its `--format terse` (added once,
/// and not when the caller already chose a format). Everything else passes
/// through unchanged, so the runner still rejects what it cannot honor.
fn harness_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut quiet = false;
    for a in args {
        if a == "--quiet" || a == "-q" {
            quiet = true;
        } else {
            out.push(a);
        }
    }
    let has_format = out.iter().any(|a| a == "--format" || a.to_string_lossy().starts_with("--format="));
    if quiet && !has_format {
        out.extend(["--format".into(), "terse".into()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::harness_args;
    use std::ffi::OsString;

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn regression_cargo_test_quiet_becomes_terse_format() {
        assert_eq!(harness_args(args(&["--quiet"])), args(&["--format", "terse"]));
        assert_eq!(harness_args(args(&["-q", "my_filter"])), args(&["my_filter", "--format", "terse"]));
    }

    #[test]
    fn an_explicit_format_wins_and_other_flags_pass_through() {
        assert_eq!(harness_args(args(&["--quiet", "--format", "terse"])), args(&["--format", "terse"]));
        assert_eq!(harness_args(args(&["--nocapture", "--skip", "x"])), args(&["--nocapture", "--skip", "x"]));
    }
}
