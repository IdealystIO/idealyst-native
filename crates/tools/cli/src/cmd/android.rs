//! `idealyst android` — Android-only tooling.
//!
//! `provisioning-qr` prints the QR code that provisions a device-owner app
//! on a freshly reset device: tap the setup wizard's welcome screen six
//! times, scan it, and Android downloads the APK from `--download-url` and
//! makes its device-admin receiver the device owner. The receiver, package
//! and signing certificate are read from the APK, so build it first
//! (`idealyst build --android --release`) and serve that exact file.

use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(clap::Subcommand, Debug)]
pub enum Action {
    /// Print the QR provisioning payload (JSON) and the QR code for a
    /// device-owner app.
    ProvisioningQr {
        /// Project directory. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// The signed APK the device will download. Defaults to the
        /// project's `dist/android/<name>-<version>.apk` from
        /// `idealyst build --android`.
        #[arg(long)]
        apk: Option<PathBuf>,
        /// The https URL serving that APK.
        #[arg(long)]
        download_url: String,
        /// A JSON object handed to the app when provisioning completes
        /// (`PROVISIONING_ADMIN_EXTRAS_BUNDLE`), e.g. its server URL and an
        /// enrolment token.
        #[arg(long)]
        extras: Option<String>,
        /// Also write the QR code as an SVG file.
        #[arg(long)]
        svg: Option<PathBuf>,
    },
}

pub fn run(args: Args) -> Result<()> {
    match args.action {
        Action::ProvisioningQr {
            dir,
            apk,
            download_url,
            extras,
            svg,
        } => provisioning_qr(dir, apk, &download_url, extras.as_deref(), svg),
    }
}

fn provisioning_qr(
    dir: PathBuf,
    apk: Option<PathBuf>,
    download_url: &str,
    extras: Option<&str>,
    svg: Option<PathBuf>,
) -> Result<()> {
    let apk = match apk {
        Some(apk) => apk,
        None => {
            let manifest = build_ios::parse_manifest(&dir)?;
            let apk = super::build::android_dist_apk(&dir, &manifest);
            if !apk.is_file() {
                anyhow::bail!(
                    "{} not found — run `idealyst build --android --release` first, or pass --apk",
                    apk.display()
                );
            }
            apk
        }
    };
    let extras: Option<serde_json::Value> = extras
        .map(|e| serde_json::from_str(e).context("--extras is not valid JSON"))
        .transpose()?;
    let info = run_android::provisioning::inspect_apk(&apk)?;
    let payload = run_android::provisioning::payload(&info, download_url, extras.as_ref())?;
    let text = serde_json::to_string(&payload)?;

    let code = qrcode::QrCode::new(text.as_bytes()).context("encode the payload as a QR code")?;
    let terminal = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build();
    eprintln!("Device owner component: {}", info.component());
    eprintln!("Signing certificate SHA-256 (checksum): {}", info.signature_checksum());
    eprintln!("adb alternative: adb shell dpm set-device-owner {}", info.component());
    println!("{}", serde_json::to_string_pretty(&payload)?);
    println!("{terminal}");
    if let Some(path) = svg {
        let image = code
            .render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(512, 512)
            .build();
        std::fs::write(&path, image).with_context(|| format!("write {}", path.display()))?;
        eprintln!("QR code written to {}", path.display());
    }
    Ok(())
}
