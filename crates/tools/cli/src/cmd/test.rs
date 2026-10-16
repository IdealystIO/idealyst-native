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
use robot_test::{default_apps_dirs, discover, discover_all, RobotClient};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Chromium-family browsers the parity runner will drive headlessly, in
/// preference order. `IDEALYST_CHROME` overrides the search entirely.
///
/// Covers macOS and Linux installs, so the web half of a parity run can
/// start off a mac. Every candidate below takes the same `--headless=new`
/// flags.
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
    /// Test against the Linux (GTK) app — on a Linux host.
    #[arg(long)]
    pub linux: bool,
    /// Test against the Windows app — on a Windows host, or on a remote
    /// Windows machine via `IDEALYST_WINDOWS_REMOTE` (see docs/windows.md).
    #[arg(long)]
    pub windows: bool,
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

/// Kills its child processes (dev session + headless browser) on drop —
/// each with its whole process group on unix, so what `idealyst dev`
/// started (cargo, an emulator install, the SSH session that keeps a remote
/// Windows app and its registration alive) goes with it. Killing only the
/// `dev` process left that SSH session running, and the next run found its
/// stale registration and drove the wrong app.
struct Kill(Vec<Child>);
impl Drop for Kill {
    fn drop(&mut self) {
        for c in &mut self.0 {
            #[cfg(unix)]
            unsafe {
                // Children are spawned as process-group leaders (`own_group`).
                libc::killpg(c.id() as libc::pid_t, libc::SIGTERM);
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Spawn `cmd` as the leader of a new process group, so [`Kill`] can stop
/// everything it starts.
fn own_group(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
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
    } else if args.linux {
        "linux"
    } else if args.windows {
        "windows"
    } else {
        "web"
    };

    let viewport = parse_viewport(&args.viewport)?;
    let apps_dirs = registry_dirs()?;
    // Registrations already live before the launch belong to another run
    // (or the app being attached to): when launching, wait for a new one.
    let preexisting: Vec<SocketAddr> = if args.attach {
        Vec::new()
    } else {
        apps_dirs.iter().flat_map(|d| discover_all(Some(&dir), d)).collect()
    };
    let mut kill = Kill(Vec::new());
    let dev_log: Option<PathBuf> = (!args.attach).then(|| dev_log_path(platform));
    let see_log = |what: &str| match &dev_log {
        Some(log) => format!("{what} — the dev session's output is in {}", log.display()),
        None => what.to_string(),
    };
    if !args.attach {
        // Launch the app by spawning a `dev` session of THIS binary. Robot is on
        // by default, so the app dials the relay; killing the child tears it down.
        let self_exe = std::env::current_exe().context("locating the idealyst binary")?;
        eprintln!("[test] launching {platform} app via `idealyst dev`…");
        let mut cmd = dev_command(&self_exe, platform, args.port, viewport);
        // The session's build + launch output goes to a log file, named when
        // the app doesn't come up — discarding it left "did not become ready"
        // with nothing to go on.
        let log = dev_log.as_ref().expect("set when not attaching");
        let file = std::fs::File::create(log).with_context(|| format!("creating {}", log.display()))?;
        cmd.arg(&dir).stdout(file.try_clone()?).stderr(file);
        eprintln!("[test] dev session log: {}", log.display());
        kill.0.push(own_group(&mut cmd).spawn().context("spawning `idealyst dev`")?);
    }

    // Wait for the relay registration for this project (a cold build can take a
    // while), then for web open the page headlessly so the app dials.
    eprintln!("[test] waiting for the app to come up…");
    let addr = wait_for_registration(&dir, &apps_dirs, &preexisting, Duration::from_secs(300))
        .with_context(|| see_log("the app never registered with the relay (build failed?)"))?;

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
    wait_until_ready(addr, Duration::from_secs(240)).with_context(|| see_log("the app did not become ready"))?;

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
    let apps_dirs = registry_dirs()?;
    eprintln!("[parity] viewport pinned to {}x{} on every platform", viewport.0, viewport.1);

    let mut kill = Kill(Vec::new());
    // Apps already registered for this project before we launch anything — so a
    // dev session the user left running isn't mistaken for one of ours.
    let mut seen: HashSet<SocketAddr> = discover_all_in(dir, &apps_dirs).into_iter().collect();
    let mut bridges: Vec<(String, SocketAddr)> = Vec::new();

    for platform in &args.parity {
        let platform = normalize_platform(platform)?;
        eprintln!("[parity] launching {platform} app via `idealyst dev`…");
        spawn_dev_app(&self_exe, dir, &platform, args.port, viewport, &mut kill)?;

        // Wait for THIS launch's registration: the first live, project-matching
        // bridge that wasn't there before. The registration file carries no
        // platform field, so "new since launch" is how we attribute it — which
        // is why launches are sequential, not concurrent.
        let addr = wait_for_new_registration(dir, &apps_dirs, &seen, Duration::from_secs(300))
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
        seen.extend(discover_all_in(dir, &apps_dirs));
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

/// The `idealyst dev --local` session a test run launches its app with
/// (the project directory is appended by the caller).
///
/// - `IDEALYST_TEST_DRIVER`: the app is externally driven, so an in-app
///   self-test stands down instead of racing the suite for shared state.
/// - `IDEALYST_WINDOW_SIZE`: the native window at the test viewport (the
///   headless browser is pinned via `--window-size` at launch).
/// - `IDEALYST_DEV_NO_RESUME`: build the sources. A web session otherwise
///   resumes the last session's base and replays its hot patches, and a
///   patch only reaches code the page calls after it lands — so the
///   suite could run code that is not the source on disk.
fn dev_command(self_exe: &Path, platform: &str, port: u16, viewport: (u32, u32)) -> Command {
    let mut cmd = Command::new(self_exe);
    cmd.arg("dev").arg("--local").arg(format!("--{platform}"));
    if platform == "web" {
        cmd.args(["--port", &port.to_string()]);
    }
    cmd.env("IDEALYST_TEST_DRIVER", "1");
    cmd.env("IDEALYST_WINDOW_SIZE", format!("{}x{}", viewport.0, viewport.1));
    cmd.env("IDEALYST_DEV_NO_RESUME", "1");
    cmd
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
    let mut cmd = dev_command(self_exe, platform, port, viewport);
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

/// The app-registry directories to scan: `~/.idealyst/apps` plus the Wine
/// one, where a `windows` app launched under Wine registers.
fn registry_dirs() -> Result<Vec<PathBuf>> {
    let dirs = default_apps_dirs();
    anyhow::ensure!(!dirs.is_empty(), "no HOME for ~/.idealyst/apps");
    Ok(dirs)
}

/// Every live, project-matching bridge across all registry directories.
fn discover_all_in(project_dir: &Path, apps_dirs: &[PathBuf]) -> Vec<SocketAddr> {
    apps_dirs.iter().flat_map(|d| discover_all(Some(project_dir), d)).collect()
}

/// Poll for a live, project-matching bridge whose address isn't already in
/// `seen` — i.e. the app the most recent launch produced.
fn wait_for_new_registration(
    project_dir: &Path,
    apps_dirs: &[PathBuf],
    seen: &HashSet<SocketAddr>,
    timeout: Duration,
) -> Option<SocketAddr> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(addr) = discover_all_in(project_dir, apps_dirs)
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

/// A live registration for THIS project that wasn't there before the
/// launch. `discover` falls back to any live app (with another dev session
/// running, the wrong one), and a registration left by an earlier run's
/// still-closing session is live too — neither is the app just launched.
fn fresh_registration(project_dir: &Path, apps_dirs: &[PathBuf], preexisting: &[SocketAddr]) -> Option<SocketAddr> {
    apps_dirs
        .iter()
        .flat_map(|d| discover_all(Some(project_dir), d))
        .find(|addr| !preexisting.contains(addr))
}

/// Where a test run's `idealyst dev` output goes: the system temp dir,
/// one file per platform and run.
fn dev_log_path(platform: &str) -> PathBuf {
    std::env::temp_dir().join(format!("idealyst-test-{platform}-{}.log", std::process::id()))
}

fn wait_for_registration(
    project_dir: &Path,
    apps_dirs: &[PathBuf],
    preexisting: &[SocketAddr],
    timeout: Duration,
) -> Option<SocketAddr> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(addr) = fresh_registration(project_dir, apps_dirs, preexisting) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A registration live before the launch is another run's (the run
    /// before, still closing its SSH session to a remote Windows app): the
    /// launcher must wait for the new one. It used the old one and drove the
    /// wrong app — the SDK gallery that held the remote bridge port.
    /// A test run builds the sources it tests: the launched dev session must
/// not resume the last session's base and replay its hot patches — that
/// ran the suite against a demo overlay that was no longer in the source.
#[test]
fn regression_the_test_session_builds_instead_of_resuming_a_patched_base() {
    let cmd = dev_command(Path::new("idealyst"), "web", 8080, (1280, 900));
    let envs: Vec<(String, String)> = cmd
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
        .collect();
    assert!(envs.contains(&("IDEALYST_DEV_NO_RESUME".into(), "1".into())), "{envs:?}");
    assert!(envs.contains(&("IDEALYST_TEST_DRIVER".into(), "1".into())), "{envs:?}");
    let argv: Vec<&str> = cmd.get_args().filter_map(|a| a.to_str()).collect();
    assert_eq!(argv, ["dev", "--local", "--web", "--port", "8080"]);
}

#[test]
    fn regression_the_launcher_ignores_registrations_from_before_its_launch() {
        use std::net::TcpListener;
        let apps = std::env::temp_dir().join(format!("idealyst-test-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&apps);
        let project = apps.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let register = |name: &str| {
            let live = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = live.local_addr().unwrap().port();
            let reg = serde_json::json!({ "name": name, "pid": 1, "port": port, "project_root": project });
            std::fs::write(apps.join(format!("{name}.json")), reg.to_string()).unwrap();
            live
        };
        let old = register("old");
        let dirs = vec![apps.clone()];
        let preexisting: Vec<SocketAddr> = discover_all(Some(&project), &apps);
        assert_eq!(preexisting.len(), 1);
        assert_eq!(fresh_registration(&project, &dirs, &preexisting), None, "the old run's app is not this one");
        let new = register("new");
        let got = fresh_registration(&project, &dirs, &preexisting).expect("the launched app");
        assert_eq!(got.port(), new.local_addr().unwrap().port());
        drop(old);
        let _ = std::fs::remove_dir_all(&apps);
    }
}
