//! `[package.metadata.idealyst.app.splash]` — the native launch screen.
//!
//! The splash is the OS launch screen: iOS draws it from `UILaunchScreen`
//! (an asset-catalog color + image), Android from the launching
//! Activity's theme (`windowSplashScreen*` on API 31+, `windowBackground`
//! below). Either way the system shows it from the icon tap until the
//! app's first frame, so it never delays the mount. The app owns any
//! loading state after that.
//!
//! ```toml
//! [package.metadata.idealyst.app.splash]
//! background = "#1a1a2e"        # fill, "#RRGGBB"
//! logo       = "assets/logo.svg" # optional, centered; .svg/.png/.jpg
//!
//! [package.metadata.idealyst.app.splash.dark]
//! background = "#000000"        # each key falls back to the light value
//! logo       = "assets/logo-dark.svg"
//! ```
//!
//! This module only parses and resolves; the per-platform assets are
//! rendered by `icon-gen` (`sync_ios_splash_catalog` /
//! `sync_android_splash_res`).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Light-mode fill when the block sets no `background`. Matches iOS's
/// `systemBackground`, which is what an empty `UILaunchScreen` shows, so
/// declaring only a logo changes nothing else.
const DEFAULT_LIGHT_BACKGROUND: &str = "#FFFFFF";
/// Dark-mode fill when neither `dark.background` nor `background` is set
/// (iOS `systemBackground` in dark mode).
const DEFAULT_DARK_BACKGROUND: &str = "#000000";

/// Keys of the timed in-app title splash. They are ignored with a
/// warning, and a block holding nothing else resolves to no splash: such
/// blocks were written to switch that splash off (`duration_ms = 0`).
const LEGACY_KEYS: &[&str] = &["title", "title_color", "duration_ms"];

/// One appearance of the launch screen, fully resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplashLook {
    /// `"#RRGGBB"`, validated at parse time.
    pub background: String,
    /// Absolute path to the logo source, checked to exist at parse time.
    pub logo: Option<PathBuf>,
}

/// The resolved launch screen. `dark` is always populated: unset dark
/// keys inherit the light value (or the dark default fill), so renderers
/// emit both appearances unconditionally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplashConfig {
    pub light: SplashLook,
    pub dark: SplashLook,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSplashConfig {
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    dark: Option<RawSplashLook>,
    #[serde(default)]
    title: Option<toml::Value>,
    #[serde(default)]
    title_color: Option<toml::Value>,
    #[serde(default)]
    duration_ms: Option<toml::Value>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSplashLook {
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    logo: Option<String>,
}

impl SplashConfig {
    /// Resolve the raw table against `project_dir` (logo paths are
    /// project-relative). `None` when the block carries only legacy keys.
    pub(crate) fn from_raw(raw: RawSplashConfig, project_dir: &Path) -> Result<Option<Self>> {
        let legacy: Vec<&str> = [
            raw.title.is_some(),
            raw.title_color.is_some(),
            raw.duration_ms.is_some(),
        ]
        .iter()
        .zip(LEGACY_KEYS)
        .filter_map(|(set, key)| set.then_some(*key))
        .collect();
        if !legacy.is_empty() {
            warn_legacy_once(&legacy);
            if raw.background.is_none() && raw.logo.is_none() && raw.dark.is_none() {
                return Ok(None);
            }
        }

        let light_bg = raw.background.as_deref().map(validate_hex).transpose()?;
        let light_logo = raw.logo.as_deref().map(|p| resolve_logo(p, project_dir)).transpose()?;
        let dark_raw = raw.dark.unwrap_or_default();
        let dark_bg = dark_raw.background.as_deref().map(validate_hex).transpose()?;
        let dark_logo = dark_raw
            .logo
            .as_deref()
            .map(|p| resolve_logo(p, project_dir))
            .transpose()?;

        Ok(Some(Self {
            dark: SplashLook {
                background: dark_bg
                    .or_else(|| light_bg.clone())
                    .unwrap_or_else(|| DEFAULT_DARK_BACKGROUND.to_string()),
                logo: dark_logo.or_else(|| light_logo.clone()),
            },
            light: SplashLook {
                background: light_bg.unwrap_or_else(|| DEFAULT_LIGHT_BACKGROUND.to_string()),
                logo: light_logo,
            },
        }))
    }
}

/// `parse_manifest` runs several times per CLI invocation; say it once.
fn warn_legacy_once(keys: &[&str]) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    eprintln!(
        "warning: `[package.metadata.idealyst.app.splash]` ignores {}: the splash is the \
         OS launch screen, shown until the app's first frame. Configure it with \
         `background`, `logo`, and `[package.metadata.idealyst.app.splash.dark]`.",
        keys.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", "),
    );
}

/// Accept `#RRGGBB` (case-insensitive) and normalize to upper case.
fn validate_hex(s: &str) -> Result<String> {
    let digits = s.strip_prefix('#').unwrap_or("");
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("splash color {s:?} must be \"#RRGGBB\"");
    }
    Ok(format!("#{}", digits.to_ascii_uppercase()))
}

fn resolve_logo(rel: &str, project_dir: &Path) -> Result<PathBuf> {
    let path = project_dir.join(rel);
    if !path.is_file() {
        bail!("splash logo {} does not exist", path.display());
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if !matches!(ext.as_deref(), Some("svg" | "png" | "jpg" | "jpeg")) {
        bail!("splash logo {} must be .svg, .png, or .jpg", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("resolve splash logo {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_src: &str, project_dir: &Path) -> Result<SplashConfig> {
        parse_opt(toml_src, project_dir).map(|c| c.expect("splash configured"))
    }

    fn parse_opt(toml_src: &str, project_dir: &Path) -> Result<Option<SplashConfig>> {
        let raw: RawSplashConfig = toml::from_str(toml_src)?;
        SplashConfig::from_raw(raw, project_dir)
    }

    #[test]
    fn dark_inherits_each_unset_key_from_light() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("logo.svg"), "<svg/>").unwrap();
        let cfg = parse(
            r##"background = "#1a1a2e"
                logo = "logo.svg""##,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(cfg.light.background, "#1A1A2E");
        assert_eq!(cfg.dark, cfg.light);
    }

    #[test]
    fn dark_overrides_win_and_defaults_match_system_background() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = parse("", tmp.path()).unwrap();
        assert_eq!(cfg.light.background, DEFAULT_LIGHT_BACKGROUND);
        assert_eq!(cfg.dark.background, DEFAULT_DARK_BACKGROUND);

        let cfg = parse(
            r##"[dark]
                background = "#000011""##,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(cfg.light.background, DEFAULT_LIGHT_BACKGROUND);
        assert_eq!(cfg.dark.background, "#000011");
    }

    #[test]
    fn regression_legacy_splash_off_block_stays_off() {
        // Projects carry `duration_ms = 0` to switch the old timed splash
        // off. That TOML must keep building and must not turn the native
        // launch screen on (which hides Android 12+'s launcher icon).
        let tmp = tempfile::tempdir().unwrap();
        let cfg = parse_opt(
            r##"duration_ms = 0
                title = "Demo"
                title_color = "#ffffff""##,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(cfg, None);

        // With a real key beside them, the legacy keys are just ignored.
        let cfg = parse(
            r##"duration_ms = 1500
                background = "#102030""##,
            tmp.path(),
        )
        .unwrap();
        assert_eq!(cfg.light.background, "#102030");
    }

    #[test]
    fn bad_color_missing_logo_and_typos_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(parse(r##"background = "red""##, tmp.path()).is_err());
        assert!(parse(r##"background = "#fff""##, tmp.path()).is_err());
        assert!(parse(r##"logo = "nope.svg""##, tmp.path()).is_err());
        assert!(parse(r##"backgrund = "#ffffff""##, tmp.path()).is_err());
    }
}
