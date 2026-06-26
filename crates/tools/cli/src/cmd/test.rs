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
use robot_test::{default_apps_dir, discover, RobotClient};
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
        match launch_headless_web(args.port) {
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

fn launch_headless_web(port: u16) -> Option<Child> {
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
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("http://127.0.0.1:{port}/"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}
