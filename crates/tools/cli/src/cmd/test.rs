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

const DEFAULT_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

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

    // Run the parity test with every platform's bridge in the env it reads.
    eprintln!("\n[parity] running `cargo test --test {test_name}`…\n");
    let mut cargo = Command::new("cargo");
    cargo.current_dir(dir).arg("test").arg("--test").arg(&test_name);
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
    cargo.arg("--");
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

fn normalize_platform(p: &str) -> Result<String> {
    match p.trim().to_lowercase().as_str() {
        "web" => Ok("web".into()),
        "macos" | "mac" => Ok("macos".into()),
        "ios" => Ok("ios".into()),
        "android" => Ok("android".into()),
        other => {
            anyhow::bail!("unknown parity platform {other:?} (use web, macos, ios, or android)")
        }
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

fn launch_headless_web(port: u16, viewport: (u32, u32)) -> Option<Child> {
    let chrome = std::env::var("IDEALYST_CHROME").unwrap_or_else(|_| DEFAULT_CHROME.to_string());
    if !Path::new(&chrome).exists() {
        return None;
    }
    let profile = std::env::temp_dir().join("idealyst-test-chrome");
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
        .stderr(Stdio::null())
        .spawn()
        .ok()
}
