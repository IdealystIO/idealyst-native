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

/// Skin painting the chrome around the user's tree for
/// `idealyst run sim`. Independent of the form factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum SimSkin {
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
    #[arg(long)]
    pub release: bool,

    /// iOS / Android only: build the AAS-client variant and have
    /// it discover a running dev-host over the local network.
    /// Default is local-render (the app renders its own tree).
    #[arg(long)]
    pub aas: bool,

    /// Roku only: device IP address (shown on the dev-mode-enable
    /// screen).
    #[arg(long)]
    pub device_ip: Option<String>,

    /// Roku only: developer password (set during dev-mode enable).
    /// Falls back to $ROKU_DEV_PASSWORD.
    #[arg(long)]
    pub password: Option<String>,

    /// Android `--aas` only: explicit dev-server port to connect to.
    /// Required for emulator targets; physical devices auto-discover
    /// over the local network.
    #[arg(long)]
    pub aas_port: Option<u16>,

    /// Roku only: stream the device's BrightScript console after
    /// install.
    #[arg(long)]
    pub console: bool,

    /// Sim only: window form factor. Defaults to `phone`.
    #[arg(long, value_enum, default_value_t = SimForm::Phone)]
    pub form: SimForm,

    /// Sim only: chrome skin. Defaults to `ios`.
    #[arg(long, value_enum, default_value_t = SimSkin::Ios)]
    pub skin: SimSkin,
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
        Platform::Ios => {
            let mode = if args.aas {
                run_ios::RunMode::Aas
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
                run_ios::RunMode::Aas => {
                    eprintln!(
                        "[idealyst run ios --aas] launched (discovering host via Bonjour)"
                    );
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
            let mode = if args.aas {
                run_android::RunMode::Aas
            } else {
                run_android::RunMode::Local
            };
            let source = crate::framework_source::resolve(&args.dir)?;
            let artifact = run_android::run(
                &args.dir,
                run_android::RunOptions {
                    release: args.release,
                    avd: None,
                    mode,
                    // `idealyst run android --aas` doesn't pre-browse
                    // Bonjour the way `idealyst dev --aas --android`
                    // does. Without `--aas-port`, falls through to
                    // the in-app Bonjour path (works for physical
                    // devices but not the QEMU-NAT emulator). Pass
                    // `--aas-port` to point the APK at an explicit
                    // host (the same port the dev-server prints on
                    // startup).
                    aas_port: args.aas_port,
                    source,
                    user_features: Vec::new(),
                },
            )?;
            eprintln!();
            match mode {
                run_android::RunMode::Local => {
                    eprintln!("[idealyst run android] launched");
                }
                run_android::RunMode::Aas => {
                    eprintln!(
                        "[idealyst run android --aas] launched (discovering host via Bonjour)"
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
                SimSkin::Ios => build_sim::SkinChoice::Ios,
                SimSkin::Android => build_sim::SkinChoice::Android,
            };
            let artifact = build_sim::build(
                &args.dir,
                build_sim::BuildOptions {
                    release: args.release,
                    form,
                    skin,
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
            let status = std::process::Command::new(&artifact.binary)
                .status()
                .with_context(|| format!("spawn sim binary {}", artifact.binary.display()))?;
            if !status.success() {
                anyhow::bail!("sim binary exited with {status}");
            }
            Ok(())
        }
        Platform::Macos => {
            if args.aas {
                anyhow::bail!(
                    "macOS AAS mode is not implemented yet; run without --aas for local-render"
                );
            }
            let source = crate::framework_source::resolve(&args.dir)?;
            let artifact = run_macos::run(
                &args.dir,
                run_macos::RunOptions {
                    release: args.release,
                    source,
                    // One-shot `idealyst run macos` is a foreground
                    // session — block on the app so Ctrl-C in the
                    // terminal still tears it down cleanly.
                    background: false,
                    user_features: Vec::new(),
                    env_vars: Vec::new(),
                },
            )?;
            eprintln!();
            eprintln!("[idealyst run macos] launched");
            eprintln!("  binary: {}", artifact.binary.display());
            Ok(())
        }
        _ => anyhow::bail!(
            "run for {} is not implemented yet — only ios, android, roku, sim, and macos are wired today",
            args.platform,
        ),
    }
}
