//! App Store Connect distribution for `idealyst publish ios`.
//!
//! The device path ([`crate::device`]) signs with a development identity and
//! installs to a connected phone. App Store distribution reuses that same
//! development-signed build for the *archive*, then re-signs for distribution
//! at export — the standard Xcode "Archive → Distribute App" flow:
//!
//! ```text
//!   build-ios::build (device, release)        → libNAME_ios_wrapper.a
//!   prepare_xcode_project                      → .xcodeproj (automatic dev signing)
//!   xcodebuild … -configuration Release archive → NAME.xcarchive (dev-signed)
//!   xcodebuild -exportArchive (ExportOptions)  → NAME.ipa   (destination=export)
//!     · method=app-store-connect re-signs the    └→ upload     (destination=upload)
//!       app with the DISTRIBUTION cert + profile
//! ```
//!
//! Hard-coding an "Apple Distribution" identity on the archive while
//! `CODE_SIGN_STYLE = Automatic` is what `xcodebuild archive` rejects with
//! "the … code signing identity has been manually specified" — which is why
//! the distribution identity lives only in the export step.
//!
//! ## Why `generic/platform=iOS`, not `id=<UDID>`
//!
//! The device-install path targets a concrete `id=<UDID>` so provisioning
//! can register *that phone* (see [`crate::device`] gotcha 3). An archive is
//! device-independent — `generic/platform=iOS` is correct, and a concrete
//! device isn't required (or wanted) for App Store builds.
//!
//! ## Credentials (both mechanisms)
//!
//! An **App Store Connect API key** ([`UploadAuth::ApiKey`] — key id +
//! issuer id + `AuthKey_<id>.p8`) is the recommended path: it lets automatic
//! signing mint the dev (archive) and distribution (export) profiles
//! headlessly and authorizes the upload, with no Apple ID password or 2FA.
//! Passed to `xcodebuild` via `-authenticationKeyPath/-authenticationKeyID/
//! -authenticationKeyIssuerID` on both the archive and export invocations.
//!
//! Without a key we fall back to the locally signed-in **Xcode account**
//! ([`UploadAuth::XcodeAccount`]) — fine for local archive/export, but not
//! viable headless/CI for upload.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{BuildOptions, FrameworkSource};

use crate::device::{prepare_xcode_project, PrepareOpts};

/// App Store Connect credentials, used for distribution signing and upload.
#[derive(Clone, Debug)]
pub enum UploadAuth {
    /// App Store Connect API key (recommended; headless/CI). `key_path`
    /// points at the `AuthKey_<KEY_ID>.p8` private key downloaded from
    /// App Store Connect → Users and Access → Integrations → App Store
    /// Connect API.
    ApiKey {
        key_id: String,
        issuer_id: String,
        key_path: PathBuf,
    },
    /// Rely on the App Store Connect session stored by Xcode. Works locally
    /// when the user is signed into Xcode (Settings → Accounts); not viable
    /// headless.
    XcodeAccount,
}

#[derive(Clone, Debug)]
pub struct PublishOptions {
    /// Apple Developer team ID (resolved by the CLI via
    /// [`crate::device::resolve_team`]).
    pub team: String,
    /// Where the wrapper sources framework crates from.
    pub source: FrameworkSource,
    /// Cargo features forwarded to the staticlib build.
    pub user_features: Vec<String>,
    /// `CFBundleVersion` override. `None` ⇒ use the manifest's
    /// `build_number` (which itself defaults to `"1"`). App Store Connect
    /// rejects a re-used build number for the same version, so this is the
    /// usual per-upload bump.
    pub build_number: Option<String>,
    /// Credentials for the distribution re-sign / upload. `None` ⇒ rely on
    /// the locally signed-in Xcode account.
    pub auth: Option<UploadAuth>,
    /// What to do after the archive is built. See [`Distribution`].
    pub distribution: Distribution,
    /// Where the `.ipa` (and `.xcarchive`) land. The CLI defaults this to
    /// `<project>/dist/ios`.
    pub output_dir: PathBuf,
}

/// What `publish` does with the archive after building it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distribution {
    /// Export a distribution-signed `.ipa` to `output_dir` and stop. The
    /// user uploads it themselves (Transporter, etc.).
    Ipa,
    /// Export and upload straight to App Store Connect (`destination=upload`).
    Upload,
    /// Stop after the `.xcarchive` — do NOT export. The caller hands the
    /// archive to Xcode's Organizer, which performs the distribution re-sign
    /// + upload interactively (the `--interactive` path). Crucially this needs
    /// no distribution certificate at CLI time — Organizer handles it — so
    /// running the export here would fail for exactly the users who chose it.
    ArchiveOnly,
}

#[derive(Debug)]
pub struct PublishArtifact {
    /// The exported `.ipa`, when one was produced. `destination=upload`
    /// runs may not leave a local `.ipa` behind, so this is `None` then.
    pub ipa: Option<PathBuf>,
    /// The `.xcarchive` xcodebuild produced (kept for re-export / Organizer).
    pub archive: PathBuf,
    /// Whether the archive was uploaded to App Store Connect.
    pub uploaded: bool,
}

/// Build a distribution-signed archive of the user's project and export it
/// to an `.ipa` (and optionally upload to App Store Connect). See module
/// docs for the full sequence and the credential model.
pub fn publish(project_dir: &Path, opts: PublishOptions) -> Result<PublishArtifact> {
    let project_dir = std::fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let mut manifest = build_ios::parse_manifest(&project_dir)?;
    // Fail fast on a missing bundle id before the slow build.
    manifest.app.require_bundle_id()?;
    // Per-upload build-number override (CFBundleVersion).
    if let Some(build_number) = &opts.build_number {
        manifest.app.build_number = build_number.clone();
    }

    if opts.distribution == Distribution::Upload
        && matches!(opts.auth, None | Some(UploadAuth::XcodeAccount))
    {
        eprintln!(
            "[publish ios] no App Store Connect API key provided — relying on the \
             Xcode-stored account for upload. Pass --api-key-id / --issuer-id / \
             --api-key-path (or set ASC_KEY_ID / ASC_ISSUER_ID / ASC_KEY_PATH) for \
             a headless/CI-safe upload.",
        );
    }

    // ── 1. Build the wrapper staticlib (device target, release) ──
    let artifact = build_ios::build(
        &project_dir,
        BuildOptions {
            release: true,
            device: true,
            source: opts.source.clone(),
            user_features: opts.user_features.clone(),
        },
    )?;

    // ── 2. Lay out the .xcodeproj (shared with the device-install path —
    // see [`prepare_xcode_project`]). The archive is built with automatic
    // DEVELOPMENT signing; the App Store *distribution* re-sign happens at
    // the `-exportArchive` step below (forcing an "Apple Distribution"
    // identity under automatic signing makes `xcodebuild archive` fail). ──
    let prepared = prepare_xcode_project(
        &project_dir,
        &manifest,
        &artifact,
        &PrepareOpts {
            team: opts.team.clone(),
            subdir: "ios-dist",
            source: opts.source.clone(),
        },
    )?;

    std::fs::create_dir_all(&opts.output_dir)
        .with_context(|| format!("create output dir {}", opts.output_dir.display()))?;
    let output_dir = std::fs::canonicalize(&opts.output_dir)
        .with_context(|| format!("resolve output dir {}", opts.output_dir.display()))?;

    // ── 3. xcodebuild archive (automatic development signing) ────
    let archive = prepared
        .project_root
        .join(format!("{}.xcarchive", prepared.scheme));
    xcodebuild_archive(
        &prepared.xcodeproj,
        &prepared.scheme,
        &archive,
        opts.auth.as_ref(),
    )?;

    // ── 4. Export / upload (skipped entirely for ArchiveOnly — see
    // [`Distribution::ArchiveOnly`]; the caller drives Organizer). ───
    let destination = match opts.distribution {
        Distribution::Ipa => ExportDestination::Export,
        Distribution::Upload => ExportDestination::Upload,
        Distribution::ArchiveOnly => {
            return Ok(PublishArtifact {
                ipa: None,
                archive,
                uploaded: false,
            });
        }
    };
    let export_options_path = prepared.project_root.join("ExportOptions.plist");
    std::fs::write(
        &export_options_path,
        export_options_plist(&opts.team, destination),
    )?;
    let export_dir = output_dir.join("export");
    // Fresh export dir each run — `xcodebuild -exportArchive` refuses to
    // write into a non-empty directory.
    if export_dir.exists() {
        std::fs::remove_dir_all(&export_dir)
            .with_context(|| format!("clear export dir {}", export_dir.display()))?;
    }
    xcodebuild_export(
        &archive,
        &export_options_path,
        &export_dir,
        opts.auth.as_ref(),
    )?;

    // The exported `.ipa` (export destination) lands in `export_dir`; move
    // it up to `output_dir` for a stable, predictable path.
    let ipa = match find_ipa(&export_dir) {
        Some(src) => {
            let dest = output_dir.join(src.file_name().expect("ipa has filename"));
            std::fs::rename(&src, &dest)
                .with_context(|| format!("move {} → {}", src.display(), dest.display()))?;
            Some(dest)
        }
        None => None,
    };

    Ok(PublishArtifact {
        ipa,
        archive,
        uploaded: opts.distribution == Distribution::Upload,
    })
}

/// Whether the export step writes an `.ipa` locally or uploads to App Store
/// Connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportDestination {
    /// Write the signed `.ipa` to the export path.
    Export,
    /// Upload directly to App Store Connect (no local `.ipa` guaranteed).
    Upload,
}

impl ExportDestination {
    fn as_plist_value(self) -> &'static str {
        match self {
            ExportDestination::Export => "export",
            ExportDestination::Upload => "upload",
        }
    }
}

/// Build the `ExportOptions.plist` body for `xcodebuild -exportArchive`.
///
/// Pure (no IO) so it's unit-testable. `method = app-store-connect`
/// (the App Store distribution method on Xcode 15.3+), automatic signing,
/// dSYM upload on. Credentials are passed to `xcodebuild` as CLI flags (see
/// [`append_auth_flags`]) rather than embedded here, so the key path never
/// has to be written into a file on disk.
fn export_options_plist(team: &str, destination: ExportDestination) -> String {
    let dest = destination.as_plist_value();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
    <key>method</key>\n\
    <string>app-store-connect</string>\n\
    <key>destination</key>\n\
    <string>{dest}</string>\n\
    <key>teamID</key>\n\
    <string>{team}</string>\n\
    <key>signingStyle</key>\n\
    <string>automatic</string>\n\
    <key>uploadSymbols</key>\n\
    <true/>\n\
</dict>\n\
</plist>\n"
    )
}

/// Append the App Store Connect API-key authentication flags to an
/// `xcodebuild` invocation when an API key is available. A no-op for
/// [`UploadAuth::XcodeAccount`] / `None` — those rely on the stored Xcode
/// session.
fn append_auth_flags(cmd: &mut Command, auth: Option<&UploadAuth>) {
    if let Some(UploadAuth::ApiKey {
        key_id,
        issuer_id,
        key_path,
    }) = auth
    {
        cmd.arg("-authenticationKeyPath")
            .arg(key_path)
            .args(["-authenticationKeyID", key_id])
            .args(["-authenticationKeyIssuerID", issuer_id]);
    }
}

/// `xcodebuild … -configuration Release -destination 'generic/platform=iOS'
/// archive`. Signed with automatic DEVELOPMENT signing (the pbxproj's
/// default); `-allowProvisioningUpdates` (+ the API key when present) lets
/// automatic signing mint the profile. The App Store distribution re-sign
/// happens at `-exportArchive` ([`xcodebuild_export`]) via the
/// `app-store-connect` method — NOT here. Hard-coding an "Apple
/// Distribution" identity under automatic signing makes archive fail.
fn xcodebuild_archive(
    xcodeproj: &Path,
    scheme: &str,
    archive: &Path,
    auth: Option<&UploadAuth>,
) -> Result<()> {
    eprintln!("[publish ios] xcodebuild archive → {}", archive.display());
    let mut cmd = Command::new("xcodebuild");
    cmd.arg("-project")
        .arg(xcodeproj)
        .args(["-scheme", scheme])
        .args(["-configuration", "Release"])
        .args(["-destination", "generic/platform=iOS"])
        .arg("-archivePath")
        .arg(archive)
        .arg("-allowProvisioningUpdates");
    append_auth_flags(&mut cmd, auth);
    cmd.arg("archive");
    let status = cmd.status().with_context(|| "spawn xcodebuild archive")?;
    if !status.success() {
        anyhow::bail!(
            "xcodebuild archive failed (exit {status}). Common causes: the team \
             can't sign for App Store distribution (no \"Apple Distribution\" \
             certificate), or automatic signing couldn't mint a distribution \
             profile. Provide an App Store Connect API key (--api-key-id / \
             --issuer-id / --api-key-path) or open {} in Xcode once and configure \
             Signing & Capabilities.",
            xcodeproj.display(),
        );
    }
    Ok(())
}

/// `xcodebuild -exportArchive …`. With `destination=export` this writes the
/// signed `.ipa`; with `destination=upload` it pushes straight to App Store
/// Connect.
fn xcodebuild_export(
    archive: &Path,
    export_options: &Path,
    export_dir: &Path,
    auth: Option<&UploadAuth>,
) -> Result<()> {
    eprintln!(
        "[publish ios] xcodebuild -exportArchive → {}",
        export_dir.display(),
    );
    let mut cmd = Command::new("xcodebuild");
    cmd.arg("-exportArchive")
        .arg("-archivePath")
        .arg(archive)
        .arg("-exportOptionsPlist")
        .arg(export_options)
        .arg("-exportPath")
        .arg(export_dir)
        .arg("-allowProvisioningUpdates");
    append_auth_flags(&mut cmd, auth);
    let status = cmd
        .status()
        .with_context(|| "spawn xcodebuild -exportArchive")?;
    if !status.success() {
        anyhow::bail!(
            "xcodebuild -exportArchive failed (exit {status}). If uploading, \
             verify the App Store Connect API key has the Developer/App Manager \
             role and the bundle id exists in App Store Connect.",
        );
    }
    Ok(())
}

/// Find the first `.ipa` in `dir` (non-recursive). `xcodebuild
/// -exportArchive` writes exactly one for app-store exports.
fn find_ipa(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("ipa") {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_options_method_is_app_store_connect() {
        let plist = export_options_plist("ABCDE12345", ExportDestination::Export);
        assert!(
            plist.contains("<key>method</key>")
                && plist.contains("<string>app-store-connect</string>"),
            "export options must declare the app-store-connect method:\n{plist}",
        );
        assert!(
            plist.contains("<string>ABCDE12345</string>"),
            "team id must be substituted:\n{plist}",
        );
        assert!(
            plist.contains("<string>automatic</string>"),
            "signing style must be automatic:\n{plist}",
        );
    }

    #[test]
    fn export_destination_export_vs_upload() {
        let export = export_options_plist("T", ExportDestination::Export);
        let upload = export_options_plist("T", ExportDestination::Upload);
        assert!(
            export.contains("<key>destination</key>\n<string>export</string>"),
            "build-only export must use destination=export:\n{export}",
        );
        assert!(
            upload.contains("<key>destination</key>\n<string>upload</string>"),
            "--upload must use destination=upload:\n{upload}",
        );
    }

    #[test]
    fn auth_flags_only_for_api_key() {
        // XcodeAccount / None add no flags; ApiKey adds the three auth flags.
        // We assert via the argument list xcodebuild would receive.
        fn flags(auth: Option<&UploadAuth>) -> Vec<String> {
            let mut cmd = Command::new("xcodebuild");
            append_auth_flags(&mut cmd, auth);
            cmd.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        }

        assert!(flags(None).is_empty());
        assert!(flags(Some(&UploadAuth::XcodeAccount)).is_empty());

        let api = UploadAuth::ApiKey {
            key_id: "KID".to_string(),
            issuer_id: "ISS".to_string(),
            key_path: PathBuf::from("/tmp/AuthKey_KID.p8"),
        };
        let f = flags(Some(&api));
        assert!(f.contains(&"-authenticationKeyID".to_string()));
        assert!(f.contains(&"KID".to_string()));
        assert!(f.contains(&"-authenticationKeyIssuerID".to_string()));
        assert!(f.contains(&"ISS".to_string()));
        assert!(f.contains(&"-authenticationKeyPath".to_string()));
        assert!(f.contains(&"/tmp/AuthKey_KID.p8".to_string()));
    }
}
