//! `idealyst test` — prepare the environment for the project's Rust E2E tests
//! and run them on any platform.
//!
//! The tests are ordinary `#[robot_test]` functions (in `tests/robot.rs` by
//! default) that drive the app over the Robot relay. `cargo test` can run them
//! directly, but only `idealyst test` sets up what they need: it launches the
//! app on the chosen platform (`--web`/`--macos`/`--ios`/`--android`), stands up
//! the relay, waits for the app to come up, then runs `cargo test` with
//! `IDEALYST_ROBOT_BRIDGE` pointed at it. Without that prep the tests skip;
//! with it they run for real. Exit code is `cargo test`'s.

use anyhow::{Context, Result};
use clap::Parser;
use robot_test::{default_apps_dir, discover, discover_all, RobotClient};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Chromium-family browsers the parity runner will drive headlessly, in
/// preference order. `IDEALYST_CHROME` overrides the search entirely.
///
/// This used to be a single hardcoded macOS bundle path, which meant
/// `--parity web,<anything>` on Linux always reported "no headless browser
/// found" and left the web app waiting for a client that never dialled — the web
/// half of a parity run simply could not start off a mac. Every candidate below
/// takes the same `--headless=new` flags.
const CHROME_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/brave-browser",
    "/usr/bin/brave",
    "/usr/bin/microsoft-edge",
];

/// The headless browser to drive: `IDEALYST_CHROME` if set (honored even when
/// it names a bare command on `PATH`), else the first existing
/// [`CHROME_CANDIDATES`] entry.
fn headless_browser() -> Option<String> {
    if let Ok(explicit) = std::env::var("IDEALYST_CHROME") {
        // A path must exist; a bare command name is left to the OS to resolve,
        // so `IDEALYST_CHROME=chromium` works too.
        if !explicit.contains('/') || Path::new(&explicit).exists() {
            return Some(explicit);
        }
        eprintln!("[parity] IDEALYST_CHROME={explicit} does not exist — falling back to a search");
    }
    CHROME_CANDIDATES
        .iter()
        .find(|c| Path::new(c).exists())
        .map(|c| c.to_string())
}

#[derive(Parser, Debug)]
pub struct Args {
    /// Project directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Cargo test target to run (the file `tests/<name>.rs`).
    #[arg(long, default_value = "robot")]
    pub test: String,
    /// Only run tests whose name contains this filter (passed to `cargo test`).
    #[arg(long, value_name = "FILTER")]
    pub filter: Option<String>,
    /// Test against web (the default).
    #[arg(long)]
    pub web: bool,
    /// Test against the macOS app.
    #[arg(long)]
    pub macos: bool,
    /// Test against the iOS simulator.
    #[arg(long)]
    pub ios: bool,
    /// Test against the Android emulator.
    #[arg(long)]
    pub android: bool,
    /// Attach to an already-running app (started with `idealyst dev`) instead of
    /// launching one.
    #[arg(long)]
    pub attach: bool,
    /// Web dev-server port to launch on.
    #[arg(long, default_value_t = 8765)]
    pub port: u16,
    /// Run a cross-platform **render-parity** check across these platforms
    /// (comma list, e.g. `web,macos`). Launches each app, then runs the parity
    /// test target with `IDEALYST_<PLATFORM>_BRIDGE` pointed at each — the test
    /// captures every element's platform-native render state and diffs them.
    /// Defaults the test target to `parity` (override with `--test`).
    #[arg(long, value_name = "PLATFORMS", value_delimiter = ',')]
    pub parity: Vec<String>,
    /// Logical viewport size `WxH` to render every platform at, so responsive
    /// layout doesn't make the trees diverge. Pins the headless browser window
    /// and (via `IDEALYST_WINDOW_SIZE`) the macOS window. Default `1280x800`.
    #[arg(long, value_name = "WxH", default_value = "1280x800")]
    pub viewport: String,
}

/// Parse a `WxH` viewport string into `(width, height)`.
fn parse_viewport(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .with_context(|| format!("--viewport must be WxH (e.g. 1280x800), got {s:?}"))?;
    Ok((w.trim().parse()?, h.trim().parse()?))
}

/// Kills its child processes (dev session + headless browser) on drop.
struct Kill(Vec<Child>);
impl Drop for Kill {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve project dir {}", args.dir.display()))?;

    if !args.parity.is_empty() {
        return run_parity(&args, &dir);
    }

    let test_file = dir.join("tests").join(format!("{}.rs", args.test));
    anyhow::ensure!(
        test_file.is_file(),
        "no test target at {} — write `#[robot_test]` functions there (or pass --test <name>)",
        test_file.display()
    );

    let platform = if args.macos {
        "macos"
    } else if args.ios {
        "ios"
    } else if args.android {
        "android"
    } else {
        "web"
    };

    let viewport = parse_viewport(&args.viewport)?;
    let mut kill = Kill(Vec::new());
    if !args.attach {
        // Launch the app by spawning a `dev` session of THIS binary. Robot is on
        // by default, so the app dials the relay; killing the child tears it down.
        let self_exe = std::env::current_exe().context("locating the idealyst binary")?;
        eprintln!("[test] launching {platform} app via `idealyst dev`…");
        let mut cmd = Command::new(&self_exe);
        cmd.arg("dev").arg("--local").arg(format!("--{platform}"));
        if platform == "web" {
            cmd.args(["--port", &args.port.to_string()]);
        }
        // Mark the app as externally-driven so it suppresses any in-app self-test
        // that would race the suite for shared state.
        cmd.env("IDEALYST_TEST_DRIVER", "1");
        cmd.env("IDEALYST_WINDOW_SIZE", format!("{}x{}", viewport.0, viewport.1));
        cmd.arg(&dir).stdout(Stdio::null()).stderr(Stdio::null());
        kill.0.push(cmd.spawn().context("spawning `idealyst dev`")?);
    }

    // Wait for the relay registration for this project (a cold build can take a
    // while), then for web open the page headlessly so the app dials.
    let apps_dir = default_apps_dir().context("no HOME for ~/.idealyst/apps")?;
    eprintln!("[test] waiting for the app to come up…");
    let addr = wait_for_registration(&dir, &apps_dir, Duration::from_secs(300))
        .context("the app never registered with the relay (build failed? run `idealyst dev` to see)")?;

    if platform == "web" && !args.attach {
        if !wait_for_web_server(args.port, Duration::from_secs(600)) {
            anyhow::bail!(
                "the web dev server never served http://127.0.0.1:{} — the wasm \
                 build probably failed; run `idealyst dev --web` to see it",
                args.port
            );
        }
        match launch_headless_web(args.port, viewport) {
            Some(child) => kill.0.push(child),
            None => eprintln!(
                "[test] no headless browser found — open http://127.0.0.1:{} to run the web app",
                args.port
            ),
        }
    }

    // Confirm the app is actually answering before we hand the suite the bridge.
    // The relay registers as soon as `dev` hosts it — well before a cold app
    // build finishes, launches, and dials — so give the app a generous budget
    // and reconnect each attempt (the bridge isn't pingable until it dials).
    wait_until_ready(addr, Duration::from_secs(240)).context("the app did not become ready")?;

    // Run the project's tests against the live app. The `#[robot_test]` harness
    // reads `IDEALYST_ROBOT_BRIDGE`; `--test-threads=1` keeps them serialized
    // against the one shared app.
    eprintln!("[test] running `cargo test --test {}` on {platform}…\n", args.test);
    let mut cargo = Command::new("cargo");
    cargo
        .current_dir(&dir)
        .arg("test")
        .arg("--test")
        .arg(&args.test)
        .env("IDEALYST_ROBOT_BRIDGE", addr.to_string())
        .arg("--");
    if let Some(filter) = &args.filter {
        cargo.arg(filter);
    }
    cargo.args(["--test-threads=1", "--nocapture"]);

    let status = cargo
        .status()
        .context("running `cargo test` (is cargo on PATH?)")?;

    drop(kill); // tear down the dev session + browser
    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// `idealyst test --parity web,macos`: launch every named platform of the same
/// project, then run the parity test target with each app's bridge address in
/// `IDEALYST_<PLATFORM>_BRIDGE` so the test can capture + diff their
/// platform-native render trees.
fn run_parity(args: &Args, dir: &Path) -> Result<()> {
    anyhow::ensure!(
        args.parity.len() >= 2,
        "--parity needs at least two platforms to compare (e.g. --parity web,macos)"
    );
    anyhow::ensure!(
        !args.attach,
        "--parity launches its own apps; --attach isn't supported (it can't tell two \
         same-project apps apart by registration alone)"
    );

    // The parity test lives in its own target by convention; default to
    // `tests/parity.rs` unless the user pointed `--test` elsewhere.
    let test_name = if args.test == "robot" {
        "parity".to_string()
    } else {
        args.test.clone()
    };
    let test_file = dir.join("tests").join(format!("{test_name}.rs"));
    anyhow::ensure!(
        test_file.is_file(),
        "no parity test target at {} — write a `#[test]` using `robot_test::parity` there \
         (or pass --test <name>)",
        test_file.display()
    );

    // Compile the parity test FIRST, before any app is launched, and run the
    // resulting binary directly further down.
    //
    // Not an optimization — a correctness requirement. A native wrapper build
    // (`--linux`, `--macos`, …) shares the project's `target/` but runs with its
    // OWN workspace and cwd, so it records the framework path crates under
    // ABSOLUTE paths while a workspace build records them RELATIVE. Both unit
    // sets land in the same target dir, and a workspace `cargo test` issued
    // AFTER the wrapper build then mixes them:
    //
    //     error[E0308]: expected `Element`, found `Element`
    //     note: there are multiple different versions of crate `runtime_scene`
    //
    // Compiling first means the only cargo invocation that touches the
    // workspace happens before any wrapper does, so there is nothing to mix.
    let test_bin = build_parity_test(dir, &test_name)?;

    let viewport = parse_viewport(&args.viewport)?;
    let self_exe = std::env::current_exe().context("locating the idealyst binary")?;
    let apps_dir = default_apps_dir().context("no HOME for ~/.idealyst/apps")?;
    eprintln!("[parity] viewport pinned to {}x{} on every platform", viewport.0, viewport.1);

    let mut kill = Kill(Vec::new());
    // Apps already registered for this project before we launch anything — so a
    // dev session the user left running isn't mistaken for one of ours.
    let mut seen: HashSet<SocketAddr> = discover_all(Some(dir), &apps_dir).into_iter().collect();
    let mut bridges: Vec<(String, SocketAddr)> = Vec::new();

    for platform in &args.parity {
        let platform = normalize_platform(platform)?;
        eprintln!("[parity] launching {platform} app via `idealyst dev`…");
        spawn_dev_app(&self_exe, dir, &platform, args.port, viewport, &mut kill)?;

        // Wait for THIS launch's registration: the first live, project-matching
        // bridge that wasn't there before. The registration file carries no
        // platform field, so "new since launch" is how we attribute it — which
        // is why launches are sequential, not concurrent.
        let addr = wait_for_new_registration(dir, &apps_dir, &seen, Duration::from_secs(300))
            .with_context(|| {
                format!(
                    "the {platform} app never registered (build failed? \
                     run `idealyst dev --{platform}` to see)"
                )
            })?;
        seen.insert(addr);

        if platform == "web" {
            if !wait_for_web_server(args.port, Duration::from_secs(600)) {
                anyhow::bail!(
                    "the web dev server never served http://127.0.0.1:{} — the wasm \
                     build probably failed; run `idealyst dev --web` to see it",
                    args.port
                );
            }
            match launch_headless_web(args.port, viewport) {
                Some(child) => kill.0.push(child),
                None => eprintln!(
                    "[parity] no headless browser found — open http://127.0.0.1:{} so the web app dials",
                    args.port
                ),
            }
        }

        wait_until_ready(addr, Duration::from_secs(240))
            .with_context(|| format!("the {platform} app did not become ready"))?;
        eprintln!("[parity] {platform} ready at {addr}");
        bridges.push((platform, addr));

        // Absorb EVERY registration now live (not just the one we connected to)
        // into `seen`, so if this launch wrote more than one (a relay + a
        // self-host, say) the extras can't be misattributed to the next
        // platform's launch.
        seen.extend(discover_all(Some(dir), &apps_dir));
    }

    // Run the pre-built parity test with every platform's bridge in the env it
    // reads. Executing the binary rather than re-entering cargo keeps the
    // workspace build out of the picture entirely (see `build_parity_test`).
    eprintln!("\n[parity] running {}…\n", test_bin.display());
    let mut cargo = Command::new(&test_bin);
    cargo.current_dir(dir);
    for (platform, addr) in &bridges {
        cargo.env(
            format!("IDEALYST_{}_BRIDGE", platform.to_uppercase()),
            addr.to_string(),
        );
    }
    // First platform also fills IDEALYST_ROBOT_BRIDGE so single-app helpers work
    // inside a parity test if it wants them.
    if let Some((_, addr)) = bridges.first() {
        cargo.env("IDEALYST_ROBOT_BRIDGE", addr.to_string());
    }
    cargo.env(
        "IDEALYST_PARITY_PLATFORMS",
        bridges
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>()
            .join(","),
    );
    if let Some(filter) = &args.filter {
        cargo.arg(filter);
    }
    cargo.args(["--test-threads=1", "--nocapture"]);

    let status = cargo
        .status()
        .context("running `cargo test` (is cargo on PATH?)")?;
    drop(kill); // tear down every dev session + browser
    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// Spawn one platform's app as a backgrounded `idealyst dev --local` child
/// (same launch the single-platform path uses).
fn spawn_dev_app(
    self_exe: &Path,
    dir: &Path,
    platform: &str,
    port: u16,
    viewport: (u32, u32),
    kill: &mut Kill,
) -> Result<()> {
    let mut cmd = Command::new(self_exe);
    cmd.arg("dev").arg("--local").arg(format!("--{platform}"));
    if platform == "web" {
        cmd.args(["--port", &port.to_string()]);
    }
    cmd.env("IDEALYST_TEST_DRIVER", "1");
    // Pin the native window to the parity viewport (the macOS host reads this);
    // the headless browser is pinned via `--window-size` at launch.
    cmd.env("IDEALYST_WINDOW_SIZE", format!("{}x{}", viewport.0, viewport.1));
    cmd.arg(dir).stdout(Stdio::null()).stderr(Stdio::null());
    kill.0.push(cmd.spawn().context("spawning `idealyst dev`")?);
    Ok(())
}

/// Compile the parity test target and return its executable path.
///
/// Uses `--no-run --message-format=json` and takes the `executable` from the
/// compiler-artifact line for the wanted test target, which is the only
/// non-guessy way to locate a test binary (the file name carries a metadata
/// hash). See the call site for WHY this has to happen before any wrapper build.
fn build_parity_test(dir: &Path, test_name: &str) -> Result<PathBuf> {
    eprintln!("[parity] building the `{test_name}` test target…");
    let out = Command::new("cargo")
        .current_dir(dir)
        .args(["test", "--test", test_name, "--no-run", "--message-format=json"])
        .output()
        .context("running `cargo test --no-run` (is cargo on PATH?)")?;
    if !out.status.success() {
        // cargo's human-readable diagnostics went to stderr; surface them.
        anyhow::bail!(
            "building the `{test_name}` test target failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut found = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let is_wanted = v
            .get("target")
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            == Some(test_name);
        if let (true, Some(exe)) = (is_wanted, v.get("executable").and_then(|e| e.as_str())) {
            found = Some(PathBuf::from(exe));
        }
    }
    found.ok_or_else(|| {
        anyhow::anyhow!(
            "cargo reported no executable for the `{test_name}` test target — \
             is `tests/{test_name}.rs` a test target of this package?"
        )
    })
}

fn normalize_platform(p: &str) -> Result<String> {
    match p.trim().to_lowercase().as_str() {
        "web" => Ok("web".into()),
        "macos" | "mac" => Ok("macos".into()),
        "ios" => Ok("ios".into()),
        "android" => Ok("android".into()),
        // Native desktop targets. `idealyst dev` has taken `--linux` / `--windows`
        // for a while and the launcher below just forwards `--<platform>`, so the
        // only thing that kept a Linux parity run from working was this list.
        "linux" => Ok("linux".into()),
        "windows" | "win" => Ok("windows".into()),
        other => anyhow::bail!(
            "unknown parity platform {other:?} \
             (use web, macos, ios, android, linux, or windows)"
        ),
    }
}

/// Poll for a live, project-matching bridge whose address isn't already in
/// `seen` — i.e. the app the most recent launch produced.
fn wait_for_new_registration(
    project_dir: &Path,
    apps_dir: &Path,
    seen: &HashSet<SocketAddr>,
    timeout: Duration,
) -> Option<SocketAddr> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(addr) = discover_all(Some(project_dir), apps_dir)
            .into_iter()
            .find(|a| !seen.contains(a))
        {
            return Some(addr);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Poll until the app answers a ping, reconnecting each attempt — the bridge
/// isn't pingable until the app dials the relay, which can be long after the
/// relay (and thus the registration) comes up.
fn wait_until_ready(addr: SocketAddr, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(mut client) = RobotClient::connect(addr) {
            if client.wait_ready(Duration::from_secs(3)).is_ok() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("app not ready within {timeout:?}");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn wait_for_registration(
    project_dir: &Path,
    apps_dir: &Path,
    timeout: Duration,
) -> Option<SocketAddr> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(addr) = discover(Some(project_dir), apps_dir) {
            return Some(addr);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Wait until the web dev server actually SERVES the app, not merely until it
/// has registered with the relay.
///
/// Registration happens as soon as `dev` hosts the relay — before the wasm
/// build finishes. Launching the browser at that moment races the build: on a
/// cold build the page loads with no wasm, never retries, and the app never
/// dials, so the run dies 240s later reporting "the app did not become ready"
/// with a browser log that shows a perfectly healthy start. Warm builds win the
/// race, which is what made this intermittent.
fn wait_for_web_server(port: u16, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    let addr = format!("127.0.0.1:{port}");
    while std::time::Instant::now() < deadline {
        if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
            use std::io::{Read as _, Write as _};
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let req = format!("GET / HTTP/1.0\r\nHost: {addr}\r\n\r\n");
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = [0u8; 64];
                if let Ok(n) = stream.read(&mut buf) {
                    if n > 0 && String::from_utf8_lossy(&buf[..n]).contains(" 200") {
                        return true;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

fn launch_headless_web(port: u16, viewport: (u32, u32)) -> Option<Child> {
    let chrome = headless_browser()?;
    // A profile directory private to THIS runner process, wiped on the way in.
    //
    // Chrome refuses to start a second instance against a profile whose
    // `SingletonLock` still exists, and a run that was killed (Ctrl-C, a
    // timeout, a stray `pkill`) leaves that lock pointing at a dead pid. The
    // next launch then hands its URL to a singleton that isn't there and exits
    // immediately — the app never dials, and the runner blames its own 240s
    // readiness wait. One killed run silently poisoned every run after it.
    let profile = std::env::temp_dir().join(format!("idealyst-test-chrome-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    // Chrome's own diagnostics, kept rather than dropped on the floor: when the
    // browser is the thing that failed, this file is the only evidence.
    let log = profile.with_extension("log");
    let errlog = std::fs::File::create(&log).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
    Command::new(&chrome)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            // Match the native window: no scrollbar eating width, 1:1 device
            // pixels (introspection reports logical px), fixed viewport so the
            // responsive layout matches the other platform.
            "--hide-scrollbars",
            "--force-device-scale-factor=1",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--window-size={},{}", viewport.0, viewport.1))
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("http://127.0.0.1:{port}/"))
        .stdout(Stdio::null())
        .stderr(errlog)
        .spawn()
        .ok()
        .and_then(|mut child| {
            // A browser that cannot start is gone within a beat. Catching it
            // here turns a silent 240s timeout into the actual reason.
            std::thread::sleep(Duration::from_millis(750));
            match child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!(
                        "[test] the headless browser exited immediately ({status}) — see {}",
                        log.display()
                    );
                    None
                }
                _ => Some(child),
            }
        })
}
