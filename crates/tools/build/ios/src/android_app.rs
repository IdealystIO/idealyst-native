//! `[package.metadata.idealyst.app.android]` — Android-only app identity and
//! packaging knobs that belong to the APP, not to any SDK it depends on.
//!
//! ```toml
//! [package.metadata.idealyst.app.android]
//! # Explicit versionCode. Omitted → derived from `app.version`
//! # (major × 1_000_000 + minor × 1_000 + patch).
//! version_code = 42
//! # The main activity's `android:lockTaskMode`. A kiosk app that a device
//! # owner allow-lists uses "if_whitelisted": it enters lock task on launch.
//! lock_task_mode = "if_whitelisted"
//! # Declare the main activity as a home screen (HOME + DEFAULT categories),
//! # so a device owner can make it the persistent launcher.
//! home_launcher = true
//! # The main activity's `android:resizeableActivity`. `false` keeps it out
//! # of split screen and free-form windows, and Android then gives it no
//! # desktop-window handle — on a kiosk that handle can close the app even
//! # in lock task. Omitted → Android's default (resizable).
//! resizeable = false
//! # The main activity's `android:screenOrientation`, spelled as the
//! # manifest spells it ("landscape", "sensorLandscape", "portrait", …).
//! screen_orientation = "landscape"
//!
//! [package.metadata.idealyst.app.android.signing]
//! # Release keystore. Relative paths resolve against the app crate.
//! keystore = "keys/release.jks"
//! alias = "release"
//! # Environment variables holding the passwords (apksigner reads them via
//! # `env:VAR`, so they never appear in a process argument list).
//! store_password_env = "IDEALYST_ANDROID_STORE_PASSWORD"   # default
//! key_password_env = "IDEALYST_ANDROID_KEY_PASSWORD"       # default: the store password
//! ```
//!
//! Why the kiosk attributes live here rather than in an SDK: whether an app
//! is a kiosk or a launcher is the app's decision. An SDK such as
//! `device-admin` is depended on by both the device-owner agent and the
//! kiosk it locks, and only one of them is the home screen.
//!
//! Signing is app-level because Android refuses an update signed with a
//! different certificate: every build of one app must use the same key, and
//! the key is the app's, never the framework's. With no `signing` block,
//! builds use the debug keystore.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::Deserialize;

/// Default env var for the keystore password.
pub const DEFAULT_STORE_PASSWORD_ENV: &str = "IDEALYST_ANDROID_STORE_PASSWORD";

/// Upper bound of `versionCode` (Google Play rejects anything larger).
const MAX_VERSION_CODE: u64 = 2_100_000_000;

/// `[package.metadata.idealyst.app.android]`, validated. Default = no kiosk
/// attributes, versionCode from `app.version`, debug signing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AndroidAppMetadata {
    /// Explicit `versionCode`; `None` derives it from the app version.
    pub version_code: Option<u32>,
    /// The main activity's `android:lockTaskMode`.
    pub lock_task_mode: Option<LockTaskMode>,
    /// Add a HOME + DEFAULT intent filter to the main activity.
    pub home_launcher: bool,
    /// The main activity's `android:resizeableActivity`; `None` leaves
    /// Android's default.
    pub resizeable: Option<bool>,
    /// The main activity's `android:screenOrientation`, a manifest value
    /// from [`SCREEN_ORIENTATIONS`].
    pub screen_orientation: Option<String>,
    /// Release signing; `None` signs with the debug keystore.
    pub signing: Option<AndroidSigning>,
}

/// Values of the manifest's `android:screenOrientation` attribute.
pub const SCREEN_ORIENTATIONS: &[&str] = &[
    "unspecified",
    "behind",
    "landscape",
    "portrait",
    "reverseLandscape",
    "reversePortrait",
    "sensorLandscape",
    "sensorPortrait",
    "userLandscape",
    "userPortrait",
    "sensor",
    "fullSensor",
    "nosensor",
    "user",
    "fullUser",
    "locked",
];

/// Values of the manifest's `android:lockTaskMode` attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockTaskMode {
    Normal,
    Never,
    Always,
    IfWhitelisted,
}

impl LockTaskMode {
    /// The attribute value as the manifest spells it.
    pub fn manifest_value(self) -> &'static str {
        match self {
            LockTaskMode::Normal => "normal",
            LockTaskMode::Never => "never",
            LockTaskMode::Always => "always",
            LockTaskMode::IfWhitelisted => "if_whitelisted",
        }
    }
}

/// A configured release key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidSigning {
    /// Keystore path as declared (resolve with [`AndroidSigning::keystore_path`]).
    pub keystore: PathBuf,
    pub alias: String,
    pub store_password_env: String,
    /// `None` → apksigner uses the store password for the key too.
    pub key_password_env: Option<String>,
}

impl AndroidSigning {
    /// The keystore path, with a relative declaration resolved against the
    /// app crate's directory.
    pub fn keystore_path(&self, project_dir: &Path) -> PathBuf {
        if self.keystore.is_absolute() {
            self.keystore.clone()
        } else {
            project_dir.join(&self.keystore)
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAndroidAppMetadata {
    #[serde(default)]
    version_code: Option<u32>,
    #[serde(default)]
    lock_task_mode: Option<String>,
    #[serde(default)]
    home_launcher: Option<bool>,
    #[serde(default)]
    resizeable: Option<bool>,
    #[serde(default)]
    screen_orientation: Option<String>,
    #[serde(default)]
    signing: Option<RawAndroidSigning>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAndroidSigning {
    keystore: String,
    alias: String,
    #[serde(default)]
    store_password_env: Option<String>,
    #[serde(default)]
    key_password_env: Option<String>,
}

impl AndroidAppMetadata {
    pub(crate) fn from_raw(raw: RawAndroidAppMetadata) -> Result<Self> {
        let lock_task_mode = match raw.lock_task_mode.as_deref() {
            None => None,
            Some("normal") => Some(LockTaskMode::Normal),
            Some("never") => Some(LockTaskMode::Never),
            Some("always") => Some(LockTaskMode::Always),
            Some("if_whitelisted") => Some(LockTaskMode::IfWhitelisted),
            Some(other) => bail!(
                "`[package.metadata.idealyst.app.android].lock_task_mode = {other:?}`: \
                 expected one of \"normal\", \"never\", \"always\", \"if_whitelisted\""
            ),
        };
        if let Some(o) = raw.screen_orientation.as_deref() {
            if !SCREEN_ORIENTATIONS.contains(&o) {
                bail!(
                    "`[package.metadata.idealyst.app.android].screen_orientation = {o:?}`: \
                     expected one of {}",
                    SCREEN_ORIENTATIONS.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join(", ")
                );
            }
        }
        if raw.version_code == Some(0) {
            bail!("`[package.metadata.idealyst.app.android].version_code` must be at least 1");
        }
        if let Some(code) = raw.version_code {
            if u64::from(code) > MAX_VERSION_CODE {
                bail!(
                    "`[package.metadata.idealyst.app.android].version_code = {code}` exceeds \
                     Android's limit of {MAX_VERSION_CODE}"
                );
            }
        }
        let signing = match raw.signing {
            None => None,
            Some(s) => {
                if s.keystore.trim().is_empty() || s.alias.trim().is_empty() {
                    bail!(
                        "`[package.metadata.idealyst.app.android.signing]` needs a non-empty \
                         `keystore` and `alias`"
                    );
                }
                Some(AndroidSigning {
                    keystore: PathBuf::from(s.keystore),
                    alias: s.alias,
                    store_password_env: s
                        .store_password_env
                        .unwrap_or_else(|| DEFAULT_STORE_PASSWORD_ENV.to_string()),
                    key_password_env: s.key_password_env,
                })
            }
        };
        Ok(Self {
            version_code: raw.version_code,
            lock_task_mode,
            home_launcher: raw.home_launcher.unwrap_or(false),
            resizeable: raw.resizeable,
            screen_orientation: raw.screen_orientation,
            signing,
        })
    }

    /// The `versionCode` to build with: the explicit one, else derived from
    /// `version` (`major.minor.patch`, pre-release/build suffix ignored) as
    /// `major × 1_000_000 + minor × 1_000 + patch`. Android refuses an update
    /// that does not raise it, so every released version must map to a
    /// larger number than the one before; the derivation preserves semver
    /// order as long as minor and patch stay below 1000.
    pub fn resolved_version_code(&self, version: &str) -> Result<u32> {
        if let Some(code) = self.version_code {
            return Ok(code);
        }
        version_code_from_version(version)
    }
}

/// See [`AndroidAppMetadata::resolved_version_code`].
pub fn version_code_from_version(version: &str) -> Result<u32> {
    let core = version
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        bail!(
            "app version {version:?} is not `major.minor.patch`; set \
             `[package.metadata.idealyst.app.android].version_code` explicitly"
        );
    }
    let mut nums = [0u64; 3];
    for (i, p) in parts.iter().enumerate() {
        nums[i] = p.parse().map_err(|_| {
            anyhow::anyhow!(
                "app version {version:?} has a non-numeric component {p:?}; set \
                 `[package.metadata.idealyst.app.android].version_code` explicitly"
            )
        })?;
    }
    let [major, minor, patch] = nums;
    if minor >= 1000 || patch >= 1000 {
        bail!(
            "app version {version:?}: minor and patch must be below 1000 to derive a \
             versionCode; set `[package.metadata.idealyst.app.android].version_code` explicitly"
        );
    }
    let code = major * 1_000_000 + minor * 1_000 + patch;
    if code == 0 {
        // `0.0.0` would be versionCode 0, which Android treats as unset.
        return Ok(1);
    }
    if code > MAX_VERSION_CODE {
        bail!("app version {version:?} derives versionCode {code}, above Android's limit");
    }
    Ok(code as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_text: &str) -> Result<AndroidAppMetadata> {
        let raw: RawAndroidAppMetadata = toml::from_str(toml_text)?;
        AndroidAppMetadata::from_raw(raw)
    }

    #[test]
    fn version_code_derives_from_semver_in_order() {
        assert_eq!(version_code_from_version("1.2.3").unwrap(), 1_002_003);
        assert_eq!(version_code_from_version("0.0.1").unwrap(), 1);
        assert_eq!(version_code_from_version("2.0").unwrap(), 2_000_000);
        assert_eq!(version_code_from_version("1.4.0-beta.2").unwrap(), 1_004_000);
        assert!(
            version_code_from_version("1.10.0").unwrap() > version_code_from_version("1.9.999").unwrap()
        );
        assert_eq!(version_code_from_version("0.0.0").unwrap(), 1);
    }

    #[test]
    fn version_code_rejects_what_it_cannot_order() {
        assert!(version_code_from_version("1.1000.0").is_err());
        assert!(version_code_from_version("one.two").is_err());
        assert!(version_code_from_version("1.2.3.4").is_err());
        assert!(version_code_from_version("").is_err());
    }

    #[test]
    fn explicit_version_code_wins() {
        let m = parse("version_code = 77").unwrap();
        assert_eq!(m.resolved_version_code("9.9.9").unwrap(), 77);
        assert!(parse("version_code = 0").is_err());
    }

    #[test]
    fn kiosk_attributes_parse_and_validate() {
        let m = parse("lock_task_mode = \"if_whitelisted\"\nhome_launcher = true").unwrap();
        assert_eq!(m.lock_task_mode, Some(LockTaskMode::IfWhitelisted));
        assert_eq!(m.lock_task_mode.unwrap().manifest_value(), "if_whitelisted");
        assert!(m.home_launcher);
        assert!(parse("lock_task_mode = \"if_allowlisted\"").is_err());
        assert!(parse("lock_task = \"always\"").is_err(), "unknown keys are rejected");
    }

    #[test]
    fn resizeable_and_orientation_parse_and_validate() {
        let m = parse("resizeable = false\nscreen_orientation = \"sensorLandscape\"").unwrap();
        assert_eq!(m.resizeable, Some(false));
        assert_eq!(m.screen_orientation.as_deref(), Some("sensorLandscape"));
        let plain = parse("").unwrap();
        assert_eq!((plain.resizeable, plain.screen_orientation), (None, None), "absent = Android's default");
        let bad = parse("screen_orientation = \"sideways\"").unwrap_err().to_string();
        assert!(bad.contains("sideways") && bad.contains("sensorLandscape"), "{bad}");
    }

    #[test]
    fn signing_defaults_the_store_password_env() {
        let m = parse("[signing]\nkeystore = \"keys/release.jks\"\nalias = \"rel\"").unwrap();
        let s = m.signing.unwrap();
        assert_eq!(s.store_password_env, DEFAULT_STORE_PASSWORD_ENV);
        assert_eq!(s.key_password_env, None);
        assert_eq!(
            s.keystore_path(Path::new("/app")),
            PathBuf::from("/app/keys/release.jks")
        );
        assert!(parse("[signing]\nkeystore = \"\"\nalias = \"rel\"").is_err());
        assert!(parse("[signing]\nalias = \"rel\"").is_err());
    }
}
