//! `idealyst publish ios` — build a distribution-signed `.ipa` and,
//! optionally, upload it to App Store Connect.
//!
//! This is the distribution sibling of `idealyst run ios --device`: that
//! command signs with a *development* identity and installs to a connected
//! phone; `publish` produces an *App Store* archive (`"Apple Distribution"`
//! signing) and exports/uploads an `.ipa`. The heavy lifting lives in
//! [`run_ios::publish`]; this handler just resolves credentials and the
//! signing team from flags/env and dispatches.
//!
//! Today only `ios` is wired — the positional platform arg keeps room to
//! add `android` (Google Play) without changing the command surface.

use std::path::PathBuf;

use anyhow::Result;
use run_ios::publish::{PublishOptions, UploadAuth};

use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target platform. Only `ios` is supported today.
    #[arg(value_enum)]
    pub platform: Platform,

    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Also upload the archive to App Store Connect. Without this flag the
    /// command stops after writing a distribution-signed `.ipa`.
    #[arg(long)]
    pub upload: bool,

    /// Override `CFBundleVersion` (the build number) for this archive.
    /// App Store Connect rejects a re-used build number for the same app
    /// version, so bump this each upload. Defaults to the manifest's
    /// `[package.metadata.idealyst.app].build_number` (itself `"1"`).
    #[arg(long)]
    pub build_number: Option<String>,

    /// Apple Developer team ID (the 10-char identifier). Falls back to
    /// `$IDEALYST_DEVELOPMENT_TEAM` / `$DEVELOPMENT_TEAM`, then auto-detects
    /// from your installed signing certificate.
    #[arg(long)]
    pub team: Option<String>,

    /// App Store Connect API key ID. Falls back to `$ASC_KEY_ID`. Pair with
    /// `--issuer-id` and `--api-key-path` for a headless/CI-safe signing +
    /// upload (recommended over relying on the Xcode-stored account).
    #[arg(long)]
    pub api_key_id: Option<String>,

    /// App Store Connect issuer ID (a UUID). Falls back to `$ASC_ISSUER_ID`.
    #[arg(long)]
    pub issuer_id: Option<String>,

    /// Path to the App Store Connect API private key (`AuthKey_<id>.p8`).
    /// Falls back to `$ASC_KEY_PATH`.
    #[arg(long)]
    pub api_key_path: Option<PathBuf>,

    /// Where the `.ipa` (and `.xcarchive`) land. Defaults to
    /// `<project>/dist/ios`.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    if args.platform != Platform::Ios {
        anyhow::bail!(
            "`idealyst publish` currently supports only `ios` (App Store \
             Connect); `{}` is not wired yet.",
            args.platform,
        );
    }

    let team = run_ios::device::resolve_team(args.team.as_deref())?;
    eprintln!("[publish ios] signing team {team}");

    let auth = resolve_auth(&args)?;
    let source = crate::framework_source::resolve(&args.dir)?;
    let output_dir = args
        .out
        .clone()
        .unwrap_or_else(|| args.dir.join("dist").join("ios"));

    let artifact = run_ios::publish::publish(
        &args.dir,
        PublishOptions {
            team,
            source,
            user_features: Vec::new(),
            build_number: args.build_number.clone(),
            auth,
            upload: args.upload,
            output_dir,
        },
    )?;

    if let Some(ipa) = &artifact.ipa {
        eprintln!("[publish ios] exported {}", ipa.display());
    }
    if artifact.uploaded {
        eprintln!(
            "[publish ios] uploaded to App Store Connect — the build will appear \
             under TestFlight / App Store once Apple finishes processing.",
        );
    } else {
        eprintln!(
            "[publish ios] done. Upload with `--upload` (provide an App Store \
             Connect API key), or drag the .ipa into Transporter.",
        );
    }
    Ok(())
}

/// Resolve App Store Connect credentials from flags, falling back to the
/// `ASC_*` env vars. Returns:
/// - `Some(ApiKey{..})` when all three of (key id, issuer id, key path) are
///   present — the recommended headless path,
/// - `Some(XcodeAccount)` when `--upload` is requested but no key was given
///   (lean on the Xcode-stored session),
/// - `None` when neither uploading nor a key is in play (local archive/export
///   signs via the logged-in Xcode account).
///
/// A partially-specified key (e.g. id but no `.p8`) is a hard error — silently
/// downgrading to the Xcode account would be a confusing surprise.
fn resolve_auth(args: &Args) -> Result<Option<UploadAuth>> {
    let key_id = args
        .api_key_id
        .clone()
        .or_else(|| env_nonempty("ASC_KEY_ID"));
    let issuer_id = args
        .issuer_id
        .clone()
        .or_else(|| env_nonempty("ASC_ISSUER_ID"));
    let key_path = args
        .api_key_path
        .clone()
        .or_else(|| env_nonempty("ASC_KEY_PATH").map(PathBuf::from));

    match (key_id, issuer_id, key_path) {
        (Some(key_id), Some(issuer_id), Some(key_path)) => {
            if !key_path.is_file() {
                anyhow::bail!(
                    "App Store Connect API key not found at {} (set via \
                     --api-key-path / $ASC_KEY_PATH)",
                    key_path.display(),
                );
            }
            Ok(Some(UploadAuth::ApiKey {
                key_id,
                issuer_id,
                key_path,
            }))
        }
        (None, None, None) => {
            // No key at all. If we're uploading we still need *some* auth —
            // lean on the Xcode account; otherwise leave it unset.
            Ok(args.upload.then_some(UploadAuth::XcodeAccount))
        }
        _ => anyhow::bail!(
            "incomplete App Store Connect API key: provide all of --api-key-id, \
             --issuer-id, and --api-key-path (or $ASC_KEY_ID / $ASC_ISSUER_ID / \
             $ASC_KEY_PATH), or none of them to use the Xcode-stored account.",
        ),
    }
}

/// Read an env var, treating empty as unset.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(
        upload: bool,
        api_key_id: Option<&str>,
        issuer_id: Option<&str>,
        api_key_path: Option<&str>,
    ) -> Args {
        Args {
            platform: Platform::Ios,
            dir: PathBuf::from("."),
            upload,
            build_number: None,
            team: None,
            api_key_id: api_key_id.map(str::to_string),
            issuer_id: issuer_id.map(str::to_string),
            api_key_path: api_key_path.map(PathBuf::from),
            out: None,
        }
    }

    #[test]
    fn no_key_no_upload_means_no_auth() {
        // Build-only with no key → rely on the logged-in Xcode account
        // implicitly (no UploadAuth needed).
        let resolved = resolve_auth(&args(false, None, None, None)).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn upload_without_key_falls_back_to_xcode_account() {
        let resolved = resolve_auth(&args(true, None, None, None)).unwrap();
        assert!(matches!(resolved, Some(UploadAuth::XcodeAccount)));
    }

    #[test]
    fn partial_key_is_an_error() {
        // Key id but no issuer / path → hard error, not a silent downgrade.
        let err = resolve_auth(&args(true, Some("KID"), None, None)).unwrap_err();
        assert!(err.to_string().contains("incomplete App Store Connect API key"));
    }

    #[test]
    fn complete_key_with_missing_file_is_an_error() {
        let err = resolve_auth(&args(
            true,
            Some("KID"),
            Some("ISS"),
            Some("/nonexistent/AuthKey_KID.p8"),
        ))
        .unwrap_err();
        assert!(err.to_string().contains("API key not found"));
    }
}
