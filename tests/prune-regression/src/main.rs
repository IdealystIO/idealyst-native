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
    // The registration-split pair. Their H2 headings live in the MAIN
    // bundle, so the marker only confirms mount; the point of these two is
    // the main.wasm SIZE DELTA asserted by `measure_registration_split`
    // after the build loop, not their DOM.
    AppCfg {
        dir: "lazy-external-split/eager",
        wasm_stem: "lazy_external_split_eager",
        expected_marker: "eager registration",
        marker_wait_ms: 10_000,
    },
    AppCfg {
        dir: "lazy-external-split/lazy",
        wasm_stem: "lazy_external_split_lazy",
        expected_marker: "lazy registration",
        marker_wait_ms: 10_000,
    },
];

/// Least main.wasm shrinkage we require between the eager and lazy
/// variants. The heavy SDK's payload is 512 KiB; a delta above ~400 KiB
/// proves the payload left main (the slack absorbs wasm-opt variance and
/// the few bytes the deferral seam itself adds). Well below 512 to stay
/// robust; far above build-to-build noise (single-KB) to be a real signal.
const MIN_MAIN_SHRINK_BYTES: u64 = 400 * 1024;

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
    let mut built: Vec<&str> = Vec::new();
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
        built.push(app.dir);
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

    // Registration-split measurement: when both variants built this run,
    // diff their main.wasm and require the lazy one to be meaningfully
    // smaller — the heavy SDK's payload must have left main.wasm.
    let eager = "lazy-external-split/eager";
    let lazy = "lazy-external-split/lazy";
    if built.contains(&eager) && built.contains(&lazy) {
        println!("\n=== registration-split main.wasm delta ===");
        match measure_registration_split(&tests_dir) {
            Ok(()) => println!("  size delta ok"),
            Err(e) => {
                eprintln!("  FAIL: {e}");
                failed.push("lazy-external-split (size delta)");
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

/// Verify the build produced an `index.html` + a non-empty main `.wasm` +
/// the wasm-bindgen JS shim. We don't try to introspect the wasm
/// itself here — that's the browser pass's job.
///
/// Release bundles are content-addressed: `{stem}_bg.wasm` ships as
/// `{stem}_bg.<hash>.wasm` and `{stem}.js` as `{stem}.<hash>.js` (see the
/// web-bundle fingerprinting pass). Match by prefix/suffix, not exact
/// name, so this stays correct across both fingerprinted and plain
/// bundles.
fn verify_artifacts(app_dir: &Path, wasm_stem: &str) -> Result<(), String> {
    let dist = app_dir.join("dist").join("web");
    let pkg = dist.join("pkg");
    let html = dist.join("index.html");

    if !html.exists() {
        return Err(format!("missing {}", html.display()));
    }
    let wasm = find_pkg_file(&pkg, &format!("{wasm_stem}_bg"), ".wasm")
        .ok_or_else(|| format!("missing {wasm_stem}_bg*.wasm in {}", pkg.display()))?;
    let wasm_len = std::fs::metadata(&wasm).map(|m| m.len()).unwrap_or(0);
    if wasm_len < 1024 {
        return Err(format!(
            "{} is suspiciously small ({} bytes)",
            wasm.display(),
            wasm_len
        ));
    }
    if find_pkg_file(&pkg, &format!("{wasm_stem}."), ".js").is_none()
        && find_pkg_file(&pkg, wasm_stem, ".js").is_none()
    {
        return Err(format!("missing {wasm_stem}*.js in {}", pkg.display()));
    }
    Ok(())
}

/// First file in `pkg` whose name starts with `prefix` and ends with
/// `suffix`. Used to locate content-addressed artifacts whose middle is a
/// build hash. Deterministic pick (lexicographically smallest) so repeat
/// runs agree, though in practice there is exactly one main-bundle match.
fn find_pkg_file(pkg: &Path, prefix: &str, suffix: &str) -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = std::fs::read_dir(pkg)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(prefix) && n.ends_with(suffix))
                .unwrap_or(false)
        })
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// Compare the two variants' main bundles. The eager app anchors the
/// heavy SDK's 512 KiB payload in `main.wasm`; the lazy app defers
/// registration into the chunk so the release data-prune drops it. Assert
/// the lazy main is smaller by at least [`MIN_MAIN_SHRINK_BYTES`], and
/// print both sizes + the delta so a regression (or a win) is legible.
fn measure_registration_split(tests_dir: &Path) -> Result<(), String> {
    let eager = main_wasm_bytes(tests_dir, "lazy-external-split/eager", "lazy_external_split_eager")?;
    let lazy = main_wasm_bytes(tests_dir, "lazy-external-split/lazy", "lazy_external_split_lazy")?;

    let delta = eager.saturating_sub(lazy);
    println!("  eager main.wasm: {} KiB", eager / 1024);
    println!("  lazy  main.wasm: {} KiB", lazy / 1024);
    println!(
        "  lazy is {} KiB smaller (need \u{2265} {} KiB)",
        delta / 1024,
        MIN_MAIN_SHRINK_BYTES / 1024,
    );

    if lazy >= eager {
        return Err(format!(
            "lazy main.wasm ({} KiB) is not smaller than eager ({} KiB) — \
             deferred registration failed to keep the heavy SDK out of main",
            lazy / 1024,
            eager / 1024,
        ));
    }
    if delta < MIN_MAIN_SHRINK_BYTES {
        return Err(format!(
            "main.wasm shrank only {} KiB (< {} KiB) — the heavy payload did not \
             fully leave main; check the deferral seam or the data-prune classification",
            delta / 1024,
            MIN_MAIN_SHRINK_BYTES / 1024,
        ));
    }
    Ok(())
}

/// Byte size of one variant's main bundle (`{stem}_bg[.<hash>].wasm`).
fn main_wasm_bytes(tests_dir: &Path, app_dir: &str, wasm_stem: &str) -> Result<u64, String> {
    let pkg = tests_dir.join(app_dir).join("dist").join("web").join("pkg");
    let wasm = find_pkg_file(&pkg, &format!("{wasm_stem}_bg"), ".wasm")
        .ok_or_else(|| format!("no {wasm_stem}_bg*.wasm in {}", pkg.display()))?;
    std::fs::metadata(&wasm)
        .map(|m| m.len())
        .map_err(|e| format!("reading {}: {}", wasm.display(), e))
}

fn workspace_tests_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap());
    manifest.parent().map(PathBuf::from).unwrap_or(manifest)
}
