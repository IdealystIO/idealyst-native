//! Runner for the `tests/` apps. Shells out to the installed `idealyst`
//! CLI to build each test app at `--web --release --data-prune` (data
//! pruning is off by default now — it corrupts main.wasm on apps with
//! indirect main→data reachability — so this suite opts in explicitly to
//! exercise it on pure-data fixtures), asserts the expected dist artifacts
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
//!
//! # Measurements performed
//!
//! 1. **Build smoke** (every app in [`APPS`]): `idealyst build --web
//!    --release --data-prune` must exit 0.
//! 2. **Artifact shape** ([`verify_artifacts`], every app): `index.html`
//!    exists, a non-empty (> 1 KiB) `{stem}_bg[.hash].wasm` exists, and a
//!    matching `{stem}[.hash].js` shim exists.
//! 3. **Browser smoke** (`--browser`, every app): the page mounts, the
//!    app's `expected_marker` text appears, and no `console.error` fires.
//! 4. **Handler-registration main.wasm delta** ([`measure_chunk_split`],
//!    the `lazy-payload-split` pair only): the `lazy` variant's
//!    `main.wasm` must be at least [`MIN_MAIN_SHRINK_BYTES`] smaller than
//!    `eager`'s.
//!
//! # What the size gate measures
//!
//! **Where a third-party payload's mount handler is registered**, and
//! whether that choice actually moves bytes.
//!
//! The `lazy-payload-split` pair is two apps that are identical — same
//! `app()`, same rendered tree, same `#[component(lazy)]` chunk body —
//! except for one line in `register_scene_extensions`:
//!
//! - `eager/`: `heavy::register(registry)` installs the handler at the
//!   boot seam, so `main.wasm` statically reaches it and the 512 KiB
//!   static it touches.
//! - `lazy/`: `registry.defer::<heavy::HeavyProps>()` only DECLARES the
//!   payload kind late-bound (a compile-time `TypeId` and nothing else);
//!   the handler installs itself from inside the chunk through
//!   `runtime_scene::defer_registration` → `Registry::register_deferred`.
//!   Realize meets the payload before the handler exists, parks it behind
//!   a placeholder, and completes the mount in place when the chunk
//!   lands.
//!
//! So a passing gate asserts three mechanisms at once: `runtime-scene`'s
//! post-boot registration seam genuinely keeps a handler out of the boot
//! module, wasm-split places the chunk-only symbol outside `main.wasm`,
//! and `--data-prune` evicts the now-unreachable static from main's data
//! segments. A regression in any one of them collapses the delta.
//!
//! This is the axis the fixtures measured before runtime v2 as well (on
//! the old `Element::External` + `defer_external_registration` seam,
//! which recorded 1294 KiB → 781 KiB). Runtime v2 deleted that seam and
//! the pair briefly degraded to measuring plain call-site reachability;
//! `runtime_scene::Registry::defer` / `register_deferred` restored the
//! capability, and this gate measures it again.

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
        // This text lives inside the fixture's `lazy!` body (that app
        // deliberately still exercises the deprecated block form), so
        // seeing it means the chunk loaded, instantiated, and mounted.
        expected_marker: "Loaded from a separate wasm chunk",
        marker_wait_ms: 15_000,
    },
    AppCfg {
        dir: "lazy-many-splits",
        wasm_stem: "lazy_many_splits_test",
        // Rendered from INSIDE page 0's chunk, dispatched through the
        // fixture's static fn-pointer catalog. Absent if main traps at
        // boot or the chunk handoff breaks with dozens of split points.
        // Shape coverage for the 2026-08 duplicate-mangled-name
        // misclassification (see the fixture's lib.rs: the collision
        // loser is hash-order luck, so the deterministic guard is the
        // emit_main_module tripwire; this keeps the many-split
        // fn-pointer shape exercised end-to-end).
        expected_marker: "chunk page #0 mounted",
        marker_wait_ms: 15_000,
    },
    // The handler-registration pair. The point of these two is the
    // main.wasm SIZE DELTA asserted by `measure_chunk_split` after the
    // build loop — but the marker is load-bearing too: "heavy payload
    // byte:" is rendered BY THE HANDLER, so seeing it proves the handler
    // ran. In the lazy variant that means the item `app()` parked was
    // drained in place after the chunk installed its handler, i.e. the
    // seam the size delta credits actually works at runtime.
    AppCfg {
        dir: "lazy-payload-split/eager",
        wasm_stem: "lazy_payload_split_eager",
        expected_marker: "heavy payload byte:",
        marker_wait_ms: 10_000,
    },
    AppCfg {
        dir: "lazy-payload-split/lazy",
        wasm_stem: "lazy_payload_split_lazy",
        // Longer: here the marker cannot appear until the chunk has been
        // fetched, instantiated, and its registration drained.
        expected_marker: "heavy payload byte:",
        marker_wait_ms: 15_000,
    },
];

/// Least main.wasm shrinkage we require between the eager and lazy
/// variants. The heavy SDK's payload is 512 KiB; a delta above ~400 KiB
/// proves the payload left main (the slack absorbs wasm-opt variance and
/// the handful of bytes the extra `app()` call site itself adds). Well
/// below 512 to stay robust; far above build-to-build noise (single-KB)
/// to be a real signal.
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

        // `--data-prune` is required: chunk-only data pruning is OFF by default
        // (its classification under-approximates main's reachability and
        // corrupts main.wasm on real apps). This suite specifically exercises
        // the prune, so it opts in explicitly — the fixtures here are pure-data
        // payloads with no indirect main→data reachability, the case the
        // heuristic handles correctly.
        let status = Command::new("idealyst")
            .args(["build", "--web", "--release", "--data-prune"])
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

    // Chunk-split measurement: when both variants built this run, diff
    // their main.wasm and require the lazy one to be meaningfully smaller
    // — the heavy SDK's payload must have left main.wasm.
    let eager = "lazy-payload-split/eager";
    let lazy = "lazy-payload-split/lazy";
    if built.contains(&eager) && built.contains(&lazy) {
        println!("\n=== chunk-split main.wasm delta ===");
        match measure_chunk_split(&tests_dir) {
            Ok(()) => println!("  size delta ok"),
            Err(e) => {
                eprintln!("  FAIL: {e}");
                failed.push("lazy-payload-split (size delta)");
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
    verify_no_build_machine_paths(&wasm)?;
    Ok(())
}

/// A release bundle must not disclose the build machine's filesystem.
///
/// Panic `Location`s (`file!()` behind every `unwrap`/`expect`/bounds check)
/// live in the wasm's `.rodata` — they are NOT debug info, so `wasm-opt
/// --strip-debug` leaves them intact. The generated wrapper spells framework
/// deps as absolute paths, which used to promote every framework `file!()` to
/// an absolute build-machine path; a deployed bundle then shipped the
/// builder's home directory, username, toolchain version, and dependency
/// inventory to every client. `cargo_build_wasm` now passes
/// `--remap-path-prefix` on release builds (`build_ios::remap_path_flags`).
///
/// Checked on the wasm only: the wasm-bindgen JS shim, the `__wasm_split`
/// loader, and `index.html` were all verified clean — the disclosure is
/// specific to the wasm's data section.
///
/// `$HOME` catches the repo, `~/.cargo`, and `~/.rustup` in one needle; the
/// framework workspace root is checked separately so a checkout outside
/// `$HOME` is still covered.
fn verify_no_build_machine_paths(wasm: &Path) -> Result<(), String> {
    let bytes =
        std::fs::read(wasm).map_err(|e| format!("reading {}: {}", wasm.display(), e))?;
    match find_leaked_path(&bytes, &build_machine_needles()) {
        Some(found) => Err(format!(
            "{} leaks the build machine's absolute paths (found {:?} in the \
             wasm data section). Release builds must pass --remap-path-prefix; \
             see build_ios::remap_path_flags.",
            wasm.display(),
            found,
        )),
        None => Ok(()),
    }
}

/// Path prefixes that must never appear in a shipped bundle. `$HOME` covers
/// the checkout, `~/.cargo`, and `~/.rustup` in one needle; the framework
/// workspace root is added separately so a checkout outside `$HOME` is
/// covered too.
fn build_machine_needles() -> Vec<String> {
    let mut needles: Vec<String> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().trim_end_matches('/').to_string();
        // Guard against a degenerate `HOME` (`/`, or set-but-empty) matching
        // essentially any path byte in the module.
        if home.len() > 1 {
            needles.push(home);
        }
    }
    if let Some(root) = workspace_tests_dir().parent() {
        let root = root.display().to_string();
        if root.len() > 1 && !needles.iter().any(|n| root.starts_with(n.as_str())) {
            needles.push(root);
        }
    }
    needles
}

/// First needle occurring anywhere in `bytes`, if any.
fn find_leaked_path<'a>(bytes: &[u8], needles: &'a [String]) -> Option<&'a str> {
    needles.iter().find_map(|needle| {
        let n = needle.as_bytes();
        (!n.is_empty() && n.len() <= bytes.len() && bytes.windows(n.len()).any(|w| w == n))
            .then_some(needle.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins that the check actually fires. Before the `--remap-path-prefix`
    /// fix, a release `baseline` bundle carried 80 absolute build-machine
    /// paths; after it, zero. This reproduces both shapes over the same
    /// matcher so the assertion can't silently degrade into a no-op.
    #[test]
    fn detects_leaked_build_machine_path() {
        let needles = vec!["/Users/somebody".to_string()];

        // Shape of a pre-fix bundle: a panic message followed by its
        // `core::panic::Location` file string.
        let leaky = b"wrap fragment trees in a view\0\
                      /Users/somebody/idealyst-native/crates/runtime/scene/src/registry.rs";
        assert_eq!(
            find_leaked_path(leaky, &needles),
            Some("/Users/somebody"),
            "must catch an absolute build-machine path",
        );

        // Shape of a post-fix bundle: same data, remapped.
        let clean = b"wrap fragment trees in a view\0\
                      /idealyst/crates/runtime/scene/src/registry.rs";
        assert_eq!(
            find_leaked_path(clean, &needles),
            None,
            "remapped paths must pass",
        );
    }

    /// A short or degenerate `$HOME` must not be turned into a needle that
    /// matches arbitrary bytes — that would fail every bundle.
    #[test]
    fn degenerate_home_is_not_used_as_a_needle() {
        let needles: Vec<String> = vec![];
        assert_eq!(find_leaked_path(b"/anything/at/all", &needles), None);
        // An empty needle must never count as a hit.
        assert_eq!(
            find_leaked_path(b"/anything", &["".to_string()]),
            None,
            "empty needle must not match",
        );
    }
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

/// Compare the two variants' main bundles. The eager app registers the
/// heavy SDK's mount handler at the boot seam, anchoring its 512 KiB
/// payload in `main.wasm`; the lazy app only DECLARES the payload kind
/// late-bound and lets the handler install itself from inside the
/// `#[component(lazy)]` chunk, so wasm-split confines it to the chunk and
/// the release data-prune drops the static from main. Assert the lazy
/// main is smaller by at least [`MIN_MAIN_SHRINK_BYTES`], and print both
/// sizes + the delta so a regression (or a win) is legible.
///
/// This is the runner's ONLY bundle-size assertion. It covers the scene's
/// post-boot registration seam, wasm-split chunk placement, and
/// data-prune eviction together (see the module docs).
fn measure_chunk_split(tests_dir: &Path) -> Result<(), String> {
    let eager = main_wasm_bytes(tests_dir, "lazy-payload-split/eager", "lazy_payload_split_eager")?;
    let lazy = main_wasm_bytes(tests_dir, "lazy-payload-split/lazy", "lazy_payload_split_lazy")?;

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
             late handler registration failed to keep the heavy SDK out of main",
            lazy / 1024,
            eager / 1024,
        ));
    }
    if delta < MIN_MAIN_SHRINK_BYTES {
        return Err(format!(
            "main.wasm shrank only {} KiB (< {} KiB) — the heavy payload did not \
             fully leave main; check that nothing in main names the SDK's handler \
             (`Registry::defer` must be the only mention), then wasm-split's chunk \
             reachability and the data-prune classification",
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
