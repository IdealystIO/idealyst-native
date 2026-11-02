//! `idealyst test` — prepare the environment for the project's Rust E2E tests
//! and run them on any platform.
//!
//! The tests are ordinary `#[robot_test]` functions (in `tests/robot.rs` by
//! default) that drive the app over the Robot relay. `cargo test` can run them
//! directly, but only `idealyst test` sets up what they need: it builds the
//! test target, runs the `--before` hooks, launches the app on the chosen
//! platform (`--web`/`--macos`/`--ios`/`--android`/`--linux`/`--windows`) from
//! a fresh build (`IDEALYST_DEV_NO_RESUME`) and marked as test-driven
//! (`IDEALYST_TEST_DRIVER`, so an in-app self-test stands down), stands up the
//! relay, waits for the app to come up, then runs the suite with
//! `IDEALYST_ROBOT_BRIDGE` pointed at it — once per requested size, device or
//! simulator — and finally the `--after` hooks. Without that prep the tests
//! skip; with it they run for real. The run fails if any size fails.

mod browser;

use anyhow::{Context, Result};
use browser::{device_preset, Emulation, HeadlessBrowser};
use clap::Parser;
use robot_test::{default_apps_dirs, discover_all, RobotClient};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
pub struct Args {
    /// Project directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Cargo test target to run (the file `tests/<name>.rs`).
    #[arg(long, default_value = "robot")]
    pub test: String,
    /// Only run tests whose name contains this filter (passed to the test binary).
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
    /// Port the test session's web server binds — on a full-stack project,
    /// the project's own server, which every platform's app talks to (it is
    /// the `IDEALYST_SERVER_URL` native apps get). Passed to `idealyst dev
    /// --port` on every platform. Default: a free port picked per run, never
    /// `[package.metadata.idealyst.app].server_port` — that one belongs to
    /// the developer's own `idealyst dev`, and a suite pointed at it would
    /// read and reset the developer's data. Tests and hooks see it as
    /// `IDEALYST_TEST_PORT`.
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,
    /// Run a cross-platform **render-parity** check across these platforms
    /// (comma list, e.g. `web,macos`). Launches each app, then runs the parity
    /// test target with `IDEALYST_<PLATFORM>_BRIDGE` pointed at each — the test
    /// captures every element's platform-native render state and diffs them.
    /// Defaults the test target to `parity` (override with `--test`). `--port`
    /// goes to the first platform; the others get free ports.
    #[arg(long, value_name = "PLATFORMS", value_delimiter = ',')]
    pub parity: Vec<String>,
    /// Viewport size(s) `WxH` to run the suite at — a comma list or repeated
    /// (`--viewport 390x844,820x1180,1440x900`) runs the whole suite once per
    /// size. Web builds and launches once and resizes the page between sizes
    /// (exactly: a DevTools metrics override, so 390 px is 390 px even though
    /// headless Chrome's window can't go below 500); desktop apps
    /// (`--macos`/`--linux`/`--windows`) relaunch at each size via
    /// `IDEALYST_WINDOW_SIZE`. iOS and Android take the device's own screen
    /// (pick devices with `--simulator`). Tests see the current size as
    /// `IDEALYST_TEST_VIEWPORT`. Default `1280x800` when no `--device` is given.
    #[arg(long, value_name = "WxH", value_delimiter = ',')]
    pub viewport: Vec<String>,
    /// Web only: emulate a named device in the headless browser — its
    /// viewport, device pixel ratio, touch input and user agent (comma list
    /// or repeated; one suite run per device, after the `--viewport` sizes).
    /// Presets: iphone-se, iphone-14, iphone-14-pro-max, iphone-15, pixel-7,
    /// galaxy-s23, ipad-mini, ipad-air, ipad-pro-12. Tests see the name as
    /// `IDEALYST_TEST_DEVICE`.
    #[arg(long, value_name = "NAME", value_delimiter = ',')]
    pub device: Vec<String>,
    /// Web only: emulate a phone at each `--viewport` size — mobile layout
    /// viewport, touch input, a 3x pixel ratio and a phone user agent.
    #[arg(long)]
    pub mobile: bool,
    /// `--ios` only: the simulator(s) to run on, by name or UDID (as
    /// `idealyst dev --simulator` takes them). A comma list or repeated flag
    /// runs the suite once per simulator, one after the other: launch, run,
    /// stop, next. Tests see the name as `IDEALYST_TEST_DEVICE`.
    #[arg(long, value_name = "NAME|UDID", value_delimiter = ',')]
    pub simulator: Vec<String>,
    /// `--android` only: the device(s) to run on, by AVD name or adb serial
    /// (as `idealyst dev --emulator` takes them). A comma list or repeated
    /// flag runs the suite once per device, one after the other. Tests see
    /// the name as `IDEALYST_TEST_DEVICE`.
    #[arg(long, value_name = "AVD|SERIAL", value_delimiter = ',')]
    pub emulator: Vec<String>,
    /// The `idealyst dev --isolated` key the test session runs under, so it
    /// runs beside the developer's own `idealyst dev` of the project instead
    /// of sharing (or being refused by) its build dirs, log, lock and relay
    /// files. Default `test`, a stable key so the session's build cache is
    /// warm from the second run on. Not available for `--web` on a project
    /// without a server (that shape stages into the project's own `pkg/`),
    /// which runs unisolated unless this flag is given explicitly.
    #[arg(long, value_name = "KEY", conflicts_with = "no_isolated")]
    pub isolated: Option<String>,
    /// Run the test session unisolated: in the project's own target dirs and
    /// `.idealyst/`, like a plain `idealyst dev`.
    #[arg(long)]
    pub no_isolated: bool,
    /// `KEY=VALUE` set for the app (passed as `idealyst dev --app-env`, which
    /// delivers it on every platform), the dev session (so a full-stack
    /// project's server inherits it), the tests, and the `--before`/`--after`
    /// hooks. Repeatable.
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// Shell command run in the project directory before the app is
    /// launched — start a database, seed fixtures. Must exit: a service it
    /// starts has to detach (`docker compose up -d`). A non-zero exit fails
    /// the run before anything launches. Repeatable; run in order.
    #[arg(long, value_name = "CMD")]
    pub before: Vec<String>,
    /// Shell command run in the project directory after the suite, whether
    /// it passed or not — tear down what `--before` started. A non-zero exit
    /// fails the run. Repeatable; run in order.
    #[arg(long, value_name = "CMD")]
    pub after: Vec<String>,
}

/// Parse a `WxH` viewport string into `(width, height)`.
fn parse_viewport(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .with_context(|| format!("--viewport must be WxH (e.g. 1280x800), got {s:?}"))?;
    Ok((
        w.trim().parse().with_context(|| format!("--viewport width in {s:?}"))?,
        h.trim().parse().with_context(|| format!("--viewport height in {s:?}"))?,
    ))
}

/// The viewport when none is asked for.
const DEFAULT_VIEWPORT: (u32, u32) = (1280, 800);

/// Parse `--env KEY=VALUE` pairs.
fn parse_env(pairs: &[String]) -> Result<Vec<(String, String)>> {
    pairs
        .iter()
        .map(|p| {
            let (k, v) = p
                .split_once('=')
                .with_context(|| format!("--env must be KEY=VALUE, got {p:?}"))?;
            anyhow::ensure!(!k.trim().is_empty(), "--env {p:?} has an empty key");
            Ok((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

/// A port nothing is listening on, for the test session's server. The OS
/// hands out an ephemeral port and it is released for `idealyst dev` to bind;
/// ephemeral ports are allocated round-robin, so another process taking it in
/// that gap is unlikely, and if one does the dev session fails to bind loudly.
fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).context("finding a free port")?;
    Ok(l.local_addr()?.port())
}

/// Kill `c` and, on unix, its whole process group — so what `idealyst dev`
/// started (cargo, an emulator install, the SSH session that keeps a remote
/// Windows app and its registration alive) or a browser's helper processes go
/// with it. Killing only the `dev` process left that SSH session running, and
/// the next run found its stale registration and drove the wrong app.
fn kill_tree(c: &mut Child) {
    #[cfg(unix)]
    unsafe {
        // Children are spawned as process-group leaders (`own_group`).
        libc::killpg(c.id() as libc::pid_t, libc::SIGTERM);
    }
    let _ = c.kill();
    let _ = c.wait();
}

/// Kills its child processes (dev sessions) on drop, see [`kill_tree`].
struct Kill(Vec<Child>);
impl Drop for Kill {
    fn drop(&mut self) {
        for c in &mut self.0 {
            kill_tree(c);
        }
    }
}

/// Spawn `cmd` as the leader of a new process group, so [`kill_tree`] can
/// stop everything it starts.
fn own_group(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// One run of the suite: the app at one size, on one device.
#[derive(Debug, Clone, PartialEq)]
struct Size {
    /// How the summary names this run.
    label: String,
    /// `IDEALYST_TEST_VIEWPORT`; `None` on iOS/Android, where the device decides.
    viewport: Option<(u32, u32)>,
    /// `IDEALYST_TEST_DEVICE`: the `--device` preset, `--simulator` or `--emulator`.
    device: Option<String>,
    /// Web: what the page is told about its screen.
    emulation: Option<Emulation>,
}

/// One app launch, and the suite runs made against it.
#[derive(Debug, Clone, PartialEq)]
struct Launch {
    /// `idealyst dev --simulator` (iOS) / `--emulator` (Android).
    target_device: Option<String>,
    /// `IDEALYST_WINDOW_SIZE` for the native window.
    window: (u32, u32),
    sizes: Vec<Size>,
}

/// Turn the size/device/simulator flags into launches. Web launches once and
/// switches sizes in the browser; a desktop app launches once per size (its
/// window size is a launch setting); iOS / Android launch once per
/// simulator / emulator.
fn plan_launches(args: &Args, platform: &str) -> Result<Vec<Launch>> {
    let viewports = args
        .viewport
        .iter()
        .map(|v| parse_viewport(v))
        .collect::<Result<Vec<_>>>()?;
    let devices = args
        .device
        .iter()
        .map(|d| device_preset(d))
        .collect::<Result<Vec<_>>>()?;
    if platform != "web" {
        anyhow::ensure!(
            devices.is_empty() && !args.mobile,
            "--device / --mobile emulate a phone in the headless browser, so they need --web \
             (on iOS pick a simulator with --simulator)"
        );
    }
    if platform != "ios" {
        anyhow::ensure!(args.simulator.is_empty(), "--simulator picks an iOS simulator, so it needs --ios");
    }
    if platform != "android" {
        anyhow::ensure!(args.emulator.is_empty(), "--emulator picks an Android device, so it needs --android");
    }
    let one_launch = |what: &str| -> Result<()> {
        anyhow::ensure!(!args.attach, "--attach drives the one app already running; it can't {what}");
        Ok(())
    };

    let launches = match platform {
        "web" => {
            let mut sizes: Vec<Size> = viewports
                .iter()
                .map(|&(w, h)| {
                    let e = if args.mobile { Emulation::mobile(w, h) } else { Emulation::desktop(w, h) };
                    Size { label: e.describe(), viewport: Some((w, h)), device: None, emulation: Some(e) }
                })
                .collect();
            sizes.extend(devices.iter().map(|d| {
                let e = d.emulation();
                Size {
                    label: format!("{} ({})", d.name, e.describe()),
                    viewport: Some((d.width, d.height)),
                    device: Some(d.name.to_string()),
                    emulation: Some(e),
                }
            }));
            if sizes.is_empty() {
                let (w, h) = DEFAULT_VIEWPORT;
                let e = if args.mobile { Emulation::mobile(w, h) } else { Emulation::desktop(w, h) };
                sizes.push(Size { label: e.describe(), viewport: Some((w, h)), device: None, emulation: Some(e) });
            }
            if args.attach && (sizes.len() > 1 || !devices.is_empty() || args.mobile) {
                one_launch("resize or emulate a browser it didn't start")?;
            }
            let window = sizes[0].viewport.unwrap_or(DEFAULT_VIEWPORT);
            vec![Launch { target_device: None, window, sizes }]
        }
        "ios" | "android" => {
            anyhow::ensure!(
                viewports.len() <= 1,
                "--{platform} runs at the device's own screen size; to cover several sizes run on \
                 several devices{}",
                if platform == "ios" {
                    " (--simulator \"iPhone 15,iPad Air 11-inch (M2)\")"
                } else {
                    " (--emulator Pixel_8_API_35,Pixel_Tablet_API_35)"
                }
            );
            let window = viewports.first().copied().unwrap_or(DEFAULT_VIEWPORT);
            let picked = if platform == "ios" { &args.simulator } else { &args.emulator };
            let sims: Vec<Option<String>> = if picked.is_empty() {
                vec![None]
            } else {
                if picked.len() > 1 {
                    one_launch("move between devices")?;
                }
                picked.iter().map(|s| Some(s.trim().to_string())).collect()
            };
            sims.into_iter()
                .map(|sim| Launch {
                    window,
                    sizes: vec![Size {
                        label: sim.clone().unwrap_or_else(|| platform.to_string()),
                        viewport: None,
                        device: sim.clone(),
                        emulation: None,
                    }],
                    target_device: sim,
                })
                .collect()
        }
        // Desktop: the window size is fixed at launch (`IDEALYST_WINDOW_SIZE`),
        // so each size is its own launch.
        _ => {
            let viewports = if viewports.is_empty() { vec![DEFAULT_VIEWPORT] } else { viewports };
            if viewports.len() > 1 {
                one_launch("relaunch at another window size")?;
            }
            viewports
                .into_iter()
                .map(|(w, h)| Launch {
                    target_device: None,
                    window: (w, h),
                    sizes: vec![Size {
                        label: format!("{w}x{h}"),
                        viewport: Some((w, h)),
                        device: None,
                        emulation: None,
                    }],
                })
                .collect()
        }
    };
    Ok(launches)
}

/// The environment one suite run gets, besides `IDEALYST_ROBOT_BRIDGE`: the
/// user's `--env`, then what the run is testing.
fn suite_env(size: &Size, port: u16, user_env: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = user_env.to_vec();
    if let Some((w, h)) = size.viewport {
        env.push(("IDEALYST_TEST_VIEWPORT".into(), format!("{w}x{h}")));
    }
    if let Some(d) = &size.device {
        env.push(("IDEALYST_TEST_DEVICE".into(), d.clone()));
    }
    env.push(("IDEALYST_TEST_PORT".into(), port.to_string()));
    env
}

/// How one suite run ended.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Passed,
    /// The suite ran and failed, with the test binary's exit code.
    Failed(Option<i32>),
    /// The suite never ran (the app didn't come up, the page didn't load).
    Error(String),
}

/// The per-size summary and the process exit code: 0 when every run passed;
/// a single run's own failure code; 1 otherwise.
fn summarize(results: &[(String, Outcome)]) -> (String, i32) {
    let mut out = String::from("[test] results:\n");
    for (label, outcome) in results {
        let line = match outcome {
            Outcome::Passed => format!("  PASS   {label}\n"),
            Outcome::Failed(_) => format!("  FAIL   {label}\n"),
            Outcome::Error(e) => format!("  ERROR  {label}: {e}\n"),
        };
        out.push_str(&line);
    }
    let code = match results {
        _ if results.iter().all(|(_, o)| *o == Outcome::Passed) => 0,
        [(_, Outcome::Failed(Some(code)))] if *code != 0 => *code,
        _ => 1,
    };
    (out, code)
}

/// A `--before` / `--after` hook: `sh -c CMD` (`cmd /C` on Windows).
fn hook_command(cmd: &str) -> Command {
    #[cfg(windows)]
    let mut c = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(cmd);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    c.stdin(Stdio::null());
    c
}

/// Run each hook in order; the first non-zero exit fails.
fn run_hooks(flag: &str, hooks: &[String], dir: &Path, env: &[(String, String)]) -> Result<()> {
    for hook in hooks {
        eprintln!("[test] {flag}: {hook}");
        let status = hook_command(hook)
            .current_dir(dir)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .status()
            .with_context(|| format!("running {flag} {hook:?}"))?;
        anyhow::ensure!(status.success(), "{flag} {hook:?} failed ({status})");
    }
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve project dir {}", args.dir.display()))?;
    let user_env = parse_env(&args.env)?;

    if !args.parity.is_empty() {
        return run_parity(&args, &dir, &user_env);
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

    let launches = plan_launches(&args, platform)?;
    let isolated = if args.attach { None } else { isolation_key(&args, platform, project_has_server(&dir))? };
    if let Some(key) = &isolated {
        eprintln!("[test] dev session isolated as `{key}` (beside any `idealyst dev` of this project)");
    }
    let port = match args.port {
        Some(p) => p,
        None => free_port()?,
    };
    let mut hook_env = user_env.clone();
    hook_env.push(("IDEALYST_TEST_PORT".into(), port.to_string()));

    // Compile the suite before launching anything: a compile error fails fast,
    // and the suite binary is then run directly per size (see
    // `build_test_binary` for why it must not be built after the app).
    let test_bin = build_test_binary(&dir, &args.test)?;

    let outcome = run_hooks("--before", &args.before, &dir, &hook_env).map(|()| {
        let mut results = Vec::new();
        for (i, launch) in launches.iter().enumerate() {
            let ctx = LaunchCtx {
                args: &args,
                dir: &dir,
                platform,
                port,
                isolated: isolated.as_deref(),
                user_env: &user_env,
                test_bin: &test_bin,
                index: i,
            };
            let before = results.len();
            if let Err(e) = run_launch(&ctx, launch, &mut results) {
                eprintln!("[test] {e:#}");
                // Every size this launch didn't reach is reported, not dropped.
                for size in &launch.sizes[results.len() - before..] {
                    results.push((size.label.clone(), Outcome::Error(format!("{e:#}"))));
                }
            }
        }
        results
    });
    // Teardown runs whatever happened above.
    let after = run_hooks("--after", &args.after, &dir, &hook_env);

    let results = outcome?;
    let (summary, mut code) = summarize(&results);
    eprintln!("\n{summary}");
    if let Err(e) = after {
        eprintln!("[test] {e:#}");
        if code == 0 {
            code = 1;
        }
    }
    if code == 0 {
        Ok(())
    } else {
        std::process::exit(code);
    }
}

/// What every launch of one run shares.
struct LaunchCtx<'a> {
    args: &'a Args,
    dir: &'a Path,
    platform: &'a str,
    port: u16,
    isolated: Option<&'a str>,
    user_env: &'a [(String, String)],
    test_bin: &'a Path,
    index: usize,
}

/// Launch the app (unless attaching), then run the suite at each of the
/// launch's sizes, appending one result per size to `results`.
fn run_launch(ctx: &LaunchCtx, launch: &Launch, results: &mut Vec<(String, Outcome)>) -> Result<()> {
    let LaunchCtx { args, dir, platform, port, isolated, user_env, test_bin, index } = *ctx;
    let apps_dirs = registry_dirs()?;
    // Registrations already live before the launch belong to another run
    // (an earlier launch of this one included) or to the app being attached
    // to: when launching, wait for a new one.
    let preexisting: Vec<SocketAddr> = if args.attach { Vec::new() } else { discover_all_in(dir, &apps_dirs) };
    let mut kill = Kill(Vec::new());
    let mut owner: Option<u32> = None;
    let dev_log: Option<PathBuf> = (!args.attach).then(|| dev_log_path(platform, index));
    let see_log = |what: &str| match &dev_log {
        Some(log) => format!("{what} — the dev session's output is in {}", log.display()),
        None => what.to_string(),
    };
    if !args.attach {
        // Launch the app by spawning a `dev` session of THIS binary. Robot is on
        // by default, so the app dials the relay; killing the child tears it down.
        let self_exe = std::env::current_exe().context("locating the idealyst binary")?;
        let on = launch.target_device.as_deref().map(|s| format!(" on {s}")).unwrap_or_default();
        eprintln!("[test] launching {platform} app{on} via `idealyst dev` (port {port})…");
        let mut cmd = dev_command(
            &self_exe,
            &DevOpts {
                platform,
                port,
                window: launch.window,
                target_device: launch.target_device.as_deref(),
                isolated,
                env: user_env,
            },
        );
        // The session's build + launch output goes to a log file, named when
        // the app doesn't come up — discarding it left "did not become ready"
        // with nothing to go on.
        let log = dev_log.as_ref().expect("set when not attaching");
        let file = std::fs::File::create(log).with_context(|| format!("creating {}", log.display()))?;
        cmd.arg(dir).stdout(file.try_clone()?).stderr(file);
        eprintln!("[test] dev session log: {}", log.display());
        let child = own_group(&mut cmd).spawn().context("spawning `idealyst dev`")?;
        owner = Some(child.id());
        kill.0.push(child);
    }

    // Wait for the relay registration for this project (a cold build can take a
    // while), then for web open the page headlessly so the app dials.
    eprintln!("[test] waiting for the app to come up…");
    let addr = wait_for_registration(dir, &apps_dirs, &preexisting, owner, Duration::from_secs(300))
        .with_context(|| see_log("the app never registered with the relay (build failed?)"))?;

    // Declared after `kill`, so it is dropped (browser closed) first.
    let mut browser: Option<HeadlessBrowser> = None;
    if platform == "web" && !args.attach {
        if !wait_for_web_server(port, Duration::from_secs(600)) {
            anyhow::bail!(
                "the web dev server never served http://127.0.0.1:{port} — the wasm \
                 build probably failed; run `idealyst dev --web` to see it"
            );
        }
        browser = Some(HeadlessBrowser::launch(&format!("http://127.0.0.1:{port}/"))?);
    }

    for size in &launch.sizes {
        let ran = (|| -> Result<Outcome> {
            if let (Some(b), Some(e)) = (browser.as_mut(), &size.emulation) {
                b.show(e).with_context(|| format!("see {}", b.log.display()))?;
            }
            // Confirm the app is actually answering before we hand the suite the
            // bridge. The relay registers as soon as `dev` hosts it — well before
            // a cold app build finishes, launches, and dials — so give the app a
            // generous budget and reconnect each attempt.
            wait_until_ready(addr, Duration::from_secs(240)).with_context(|| see_log("the app did not become ready"))?;
            eprintln!("[test] running `{}` on {platform} at {}…\n", args.test, size.label);
            let mut suite = Command::new(test_bin);
            suite
                .current_dir(dir)
                // What `cargo test` would set, for suites that read it at runtime.
                .env("CARGO_MANIFEST_DIR", dir)
                .envs(suite_env(size, port, user_env))
                .env("IDEALYST_ROBOT_BRIDGE", addr.to_string());
            if let Some(filter) = &args.filter {
                suite.arg(filter);
            }
            // One shared app: tests run serialized against it.
            suite.args(["--test-threads=1", "--nocapture"]);
            let status = suite.status().with_context(|| format!("running {}", test_bin.display()))?;
            Ok(if status.success() { Outcome::Passed } else { Outcome::Failed(status.code()) })
        })();
        let outcome = ran.unwrap_or_else(|e| {
            eprintln!("[test] {}: {e:#}", size.label);
            Outcome::Error(format!("{e:#}"))
        });
        results.push((size.label.clone(), outcome));
    }
    drop(browser);
    drop(kill); // tear down the dev session
    Ok(())
}

/// `idealyst test --parity web,macos`: launch every named platform of the same
/// project, then run the parity test target with each app's bridge address in
/// `IDEALYST_<PLATFORM>_BRIDGE` so the test can capture + diff their
/// platform-native render trees.
fn run_parity(args: &Args, dir: &Path, user_env: &[(String, String)]) -> Result<()> {
    anyhow::ensure!(
        args.parity.len() >= 2,
        "--parity needs at least two platforms to compare (e.g. --parity web,macos)"
    );
    anyhow::ensure!(
        !args.attach,
        "--parity launches its own apps; --attach isn't supported (it can't tell two \
         same-project apps apart by registration alone)"
    );
    anyhow::ensure!(
        args.viewport.len() <= 1
            && args.device.is_empty()
            && !args.mobile
            && args.simulator.is_empty()
            && args.emulator.is_empty(),
        "--parity renders every platform at ONE --viewport; --device, --mobile, --simulator, \
         --emulator and several viewports are for single-platform runs"
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

    let test_bin = build_test_binary(dir, &test_name)?;

    let viewport = match args.viewport.first() {
        Some(v) => parse_viewport(v)?,
        None => DEFAULT_VIEWPORT,
    };
    let self_exe = std::env::current_exe().context("locating the idealyst binary")?;
    let apps_dirs = registry_dirs()?;
    eprintln!("[parity] viewport pinned to {}x{} on every platform", viewport.0, viewport.1);

    let has_server = project_has_server(dir);
    let mut kill = Kill(Vec::new());
    let mut browsers: Vec<HeadlessBrowser> = Vec::new();
    // Apps already registered for this project before we launch anything — so a
    // dev session the user left running isn't mistaken for one of ours.
    let mut seen: HashSet<SocketAddr> = discover_all_in(dir, &apps_dirs).into_iter().collect();
    let mut bridges: Vec<(String, SocketAddr)> = Vec::new();

    for (i, platform) in args.parity.iter().enumerate() {
        let platform = normalize_platform(platform)?;
        // Each platform's session runs its own server, so each needs its own
        // port; `--port` goes to the first.
        let port = match (i, args.port) {
            (0, Some(p)) => p,
            _ => free_port()?,
        };
        eprintln!("[parity] launching {platform} app via `idealyst dev` (port {port})…");
        // Every platform's session is its own isolated session (one session
        // per key per project), keyed `<key>-<platform>`.
        let isolated = isolation_key(args, &platform, has_server)?.map(|k| format!("{k}-{platform}"));
        let mut cmd = dev_command(
            &self_exe,
            &DevOpts {
                platform: &platform,
                port,
                window: viewport,
                target_device: None,
                isolated: isolated.as_deref(),
                env: user_env,
            },
        );
        cmd.arg(dir).stdout(Stdio::null()).stderr(Stdio::null());
        kill.0.push(own_group(&mut cmd).spawn().context("spawning `idealyst dev`")?);

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
            if !wait_for_web_server(port, Duration::from_secs(600)) {
                anyhow::bail!(
                    "the web dev server never served http://127.0.0.1:{port} — the wasm \
                     build probably failed; run `idealyst dev --web` to see it"
                );
            }
            let mut b = HeadlessBrowser::launch(&format!("http://127.0.0.1:{port}/"))?;
            b.show(&Emulation::desktop(viewport.0, viewport.1))?;
            browsers.push(b);
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
    // workspace build out of the picture entirely (see `build_test_binary`).
    eprintln!("\n[parity] running {}…\n", test_bin.display());
    let mut suite = Command::new(&test_bin);
    suite.current_dir(dir).env("CARGO_MANIFEST_DIR", dir).envs(user_env.iter().cloned());
    for (platform, addr) in &bridges {
        suite.env(
            format!("IDEALYST_{}_BRIDGE", platform.to_uppercase()),
            addr.to_string(),
        );
    }
    // First platform also fills IDEALYST_ROBOT_BRIDGE so single-app helpers work
    // inside a parity test if it wants them.
    if let Some((_, addr)) = bridges.first() {
        suite.env("IDEALYST_ROBOT_BRIDGE", addr.to_string());
    }
    suite.env(
        "IDEALYST_PARITY_PLATFORMS",
        bridges
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>()
            .join(","),
    );
    suite.env("IDEALYST_TEST_VIEWPORT", format!("{}x{}", viewport.0, viewport.1));
    if let Some(filter) = &args.filter {
        suite.arg(filter);
    }
    suite.args(["--test-threads=1", "--nocapture"]);

    let status = suite
        .status()
        .with_context(|| format!("running {}", test_bin.display()))?;
    drop(browsers);
    drop(kill); // tear down every dev session
    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// Whether the project declares a server (`server_bin` / `server_manifest`):
/// the full-stack shape, whose dev web bundle is staged per session. An
/// unreadable manifest counts as no server; `idealyst dev` reports it.
fn project_has_server(dir: &Path) -> bool {
    build_ios::parse_manifest(dir)
        .map(|m| m.app.server_bin.is_some() || m.app.server_manifest.is_some())
        .unwrap_or(false)
}

/// The key the test session runs `idealyst dev --isolated` under, decided
/// before launching: `--isolated <KEY>`, else `test`, else none with
/// `--no-isolated`. `idealyst dev` refuses `--isolated` for `--web` on a
/// project without a server (that shape stages into the project's own
/// `index.html`/`pkg/`, shared by every session), so the default falls back
/// to unisolated there, and an explicit key is refused up front instead of
/// after a launch.
fn isolation_key(args: &Args, platform: &str, has_server: bool) -> Result<Option<String>> {
    if args.no_isolated {
        return Ok(None);
    }
    let static_web = platform == "web" && !has_server;
    match &args.isolated {
        Some(key) => {
            build_ios::validate_isolation_key(key)?;
            anyhow::ensure!(
                !static_web,
                "--isolated {key} can't run --web on a project without a server: that shape serves \
                 the project's own `index.html` and `pkg/`, which every session shares (drop \
                 --isolated, or give the project a `server_bin`)"
            );
            Ok(Some(key.clone()))
        }
        None if static_web => Ok(None),
        None => Ok(Some(DEFAULT_ISOLATION_KEY.to_string())),
    }
}

/// The `idealyst dev --isolated` key a test session uses by default: stable,
/// so the session's own build cache is warm from the second run on.
const DEFAULT_ISOLATION_KEY: &str = "test";

/// How one test session's `idealyst dev` is launched.
struct DevOpts<'a> {
    platform: &'a str,
    port: u16,
    window: (u32, u32),
    /// iOS simulator / Android device.
    target_device: Option<&'a str>,
    isolated: Option<&'a str>,
    env: &'a [(String, String)],
}

/// The `idealyst dev --local` session a test run launches its app with
/// (the project directory is appended by the caller).
///
/// - `--port`: on EVERY platform. A full-stack project's server binds it and
///   advertises it to native apps as `IDEALYST_SERVER_URL`; left out, that
///   server took `[app].server_port` — the developer's own dev session's
///   port — and the suite drove the developer's server and data.
/// - `--isolated`: the session's build dirs, log, lock and relay files
///   live apart from a developer's session of the same project.
/// - `--simulator` / `--emulator`: the iOS simulator / Android device.
/// - `env`: each pair as `--app-env`, which hands it to the app on every
///   platform, and in the session's own environment, which the project's
///   server inherits.
/// - `IDEALYST_TEST_DRIVER`: the app is externally driven, so an in-app
///   self-test stands down instead of racing the suite for shared state.
/// - `IDEALYST_WINDOW_SIZE`: the native window at the test viewport (the
///   headless browser's page is sized over DevTools instead).
/// - `IDEALYST_DEV_NO_RESUME`: build the sources. A web session otherwise
///   resumes the last session's base and replays its hot patches, and a
///   patch only reaches code the page calls after it lands — so the
///   suite could run code that is not the source on disk.
fn dev_command(self_exe: &Path, o: &DevOpts) -> Command {
    let mut cmd = Command::new(self_exe);
    cmd.arg("dev").arg("--local").arg(format!("--{}", o.platform));
    cmd.args(["--port", &o.port.to_string()]);
    if let Some(key) = o.isolated {
        cmd.args(["--isolated", key]);
    }
    if let Some(device) = o.target_device {
        let flag = if o.platform == "android" { "--emulator" } else { "--simulator" };
        cmd.args([flag, device]);
    }
    for (k, v) in o.env {
        cmd.arg("--app-env").arg(format!("{k}={v}"));
    }
    cmd.envs(o.env.iter().map(|(k, v)| (k, v)));
    cmd.env("IDEALYST_TEST_DRIVER", "1");
    cmd.env("IDEALYST_WINDOW_SIZE", format!("{}x{}", o.window.0, o.window.1));
    cmd.env("IDEALYST_DEV_NO_RESUME", "1");
    cmd
}

/// Compile the test target and return its executable path. cargo's progress
/// and diagnostics stream to the terminal.
///
/// Uses `--no-run --message-format=json` and takes the `executable` from the
/// compiler-artifact line for the wanted test target, which is the only
/// non-guessy way to locate a test binary (the file name carries a metadata
/// hash).
///
/// Built BEFORE any app is launched, and the binary then run directly rather
/// than through `cargo test` — a correctness requirement, not an
/// optimization. A native wrapper build (`--linux`, `--macos`, …) shares the
/// project's `target/` but runs with its OWN workspace and cwd, so it records
/// the framework path crates under ABSOLUTE paths while a workspace build
/// records them RELATIVE. Both unit sets land in the same target dir, and a
/// workspace `cargo test` issued AFTER the wrapper build then mixes them:
///
/// ```text
/// error[E0308]: expected `Element`, found `Element`
/// note: there are multiple different versions of crate `runtime_scene`
/// ```
fn build_test_binary(dir: &Path, test_name: &str) -> Result<PathBuf> {
    eprintln!("[test] building the `{test_name}` test target…");
    let out = Command::new("cargo")
        .current_dir(dir)
        .args(["test", "--test", test_name, "--no-run", "--message-format=json"])
        .stderr(Stdio::inherit())
        .output()
        .context("running `cargo test --no-run` (is cargo on PATH?)")?;
    anyhow::ensure!(
        out.status.success(),
        "building the `{test_name}` test target failed (cargo's errors are above)"
    );
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

/// Where a test run's `idealyst dev` output goes: the system temp dir, one
/// file per platform, run and launch.
fn dev_log_path(platform: &str, launch: usize) -> PathBuf {
    std::env::temp_dir().join(format!("idealyst-test-{platform}-{}-{launch}.log", std::process::id()))
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

/// Every live registration for `project_dir` across `apps_dirs`, with the
/// pid that wrote it (the relay's registration carries the `idealyst dev`
/// process's pid; an app that self-hosts its bridge writes its own).
fn live_registrations(project_dir: &Path, apps_dirs: &[PathBuf]) -> Vec<(SocketAddr, Option<u32>)> {
    let want = std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let mut out = Vec::new();
    for dir in apps_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(reg) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            else {
                continue;
            };
            let Some(port) = reg["port"].as_u64().and_then(|p| u16::try_from(p).ok()) else { continue };
            let root = reg["project_root"].as_str().unwrap_or_default();
            if std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root)) != want {
                continue;
            }
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_err() {
                continue; // stale registration
            }
            out.push((addr, reg["pid"].as_u64().and_then(|p| u32::try_from(p).ok())));
        }
    }
    out
}

/// The registration of the app just launched: live, for THIS project, and
/// not there before the launch — `discover` falls back to any live app, and
/// a registration left by an earlier run's still-closing session is live
/// too. Among those, the one the launched `idealyst dev` (pid `owner`)
/// wrote wins, so a developer's own session of the project that registers
/// meanwhile is never the one driven; failing that (an app that self-hosts
/// its bridge, a remote Windows app), the first new one.
fn fresh_registration(
    project_dir: &Path,
    apps_dirs: &[PathBuf],
    preexisting: &[SocketAddr],
    owner: Option<u32>,
) -> Option<SocketAddr> {
    let fresh: Vec<(SocketAddr, Option<u32>)> = live_registrations(project_dir, apps_dirs)
        .into_iter()
        .filter(|(addr, _)| !preexisting.contains(addr))
        .collect();
    fresh
        .iter()
        .find(|(_, pid)| owner.is_some() && *pid == owner)
        .or_else(|| fresh.first())
        .map(|(addr, _)| *addr)
}

fn wait_for_registration(
    project_dir: &Path,
    apps_dirs: &[PathBuf],
    preexisting: &[SocketAddr],
    owner: Option<u32>,
    timeout: Duration,
) -> Option<SocketAddr> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(addr) = fresh_registration(project_dir, apps_dirs, preexisting, owner) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(argv: &[&str]) -> Args {
        let mut full = vec!["idealyst-test"];
        full.extend_from_slice(argv);
        Args::try_parse_from(full).expect("parses")
    }

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args().filter_map(|a| a.to_str().map(str::to_string)).collect()
    }

    fn opts<'a>(platform: &'a str, port: u16, device: Option<&'a str>, env: &'a [(String, String)]) -> DevOpts<'a> {
        DevOpts { platform, port, window: (1280, 800), target_device: device, isolated: None, env }
    }

    fn envs(cmd: &Command) -> Vec<(String, String)> {
        cmd.get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect()
    }

    /// A test run builds the sources it tests: the launched dev session must
    /// not resume the last session's base and replay its hot patches — that
    /// ran the suite against a demo overlay that was no longer in the source.
    #[test]
    fn regression_the_test_session_builds_instead_of_resuming_a_patched_base() {
        let cmd = dev_command(Path::new("idealyst"), &opts("web", 8080, None, &[]));
        let envs = envs(&cmd);
        assert!(envs.contains(&("IDEALYST_DEV_NO_RESUME".into(), "1".into())), "{envs:?}");
        assert!(envs.contains(&("IDEALYST_TEST_DRIVER".into(), "1".into())), "{envs:?}");
        assert_eq!(argv(&cmd), ["dev", "--local", "--web", "--port", "8080"]);
    }

    /// `--port` reaches the dev session on every platform. Passed on web
    /// only, a full-stack project's server on iOS/Android/macOS bound
    /// `[app].server_port` — the developer's own dev session's port — and
    /// `IDEALYST_SERVER_URL` pointed the test app at the developer's server
    /// and database, which a suite that resets data then wiped.
    #[test]
    fn regression_native_test_sessions_get_the_test_port_not_the_dev_one() {
        for platform in ["ios", "android", "macos", "linux", "windows"] {
            let cmd = dev_command(Path::new("idealyst"), &opts(platform, 51234, None, &[]));
            let argv = argv(&cmd);
            let at = argv.iter().position(|a| a == "--port").unwrap_or_else(|| panic!("{platform}: {argv:?}"));
            assert_eq!(argv[at + 1], "51234", "{platform}: {argv:?}");
        }
    }

    /// With no `--port`, the run picks a free port rather than inheriting
    /// a configured one.
    #[test]
    fn regression_the_default_test_port_is_free_not_the_configured_dev_port() {
        assert_eq!(args(&["--ios"]).port, None, "no clap default that could be a dev port");
        // An OS-assigned ephemeral port. (Not re-bound here: tests binding
        // `:0` in parallel may be handed the same released port.)
        let p = free_port().unwrap();
        assert!(p >= 1024, "{p}");
    }

    #[test]
    fn the_simulator_and_env_reach_the_dev_session() {
        let env = parse_env(&["DATABASE_URL=postgres://x/test?a=b".into(), "EMPTY=".into()]).unwrap();
        assert_eq!(env[0], ("DATABASE_URL".into(), "postgres://x/test?a=b".into()));
        assert_eq!(env[1], ("EMPTY".into(), "".into()));
        let cmd = dev_command(Path::new("idealyst"), &opts("ios", 9000, Some("iPhone 15"), &env));
        assert_eq!(
            argv(&cmd),
            [
                "dev", "--local", "--ios", "--port", "9000", "--simulator", "iPhone 15",
                "--app-env", "DATABASE_URL=postgres://x/test?a=b", "--app-env", "EMPTY=",
            ]
        );
        assert!(envs(&cmd).contains(&("DATABASE_URL".into(), "postgres://x/test?a=b".into())), "the server inherits it");
        let cmd = dev_command(Path::new("idealyst"), &opts("android", 9000, Some("Pixel_8_API_35"), &[]));
        assert_eq!(argv(&cmd), ["dev", "--local", "--android", "--port", "9000", "--emulator", "Pixel_8_API_35"]);
        assert!(parse_env(&["NOEQUALS".into()]).is_err());
        assert!(parse_env(&["=v".into()]).is_err());
    }

    /// Web: one launch, one suite run per viewport then per device, each with
    /// its own emulation and the env a test can branch on.
    #[test]
    fn web_runs_once_per_size_in_one_launch() {
        let a = args(&["--web", "--viewport", "390x844,1440x900", "--device", "iphone-14"]);
        let launches = plan_launches(&a, "web").unwrap();
        assert_eq!(launches.len(), 1, "build + launch once");
        let sizes = &launches[0].sizes;
        let labels: Vec<&str> = sizes.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["390x844", "1440x900", "iphone-14 (390x844 @3x, touch)"]);
        assert_eq!(sizes[0].emulation, Some(Emulation::desktop(390, 844)));
        assert!(sizes[2].emulation.as_ref().unwrap().touch);
        assert_eq!(launches[0].window, (390, 844));

        let env = suite_env(&sizes[2], 4321, &[("K".into(), "V".into())]);
        assert!(env.contains(&("IDEALYST_TEST_VIEWPORT".into(), "390x844".into())));
        assert!(env.contains(&("IDEALYST_TEST_DEVICE".into(), "iphone-14".into())));
        assert!(env.contains(&("IDEALYST_TEST_PORT".into(), "4321".into())));
        assert!(env.contains(&("K".into(), "V".into())));
        assert!(!suite_env(&sizes[0], 1, &[]).iter().any(|(k, _)| k == "IDEALYST_TEST_DEVICE"));
    }

    #[test]
    fn repeated_viewport_flags_add_up_and_default_applies_only_without_devices() {
        let a = args(&["--viewport", "390x844", "--viewport", "820x1180"]);
        assert_eq!(plan_launches(&a, "web").unwrap()[0].sizes.len(), 2);
        let only_device = plan_launches(&args(&["--device", "pixel-7"]), "web").unwrap();
        assert_eq!(only_device[0].sizes.len(), 1);
        let none = plan_launches(&args(&[]), "web").unwrap();
        assert_eq!(none[0].sizes[0].viewport, Some((1280, 800)));
        let mobile = plan_launches(&args(&["--mobile", "--viewport", "390x844"]), "web").unwrap();
        assert_eq!(mobile[0].sizes[0].emulation, Some(Emulation::mobile(390, 844)));
    }

    /// Desktop apps take their window size at launch: one launch per size.
    #[test]
    fn desktop_relaunches_per_size() {
        let a = args(&["--macos", "--viewport", "390x844,1440x900"]);
        let launches = plan_launches(&a, "macos").unwrap();
        assert_eq!(launches.iter().map(|l| l.window).collect::<Vec<_>>(), [(390, 844), (1440, 900)]);
        assert!(launches.iter().all(|l| l.sizes.len() == 1 && l.sizes[0].emulation.is_none()));
    }

    /// iOS: one launch per simulator, in order, named in the env.
    #[test]
    fn ios_runs_once_per_simulator() {
        let a = args(&["--ios", "--simulator", "iPhone 15,iPad Air 11-inch (M2)"]);
        let launches = plan_launches(&a, "ios").unwrap();
        let sims: Vec<_> = launches.iter().map(|l| l.target_device.as_deref()).collect();
        assert_eq!(sims, [Some("iPhone 15"), Some("iPad Air 11-inch (M2)")]);
        assert_eq!(launches[1].sizes[0].device.as_deref(), Some("iPad Air 11-inch (M2)"));
        assert_eq!(launches[1].sizes[0].viewport, None);
        let default = plan_launches(&args(&["--ios"]), "ios").unwrap();
        assert_eq!(default.len(), 1);
        assert_eq!(default[0].target_device, None);
    }

    #[test]
    fn android_runs_once_per_emulator() {
        let a = args(&["--android", "--emulator", "Pixel_8_API_35", "--emulator", "emulator-5556"]);
        let launches = plan_launches(&a, "android").unwrap();
        let devs: Vec<_> = launches.iter().map(|l| l.target_device.as_deref()).collect();
        assert_eq!(devs, [Some("Pixel_8_API_35"), Some("emulator-5556")]);
        assert_eq!(launches[1].sizes[0].device.as_deref(), Some("emulator-5556"));
        assert!(plan_launches(&args(&["--ios", "--emulator", "x"]), "ios").is_err());
    }

    /// The test session runs isolated by default, so it builds and registers
    /// beside the developer's own `idealyst dev` of the project — except the
    /// static web shape, which `idealyst dev` refuses to isolate.
    #[test]
    fn the_test_session_is_isolated_by_default_except_static_web() {
        assert_eq!(isolation_key(&args(&["--ios"]), "ios", false).unwrap().as_deref(), Some("test"));
        assert_eq!(isolation_key(&args(&["--web"]), "web", true).unwrap().as_deref(), Some("test"));
        assert_eq!(isolation_key(&args(&["--web"]), "web", false).unwrap(), None, "static web falls back");
        assert!(isolation_key(&args(&["--web", "--isolated", "ci"]), "web", false).is_err(), "refused up front");
        assert_eq!(isolation_key(&args(&["--isolated", "ci"]), "macos", false).unwrap().as_deref(), Some("ci"));
        assert_eq!(isolation_key(&args(&["--no-isolated"]), "macos", true).unwrap(), None);
        assert!(isolation_key(&args(&["--isolated", "bad key"]), "macos", false).is_err());

        let o = DevOpts { isolated: Some("test"), ..opts("macos", 9000, None, &[]) };
        let argv = argv(&dev_command(Path::new("idealyst"), &o));
        let at = argv.iter().position(|a| a == "--isolated").expect("passed to dev");
        assert_eq!(argv[at + 1], "test");
    }

    /// The bridge driven is the one the launched `idealyst dev` registered,
    /// even when another session of the same project (the developer's)
    /// registers during the launch.
    #[test]
    fn regression_the_launcher_drives_its_own_session_not_a_concurrent_one() {
        use std::net::TcpListener;
        let apps = std::env::temp_dir().join(format!("idealyst-test-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&apps);
        let project = apps.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let register = |name: &str, pid: u32| {
            let live = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = live.local_addr().unwrap().port();
            let reg = serde_json::json!({ "name": name, "pid": pid, "port": port, "project_root": project });
            std::fs::write(apps.join(format!("{name}.json")), reg.to_string()).unwrap();
            live
        };
        let dirs = vec![apps.clone()];
        let developers = register("a-developer", 111);
        let ours = register("b-ours", 222);
        let got = fresh_registration(&project, &dirs, &[], Some(222)).unwrap();
        assert_eq!(got.port(), ours.local_addr().unwrap().port());
        let got = fresh_registration(&project, &dirs, &[], Some(111)).unwrap();
        assert_eq!(got.port(), developers.local_addr().unwrap().port());
        let _ = std::fs::remove_dir_all(&apps);
    }

    #[test]
    fn flags_for_another_platform_are_refused() {
        assert!(plan_launches(&args(&["--ios", "--device", "iphone-14"]), "ios").is_err());
        assert!(plan_launches(&args(&["--macos", "--mobile"]), "macos").is_err());
        assert!(plan_launches(&args(&["--web", "--simulator", "iPhone 15"]), "web").is_err());
        assert!(plan_launches(&args(&["--ios", "--viewport", "390x844,820x1180"]), "ios").is_err());
        assert!(plan_launches(&args(&["--attach", "--viewport", "390x844,820x1180"]), "web").is_err());
        assert!(plan_launches(&args(&["--device", "nokia"]), "web").is_err());
        assert!(plan_launches(&args(&["--viewport", "wide"]), "web").is_err());
    }

    #[test]
    fn the_summary_fails_the_run_if_any_size_fails() {
        let pass = ("a".to_string(), Outcome::Passed);
        assert_eq!(summarize(&[pass.clone(), pass.clone()]).1, 0);
        let (text, code) = summarize(&[pass.clone(), ("b".into(), Outcome::Failed(Some(101)))]);
        assert_eq!(code, 1);
        assert!(text.contains("PASS   a") && text.contains("FAIL   b"), "{text}");
        assert_eq!(summarize(&[("b".into(), Outcome::Failed(Some(101)))]).1, 101, "a single run keeps its code");
        let (text, code) = summarize(&[("c".into(), Outcome::Error("app never came up".into()))]);
        assert_eq!(code, 1);
        assert!(text.contains("ERROR  c: app never came up"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn hooks_get_the_env_and_a_failing_one_fails_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let env = vec![("IDEALYST_TEST_PORT".to_string(), "4242".to_string())];
        run_hooks("--before", &["echo $IDEALYST_TEST_PORT > port.txt".into()], dir.path(), &env).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("port.txt")).unwrap().trim(), "4242");
        let err = run_hooks("--before", &["exit 3".into(), "touch never".into()], dir.path(), &env).unwrap_err();
        assert!(err.to_string().contains("exit 3"), "{err}");
        assert!(!dir.path().join("never").exists(), "hooks stop at the first failure");
    }

    /// A registration live before the launch is another run's (the run
    /// before, still closing its SSH session to a remote Windows app): the
    /// launcher must wait for the new one. It used the old one and drove the
    /// wrong app — the SDK gallery that held the remote bridge port.
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
        assert_eq!(fresh_registration(&project, &dirs, &preexisting, None), None, "the old run's app is not this one");
        let new = register("new");
        let got = fresh_registration(&project, &dirs, &preexisting, None).expect("the launched app");
        assert_eq!(got.port(), new.local_addr().unwrap().port());
        drop(old);
        let _ = std::fs::remove_dir_all(&apps);
    }
}
