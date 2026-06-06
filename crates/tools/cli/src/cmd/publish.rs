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

use anyhow::{Context, Result};
use run_ios::publish::{Distribution, PublishOptions, UploadAuth};

use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target platform: `ios` (App Store Connect) or `macos` (Mac App Store
    /// or Developer ID notarization).
    #[arg(value_enum)]
    pub platform: Platform,

    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Also upload to App Store Connect. iOS: exports + uploads the `.ipa`.
    /// macOS (with `--app-store`): uploads the `.pkg`. Without this the
    /// command stops after producing the signed artifact.
    #[arg(long)]
    pub upload: bool,

    /// Finish the upload in the platform's GUI uploader instead of via an API
    /// key — no key needed, just an Apple ID. iOS → opens the `.xcarchive` in
    /// Xcode's Organizer; macOS → builds the App Store `.pkg` and opens
    /// Transporter. (On macOS this implies the Mac App Store path.)
    #[arg(long, conflicts_with_all = ["upload", "notarize"])]
    pub interactive: bool,

    /// macOS only. Mac App Store path: sandboxed, Apple-Distribution-signed
    /// `.app` → signed `.pkg`. Add `--upload` (API key) or `--interactive`
    /// (Transporter) to submit it.
    #[arg(long, conflicts_with_all = ["notarize"])]
    pub app_store: bool,

    /// macOS only. Developer ID path: hardened-runtime `.app` → notarize →
    /// staple → `.dmg` you distribute yourself (not the App Store). Requires
    /// an App Store Connect API key for notarization.
    #[arg(long, conflicts_with_all = ["interactive", "app_store", "upload"])]
    pub notarize: bool,

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
    /// Falls back to `$ASC_KEY_PATH`. Optional — if omitted, the key is
    /// auto-located by id in the standard dirs (`~/.appstoreconnect/
    /// private_keys/`, `~/.private_keys/`, `./private_keys/`). A leading
    /// `~/` is expanded.
    #[arg(long)]
    pub api_key_path: Option<PathBuf>,

    /// macOS App Store only. Path to the `.provisionprofile` to embed in the
    /// `.app` (TestFlight requires one). Optional — if omitted, it's
    /// auto-located by bundle id in `~/Library/MobileDevice/Provisioning
    /// Profiles/` (where Xcode downloads them).
    #[arg(long)]
    pub provisioning_profile: Option<PathBuf>,

    /// Where the build artifacts land. Defaults to `<project>/dist/<platform>`
    /// (`.ipa`/`.xcarchive` for iOS; `.pkg`/`.dmg` for macOS).
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    match args.platform {
        Platform::Ios => {
            if args.app_store || args.notarize {
                anyhow::bail!(
                    "`--app-store` / `--notarize` are macOS-only. For iOS use \
                     `--upload` (App Store Connect) or `--interactive` (Xcode \
                     Organizer GUI).",
                );
            }
            run_ios(args)
        }
        Platform::Macos => {
            if !args.app_store && !args.notarize && !args.interactive {
                anyhow::bail!(
                    "choose a macOS distribution path: `--app-store` (Mac App \
                     Store; add `--upload` for API-key submit or `--interactive` \
                     for Transporter) or `--notarize` (Developer ID, direct .dmg).",
                );
            }
            run_macos(args)
        }
        other => anyhow::bail!(
            "`idealyst publish` supports `ios` and `macos`; `{other}` is not a \
             distributable target.",
        ),
    }
}

fn run_ios(args: Args) -> Result<()> {
    // Canonicalize the project dir BEFORE resolving the framework source.
    // `FrameworkSource::detect` walks `project_dir.ancestors()` to find the
    // workspace root; a relative `.` has no real ancestors, so it would fall
    // through to git-mode and the generated wrapper's `runtime_core` would
    // diverge from the app crate's (two `Element` types → `mount` bound
    // failure). `run`/`build`/`dev` all canonicalize first; match them.
    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve project dir {}", args.dir.display()))?;

    let team = run_ios::device::resolve_team(args.team.as_deref())?;
    eprintln!("[publish ios] signing team {team}");

    let auth = resolve_auth(&args)?;
    let source = crate::framework_source::resolve(&dir)?;
    let output_dir = args
        .out
        .clone()
        .unwrap_or_else(|| dir.join("dist").join("ios"));

    // `--interactive` archives only and hands off to Xcode Organizer (no CLI
    // export — Organizer does the distribution re-sign); `--upload`
    // exports+uploads; the bare command exports a signed `.ipa`.
    // `--interactive`/`--upload` are mutually exclusive at the clap layer.
    let distribution = if args.interactive {
        Distribution::ArchiveOnly
    } else if args.upload {
        Distribution::Upload
    } else {
        Distribution::Ipa
    };

    let artifact = run_ios::publish::publish(
        &dir,
        PublishOptions {
            team,
            source,
            user_features: Vec::new(),
            build_number: args.build_number.clone(),
            auth,
            distribution,
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
    } else if args.interactive {
        open_in_organizer(&artifact.archive)?;
    } else {
        eprintln!(
            "[publish ios] done. Upload with `--upload` (App Store Connect API \
             key), `--interactive` (Xcode Organizer), or drag the .ipa into \
             Transporter.",
        );
    }
    Ok(())
}

fn run_macos(args: Args) -> Result<()> {
    use run_macos::publish::MacPublishOptions;

    let dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve project dir {}", args.dir.display()))?;

    let team = run_ios::device::resolve_team(args.team.as_deref())?;
    eprintln!("[publish macos] signing team {team}");

    let api_key = resolve_asc_api_key(&args)?;
    let source = crate::framework_source::resolve(&dir)?;
    let output_dir = args
        .out
        .clone()
        .unwrap_or_else(|| dir.join("dist").join("macos"));

    let distribution = mac_distribution(args.notarize, args.upload);

    let artifact = run_macos::publish::publish(
        &dir,
        MacPublishOptions {
            team,
            source,
            user_features: Vec::new(),
            build_number: args.build_number.clone(),
            distribution,
            api_key,
            provisioning_profile: args.provisioning_profile.clone(),
            output_dir,
        },
    )?;

    if let Some(pkg) = &artifact.pkg {
        eprintln!("[publish macos] built {}", pkg.display());
    }
    if let Some(dmg) = &artifact.dmg {
        eprintln!("[publish macos] built {}", dmg.display());
    }
    if artifact.uploaded {
        eprintln!(
            "[publish macos] uploaded to App Store Connect — the build appears \
             under TestFlight / the Mac App Store once Apple finishes processing.",
        );
    } else if artifact.notarized {
        eprintln!(
            "[publish macos] notarized + stapled. Ship the .dmg directly — it \
             passes Gatekeeper on any Mac.",
        );
    } else if args.interactive {
        if let Some(pkg) = &artifact.pkg {
            open_in_transporter(pkg)?;
        }
    } else {
        eprintln!(
            "[publish macos] signed .pkg ready. Add `--upload` (API key) or \
             `--interactive` (Transporter) to submit it to App Store Connect.",
        );
    }
    Ok(())
}

/// Launch Transporter and reveal the `.pkg` in Finder so the user can drag it
/// in and click **Deliver** — the macOS Apple-ID-only upload path (no API
/// key), analogous to iOS's [`open_in_organizer`]. Unlike `.xcarchive`
/// (which Xcode registers as a handler), macOS routes `.pkg` to Installer, so
/// we can't `open` the file straight into Transporter — we launch the app and
/// surface the file for the one manual drag.
fn open_in_transporter(pkg: &std::path::Path) -> Result<()> {
    eprintln!("[publish macos] opening Transporter…");
    let launched = std::process::Command::new("open")
        .args(["-a", "Transporter"])
        .status();
    let ok = matches!(launched, Ok(s) if s.success());
    if !ok {
        anyhow::bail!(
            "couldn't launch Transporter. Install it free from the Mac App \
             Store, then drag {} in and click Deliver.",
            pkg.display(),
        );
    }
    // Reveal the .pkg in Finder for the drag-and-drop (best-effort).
    let _ = std::process::Command::new("open").arg("-R").arg(pkg).status();
    eprintln!(
        "[publish macos] drag {} into Transporter and click Deliver to upload \
         with your Apple ID.",
        pkg.display(),
    );
    Ok(())
}

/// Open the `.xcarchive` in Xcode's Organizer via `open`. macOS routes
/// `.xcarchive` to Xcode, which surfaces it in the Organizer's Archives list
/// ready for **Distribute App** — an Apple-ID-only upload path that needs no
/// API key.
fn open_in_organizer(archive: &std::path::Path) -> Result<()> {
    eprintln!("[publish ios] opening {} in Xcode Organizer…", archive.display());
    let status = std::process::Command::new("open")
        .arg(archive)
        .status()
        .with_context(|| "spawn `open` to launch Xcode Organizer")?;
    if !status.success() {
        anyhow::bail!(
            "`open` failed (exit {status}). Open it by hand:\n  open {}\n(or \
             double-click it in Finder) and use Distribute App → App Store Connect.",
            archive.display(),
        );
    }
    eprintln!(
        "[publish ios] in the Organizer window, select the archive and click \
         Distribute App → App Store Connect to upload with your Apple ID.",
    );
    Ok(())
}

/// Resolve App Store Connect credentials from flags, falling back to the
/// `ASC_*` env vars (which an auto-loaded `.env` can supply). Returns:
/// - `Some(ApiKey{..})` when a key id + issuer id are present and the `.p8`
///   is found (explicit path, or auto-located in Apple's standard
///   `private_keys` dirs) — the recommended headless path,
/// - `Some(XcodeAccount)` when `--upload` is requested but no key was given
///   (lean on the Xcode-stored session),
/// - `None` when neither uploading nor a key is in play (local archive/export
///   signs via the logged-in Xcode account).
///
/// A partially-specified key (id but no issuer, or a key whose `.p8` can't be
/// found) is a hard error — silently downgrading to the Xcode account would be
/// a confusing surprise.
fn resolve_auth(args: &Args) -> Result<Option<UploadAuth>> {
    match resolve_api_key_parts(args)? {
        Some((key_id, issuer_id, key_path)) => Ok(Some(UploadAuth::ApiKey {
            key_id,
            issuer_id,
            key_path,
        })),
        // No key at all. If we're uploading we still need *some* auth — lean
        // on the Xcode account; otherwise leave it unset.
        None => Ok(args.upload.then_some(UploadAuth::XcodeAccount)),
    }
}

/// Shared App Store Connect API-key resolution (used by both the iOS and
/// macOS paths). Resolves `(key_id, issuer_id, key_path)` from flags →
/// `ASC_*` env (which an auto-loaded `.env` can supply), with the `.p8`
/// auto-located by id in Apple's standard `private_keys` dirs when no
/// explicit path is given. Returns `None` when no key is in play; a
/// partially-specified key is a hard error.
fn resolve_api_key_parts(args: &Args) -> Result<Option<(String, String, PathBuf)>> {
    let key_id = args
        .api_key_id
        .clone()
        .or_else(|| env_nonempty("ASC_KEY_ID"));
    let issuer_id = args
        .issuer_id
        .clone()
        .or_else(|| env_nonempty("ASC_ISSUER_ID"));
    let explicit_path = args
        .api_key_path
        .clone()
        .or_else(|| env_nonempty("ASC_KEY_PATH").map(PathBuf::from))
        .map(expand_tilde);

    match (key_id, issuer_id) {
        (Some(key_id), Some(issuer_id)) => {
            // Path precedence: explicit (--api-key-path / $ASC_KEY_PATH) →
            // Apple's standard private_keys dirs (AuthKey_<KEYID>.p8).
            let key_path = match explicit_path {
                Some(p) => {
                    if !p.is_file() {
                        anyhow::bail!(
                            "App Store Connect API key not found at {} (set via \
                             --api-key-path / $ASC_KEY_PATH)",
                            p.display(),
                        );
                    }
                    p
                }
                None => locate_api_key(&key_id).with_context(|| {
                    format!(
                        "couldn't find AuthKey_{key_id}.p8 in any of the standard \
                         App Store Connect key dirs ({}). Drop the .p8 there, or \
                         pass --api-key-path / set $ASC_KEY_PATH.",
                        standard_key_dirs()
                            .iter()
                            .map(|d| d.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                })?,
            };
            Ok(Some((key_id, issuer_id, key_path)))
        }
        (None, None) if explicit_path.is_none() => Ok(None),
        _ => anyhow::bail!(
            "incomplete App Store Connect API key: provide both --api-key-id and \
             --issuer-id (or $ASC_KEY_ID / $ASC_ISSUER_ID). The .p8 is then \
             auto-located in ~/.appstoreconnect/private_keys/ (or pass \
             --api-key-path).",
        ),
    }
}

/// Map the macOS path flags to a distribution mode. `--notarize` →
/// Developer ID; `--app-store --upload` → upload; `--app-store` alone → just
/// the signed `.pkg`. (clap guarantees `--notarize` and `--app-store` are
/// mutually exclusive, so `notarize` wins cleanly here.)
fn mac_distribution(notarize: bool, upload: bool) -> run_macos::publish::MacDistribution {
    use run_macos::publish::MacDistribution;
    if notarize {
        MacDistribution::DeveloperId
    } else if upload {
        MacDistribution::AppStoreUpload
    } else {
        MacDistribution::AppStorePkg
    }
}

/// macOS API-key resolution → [`run_macos::publish::AscApiKey`]. Same
/// resolution as the iOS auth, but with no Xcode-account fallback (macOS
/// upload/notarization always go through the API key).
fn resolve_asc_api_key(args: &Args) -> Result<Option<run_macos::publish::AscApiKey>> {
    Ok(resolve_api_key_parts(args)?.map(|(key_id, issuer_id, key_path)| {
        run_macos::publish::AscApiKey {
            key_id,
            issuer_id,
            key_path,
        }
    }))
}

/// The directories Apple's tools (altool / notarytool / Transporter) search
/// for `AuthKey_<KEYID>.p8`, in precedence order. We mirror them so a key
/// dropped in the conventional spot is found without a `--api-key-path`.
fn standard_key_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("./private_keys")];
    if let Some(home) = home_dir() {
        dirs.push(home.join("private_keys"));
        dirs.push(home.join(".private_keys"));
        dirs.push(home.join(".appstoreconnect").join("private_keys"));
    }
    dirs
}

/// Find `AuthKey_<key_id>.p8` in the standard key dirs.
fn locate_api_key(key_id: &str) -> Option<PathBuf> {
    locate_api_key_in(key_id, &standard_key_dirs())
}

/// Pure inner: find `AuthKey_<key_id>.p8` in `dirs` (precedence order). Split
/// out so tests can inject a temp dir instead of mutating the global `$HOME`.
fn locate_api_key_in(key_id: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let filename = format!("AuthKey_{key_id}.p8");
    dirs.iter()
        .map(|d| d.join(&filename))
        .find(|p| p.is_file())
}

/// Expand a leading `~/` (or bare `~`) to the home dir. `.env` values aren't
/// shell-expanded, so `ASC_KEY_PATH=~/.appstoreconnect/...` would otherwise be
/// taken literally.
fn expand_tilde(path: PathBuf) -> PathBuf {
    expand_tilde_with(path, home_dir().as_deref())
}

/// Pure inner for [`expand_tilde`] — takes the home dir explicitly so tests
/// don't touch the process environment.
fn expand_tilde_with(path: PathBuf, home: Option<&std::path::Path>) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path;
    };
    match (s, home) {
        ("~", Some(home)) => home.to_path_buf(),
        (s, Some(home)) if s.starts_with("~/") => home.join(&s[2..]),
        _ => path,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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
            interactive: false,
            app_store: false,
            notarize: false,
            build_number: None,
            team: None,
            api_key_id: api_key_id.map(str::to_string),
            issuer_id: issuer_id.map(str::to_string),
            api_key_path: api_key_path.map(PathBuf::from),
            provisioning_profile: None,
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

    #[test]
    fn api_key_auto_located_in_standard_dir() {
        // The .p8 dropped in a standard private_keys dir is found by
        // filename (AuthKey_<KEYID>.p8) without an explicit --api-key-path.
        let tmp = tempfile::tempdir().expect("tempdir");
        let key_dir = tmp.path().join(".appstoreconnect").join("private_keys");
        std::fs::create_dir_all(&key_dir).unwrap();
        let key_file = key_dir.join("AuthKey_AUTOLOC.p8");
        std::fs::write(&key_file, b"-----BEGIN PRIVATE KEY-----\n").unwrap();

        // Present in the dir → located; absent dir → None.
        assert_eq!(
            locate_api_key_in("AUTOLOC", &[key_dir.clone()]),
            Some(key_file),
        );
        assert_eq!(locate_api_key_in("MISSING", &[key_dir]), None);
    }

    #[test]
    fn expand_tilde_resolves_home() {
        let home = std::path::Path::new("/home/example");
        assert_eq!(
            expand_tilde_with(PathBuf::from("~/.appstoreconnect/k.p8"), Some(home)),
            PathBuf::from("/home/example/.appstoreconnect/k.p8"),
        );
        // Bare `~` → home; a non-tilde path is untouched.
        assert_eq!(expand_tilde_with(PathBuf::from("~"), Some(home)), home);
        assert_eq!(
            expand_tilde_with(PathBuf::from("/abs/k.p8"), Some(home)),
            PathBuf::from("/abs/k.p8"),
        );
    }

    #[test]
    fn mac_distribution_flag_mapping() {
        use run_macos::publish::MacDistribution;
        assert_eq!(mac_distribution(true, false), MacDistribution::DeveloperId);
        assert_eq!(mac_distribution(true, true), MacDistribution::DeveloperId); // notarize wins
        assert_eq!(mac_distribution(false, true), MacDistribution::AppStoreUpload);
        assert_eq!(mac_distribution(false, false), MacDistribution::AppStorePkg);
    }

    /// clap must reject contradictory path flags so a caller can't ask for two
    /// distribution mechanisms at once.
    #[test]
    fn conflicting_path_flags_are_rejected() {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrap {
            #[command(flatten)]
            args: Args,
        }
        let bad = [
            ["x", "macos", "--app-store", "--notarize"].as_slice(),
            ["x", "ios", "--interactive", "--upload"].as_slice(),
            ["x", "macos", "--notarize", "--upload"].as_slice(),
            ["x", "macos", "--interactive", "--notarize"].as_slice(),
        ];
        for argv in bad {
            assert!(
                Wrap::try_parse_from(argv).is_err(),
                "expected clap to reject {argv:?}",
            );
        }
        // Valid combos parse: API-key submit, and Transporter (interactive).
        assert!(Wrap::try_parse_from(["x", "macos", "--app-store", "--upload"]).is_ok());
        assert!(Wrap::try_parse_from(["x", "macos", "--interactive"]).is_ok());
        assert!(Wrap::try_parse_from(["x", "ios", "--interactive"]).is_ok());
    }
}
