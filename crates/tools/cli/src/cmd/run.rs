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
        _ => anyhow::bail!(
            "run for {} is not implemented yet — only ios, android, roku, sim, macos, and terminal are wired today",
            args.platform,
        ),
    }
}
