//! macOS distribution for `idealyst publish macos`.
//!
//! Two paths, mirroring how Mac apps actually ship:
//!
//! ```text
//!   build-macos::build (release)        → bare binary
//!   assemble_app_bundle                  → Foo.app (Info.plist + .icns)
//!   entitlements.plist (from capabilities)
//!   codesign … --entitlements … Foo.app  → distribution-signed .app
//!
//!   ── Mac App Store (App Store Connect) ──
//!     productbuild --component Foo.app /Applications --sign <installer> Foo.pkg
//!     xcrun altool --upload-app -f Foo.pkg -t macos --apiKey … --apiIssuer …
//!
//!   ── Developer ID (direct, notarized) ──
//!     codesign … --options runtime         (hardened runtime)
//!     ditto -c -k --keepParent Foo.app Foo.zip
//!     xcrun notarytool submit Foo.zip --key … --wait
//!     xcrun stapler staple Foo.app
//!     hdiutil create … Foo.dmg             (from the stapled app)
//! ```
//!
//! ## Why this can't reuse the iOS publish
//!
//! iOS goes through `xcodebuild archive → -exportArchive`. macOS has **no
//! `.xcodeproj`** — [`build_macos::build`] emits a bare binary and
//! [`crate::assemble_app_bundle`] hand-wraps the `.app`. So distribution is
//! direct `codesign` + `productbuild`/`notarytool`, not an Xcode export.
//!
//! ## Signing model
//!
//! - **App Store**: App Sandbox is mandatory — the entitlements carry
//!   `com.apple.security.app-sandbox`. The `.app` is signed "Apple
//!   Distribution"; the `.pkg` with a "3rd Party Mac Developer Installer"
//!   cert (an *installer* identity, found under the `basic` keychain policy,
//!   not `codesigning`).
//! - **Developer ID**: no sandbox; hardened runtime (`--options runtime`) is
//!   required for notarization. Signed "Developer ID Application".
//!
//! ## Verification
//!
//! The deterministic pieces (entitlements rendering, identity selection,
//! flag mapping) are unit-tested. The live chain — distribution/installer/
//! Developer-ID certs, `productbuild`, `notarytool`, `altool` — needs a real
//! Mac + certs + an App Store Connect account and is **manually verified**.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::{capabilities, parse_manifest, FrameworkSource};

use crate::{assemble_app_bundle, resolve_signing_identity};

/// App Store Connect API key (key id + issuer id + `AuthKey_<id>.p8`). Drives
/// `altool` upload (App Store) and `notarytool` (Developer ID). The CLI
/// resolves it from `--api-key-*` / `ASC_*` env (auto-located in the standard
/// `private_keys` dirs), exactly like the iOS path.
#[derive(Clone, Debug)]
pub struct AscApiKey {
    pub key_id: String,
    pub issuer_id: String,
    pub key_path: PathBuf,
}

/// What `publish macos` does after building the distribution-signed `.app`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacDistribution {
    /// Mac App Store: produce a signed `.pkg` and stop (manual upload).
    AppStorePkg,
    /// Mac App Store: produce the `.pkg` and upload to App Store Connect.
    AppStoreUpload,
    /// Developer ID: hardened-runtime `.app` → notarize → staple → `.dmg`.
    DeveloperId,
}

impl MacDistribution {
    fn is_app_store(self) -> bool {
        matches!(self, Self::AppStorePkg | Self::AppStoreUpload)
    }
}

#[derive(Clone, Debug)]
pub struct MacPublishOptions {
    /// Apple Developer team ID (resolved by the CLI). Disambiguates the
    /// signing identity when multiple teams' certs are installed.
    pub team: String,
    pub source: FrameworkSource,
    pub user_features: Vec<String>,
    /// `CFBundleVersion` override. `None` ⇒ manifest `build_number`.
    pub build_number: Option<String>,
    pub distribution: MacDistribution,
    /// App Store Connect API key — required for `AppStoreUpload` and
    /// `DeveloperId` (notarization). Unused for `AppStorePkg`.
    pub api_key: Option<AscApiKey>,
    /// Explicit Mac App Store `.provisionprofile` to embed. `None` ⇒
    /// auto-locate by bundle id in the standard provisioning-profile dirs.
    /// App Store paths only (Developer ID apps embed no profile).
    pub provisioning_profile: Option<PathBuf>,
    /// Where the `.pkg` / `.dmg` land. CLI defaults to `<project>/dist/macos`.
    pub output_dir: PathBuf,
}

#[derive(Debug)]
pub struct MacPublishArtifact {
    /// The distribution-signed `.app`.
    pub app: PathBuf,
    /// The `.pkg` (App Store paths).
    pub pkg: Option<PathBuf>,
    /// The notarized, stapled `.dmg` (Developer ID path).
    pub dmg: Option<PathBuf>,
    pub uploaded: bool,
    pub notarized: bool,
}

pub fn publish(project_dir: &Path, opts: MacPublishOptions) -> Result<MacPublishArtifact> {
    let project_dir = std::fs::canonicalize(project_dir)
        .with_context(|| format!("resolve project dir {}", project_dir.display()))?;
    let mut manifest = parse_manifest(&project_dir)?;
    let bundle_id = manifest.app.require_bundle_id()?.to_string();
    if let Some(bn) = &opts.build_number {
        manifest.app.build_number = bn.clone();
    }

    // Mac App Store requires an app category. Fail before the slow build.
    if opts.distribution.is_app_store() && manifest.app.macos.category.is_none() {
        anyhow::bail!(
            "Mac App Store submission requires an app category. Add it to \
             Cargo.toml:\n\n  [package.metadata.idealyst.app.macos]\n  category = \
             \"public.app-category.productivity\"\n\n(see Apple's \
             LSApplicationCategoryType list for the right value).",
        );
    }
    if matches!(
        opts.distribution,
        MacDistribution::AppStoreUpload | MacDistribution::DeveloperId
    ) && opts.api_key.is_none()
    {
        anyhow::bail!(
            "this path needs an App Store Connect API key. Pass --api-key-id / \
             --issuer-id / --api-key-path (or set ASC_KEY_ID / ASC_ISSUER_ID / \
             ASC_KEY_PATH).",
        );
    }

    // Resolve EVERY signing input the chosen path needs up front — before the
    // multi-minute build — and report all missing ones at once (certs, and
    // the Mac App Store provisioning profile).
    let signing = preflight_signing(
        opts.distribution,
        &opts.team,
        &bundle_id,
        opts.provisioning_profile.as_deref(),
    )?;

    std::fs::create_dir_all(&opts.output_dir)
        .with_context(|| format!("create output dir {}", opts.output_dir.display()))?;
    let output_dir = std::fs::canonicalize(&opts.output_dir)
        .with_context(|| format!("resolve output dir {}", opts.output_dir.display()))?;

    // ── 1. Build the release binary ──────────────────────────────
    let built = build_macos::build(
        &project_dir,
        build_macos::BuildOptions {
            release: true,
            mode: build_macos::BuildMode::Local,
            source: opts.source.clone(),
            user_features: opts.user_features.clone(),
            // Universal (arm64 + x86_64) so the shipped app runs on Intel
            // Macs too — the App Store rejects an arm64-only build below the
            // 12.0 deployment target (error 409).
            universal: true,
        },
    )?;

    // ── 2. Capabilities → Info.plist usage strings + entitlements ─
    let resolved = match capabilities::discover(&built.wrapper_dir.join("Cargo.toml")) {
        Ok(discovered) => capabilities::resolve(&discovered, &manifest.app.permissions),
        Err(e) => {
            eprintln!("warning: could not discover app capabilities: {e}");
            Default::default()
        }
    };

    // ── 3. Assemble the .app (icons + Info.plist incl. macOS keys) ─
    let assembled = assemble_app_bundle(&project_dir, &built.binary, &resolved.macos_plist)?;
    let app = assembled.app_dir;

    // ── 4. Embed the provisioning profile (App Store) + sign ─────
    // The profile MUST be copied in BEFORE codesign so the signature seals
    // it — TestFlight rejects a main bundle with no `embedded.provisionprofile`
    // (error 90889). Developer ID apps embed no profile.
    let app_store = opts.distribution.is_app_store();
    if let Some(profile) = &signing.profile {
        embed_provisioning_profile(&app, profile)?;
    }

    let entitlements = entitlements_plist(/* sandbox */ app_store, &resolved.macos_entitlements);
    let entitlements_path = app
        .parent()
        .unwrap_or(&output_dir)
        .join("idealyst.entitlements.plist");
    std::fs::write(&entitlements_path, entitlements)
        .with_context(|| format!("write {}", entitlements_path.display()))?;

    codesign_app(
        &app,
        &signing.app,
        &entitlements_path,
        /* hardened_runtime */ !app_store,
    )?;

    // ── 5. Package + ship per path ───────────────────────────────
    let app_name = app
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("App")
        .to_string();

    match opts.distribution {
        MacDistribution::AppStorePkg | MacDistribution::AppStoreUpload => {
            let installer = signing
                .installer
                .as_ref()
                .expect("preflight resolved an installer identity for the App Store path");
            let pkg = output_dir.join(format!("{app_name}.pkg"));
            productbuild(&app, installer, &pkg)?;
            let uploaded = if opts.distribution == MacDistribution::AppStoreUpload {
                let key = opts.api_key.as_ref().expect("checked above");
                altool_upload(&pkg, key)?;
                true
            } else {
                false
            };
            Ok(MacPublishArtifact {
                app,
                pkg: Some(pkg),
                dmg: None,
                uploaded,
                notarized: false,
            })
        }
        MacDistribution::DeveloperId => {
            let key = opts.api_key.as_ref().expect("checked above");
            // Notarize a zip of the signed app, staple the app, then package
            // the stapled app into the shippable .dmg.
            let zip = output_dir.join(format!("{app_name}.zip"));
            ditto_zip(&app, &zip)?;
            notarytool_submit(&zip, key)?;
            stapler_staple(&app)?;
            let _ = std::fs::remove_file(&zip);
            let dmg = output_dir.join(format!("{app_name}.dmg"));
            hdiutil_create(&app, &app_name, &dmg)?;
            Ok(MacPublishArtifact {
                app,
                pkg: None,
                dmg: Some(dmg),
                uploaded: false,
                notarized: true,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Entitlements
// ---------------------------------------------------------------------------

/// Render an `entitlements.plist`. App Store builds get
/// `com.apple.security.app-sandbox` (mandatory); every capability-derived
/// entitlement (e.g. `com.apple.security.device.camera`) is added as a
/// boolean `true`. Pure (no IO) so it's unit-testable.
fn entitlements_plist(sandbox: bool, entitlements: &[String]) -> String {
    let mut keys: Vec<String> = Vec::new();
    if sandbox {
        keys.push("com.apple.security.app-sandbox".to_string());
    }
    keys.extend(entitlements.iter().cloned());
    keys.sort();
    keys.dedup();
    let body: String = keys
        .iter()
        .map(|k| format!("    <key>{k}</key>\n    <true/>\n"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>\n"
    )
}

// ---------------------------------------------------------------------------
// Signing-identity resolution
// ---------------------------------------------------------------------------

/// Resolve the app-signing identity for the chosen path. App Store → "Apple
/// Distribution" (or legacy "3rd Party Mac Developer Application"); Developer
/// ID → "Developer ID Application". Honors the team to disambiguate.
/// The signing inputs a publish run needs.
struct Signing {
    /// Identity that signs the `.app` (Apple Distribution / Developer ID).
    app: String,
    /// Identity that signs the `.pkg` (App Store paths only).
    installer: Option<String>,
    /// Mac App Store `.provisionprofile` to embed (App Store paths only).
    profile: Option<PathBuf>,
}

/// Resolve every signing certificate the chosen path needs, BEFORE the slow
/// build, reporting ALL missing ones in a single message. App Store needs an
/// app cert (Apple Distribution) AND an installer cert (Mac Installer
/// Distribution — note: an *installer* identity, found under the `basic`
/// keychain policy, not `codesigning`); Developer ID needs only the Developer
/// ID Application cert. Failing fast and complete beats discovering the
/// installer cert is missing only after the build + app-sign succeed.
fn preflight_signing(
    dist: MacDistribution,
    team: &str,
    bundle_id: &str,
    explicit_profile: Option<&Path>,
) -> Result<Signing> {
    let app_kinds: &[&str] = if dist.is_app_store() {
        &["Apple Distribution", "3rd Party Mac Developer Application"]
    } else {
        &["Developer ID Application"]
    };
    let app = with_team(team, || resolve_signing_identity("codesigning", app_kinds));

    // Installer cert + provisioning profile only matter for the App Store.
    let installer = if dist.is_app_store() {
        with_team(team, || {
            resolve_signing_identity(
                "basic",
                &["3rd Party Mac Developer Installer", "Mac Installer Distribution"],
            )
        })
    } else {
        None
    };
    let profile = if dist.is_app_store() {
        resolve_provisioning_profile(explicit_profile, bundle_id, team)
    } else {
        None
    };

    let mut missing: Vec<String> = Vec::new();
    if app.is_none() {
        missing.push(
            if dist.is_app_store() {
                "\"Apple Distribution\" certificate — signs the .app"
            } else {
                "\"Developer ID Application\" certificate — signs the .app"
            }
            .to_string(),
        );
    }
    if dist.is_app_store() && installer.is_none() {
        missing.push(
            "\"Mac Installer Distribution\" (aka 3rd Party Mac Developer Installer) certificate — signs the .pkg"
                .to_string(),
        );
    }
    if dist.is_app_store() && profile.is_none() {
        missing.push(format!(
            "Mac App Store provisioning profile for {team}.{bundle_id} — embeds \
             into the .app (TestFlight requires it). Create one at \
             developer.apple.com → Profiles → \"+\" → Mac App Store, then \
             download it (Xcode → Settings → Accounts → Download Manual \
             Profiles), or pass --provisioning-profile <path>",
        ));
    }
    if !missing.is_empty() {
        let list = missing
            .iter()
            .map(|m| format!("  • {m}"))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "missing signing input(s) for team {team}:\n{list}\n\nCerts: Xcode → \
             Settings → Accounts → (select the team) → Manage Certificates → \
             \"+\" (or developer.apple.com → Certificates). Distribution certs / \
             profiles need an Admin / App Manager role in the team. Confirm \
             {team} is the team you mean to publish under, then re-run.",
        );
    }

    Ok(Signing {
        app: app.expect("checked above"),
        installer,
        profile,
    })
}

/// Resolve the Mac App Store `.provisionprofile` to embed: an explicit path
/// when given, else the first profile in the standard dirs whose
/// `application-identifier` matches `<team>.<bundle_id>` (or a `<team>.*`
/// wildcard), is a *distribution* profile (no `ProvisionedDevices`), and
/// targets macOS (`OSX`). Returns `None` when none is found — the caller
/// turns that into the actionable preflight error.
fn resolve_provisioning_profile(
    explicit: Option<&Path>,
    bundle_id: &str,
    team: &str,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exact = format!("{team}.{bundle_id}");
    let wildcard = format!("{team}.*");
    for dir in provisioning_profile_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("provisionprofile") {
                continue;
            }
            let Some(plist) = decode_provisioning_profile(&path) else {
                continue;
            };
            let app_id_matches = plist.contains(&format!("<string>{exact}</string>"))
                || plist.contains(&format!("<string>{wildcard}</string>"));
            // Distribution (not development) profiles list no devices; macOS
            // profiles carry `OSX` in their Platform array.
            let is_distribution = !plist.contains("ProvisionedDevices");
            let is_macos = plist.contains("OSX");
            if app_id_matches && is_distribution && is_macos {
                return Some(path);
            }
        }
    }
    None
}

/// The dirs Xcode drops downloaded provisioning profiles into. The
/// `MobileDevice` path is the long-standing one; Xcode 16+ also uses the
/// `UserData` path.
fn provisioning_profile_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    vec![
        home.join("Library/MobileDevice/Provisioning Profiles"),
        home.join("Library/Developer/Xcode/UserData/Provisioning Profiles"),
    ]
}

/// Decode a CMS-signed `.provisionprofile` to its embedded XML plist via
/// `security cms -D`. Returns `None` if decoding fails.
fn decode_provisioning_profile(path: &Path) -> Option<String> {
    let out = Command::new("security")
        .arg("cms")
        .arg("-D")
        .arg("-i")
        .arg(path)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Copy the provisioning profile into the bundle at
/// `Contents/embedded.provisionprofile`. MUST run before `codesign` so the
/// signature seals it (TestFlight error 90889 otherwise).
fn embed_provisioning_profile(app: &Path, profile: &Path) -> Result<()> {
    let dest = app.join("Contents").join("embedded.provisionprofile");
    eprintln!(
        "[publish macos] embed provisioning profile → {}",
        dest.display(),
    );
    std::fs::copy(profile, &dest).with_context(|| {
        format!("copy {} → {}", profile.display(), dest.display())
    })?;
    Ok(())
}

/// Run `f` with the team pinned into `IDEALYST_DEVELOPMENT_TEAM` so
/// [`resolve_signing_identity`]'s env-based team filter selects the right
/// cert. Restores the prior value after.
fn with_team<T>(team: &str, f: impl FnOnce() -> T) -> T {
    let prev = std::env::var_os("IDEALYST_DEVELOPMENT_TEAM");
    // SAFETY: the publish command is single-threaded at this point.
    unsafe { std::env::set_var("IDEALYST_DEVELOPMENT_TEAM", team) };
    let out = f();
    match prev {
        Some(v) => unsafe { std::env::set_var("IDEALYST_DEVELOPMENT_TEAM", v) },
        None => unsafe { std::env::remove_var("IDEALYST_DEVELOPMENT_TEAM") },
    }
    out
}

// ---------------------------------------------------------------------------
// codesign / productbuild / dmg / notarytool / altool
// ---------------------------------------------------------------------------

/// `codesign --force --deep --timestamp [--options runtime] --entitlements
/// <ent> --sign <id> <app>`. `--deep` is acceptable here because the bundle
/// is a single binary with no nested frameworks. `--timestamp` is required
/// for notarization; `--options runtime` (hardened runtime) for Developer ID.
fn codesign_app(
    app: &Path,
    identity: &str,
    entitlements: &Path,
    hardened_runtime: bool,
) -> Result<()> {
    eprintln!("[publish macos] codesign {}", app.display());
    let mut cmd = Command::new("codesign");
    cmd.args(["--force", "--deep", "--timestamp"]);
    if hardened_runtime {
        cmd.args(["--options", "runtime"]);
    }
    cmd.arg("--entitlements")
        .arg(entitlements)
        .args(["--sign", identity])
        .arg(app);
    run(cmd, "codesign")
}

/// `productbuild --component <app> /Applications --sign <installer> <pkg>`.
fn productbuild(app: &Path, installer_identity: &str, out_pkg: &Path) -> Result<()> {
    eprintln!("[publish macos] productbuild → {}", out_pkg.display());
    let mut cmd = Command::new("productbuild");
    cmd.arg("--component")
        .arg(app)
        .arg("/Applications")
        .args(["--sign", installer_identity])
        .arg(out_pkg);
    run(cmd, "productbuild")
}

/// `xcrun altool --upload-app -f <pkg> -t macos --apiKey <id> --apiIssuer
/// <issuer>`. altool auto-locates the `.p8` by key id in the standard
/// `private_keys` dirs (the same dirs the CLI auto-locates from).
fn altool_upload(pkg: &Path, key: &AscApiKey) -> Result<()> {
    eprintln!("[publish macos] altool --upload-app {}", pkg.display());
    ensure_key_discoverable(key)?;
    let mut cmd = Command::new("xcrun");
    cmd.args(["altool", "--upload-app", "-t", "macos", "-f"])
        .arg(pkg)
        .args(["--apiKey", &key.key_id, "--apiIssuer", &key.issuer_id]);
    run(cmd, "altool --upload-app")
}

/// `ditto -c -k --keepParent <app> <zip>` — the notarization-friendly zip
/// (preserves symlinks + the `.app` wrapper, unlike `zip`).
fn ditto_zip(app: &Path, zip: &Path) -> Result<()> {
    let mut cmd = Command::new("ditto");
    cmd.args(["-c", "-k", "--keepParent"]).arg(app).arg(zip);
    run(cmd, "ditto (zip for notarization)")
}

/// `xcrun notarytool submit <zip> --key <p8> --key-id <id> --issuer <issuer>
/// --wait` — blocks until Apple finishes notarizing.
fn notarytool_submit(zip: &Path, key: &AscApiKey) -> Result<()> {
    eprintln!("[publish macos] notarytool submit (waiting for Apple)…");
    let mut cmd = Command::new("xcrun");
    cmd.arg("notarytool")
        .arg("submit")
        .arg(zip)
        .arg("--key")
        .arg(&key.key_path)
        .args(["--key-id", &key.key_id, "--issuer", &key.issuer_id, "--wait"]);
    run(cmd, "notarytool submit")
}

/// `xcrun stapler staple <app>` — attach the notarization ticket so the app
/// passes Gatekeeper offline.
fn stapler_staple(app: &Path) -> Result<()> {
    let mut cmd = Command::new("xcrun");
    cmd.arg("stapler").arg("staple").arg(app);
    run(cmd, "stapler staple")
}

/// `hdiutil create -volname <name> -srcfolder <app> -ov -format UDZO <dmg>`.
fn hdiutil_create(app: &Path, volname: &str, dmg: &Path) -> Result<()> {
    eprintln!("[publish macos] hdiutil create → {}", dmg.display());
    let mut cmd = Command::new("hdiutil");
    cmd.arg("create")
        .args(["-volname", volname])
        .arg("-srcfolder")
        .arg(app)
        .args(["-ov", "-format", "UDZO"])
        .arg(dmg);
    run(cmd, "hdiutil create")
}

/// `altool` finds the key by id only in the standard `private_keys` dirs.
/// If the user's key lives elsewhere (explicit `--api-key-path`), warn —
/// notarytool takes an explicit path, but altool does not.
fn ensure_key_discoverable(key: &AscApiKey) -> Result<()> {
    let expected = format!("AuthKey_{}.p8", key.key_id);
    let in_standard_dir = key
        .key_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == expected)
        .unwrap_or(false);
    if !in_standard_dir {
        eprintln!(
            "[publish macos] note: altool locates the API key by id in the \
             standard private_keys dirs (~/.appstoreconnect/private_keys/ etc). \
             Your key is {}; if upload fails to find it, move it there as {}.",
            key.key_path.display(),
            expected,
        );
    }
    Ok(())
}

/// Spawn `cmd`, mapping a non-zero exit into a clear error tagged with `what`.
fn run(mut cmd: Command, what: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawn {what}"))?;
    if !status.success() {
        anyhow::bail!("{what} failed (exit {status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_store_entitlements_require_sandbox() {
        let ent = entitlements_plist(true, &["com.apple.security.device.camera".to_string()]);
        assert!(
            ent.contains("<key>com.apple.security.app-sandbox</key>"),
            "App Store entitlements must enable the sandbox:\n{ent}",
        );
        assert!(ent.contains("<key>com.apple.security.device.camera</key>"));
    }

    #[test]
    fn developer_id_entitlements_have_no_sandbox() {
        let ent = entitlements_plist(false, &["com.apple.security.device.audio-input".to_string()]);
        assert!(
            !ent.contains("app-sandbox"),
            "Developer ID builds must NOT force the sandbox:\n{ent}",
        );
        assert!(ent.contains("<key>com.apple.security.device.audio-input</key>"));
    }

    #[test]
    fn identity_selection_picks_kind_and_team() {
        // A realistic `security find-identity -v` listing with two teams.
        let listing = "\
  1) AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA \"Apple Development: Jane (TEAMAAAAAA)\"
  2) BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB \"Apple Distribution: Acme Inc (TEAMBBBBBB)\"
  3) CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC \"Apple Distribution: Other Inc (TEAMCCCCCC)\"
  4) DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD \"Developer ID Application: Acme Inc (TEAMBBBBBB)\"
     4 valid identities found";

        // App Store kind, team B → the matching Apple Distribution cert.
        assert_eq!(
            crate::select_identity_from_listing(
                listing,
                Some("TEAMBBBBBB"),
                &["Apple Distribution"],
            )
            .as_deref(),
            Some("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"),
        );
        // Developer ID kind → the Developer ID Application cert.
        assert_eq!(
            crate::select_identity_from_listing(
                listing,
                Some("TEAMBBBBBB"),
                &["Developer ID Application"],
            )
            .as_deref(),
            Some("DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD"),
        );
        // No matching kind → None (we never silently sign with the wrong cert).
        assert!(
            crate::select_identity_from_listing(listing, None, &["Mac Installer Distribution"])
                .is_none(),
        );
    }
}
