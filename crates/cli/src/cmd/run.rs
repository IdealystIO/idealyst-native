use std::path::PathBuf;

use crate::Platform;

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

    /// iOS only: build the AAS-client variant of the app and have
    /// it discover a running AAS dev-host via Bonjour. Without this
    /// flag, the iOS app renders the user's tree locally and ships
    /// as a self-contained process. With it, the iOS process is a
    /// thin client driven by the dev-host's wire stream — no URL
    /// needed, the shell finds the right server by matching the
    /// project's bundle id against `_idealyst-dev._tcp.` records.
    #[arg(long)]
    pub aas: bool,

    /// Roku only: IP address of the device in developer mode. The
    /// device prints its IP on the dev-mode-enable screen.
    #[arg(long)]
    pub device_ip: Option<String>,

    /// Roku only: developer password set during dev-mode enable.
    /// Read from $ROKU_DEV_PASSWORD if not supplied.
    #[arg(long)]
    pub password: Option<String>,

    /// Roku only: after a successful install, stream the device's
    /// BrightScript debug console (port 8085) to stdout.
    #[arg(long)]
    pub console: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    match args.platform {
        Platform::Ios => {
            let mode = if args.aas {
                run_ios::RunMode::Aas
            } else {
                run_ios::RunMode::Local
            };
            let artifact = run_ios::run(
                &args.dir,
                run_ios::RunOptions {
                    release: args.release,
                    mode,
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
            let built = build_roku::build(
                &args.dir,
                build_roku::BuildOptions::default(),
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
            let artifact = run_android::run(
                &args.dir,
                run_android::RunOptions {
                    release: args.release,
                    avd: None,
                    mode,
                    // `idealyst run android --aas` doesn't pre-browse
                    // Bonjour the way `idealyst dev --aas --android`
                    // does. Falls through to the in-app Bonjour path,
                    // which works for physical devices but not the
                    // QEMU-NAT emulator. Use `dev --aas --android`
                    // for the emulator-friendly path.
                    aas_port: None,
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
        _ => anyhow::bail!(
            "run for {} is not implemented yet — only ios, android, and roku are wired today",
            args.platform,
        ),
    }
}
