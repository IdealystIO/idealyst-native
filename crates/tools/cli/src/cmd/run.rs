use std::path::PathBuf;

use anyhow::Context;

use crate::Platform;

/// Window form factor for `idealyst run sim`. Drives both the
/// window size (each variant has its own `native-*` crate) and
/// which preset the chosen skin uses by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum SimForm {
    #[default]
    Phone,
    Tablet,
    Tv,
}

/// Painter painting the chrome around the user's tree for
/// `idealyst run sim`. Independent of the form factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum SimPainter {
    #[default]
    Ios,
    Android,
}

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target platform.
    #[arg(value_enum)]
    pub platform: Platform,

    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Build in release mode. Default is debug.
    ///
    /// Note: `idealyst run ios --device` defaults to RELEASE regardless
    /// (debug staticlibs are unusably slow on-device); pass `--debug`
    /// there to opt out.
    #[arg(long)]
    pub release: bool,

    /// iOS only: build, code-sign, and install onto a connected physical
    /// iPhone (instead of the simulator). Requires a development signing
    /// identity + `ios-deploy` (`brew install ios-deploy`). The Rust
    /// staticlib is built in RELEASE by default for this path — pass
    /// `--debug` to override.
    #[arg(long)]
    pub device: bool,

    /// iOS `--device` only: build the staticlib in DEBUG instead of the
    /// default release. Slower on-device but faster to compile; handy when
    /// iterating on signing/install rather than runtime performance.
    #[arg(long)]
    pub debug: bool,

    /// iOS `--device` only: Apple Developer team ID to sign with (the
    /// 10-char identifier). Falls back to `$IDEALYST_DEVELOPMENT_TEAM` /
    /// `$DEVELOPMENT_TEAM`, then auto-detects from your installed
    /// "Apple Development" signing certificate.
    #[arg(long)]
    pub team: Option<String>,

    /// iOS `--device` only: target a specific device by UDID. Defaults to
    /// the first connected iPhone found via `xcrun xctrace list devices`.
    #[arg(long)]
    pub udid: Option<String>,

    /// iOS only: force a fully clean reinstall. The default flow already
    /// terminates the running app before installing (so `launch` can't
    /// re-foreground a stale process); `--clean` additionally uninstalls
    /// the app first — on the simulator via `simctl uninstall`, on a device
    /// via `ios-deploy --uninstall` — so even SpringBoard's cached
    /// executable is dropped. This WIPES the app's persisted data
    /// (UserDefaults / Keychain / sandbox files).
    #[arg(long)]
    pub clean: bool,

    /// Build the runtime-server-client variant and connect to an
    /// already-running dev-host. Requires `--runtime-server-port`
    /// (the dev-host's bound port). Default is local-render (the app
    /// renders its own tree). `--aas` accepted as a deprecated alias.
    #[arg(long, alias = "aas")]
    pub runtime_server: bool,

    /// Roku only: device IP address (shown on the dev-mode-enable
    /// screen).
    #[arg(long)]
    pub device_ip: Option<String>,

    /// Roku only: developer password (set during dev-mode enable).
    /// Falls back to $ROKU_DEV_PASSWORD.
    #[arg(long)]
    pub password: Option<String>,

    /// `--runtime-server` only: the dev-host's bound port. The
    /// wrapper is built with `IDEALYST_DEV_ENDPOINT=ws://127.0.0.1:<port>`
    /// for spawned platforms (sim/emulator/macOS/terminal) or the
    /// LAN-IP-baked equivalent for device builds. `--aas-port`
    /// accepted as a deprecated alias.
    #[arg(long, alias = "aas-port")]
    pub runtime_server_port: Option<u16>,

    /// Roku only: stream the device's BrightScript console after
    /// install.
    #[arg(long)]
    pub console: bool,

    /// Sim only: window form factor. Defaults to `phone`.
    #[arg(long, value_enum, default_value_t = SimForm::Phone)]
    pub form: SimForm,

    /// Sim only: chrome skin. Defaults to `ios`.
    #[arg(long, value_enum, default_value_t = SimPainter::Ios)]
    pub skin: SimPainter,

    /// `run server` only: skip building the web bundle first. By
    /// default `idealyst run server` runs `build --web` so `dist/web`
    /// is fresh before launching (the server serves that directory);
    /// pass this to run the server bin against whatever is already
    /// staged.
    #[arg(long)]
    pub no_build: bool,

    /// `run server` only: port for the server to bind, forwarded to the
    /// server binary via the `PORT` environment variable. The server
    /// must read `PORT` to honor it (the convention the server SDK
    /// examples follow); the SDK default is 3000.
    #[arg(long)]
    pub port: Option<u16>,
}

fn loopback_endpoint(args: &Args) -> anyhow::Result<String> {
    let port = args.runtime_server_port.ok_or_else(|| {
        anyhow::anyhow!(
            "--runtime-server requires --runtime-server-port <PORT> in `idealyst run`. \
             Start a dev-host first (`idealyst dev`) and pass its bound port here."
        )
    })?;
    Ok(format!("ws://127.0.0.1:{port}"))
}

pub fn run(mut args: Args) -> anyhow::Result<()> {
    // Canonicalize the project dir so framework-source detection
    // can walk ancestors reliably. Without this, `.` (the default)
    // has no parent path components for `find_framework_workspace`
    // to inspect, so in-workspace projects silently fall through to
    // git mode. Mirrors what `cmd::dev::run` already does.
    args.dir = std::fs::canonicalize(&args.dir).with_context(|| {
        format!("cannot resolve project dir {}", args.dir.display())
    })?;
    match args.platform {
        Platform::Ios if args.device => {
            // Physical-device path: build a signed .app via xcodebuild and
            // install with ios-deploy. Mutually exclusive with
            // `--runtime-server` (the device path is local-mount only today).
            if args.runtime_server {
                anyhow::bail!(
                    "`idealyst run ios --device` does not support `--runtime-server` yet \
                     — the device path is local-mount only. Drop `--runtime-server` to \
                     deploy a standalone signed build to the phone."
                );
            }
            let team = run_ios::device::resolve_team(args.team.as_deref())?;
            eprintln!("[idealyst run ios --device] signing team {team}");
            // Default RELEASE for device (debug is unusably slow on-device);
            // `--debug` flips it back, `--release` is a no-op (already on).
            let release = !args.debug;
            let source = crate::framework_source::resolve(&args.dir)?;
            let artifact = run_ios::device::run(
                &args.dir,
                run_ios::device::DeviceOptions {
                    release,
                    source,
                    user_features: Vec::new(),
                    team,
                    udid: args.udid.clone(),
                    clean: args.clean,
                },
            )?;
            eprintln!();
            eprintln!("[idealyst run ios --device] installed + launched");
            eprintln!("  app:    {}", artifact.app_bundle.display());
            eprintln!("  device: {}", artifact.device_udid);
            eprintln!("  proj:   {}", artifact.xcodeproj.display());
            Ok(())
        }
        Platform::Ios => {
            let mode = if args.runtime_server {
                run_ios::RunMode::RuntimeServer {
                    endpoint: loopback_endpoint(&args)?,
                }
            } else {
                run_ios::RunMode::Local
            };
            let source = crate::framework_source::resolve(&args.dir)?;
            let artifact = run_ios::run(
                &args.dir,
                run_ios::RunOptions {
                    release: args.release,
                    mode,
                    source,
                    user_features: Vec::new(),
                    clean: args.clean,
                },
            )?;
            eprintln!();
            match &artifact.mode {
                run_ios::RunMode::Local => {
                    eprintln!("[idealyst run ios] launched");
                }
                run_ios::RunMode::RuntimeServer { endpoint } => {
                    eprintln!("[idealyst run ios --runtime-server] launched (endpoint {endpoint})");
                }
            }
            eprintln!("  app:       {}", artifact.app_bundle.display());
            eprintln!("  simulator: {}", artifact.simulator_udid);
            Ok(())
        }
        Platform::Roku => {
            // Build first (auto-snapshots via the wrapper + repacks
            // every invocation). Side-loading without a fresh build
            // is an antipattern; we don't expose `--no-build` yet.
            let source = crate::framework_source::resolve(&args.dir)?;
            let built = build_roku::build(
                &args.dir,
                build_roku::BuildOptions {
                    output_dir: None,
                    ui_json: None,
                    title: None,
                    source,
                },
            )?;
            eprintln!(
                "[idealyst run roku] built {} ({} methods, {} commands)",
                built.package_dir.display(),
                built.method_count,
                built.command_count
            );

            let device_ip = args
                .device_ip
                .or_else(|| std::env::var("ROKU_DEV_IP").ok())
                .ok_or_else(|| anyhow::anyhow!(
                    "missing --device-ip (or $ROKU_DEV_IP). Enable developer mode on \
                     your Roku (Home×3, Up×2, Right, Left, Right, Left, Right) and \
                     re-run with --device-ip <ip>"
                ))?;
            let password = args
                .password
                .or_else(|| std::env::var("ROKU_DEV_PASSWORD").ok())
                .ok_or_else(|| anyhow::anyhow!(
                    "missing --password (or $ROKU_DEV_PASSWORD). The password was set \
                     when you enabled developer mode on the device"
                ))?;

            let artifact = run_roku::run(run_roku::RunOptions {
                device_ip: device_ip.clone(),
                password,
                console: args.console,
                zip_path: built.zip_path,
            })?;
            eprintln!();
            eprintln!("[idealyst run roku] installed");
            eprintln!("  device: {}", device_ip);
            eprintln!("  zip:    {}", artifact.zip_path.display());
            if !args.console {
                eprintln!();
                eprintln!(
                    "Tip: pass --console to stream `?` output and crash dumps from \
                     the device's debug port (8085)."
                );
            }
            Ok(())
        }
        Platform::Android => {
            let mode = if args.runtime_server {
                run_android::RunMode::RuntimeServer
            } else {
                run_android::RunMode::Local
            };
            // `idealyst run android --runtime-server` requires the
            // dev-host's port explicitly. We don't pre-discover; the
            // user runs `idealyst dev` separately or passes the port
            // they noted from a prior dev session.
            let port = if args.runtime_server {
                Some(loopback_endpoint(&args).map(|_| args.runtime_server_port.unwrap())?)
            } else {
                None
            };
            let source = crate::framework_source::resolve(&args.dir)?;
            let artifact = run_android::run(
                &args.dir,
                run_android::RunOptions {
                    release: args.release,
                    avd: None,
                    mode,
                    runtime_server_port: port,
                    source,
                    user_features: Vec::new(),
                    // `idealyst run android` doesn't host a dev relay.
                    robot_relay_url: None,
                },
            )?;
            eprintln!();
            match mode {
                run_android::RunMode::Local => {
                    eprintln!("[idealyst run android] launched");
                }
                run_android::RunMode::RuntimeServer => {
                    eprintln!(
                        "[idealyst run android --runtime-server] launched (port {})",
                        port.unwrap(),
                    );
                }
            }
            eprintln!("  apk:    {}", artifact.apk.display());
            eprintln!("  device: {}", artifact.serial);
            Ok(())
        }
        Platform::Sim => {
            let source = crate::framework_source::resolve(&args.dir)?;
            let form = match args.form {
                SimForm::Phone => build_sim::FormFactor::Phone,
                SimForm::Tablet => build_sim::FormFactor::Tablet,
                SimForm::Tv => build_sim::FormFactor::Tv,
            };
            let skin = match args.skin {
                SimPainter::Ios => build_sim::PainterChoice::Ios,
                SimPainter::Android => build_sim::PainterChoice::Android,
            };
            let mode = if args.runtime_server {
                build_sim::BuildMode::RuntimeServer
            } else {
                build_sim::BuildMode::Local
            };
            let endpoint = if args.runtime_server {
                Some(loopback_endpoint(&args)?)
            } else {
                None
            };
            let artifact = build_sim::build(
                &args.dir,
                build_sim::BuildOptions {
                    release: args.release,
                    form,
                    skin,
                    mode,
                    source,
                },
            )?;
            eprintln!();
            eprintln!(
                "[idealyst run sim] launching {} ({} / {})",
                artifact.binary.display(),
                form.as_str(),
                skin.as_str(),
            );
            // Foreground the sim binary — exits when the user closes
            // the window. Inherit stdio so framework logs surface
            // alongside the CLI's.
            let mut cmd = std::process::Command::new(&artifact.binary);
            if let Some(endpoint) = endpoint {
                cmd.env("IDEALYST_DEV_ENDPOINT", endpoint);
            }
            let status = cmd
                .status()
                .with_context(|| format!("spawn sim binary {}", artifact.binary.display()))?;
            if !status.success() {
                anyhow::bail!("sim binary exited with {status}");
            }
            Ok(())
        }
        Platform::Macos => {
            let mode = if args.runtime_server {
                run_macos::RunMode::RuntimeServer
            } else {
                run_macos::RunMode::Local
            };
            let mut env_vars: Vec<(String, String)> = Vec::new();
            if args.runtime_server {
                env_vars.push((
                    "IDEALYST_DEV_ENDPOINT".to_string(),
                    loopback_endpoint(&args)?,
                ));
            }
            let source = crate::framework_source::resolve(&args.dir)?;
            let artifact = run_macos::run(
                &args.dir,
                run_macos::RunOptions {
                    release: args.release,
                    mode,
                    source,
                    // One-shot `idealyst run macos` is a foreground
                    // session — block on the app so Ctrl-C in the
                    // terminal still tears it down cleanly.
                    background: false,
                    user_features: Vec::new(),
                    env_vars,
                },
            )?;
            eprintln!();
            eprintln!("[idealyst run macos] launched");
            eprintln!("  binary: {}", artifact.binary.display());
            Ok(())
        }
        Platform::Terminal => {
            let mode = if args.runtime_server {
                run_terminal::RunMode::RuntimeServer
            } else {
                run_terminal::RunMode::Local
            };
            let mut env_vars: Vec<(String, String)> = Vec::new();
            if args.runtime_server {
                env_vars.push((
                    "IDEALYST_DEV_ENDPOINT".to_string(),
                    loopback_endpoint(&args)?,
                ));
            }
            let source = crate::framework_source::resolve(&args.dir)?;
            let artifact = run_terminal::run(
                &args.dir,
                run_terminal::RunOptions {
                    release: args.release,
                    mode,
                    source,
                    user_features: Vec::new(),
                    env_vars,
                },
            )?;
            eprintln!();
            eprintln!("[idealyst run terminal] exited");
            eprintln!("  binary: {}", artifact.binary.display());
            Ok(())
        }
        Platform::Server => run_server(&args),
        _ => anyhow::bail!(
            "run for {} is not implemented yet — only ios, android, roku, sim, macos, terminal, and server are wired today",
            args.platform,
        ),
    }
}

/// `idealyst run server` — build the web bundle (unless `--no-build`),
/// then foreground the project's `#[server]` backend.
///
/// Two project shapes are supported, both declared under
/// `[package.metadata.idealyst.app]`:
///
/// - **Standalone server workspace** (`server_manifest = "…/Cargo.toml"`):
///   the recommended shape for real apps. The server is its own
///   `[workspace]` so its server-only deps never feature-unify into the
///   client build. We `cargo run --manifest-path <server_manifest>`
///   (adding `--bin <server_bin>` when set).
/// - **In-crate bin** (`server_bin = "…"` only): the toy / single-crate
///   shape used by `examples/server-fn-demo`. We `cargo run -p <pkg>
///   --bin <server_bin> --features server`.
///
/// Either way the server serves the static `dist/web` bundle at `/`, so
/// we stage that first (a missing `dist/web` is the classic
/// 404-on-root trap). stdio is inherited and we block so Ctrl-C tears
/// the server down.
fn run_server(args: &Args) -> anyhow::Result<()> {
    let manifest = build_ios::parse_manifest(&args.dir)
        .with_context(|| format!("read manifest for {}", args.dir.display()))?;
    let app = &manifest.app;

    if app.server_manifest.is_none() && app.server_bin.is_none() {
        anyhow::bail!(
            "no server declared for this project. Pick one and add it to \
             {}/Cargo.toml:\n\n\
             \x20 # Standalone server workspace (recommended for real apps):\n\
             \x20 [package.metadata.idealyst.app]\n\
             \x20 server_manifest = \"../../server/Cargo.toml\"\n\
             \x20 server_bin       = \"server\"   # optional: names the bin\n\n\
             \x20 # …or an in-crate bin gated by the `server` feature:\n\
             \x20 [package.metadata.idealyst.app]\n\
             \x20 server_bin = \"server\"   # → src/bin/server.rs\n\n\
             then re-run `idealyst run server`.",
            args.dir.display(),
        );
    }

    // Stage `dist/web` before launching so the server has assets to
    // serve at `/`. Mirrors `idealyst build --web`'s output dir.
    // `--no-build` skips this to run against an already-staged bundle.
    if !args.no_build {
        let source = crate::framework_source::resolve(&args.dir)?;
        eprintln!("[idealyst run server] building web bundle → dist/web");
        build_web::build(
            &args.dir,
            build_web::BuildOptions {
                release: args.release,
                source,
                user_features: Vec::new(),
                bundle_out_dir: Some(args.dir.join("dist").join("web")),
                gzip: false,
                strip_panics: false,
                hydrate: false,
                prune_dead_data_min: None,
            },
        )
        .context("web bundle build for `run server` failed")?;
    }

    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("run");
    let how: String;
    if let Some(rel) = &app.server_manifest {
        // Standalone server workspace. Resolve the manifest path
        // relative to the app crate dir, run it as its own package.
        // No `--features server` here: the standalone workspace's own
        // deps already enable server mode (that isolation is the whole
        // point of it being a separate workspace).
        let joined = args.dir.join(rel);
        if !joined.is_file() {
            anyhow::bail!(
                "server_manifest points at {}, which doesn't exist. The path is \
                 resolved relative to the app crate dir ({}); fix \
                 `[package.metadata.idealyst.app].server_manifest`.",
                joined.display(),
                args.dir.display(),
            );
        }
        // Collapse the `../..` segments for tidy logging + a clean
        // `--manifest-path`. canonicalize can't fail — we just confirmed
        // it's a file.
        let manifest_path = std::fs::canonicalize(&joined).unwrap_or(joined);
        cmd.arg("--manifest-path").arg(&manifest_path);
        if let Some(bin) = &app.server_bin {
            cmd.arg("--bin").arg(bin);
        }
        how = format!(
            "--manifest-path {}{}",
            manifest_path.display(),
            app.server_bin
                .as_ref()
                .map(|b| format!(" --bin {b}"))
                .unwrap_or_default(),
        );
    } else {
        // In-crate bin gated by the `server` feature on the app package.
        let bin = app.server_bin.as_ref().expect("checked above");
        cmd.arg("-p")
            .arg(&manifest.name)
            .arg("--bin")
            .arg(bin)
            .arg("--features")
            .arg("server");
        how = format!("-p {} --bin {} --features server", manifest.name, bin);
    }
    if args.release {
        cmd.arg("--release");
    }
    if let Some(port) = args.port {
        cmd.env("PORT", port.to_string());
    }

    eprintln!(
        "[idealyst run server] cargo run {how}{}",
        if args.release { " --release" } else { "" },
    );
    let status = cmd
        .status()
        .with_context(|| format!("spawn `cargo run {how}`"))?;
    if !status.success() {
        anyhow::bail!(
            "server exited with {}",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! `idealyst run server` runs the project's `#[server]` backend.
    //! Two invariants worth pinning: the `server` value-enum variant
    //! must keep parsing (a rename would silently break the documented
    //! command), and a project without a declared `server_bin` must get
    //! an actionable error that names the manifest key — not a raw
    //! cargo failure or a panic.

    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: TestCmd,
    }

    #[derive(clap::Subcommand)]
    enum TestCmd {
        Run(Args),
    }

    fn parse(argv: &[&str]) -> Args {
        match TestCli::parse_from(argv).cmd {
            TestCmd::Run(a) => a,
        }
    }

    #[test]
    fn server_platform_parses_with_default_dir() {
        let args = parse(&["idealyst", "run", "server"]);
        assert_eq!(args.platform, Platform::Server);
        assert_eq!(args.dir, PathBuf::from("."));
        assert!(!args.no_build);
        assert!(args.port.is_none());
    }

    #[test]
    fn server_flags_parse() {
        let args = parse(&["idealyst", "run", "server", "--no-build", "--port", "4000", "--release"]);
        assert!(args.no_build);
        assert_eq!(args.port, Some(4000));
        assert!(args.release);
    }

    fn project_with(metadata: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            format!("[package]\nname = \"demo-app\"\nversion = \"0.0.0\"\n{metadata}"),
        )
        .unwrap();
        tmp
    }

    #[test]
    fn run_server_errors_without_any_server_declaration() {
        // A valid idealyst project manifest that declares neither a
        // server_bin nor a server_manifest.
        let tmp = project_with("");
        let mut args = parse(&["idealyst", "run", "server"]);
        // `--no-build` so the error path is reached without invoking a
        // web build (which would need the full framework toolchain).
        args.no_build = true;
        args.dir = tmp.path().to_path_buf();

        let err = run_server(&args).unwrap_err();
        let msg = format!("{err:#}");
        // The error must offer both shapes so the user isn't pushed
        // toward the in-crate `--features server` bin when their real
        // app needs the standalone-workspace `server_manifest`.
        assert!(
            msg.contains("server_manifest") && msg.contains("server_bin"),
            "missing-server error should name both manifest keys, got: {msg}",
        );
    }

    #[test]
    fn run_server_errors_when_server_manifest_path_missing() {
        // server_manifest is declared but points nowhere — the error
        // must name the resolved path and the relative-to-app-dir rule
        // rather than handing cargo a bogus --manifest-path.
        let tmp = project_with(
            "[package.metadata.idealyst.app]\nserver_manifest = \"../nope/Cargo.toml\"\n",
        );
        let mut args = parse(&["idealyst", "run", "server"]);
        args.no_build = true;
        args.dir = tmp.path().to_path_buf();

        let err = run_server(&args).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("server_manifest") && msg.contains("doesn't exist"),
            "bad server_manifest path should be reported, got: {msg}",
        );
    }
}
