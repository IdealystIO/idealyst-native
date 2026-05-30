//! Runner for the `tests/` apps. Shells out to the installed `idealyst`
//! CLI to build each test app at `--web --release` (which turns on the
//! data-segment pruning default), asserts the expected dist artifacts
//! exist, and optionally (`--browser`) drives a headless Chrome to
//! assert DOM + console state.
//!
//! Build-only smoke (always runs) catches:
//! - wasm-split-cli crashes during the post-build split pass
//! - linker errors from a chunk that lost a symbol it imported from main
//! - wasm-bindgen failures on the post-split bundle
//! - a missing `idealyst` install
//!
//! Browser smoke (`--browser`) catches:
//! - `RuntimeError: null function` from a zeroed vtable byte
//! - `panicked at :` with an empty message from a zeroed panic string
//! - the page failing to mount at all (marker text never appears)
//! - any `console.error` produced during boot or interaction
//!
//! Usage:
//!   cargo run -p prune-regression                     # build all apps
//!   cargo run -p prune-regression -- vtable-dispatch  # one app
//!   cargo run -p prune-regression -- --no-clean       # keep dist/ between runs
//!   cargo run -p prune-regression -- --browser        # add headless Chrome checks
//!   cargo run -p prune-regression -- --build-only     # skip browser even if --browser
//!                                                     # was set via env / default
//!
//! Requires `idealyst` on PATH (`cargo install --path crates/tools/cli
//! --force`). `--browser` additionally requires a system Chrome /
//! Chromium install discoverable by `headless_chrome`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[cfg(feature = "browser")]
mod browser;

/// One test app's config.
struct AppCfg {
    /// Directory under `tests/`.
    dir: &'static str,
    /// wasm-bindgen output stem (cargo's `name`-with-underscores rule).
    wasm_stem: &'static str,
    /// Substring that must be present in the page's rendered text once
    /// the app has fully mounted. Used by `--browser` to know "I've
    /// waited long enough for the wasm to load and render."
    expected_marker: &'static str,
    /// How long to wait for `expected_marker` to appear, in ms. The
    /// lazy-chunk-handoff app needs longer because the marker is inside
    /// the chunk and we have to wait for the chunk fetch + instantiate.
    marker_wait_ms: u64,
}

const APPS: &[AppCfg] = &[
    AppCfg {
        dir: "vtable-dispatch",
        wasm_stem: "vtable_dispatch_test",
        // From src/lib.rs's expected output: greet line is the most
        // specific (proves all three Greet impls dispatched).
        expected_marker: "greet: hello hola bonjour",
        marker_wait_ms: 8_000,
    },
    AppCfg {
        dir: "theme-swap",
        wasm_stem: "theme_swap_test",
        // Initial render shows "theme: light" — proves the rx! body
        // evaluated and the cohort driver wrote tokens.
        expected_marker: "theme: light",
        marker_wait_ms: 8_000,
    },
    AppCfg {
        dir: "lazy-chunk-handoff",
        wasm_stem: "lazy_chunk_handoff_test",
        // This text lives inside the lazy! body, so seeing it means the
        // chunk loaded, instantiated, and mounted.
        expected_marker: "Loaded from a separate wasm chunk",
        marker_wait_ms: 15_000,
    },
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let clean = !args.iter().any(|a| a == "--no-clean");
    let want_browser = args.iter().any(|a| a == "--browser");
    let no_browser = args.iter().any(|a| a == "--build-only");
    let only: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|s| s.as_str())
        .collect();

    let tests_dir = workspace_tests_dir();
    if !tests_dir.exists() {
        eprintln!("tests/ directory not found at {}", tests_dir.display());
        return ExitCode::from(2);
    }

    if Command::new("idealyst").arg("--version").output().is_err() {
        eprintln!(
            "error: `idealyst` CLI not on PATH. Install via\n  \
             cargo install --path crates/tools/cli --force\n  \
             (re-install after touching the splitter so the bin picks up changes.)"
        );
        return ExitCode::from(2);
    }

    let run_browser = want_browser && !no_browser;
    #[cfg(not(feature = "browser"))]
    if run_browser {
        eprintln!(
            "error: --browser passed but this binary was built without the `browser` \
             feature. Rebuild with `--features browser` (default) or drop --browser."
        );
        return ExitCode::from(2);
    }

    let mut failed: Vec<&str> = Vec::new();
    for app in APPS {
        if !only.is_empty() && !only.iter().any(|a| *a == app.dir) {
            continue;
        }
        let app_dir = tests_dir.join(app.dir);
        println!("\n=== {} ===", app.dir);

        if clean {
            let dist = app_dir.join("dist");
            if dist.exists() {
                if let Err(e) = std::fs::remove_dir_all(&dist) {
                    eprintln!("  warn: could not remove {}: {}", dist.display(), e);
                }
            }
        }

        let status = Command::new("idealyst")
            .args(["build", "--web", "--release"])
            .current_dir(&app_dir)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("  FAIL: idealyst build exited with {}", s);
                failed.push(app.dir);
                continue;
            }
            Err(e) => {
                eprintln!("  FAIL: could not spawn idealyst: {}", e);
                failed.push(app.dir);
                continue;
            }
        }

        if let Err(msg) = verify_artifacts(&app_dir, app.wasm_stem) {
            eprintln!("  FAIL: {}", msg);
            failed.push(app.dir);
            continue;
        }
        println!("  build ok");

        if run_browser {
            #[cfg(feature = "browser")]
            match browser::run_browser_check(&app_dir.join("dist").join("web"), app) {
                Ok(()) => println!("  browser ok"),
                Err(e) => {
                    eprintln!("  FAIL (browser): {e}");
                    failed.push(app.dir);
                }
            }
        }
    }

    if failed.is_empty() {
        println!(
            "\nAll apps passed{}",
            if run_browser { " (build + browser)" } else { " (build)" }
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("\n{} app(s) failed: {:?}", failed.len(), failed);
        ExitCode::FAILURE
    }
}

/// Verify the build produced an `index.html` + a non-empty `.wasm` +
/// the wasm-bindgen JS shim. We don't try to introspect the wasm
/// itself here — that's the browser pass's job.
fn verify_artifacts(app_dir: &Path, wasm_stem: &str) -> Result<(), String> {
    let dist = app_dir.join("dist").join("web");
    let pkg = dist.join("pkg");
    let html = dist.join("index.html");
    let wasm = pkg.join(format!("{wasm_stem}_bg.wasm"));
    let js = pkg.join(format!("{wasm_stem}.js"));

    if !html.exists() {
        return Err(format!("missing {}", html.display()));
    }
    let wasm_meta = std::fs::metadata(&wasm)
        .map_err(|e| format!("missing {}: {}", wasm.display(), e))?;
    if wasm_meta.len() < 1024 {
        return Err(format!(
            "{} is suspiciously small ({} bytes)",
            wasm.display(),
            wasm_meta.len()
        ));
    }
    if !js.exists() {
        return Err(format!("missing {}", js.display()));
    }
    Ok(())
}

fn workspace_tests_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap());
    manifest.parent().map(PathBuf::from).unwrap_or(manifest)
}
