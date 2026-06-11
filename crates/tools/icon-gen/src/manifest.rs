//! TOML schema + per-platform merge for the icon block.
//!
//! Schema (under `[package.metadata.idealyst.app.icon]`):
//!
//! ```toml
//! # Common base — applies to every platform unless overridden.
//! source = "assets/master.svg"            # complete icon
//! foreground = "assets/glyph.svg"         # glyph for composited variants
//! background = "#EFDD74"                  # solid hex
//! # or:
//! # background = { kind = "linear", angle = 180, stops = [
//! #     { offset = 0.0, color = "#EFDD74" },
//! #     { offset = 1.0, color = "#ffffff" },
//! # ] }
//!
//! # Per-platform overrides. Field-by-field merge over base.
//! [package.metadata.idealyst.app.icon.web]
//! source = "assets/light-outlined.svg"
//!
//! [package.metadata.idealyst.app.icon.ios]
//! foreground = "assets/glyph-ios.svg"
//! ```
//!
//! Each platform's `sync_*_icons` function consumes the merged
//! [`IconBlock`] returned by [`IconConfig::resolved_for`].

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::Target;

/// Top-level parsed icon configuration. Holds the common [`base`]
/// block plus optional per-platform overrides keyed by target.
#[derive(Debug, Clone, Default)]
pub struct IconConfig {
    pub base: IconBlock,
    pub web: Option<IconBlock>,
    pub ios: Option<IconBlock>,
    pub android: Option<IconBlock>,
    pub macos: Option<IconBlock>,
}

impl IconConfig {
    /// Merge per-platform overrides over [`base`] field-by-field.
    /// Any field set on the override block wins; unset fields fall
    /// through to the base. The returned block is what each
    /// `sync_*_icons` function expects.
    pub fn resolved_for(&self, target: Target) -> IconBlock {
        let over = match target {
            Target::Web => self.web.as_ref(),
            Target::Ios => self.ios.as_ref(),
            Target::Android => self.android.as_ref(),
            Target::Macos => self.macos.as_ref(),
        };
        let pick_path = |o: fn(&IconBlock) -> &Option<PathBuf>| {
            over.and_then(|b| o(b).clone()).or_else(|| o(&self.base).clone())
        };
        let pick_bg = || {
            over.and_then(|b| b.background.clone())
                .or_else(|| self.base.background.clone())
        };
        let pick_padding = || {
            over.and_then(|b| b.foreground_padding)
                .or(self.base.foreground_padding)
        };
        IconBlock {
            source: pick_path(|b| &b.source),
            foreground: pick_path(|b| &b.foreground),
            background: pick_bg(),
            foreground_padding: pick_padding(),
        }
    }
}

/// One layer of the merged config. Each field is independently
/// optional so a platform override can replace just one piece (e.g.
/// only the foreground) without re-stating the rest.
#[derive(Debug, Clone, Default)]
pub struct IconBlock {
    /// Complete icon source rasterized as-is. Preferred input for
    /// favicons; fallback input for iOS/Android when `foreground` +
    /// `background` aren't both set.
    pub source: Option<PathBuf>,
    /// Glyph layer composited over `background` for iOS/Android
    /// home-screen icons. Ignored when `source` carries the same
    /// content (web favicon case).
    pub foreground: Option<PathBuf>,
    /// Backdrop painted before the foreground layer.
    pub background: Option<Background>,
    /// Fractional safe-area margin around the foreground when
    /// composited over `background`. `0.10` (the default applied at
    /// render time) means the foreground occupies the central 80% of
    /// the canvas with 10% padding on each side — matches Apple's
    /// human-interface guidelines for the iOS home-screen icon and
    /// gives Android adaptive masks room to crop without clipping
    /// the glyph. Range: `0.0` (no padding, glyph fills the canvas)
    /// to `0.4` (very tight glyph). Has no effect on the standalone
    /// `source` path — that input is assumed to already include its
    /// own framing.
    pub foreground_padding: Option<f32>,
}

/// Backdrop that fills the icon canvas behind the foreground glyph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Background {
    /// Solid color, `#RRGGBB` or `#RRGGBBAA` (3/4-digit shorthand
    /// also accepted). Parsed by [`crate::render::parse_color`].
    Color(String),
    /// Geometric fill. Phase 2 ships linear; radial is a Phase 3
    /// addition (held off because the rendering math is more
    /// involved and the user's first ask was specifically linear).
    Gradient(Gradient),
    /// An image (SVG / PNG / JPEG) rasterized to fill the canvas as the
    /// backdrop — e.g. a designed background layer for an Android adaptive
    /// icon. Path is resolved + existence-checked against the project dir at
    /// load time. A square source fills exactly; a non-square one is
    /// uniformly scaled and centered (transparent letterbox), same as the
    /// foreground rasterizer.
    Image(PathBuf),
}

/// Geometric fill description. CSS-convention angle: `0` points up,
/// increasing clockwise (so `180` = top→bottom, `90` = left→right).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Gradient {
    Linear {
        angle_deg: f32,
        stops: Vec<Stop>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stop {
    pub offset: f32,
    pub color: String,
}

/// Parse `[package.metadata.idealyst.app.icon]` from a project's
/// `Cargo.toml`. Returns:
///
/// - `Ok(None)` when the manifest is missing or the icon block is
///   absent — the pipeline is a no-op for that project.
/// - `Ok(Some(_))` with paths resolved against `project_dir`.
/// - `Err(_)` when the TOML can't be parsed, or when a declared
///   image path doesn't exist on disk (loud failure so authoring
///   typos surface immediately).
pub fn load_config_from_manifest(project_dir: &Path) -> Result<Option<IconConfig>> {
    let manifest_path = project_dir.join("Cargo.toml");
    let raw = match fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("read {}", manifest_path.display()))
        }
    };

    #[derive(Deserialize)]
    struct Outer {
        #[serde(default)]
        package: Option<Pkg>,
    }
    #[derive(Deserialize)]
    struct Pkg {
        #[serde(default)]
        metadata: Option<Meta>,
    }
    #[derive(Deserialize)]
    struct Meta {
        #[serde(default)]
        idealyst: Option<Idealyst>,
    }
    #[derive(Deserialize)]
    struct Idealyst {
        #[serde(default)]
        app: Option<App>,
    }
    #[derive(Deserialize)]
    struct App {
        #[serde(default)]
        icon: Option<IconRaw>,
    }

    let outer: Outer = toml::from_str(&raw)
        .with_context(|| format!("parse {}", manifest_path.display()))?;
    let Some(raw_icon) = outer
        .package
        .and_then(|p| p.metadata)
        .and_then(|m| m.idealyst)
        .and_then(|i| i.app)
        .and_then(|a| a.icon)
    else {
        return Ok(None);
    };

    // A block with NO fields anywhere is treated as "user wrote
    // `[icon]` but left it empty" — equivalent to not declaring it
    // at all. Avoids surprising the caller with an empty config
    // that has nothing to render.
    if raw_icon_is_empty(&raw_icon) {
        return Ok(None);
    }

    let base = build_block(&raw_icon, project_dir)?;
    let web = raw_icon
        .web
        .as_deref()
        .map(|r| build_block(r, project_dir))
        .transpose()?;
    let ios = raw_icon
        .ios
        .as_deref()
        .map(|r| build_block(r, project_dir))
        .transpose()?;
    let android = raw_icon
        .android
        .as_deref()
        .map(|r| build_block(r, project_dir))
        .transpose()?;
    let macos = raw_icon
        .macos
        .as_deref()
        .map(|r| build_block(r, project_dir))
        .transpose()?;

    Ok(Some(IconConfig {
        base,
        web,
        ios,
        android,
        macos,
    }))
}

fn raw_icon_is_empty(r: &IconRaw) -> bool {
    r.source.is_none()
        && r.foreground.is_none()
        && r.background.is_none()
        && r.foreground_padding.is_none()
        && r.web.is_none()
        && r.ios.is_none()
        && r.android.is_none()
        && r.macos.is_none()
}

fn build_block(raw: &IconRaw, project_dir: &Path) -> Result<IconBlock> {
    if let Some(p) = raw.foreground_padding {
        if !(0.0..=0.5).contains(&p) {
            anyhow::bail!(
                "[package.metadata.idealyst.app.icon].foreground_padding = {p} \
                 is out of range (expected 0.0 ≤ value ≤ 0.5)",
            );
        }
    }
    Ok(IconBlock {
        source: resolve_path("source", raw.source.as_deref(), project_dir)?,
        foreground: resolve_path("foreground", raw.foreground.as_deref(), project_dir)?,
        background: raw
            .background
            .as_ref()
            .map(|b| build_background(b, project_dir))
            .transpose()?,
        foreground_padding: raw.foreground_padding,
    })
}

/// Resolve a project-relative path. Returns `Ok(None)` when the
/// field is unset; surfaces a clear error when the field IS set
/// but the file is missing.
fn resolve_path(
    field: &str,
    rel: Option<&str>,
    project_dir: &Path,
) -> Result<Option<PathBuf>> {
    let Some(rel) = rel else { return Ok(None) };
    let p = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        project_dir.join(rel)
    };
    if !p.is_file() {
        anyhow::bail!(
            "[package.metadata.idealyst.app.icon].{field} = {rel:?} \
             does not exist (resolved to {})",
            p.display(),
        );
    }
    Ok(Some(p))
}

// Inner-struct types used by both the parse fn and the helpers
// above. Kept module-private because they only exist to mirror the
// TOML shape — the public surface is the typed [`IconConfig`].
#[derive(Deserialize, Default)]
struct IconRaw {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    foreground: Option<String>,
    #[serde(default)]
    background: Option<BackgroundRaw>,
    #[serde(default)]
    foreground_padding: Option<f32>,
    #[serde(default)]
    web: Option<Box<IconRaw>>,
    #[serde(default)]
    ios: Option<Box<IconRaw>>,
    #[serde(default)]
    android: Option<Box<IconRaw>>,
    #[serde(default)]
    macos: Option<Box<IconRaw>>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum BackgroundRaw {
    /// Solid color shorthand: `background = "#EFDD74"`. Validated at
    /// render time; not at parse time, because the TOML parser sees
    /// a free-form string here either way.
    Color(String),
    Table(BackgroundTable),
}

#[derive(Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum BackgroundTable {
    Linear {
        #[serde(default)]
        angle: Option<f32>,
        stops: Vec<StopRaw>,
    },
    /// `background = { kind = "image", source = "path/to/bg.svg" }` — a
    /// rasterized image backdrop (SVG/PNG/JPEG).
    Image {
        source: String,
    },
}

#[derive(Deserialize, Clone)]
struct StopRaw {
    offset: f32,
    color: String,
}

/// Build a [`Background`] from its raw TOML form, resolving + validating the
/// image path (relative to `project_dir`) for the image variant. Color and
/// gradient carry no path, so they pass through unchanged.
fn build_background(raw: &BackgroundRaw, project_dir: &Path) -> Result<Background> {
    Ok(match raw {
        BackgroundRaw::Color(c) => Background::Color(c.clone()),
        BackgroundRaw::Table(BackgroundTable::Linear { angle, stops }) => {
            Background::Gradient(Gradient::Linear {
                angle_deg: angle.unwrap_or(180.0),
                stops: stops
                    .iter()
                    .map(|s| Stop {
                        offset: s.offset,
                        color: s.color.clone(),
                    })
                    .collect(),
            })
        }
        BackgroundRaw::Table(BackgroundTable::Image { source }) => {
            let path = resolve_path("background.source", Some(source), project_dir)?
                .expect("source is Some");
            Background::Image(path)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, body: &str) {
        fs::write(dir.join("Cargo.toml"), body).unwrap();
    }

    fn dummy_asset(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, "<svg/>").unwrap();
        p
    }

    #[test]
    fn returns_none_when_block_absent() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r##"
[package]
name = "demo"
version = "0.1.0"
edition = "2021"
"##,
        );
        assert!(load_config_from_manifest(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn parses_simple_source_only() {
        let tmp = tempfile::tempdir().unwrap();
        dummy_asset(tmp.path(), "brand.svg");
        write_manifest(
            tmp.path(),
            r##"
[package]
name = "demo"
version = "0.1.0"
edition = "2021"

[package.metadata.idealyst.app.icon]
source = "brand.svg"
"##,
        );
        let cfg = load_config_from_manifest(tmp.path()).unwrap().unwrap();
        assert_eq!(cfg.base.source.unwrap(), tmp.path().join("brand.svg"));
        assert!(cfg.base.foreground.is_none());
        assert!(cfg.web.is_none());
    }

    #[test]
    fn parses_inline_linear_gradient() {
        let tmp = tempfile::tempdir().unwrap();
        dummy_asset(tmp.path(), "glyph.svg");
        write_manifest(
            tmp.path(),
            r##"
[package]
name = "demo"
version = "0.1.0"
edition = "2021"

[package.metadata.idealyst.app.icon]
foreground = "glyph.svg"
background = { kind = "linear", angle = 180, stops = [
    { offset = 0.0, color = "#EFDD74" },
    { offset = 1.0, color = "#ffffff" },
] }
"##,
        );
        let cfg = load_config_from_manifest(tmp.path()).unwrap().unwrap();
        let bg = cfg.base.background.unwrap();
        let Background::Gradient(Gradient::Linear { angle_deg, stops }) = bg else {
            panic!("expected linear gradient");
        };
        assert_eq!(angle_deg, 180.0);
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].offset, 0.0);
        assert_eq!(stops[0].color, "#EFDD74");
        assert_eq!(stops[1].color, "#ffffff");
    }

    #[test]
    fn parses_solid_color_shorthand() {
        let tmp = tempfile::tempdir().unwrap();
        dummy_asset(tmp.path(), "glyph.svg");
        write_manifest(
            tmp.path(),
            r##"
[package]
name = "demo"
version = "0.1.0"
edition = "2021"

[package.metadata.idealyst.app.icon]
foreground = "glyph.svg"
background = "#ff7a00"
"##,
        );
        let cfg = load_config_from_manifest(tmp.path()).unwrap().unwrap();
        match cfg.base.background.unwrap() {
            Background::Color(c) => assert_eq!(c, "#ff7a00"),
            other => panic!("expected solid color, got {other:?}"),
        }
    }

    #[test]
    fn per_platform_override_merges_field_by_field() {
        // Base has foreground + background; web override replaces
        // only `source`; iOS override replaces only `foreground`.
        // The merge result must combine all three field-by-field.
        let tmp = tempfile::tempdir().unwrap();
        dummy_asset(tmp.path(), "light.svg");
        dummy_asset(tmp.path(), "light-outlined.svg");
        dummy_asset(tmp.path(), "glyph-ios.svg");
        write_manifest(
            tmp.path(),
            r##"
[package]
name = "demo"
version = "0.1.0"
edition = "2021"

[package.metadata.idealyst.app.icon]
foreground = "light.svg"
background = "#EFDD74"

[package.metadata.idealyst.app.icon.web]
source = "light-outlined.svg"

[package.metadata.idealyst.app.icon.ios]
foreground = "glyph-ios.svg"
"##,
        );
        let cfg = load_config_from_manifest(tmp.path()).unwrap().unwrap();

        // Web: source from override, foreground+background inherited.
        let web = cfg.resolved_for(Target::Web);
        assert_eq!(web.source.unwrap(), tmp.path().join("light-outlined.svg"));
        assert_eq!(web.foreground.unwrap(), tmp.path().join("light.svg"));
        assert!(matches!(web.background, Some(Background::Color(_))));

        // iOS: foreground from override, background inherited, no source.
        let ios = cfg.resolved_for(Target::Ios);
        assert!(ios.source.is_none());
        assert_eq!(ios.foreground.unwrap(), tmp.path().join("glyph-ios.svg"));
        assert!(matches!(ios.background, Some(Background::Color(_))));

        // Android: every field inherited from base.
        let android = cfg.resolved_for(Target::Android);
        assert!(android.source.is_none());
        assert_eq!(android.foreground.unwrap(), tmp.path().join("light.svg"));
    }

    #[test]
    fn errors_when_declared_path_missing() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r##"
[package]
name = "demo"
version = "0.1.0"
edition = "2021"

[package.metadata.idealyst.app.icon]
source = "ghost.svg"
"##,
        );
        let err = load_config_from_manifest(tmp.path()).err().unwrap();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("ghost.svg") && msg.contains("does not exist"),
            "expected missing-file diagnostic, got: {msg}",
        );
    }
}
