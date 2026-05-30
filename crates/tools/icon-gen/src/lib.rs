//! Icon generation pipeline.
//!
//! One [`IconConfig`] declared in TOML becomes every platform's
//! app-icon asset. The config carries a common base plus optional
//! per-platform overrides (web / ios / android); each platform's
//! `sync_*_icons` function takes the resolved block for its target.
//!
//! ## Rendering rules
//!
//! Each platform's render picks ONE of:
//!
//! 1. `source` is set → rasterize it directly. The favicon path
//!    treats this as the preferred input; iOS/Android use it as a
//!    fallback when no foreground/background pair is configured.
//! 2. `foreground` + `background` are set → composite the
//!    foreground SVG over the painted background. The iOS/Android
//!    home-screen path treats this as preferred (filling the canvas
//!    means the system mask can round corners without leaking
//!    transparency).
//! 3. Neither → error.
//!
//! The `sync_*` functions return `Ok(None)` when their input is
//! `None` (no icon configured) — opt-in by design.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

mod cache;
mod manifest;
mod render;

pub use manifest::{load_config_from_manifest, Background, Gradient, IconBlock, IconConfig, Stop};

/// Targets that get their own [`IconBlock`] in the merged config.
/// Mirrors the per-platform override sub-tables in TOML
/// (`[icon.web]`, `[icon.ios]`, `[icon.android]`, `[icon.macos]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Web,
    Ios,
    Android,
    Macos,
}

// ---------------------------------------------------------------------------
// Web
// ---------------------------------------------------------------------------

/// Files written by [`sync_web_icons`].
#[derive(Debug, Serialize, Deserialize)]
pub struct WebOutputs {
    pub favicon_ico: PathBuf,
    pub favicon_192: PathBuf,
    pub favicon_512: PathBuf,
    pub apple_touch_icon: PathBuf,
}

impl cache::AllOutputs for WebOutputs {
    fn all_files_exist(&self) -> bool {
        self.favicon_ico.is_file()
            && self.favicon_192.is_file()
            && self.favicon_512.is_file()
            && self.apple_touch_icon.is_file()
    }
}

const FAVICON_ICO_SIZES: &[u32] = &[16, 32, 48];
const FAVICON_PNG_192: u32 = 192;
const FAVICON_PNG_512: u32 = 512;
const APPLE_TOUCH_ICON: u32 = 180;

/// `<link>` tag block referencing the four standard web outputs at
/// their bundle-root paths. Used by both `build-web` (splices into
/// the staged `index.html` at build time) and `dev-http` (injects
/// into served HTML at request time). Centralizing the tag list
/// here keeps build / dev output identical when the icon config
/// matches.
///
/// - `favicon.ico` carries 16/32/48 — browsers pick the best fit
///   for tab strip and Windows PWA pin from one `<link>`.
/// - The 192/512 PNGs feed the web app manifest / Add-to-Home-Screen
///   on Android Chrome.
/// - `apple-touch-icon` is what Safari pins to the iOS home screen.
pub fn web_icon_link_tags() -> String {
    [
        r#"<link rel="icon" type="image/x-icon" href="/favicon.ico" sizes="16x16 32x32 48x48">"#,
        r#"<link rel="icon" type="image/png" href="/favicon-192.png" sizes="192x192">"#,
        r#"<link rel="icon" type="image/png" href="/favicon-512.png" sizes="512x512">"#,
        r#"<link rel="apple-touch-icon" href="/apple-touch-icon.png" sizes="180x180">"#,
    ]
    .join("\n  ")
}

pub fn sync_web_icons(
    icon: Option<&IconBlock>,
    out_dir: &Path,
) -> Result<Option<WebOutputs>> {
    let Some(icon) = icon else {
        return Ok(None);
    };
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create icon output dir {}", out_dir.display()))?;

    let cache_path = out_dir.join(cache::CACHE_FILE_NAME);
    if let Some(cached) = cache::try_hit::<WebOutputs>(&cache_path, icon)? {
        return Ok(Some(cached));
    }

    let source = render::Source::from_block(icon)?;

    let favicon_ico = out_dir.join("favicon.ico");
    write_ico_bundle(&source, FAVICON_ICO_SIZES, &favicon_ico)?;

    let favicon_192 = out_dir.join("favicon-192.png");
    write_png(&source, FAVICON_PNG_192, &favicon_192)?;

    let favicon_512 = out_dir.join("favicon-512.png");
    write_png(&source, FAVICON_PNG_512, &favicon_512)?;

    let apple_touch_icon = out_dir.join("apple-touch-icon.png");
    write_png(&source, APPLE_TOUCH_ICON, &apple_touch_icon)?;

    let outputs = WebOutputs {
        favicon_ico,
        favicon_192,
        favicon_512,
        apple_touch_icon,
    };
    cache::write(&cache_path, icon, &outputs)?;
    Ok(Some(outputs))
}

// ---------------------------------------------------------------------------
// iOS
// ---------------------------------------------------------------------------

/// Files written by [`sync_ios_icons`].
///
/// `entries` is the list of `(filename, pixel_size)` pairs written
/// at the bundle root, suitable for splicing into `Info.plist`'s
/// `CFBundleIcons.CFBundlePrimaryIcon.CFBundleIconFiles` array.
#[derive(Debug, Serialize, Deserialize)]
pub struct IosOutputs {
    pub entries: Vec<IosIconEntry>,
    pub marketing_1024: PathBuf,
}

impl cache::AllOutputs for IosOutputs {
    fn all_files_exist(&self) -> bool {
        self.marketing_1024.is_file() && self.entries.iter().all(|e| e.path.is_file())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IosIconEntry {
    /// Stem used in `Info.plist` (e.g. `"AppIcon60x60"`). iOS appends
    /// `@2x.png` / `@3x.png` itself at lookup time when the right
    /// retina-suffix files are present.
    pub plist_stem: String,
    /// Concrete filename on disk (e.g. `"AppIcon60x60@2x.png"`).
    pub file_name: String,
    /// Pixel size of the file (e.g. 120 for `AppIcon60x60@2x.png`).
    pub size_px: u32,
    pub path: PathBuf,
}

/// Standard non-Xcode iOS app-icon set. Each entry has a logical
/// "point" stem (`AppIcon{pt}x{pt}`) and a concrete file at the
/// matching retina scale. iOS reads `CFBundleIcons.CFBundleIconFiles`
/// listing the *stems*; the runtime picks the matching `@1x` / `@2x`
/// / `@3x` PNG based on device density.
///
/// Sizes covered:
/// - 20pt @2x/@3x — notifications
/// - 29pt @2x/@3x — Settings.app icon
/// - 40pt @2x/@3x — Spotlight result icon
/// - 60pt @2x/@3x — iPhone home-screen icon
/// - 76pt @2x — iPad home-screen icon (legacy iPads still on iOS 18)
///
/// iPad Pro's 83.5pt is omitted — `idealyst run ios` is iPhone-form
/// only today; adding it costs a render but the asset never gets
/// requested.
const IOS_ICON_ENTRIES: &[IosEntrySpec] = &[
    IosEntrySpec { point: 20, scale: 2 },
    IosEntrySpec { point: 20, scale: 3 },
    IosEntrySpec { point: 29, scale: 2 },
    IosEntrySpec { point: 29, scale: 3 },
    IosEntrySpec { point: 40, scale: 2 },
    IosEntrySpec { point: 40, scale: 3 },
    IosEntrySpec { point: 60, scale: 2 },
    IosEntrySpec { point: 60, scale: 3 },
    IosEntrySpec { point: 76, scale: 2 },
];

struct IosEntrySpec {
    point: u32,
    scale: u32,
}

pub fn sync_ios_icons(
    icon: Option<&IconBlock>,
    out_dir: &Path,
) -> Result<Option<IosOutputs>> {
    let Some(icon) = icon else {
        return Ok(None);
    };
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create iOS icon output dir {}", out_dir.display()))?;

    let cache_path = out_dir.join(cache::CACHE_FILE_NAME);
    if let Some(cached) = cache::try_hit::<IosOutputs>(&cache_path, icon)? {
        return Ok(Some(cached));
    }

    let source = render::Source::from_block(icon)?;

    let mut entries = Vec::with_capacity(IOS_ICON_ENTRIES.len());
    for spec in IOS_ICON_ENTRIES {
        let size_px = spec.point * spec.scale;
        let plist_stem = format!("AppIcon{pt}x{pt}", pt = spec.point);
        let file_name = format!("{plist_stem}@{}x.png", spec.scale);
        let path = out_dir.join(&file_name);
        write_png(&source, size_px, &path)?;
        entries.push(IosIconEntry {
            plist_stem,
            file_name,
            size_px,
            path,
        });
    }

    // Marketing icon — the App Store ingestion size. Always 1024px,
    // never gets a `@<scale>x` suffix because it never appears on a
    // retina surface.
    let marketing_1024 = out_dir.join("AppIcon-1024.png");
    write_png(&source, 1024, &marketing_1024)?;

    let outputs = IosOutputs {
        entries,
        marketing_1024,
    };
    cache::write(&cache_path, icon, &outputs)?;
    Ok(Some(outputs))
}

// ---------------------------------------------------------------------------
// Android
// ---------------------------------------------------------------------------

/// Files written by [`sync_android_icons`].
///
/// Always includes the flat-PNG legacy set
/// (`mipmap-{m,h,xh,xxh,xxxh}dpi/ic_launcher.png` +
/// `ic_launcher_round.png`) for backward compatibility. When the
/// resolved block carries BOTH `foreground` and `background`,
/// [`adaptive`] is `Some` with the API-26+ adaptive set
/// (`mipmap-anydpi-v26/ic_launcher.xml` + foreground/background
/// layer PNGs per dpi). The Android system picks adaptive on
/// API 26+ and falls back to flat on older devices automatically;
/// no manifest change is needed.
#[derive(Debug, Serialize, Deserialize)]
pub struct AndroidOutputs {
    pub launcher_pngs: Vec<PathBuf>,
    pub round_pngs: Vec<PathBuf>,
    pub adaptive: Option<AdaptiveAndroidOutputs>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdaptiveAndroidOutputs {
    /// `mipmap-anydpi-v26/ic_launcher.xml` — single XML referencing
    /// the foreground + background drawables. Same XML is duplicated
    /// to `ic_launcher_round.xml` so round-mask launchers see the
    /// adaptive variant too.
    pub launcher_xml: PathBuf,
    pub launcher_round_xml: PathBuf,
    pub foreground_pngs: Vec<PathBuf>,
    pub background_pngs: Vec<PathBuf>,
}

impl cache::AllOutputs for AndroidOutputs {
    fn all_files_exist(&self) -> bool {
        let flat = self.launcher_pngs.iter().all(|p| p.is_file())
            && self.round_pngs.iter().all(|p| p.is_file());
        let adaptive = match &self.adaptive {
            Some(a) => {
                a.launcher_xml.is_file()
                    && a.launcher_round_xml.is_file()
                    && a.foreground_pngs.iter().all(|p| p.is_file())
                    && a.background_pngs.iter().all(|p| p.is_file())
            }
            None => true,
        };
        flat && adaptive
    }
}

/// dpi bucket → ic_launcher.png pixel size. Standard Android
/// densities; 48px at mdpi is the design-time baseline that
/// scales linearly through xxxhdpi at 4x.
const ANDROID_DPI_SIZES: &[(&str, u32)] = &[
    ("mdpi", 48),
    ("hdpi", 72),
    ("xhdpi", 96),
    ("xxhdpi", 144),
    ("xxxhdpi", 192),
];

/// Adaptive icon layer dimensions per dpi bucket. The adaptive spec
/// canvas is 108dp regardless of dpi, so each bucket gets the same
/// multiplier we apply to ic_launcher.png (mdpi 1×, hdpi 1.5×, …)
/// against the 108dp baseline. mdpi = 108, hdpi = 162, xhdpi = 216,
/// xxhdpi = 324, xxxhdpi = 432.
const ANDROID_ADAPTIVE_SIZES: &[(&str, u32)] = &[
    ("mdpi", 108),
    ("hdpi", 162),
    ("xhdpi", 216),
    ("xxhdpi", 324),
    ("xxxhdpi", 432),
];

/// Safe-zone padding for the adaptive foreground. Android's
/// adaptive-icon spec defines a 66dp visible-area diameter inside
/// the 108dp canvas, so the glyph must fit within the central
/// 66×66dp area. Distance from edge to safe-zone edge is
/// (108-66)/2 = 21dp out of 108dp ≈ 0.194. The remaining 21dp
/// outer ring gets cropped by the system mask on most launchers.
const ADAPTIVE_FOREGROUND_PADDING: f32 = 21.0 / 108.0;

/// `mipmap-anydpi-v26/ic_launcher.xml` body. Identical for every
/// project — the foreground/background drawables are referenced by
/// resource ID, not by literal value.
const ADAPTIVE_ICON_XML: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@mipmap/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
"##;

pub fn sync_android_icons(
    icon: Option<&IconBlock>,
    out_dir: &Path,
) -> Result<Option<AndroidOutputs>> {
    let Some(icon) = icon else {
        return Ok(None);
    };
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create Android icon output dir {}", out_dir.display()))?;

    let cache_path = out_dir.join(cache::CACHE_FILE_NAME);
    if let Some(cached) = cache::try_hit::<AndroidOutputs>(&cache_path, icon)? {
        return Ok(Some(cached));
    }

    let source = render::Source::from_block(icon)?;

    let mut launcher_pngs = Vec::new();
    let mut round_pngs = Vec::new();
    for (dpi, size) in ANDROID_DPI_SIZES {
        let dpi_dir = out_dir.join(format!("mipmap-{dpi}"));
        fs::create_dir_all(&dpi_dir)
            .with_context(|| format!("create {}", dpi_dir.display()))?;
        let launcher = dpi_dir.join("ic_launcher.png");
        write_png(&source, *size, &launcher)?;
        // Round-mask launchers (Pixel pre-API 26 fallback) request
        // `ic_launcher_round.png`. Same render as the square flat
        // launcher — system rounds it on the way out. API 26+
        // adaptive launchers prefer the XML below.
        let round = dpi_dir.join("ic_launcher_round.png");
        fs::copy(&launcher, &round)
            .with_context(|| format!("copy {} → {}", launcher.display(), round.display()))?;
        launcher_pngs.push(launcher);
        round_pngs.push(round);
    }

    // Adaptive variant — only when the block carries BOTH a
    // foreground glyph and a background, since adaptive icons
    // require layering. The XML + per-dpi layer PNGs land in
    // canonical positions; API 26+ devices auto-pick this over
    // the flat fallback above.
    let adaptive = if icon.foreground.is_some() && icon.background.is_some() {
        let mut foreground_pngs = Vec::new();
        let mut background_pngs = Vec::new();
        for (dpi, size) in ANDROID_ADAPTIVE_SIZES {
            let dpi_dir = out_dir.join(format!("mipmap-{dpi}"));
            fs::create_dir_all(&dpi_dir)
                .with_context(|| format!("create {}", dpi_dir.display()))?;

            let fg_path = dpi_dir.join("ic_launcher_foreground.png");
            let fg_bytes = render::render_foreground_only_png(
                icon,
                *size,
                ADAPTIVE_FOREGROUND_PADDING,
            )?;
            fs::write(&fg_path, fg_bytes)
                .with_context(|| format!("write {}", fg_path.display()))?;
            foreground_pngs.push(fg_path);

            let bg_path = dpi_dir.join("ic_launcher_background.png");
            let bg_bytes = render::render_background_only_png(icon, *size)?;
            fs::write(&bg_path, bg_bytes)
                .with_context(|| format!("write {}", bg_path.display()))?;
            background_pngs.push(bg_path);
        }

        // Single XML serves all dpi buckets — that's the whole
        // point of `mipmap-anydpi-v26` (resource qualifier: "any
        // density, API 26+"). The system resolves the
        // `@mipmap/ic_launcher_foreground` reference to whichever
        // dpi bucket matches the device.
        let xml_dir = out_dir.join("mipmap-anydpi-v26");
        fs::create_dir_all(&xml_dir)
            .with_context(|| format!("create {}", xml_dir.display()))?;
        let launcher_xml = xml_dir.join("ic_launcher.xml");
        fs::write(&launcher_xml, ADAPTIVE_ICON_XML)
            .with_context(|| format!("write {}", launcher_xml.display()))?;
        // Round variant gets the exact same XML — adaptive
        // foreground+background is shape-agnostic; the launcher's
        // mask decides whether to crop circular or squircle.
        let launcher_round_xml = xml_dir.join("ic_launcher_round.xml");
        fs::copy(&launcher_xml, &launcher_round_xml).with_context(|| {
            format!(
                "copy {} → {}",
                launcher_xml.display(),
                launcher_round_xml.display(),
            )
        })?;

        Some(AdaptiveAndroidOutputs {
            launcher_xml,
            launcher_round_xml,
            foreground_pngs,
            background_pngs,
        })
    } else {
        None
    };

    let outputs = AndroidOutputs {
        launcher_pngs,
        round_pngs,
        adaptive,
    };
    cache::write(&cache_path, icon, &outputs)?;
    Ok(Some(outputs))
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

/// Files written by [`sync_macos_icns`].
#[derive(Debug, Serialize, Deserialize)]
pub struct MacosOutputs {
    /// `.icns` file ready to drop into `<App>.app/Contents/Resources/`.
    /// Info.plist's `CFBundleIconFile` should reference its stem
    /// (`"AppIcon"`, no extension).
    pub icns: PathBuf,
}

impl cache::AllOutputs for MacosOutputs {
    fn all_files_exist(&self) -> bool {
        self.icns.is_file()
    }
}

/// Standard Apple `.icns` slot set. Each entry maps a pixel size to
/// the `icns::IconType` slot that holds it; the @2x retina slots
/// store double-resolution PNGs that macOS picks for retina screens.
/// Matches what `iconutil` produces from an `.iconset/` directory.
const ICNS_SLOTS: &[(u32, icns::IconType)] = &[
    (16, icns::IconType::RGBA32_16x16),
    (32, icns::IconType::RGBA32_16x16_2x),
    (32, icns::IconType::RGBA32_32x32),
    (64, icns::IconType::RGBA32_32x32_2x),
    (128, icns::IconType::RGBA32_128x128),
    (256, icns::IconType::RGBA32_128x128_2x),
    (256, icns::IconType::RGBA32_256x256),
    (512, icns::IconType::RGBA32_256x256_2x),
    (512, icns::IconType::RGBA32_512x512),
    (1024, icns::IconType::RGBA32_512x512_2x),
];

pub fn sync_macos_icns(
    icon: Option<&IconBlock>,
    out_dir: &Path,
) -> Result<Option<MacosOutputs>> {
    let Some(icon) = icon else {
        return Ok(None);
    };
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create macOS icon output dir {}", out_dir.display()))?;

    let cache_path = out_dir.join(cache::CACHE_FILE_NAME);
    if let Some(cached) = cache::try_hit::<MacosOutputs>(&cache_path, icon)? {
        return Ok(Some(cached));
    }

    let source = render::Source::from_block(icon)?;

    // Dedupe sizes — several slot pairs share a pixel size (e.g.
    // `RGBA32_16x16_2x` and `RGBA32_32x32` are both 32 px). Render
    // each unique size once, then assign the same Image to every
    // slot that needs it.
    let mut family = icns::IconFamily::new();
    let mut rendered: std::collections::HashMap<u32, icns::Image> =
        std::collections::HashMap::new();
    for (size, slot) in ICNS_SLOTS {
        let img = match rendered.get(size) {
            Some(img) => img.clone(),
            None => {
                let png_bytes = source.render_png(*size)?;
                let img = icns::Image::read_png(Cursor::new(png_bytes))
                    .with_context(|| format!("decode {size}px PNG for icns slot"))?;
                rendered.insert(*size, img.clone());
                img
            }
        };
        family
            .add_icon_with_type(&img, *slot)
            .with_context(|| format!("add {size}px slot to icns family"))?;
    }

    let icns_path = out_dir.join("AppIcon.icns");
    let file = fs::File::create(&icns_path)
        .with_context(|| format!("create {}", icns_path.display()))?;
    family
        .write(file)
        .with_context(|| format!("write {}", icns_path.display()))?;

    let outputs = MacosOutputs { icns: icns_path };
    cache::write(&cache_path, icon, &outputs)?;
    Ok(Some(outputs))
}

// ---------------------------------------------------------------------------
// Shared rendering helpers
// ---------------------------------------------------------------------------

fn write_png(source: &render::Source, size: u32, out: &Path) -> Result<()> {
    let png = source.render_png(size)?;
    fs::write(out, png).with_context(|| format!("write {}", out.display()))
}

fn write_ico_bundle(source: &render::Source, sizes: &[u32], out: &Path) -> Result<()> {
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in sizes {
        // Round-trip through PNG is the simplest correct path: our
        // pixmap stores premultiplied alpha, ico wants straight
        // alpha, and the cost at favicon sizes is negligible.
        let png = source.render_png(size)?;
        let img = ico::IconImage::read_png(Cursor::new(png))
            .with_context(|| format!("re-decode {size}px PNG into ico entry"))?;
        let entry = ico::IconDirEntry::encode(&img)
            .with_context(|| format!("encode {size}px ico entry"))?;
        icon_dir.add_entry(entry);
    }
    let file =
        fs::File::create(out).with_context(|| format!("create {}", out.display()))?;
    icon_dir
        .write(file)
        .with_context(|| format!("write {}", out.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SVG: &str = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
  <circle cx="32" cy="32" r="28" fill="#1e90ff"/>
</svg>"##;

    fn write_sample(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    fn block_with_source(path: PathBuf) -> IconBlock {
        IconBlock {
            source: Some(path),
            foreground: None,
            background: None,
            foreground_padding: None,
        }
    }

    #[test]
    fn sync_web_icons_returns_none_when_unconfigured() {
        let tmp = tempfile::tempdir().unwrap();
        let outputs = sync_web_icons(None, tmp.path()).unwrap();
        assert!(outputs.is_none());
        assert!(fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn sync_web_icons_from_svg_writes_full_set() {
        let tmp = tempfile::tempdir().unwrap();
        let svg = write_sample(tmp.path(), "icon.svg", SAMPLE_SVG);
        let out = tmp.path().join("out");

        let icon = block_with_source(svg);
        let outs = sync_web_icons(Some(&icon), &out).unwrap().unwrap();

        assert!(outs.favicon_ico.is_file());
        assert!(outs.favicon_192.is_file());
        assert!(outs.favicon_512.is_file());
        assert!(outs.apple_touch_icon.is_file());

        let img = image::open(&outs.favicon_512).unwrap();
        assert_eq!(img.width(), 512);
        assert_eq!(img.height(), 512);

        let bytes = fs::read(&outs.favicon_ico).unwrap();
        let dir = ico::IconDir::read(Cursor::new(&bytes)).unwrap();
        let widths: Vec<u32> = dir.entries().iter().map(|e| e.width()).collect();
        for size in FAVICON_ICO_SIZES {
            assert!(
                widths.contains(size),
                "favicon.ico missing {size}px entry (got {widths:?})"
            );
        }
    }

    #[test]
    fn sync_web_icons_from_png_resamples() {
        let tmp = tempfile::tempdir().unwrap();
        let mut img = image::RgbaImage::new(32, 32);
        for px in img.pixels_mut() {
            *px = image::Rgba([220, 20, 60, 255]);
        }
        let png_path = tmp.path().join("icon.png");
        img.save(&png_path).unwrap();
        let out = tmp.path().join("out");

        let icon = block_with_source(png_path);
        let outs = sync_web_icons(Some(&icon), &out).unwrap().unwrap();

        let upscaled = image::open(&outs.favicon_512).unwrap();
        assert_eq!(upscaled.width(), 512);
        assert_eq!(upscaled.height(), 512);
    }

    #[test]
    fn sync_web_icons_rejects_unknown_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = write_sample(tmp.path(), "icon.bmp", "not an image");
        let icon = block_with_source(bad);
        let err = sync_web_icons(Some(&icon), &tmp.path().join("out"))
            .err()
            .expect("bmp source should be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unsupported icon source extension"),
            "error should explain the rejection, got: {msg}"
        );
    }

    #[test]
    fn sync_ios_icons_writes_standard_set() {
        let tmp = tempfile::tempdir().unwrap();
        let svg = write_sample(tmp.path(), "icon.svg", SAMPLE_SVG);
        let out = tmp.path().join("ios");

        let icon = block_with_source(svg);
        let outs = sync_ios_icons(Some(&icon), &out).unwrap().unwrap();

        // Marketing icon is always 1024.
        assert!(outs.marketing_1024.is_file());
        let m = image::open(&outs.marketing_1024).unwrap();
        assert_eq!((m.width(), m.height()), (1024, 1024));

        // Spot-check the 60pt @3x = 180px iPhone home-screen icon —
        // that's the most user-visible asset.
        let main = outs
            .entries
            .iter()
            .find(|e| e.plist_stem == "AppIcon60x60" && e.size_px == 180)
            .expect("AppIcon60x60@3x must be written");
        assert!(main.file_name.ends_with("@3x.png"));
        let img = image::open(&main.path).unwrap();
        assert_eq!((img.width(), img.height()), (180, 180));

        // 9 entries from the table.
        assert_eq!(outs.entries.len(), IOS_ICON_ENTRIES.len());
    }

    #[test]
    fn composite_respects_foreground_padding() {
        // A foreground that's a single fully-opaque pixel-sized
        // rect would be hard to see; instead use the standard SVG
        // (which the circle path fills). Padding 0.25 must leave a
        // ring of pure background color around the foreground —
        // sample the corner pixels (background only) vs the center
        // (foreground + background blend, but center pixel of a
        // centered circle is on the foreground).
        let tmp = tempfile::tempdir().unwrap();
        let fg = write_sample(tmp.path(), "fg.svg", SAMPLE_SVG);
        let out_dir = tmp.path().join("out");
        fs::create_dir_all(&out_dir).unwrap();

        let icon = IconBlock {
            source: None,
            foreground: Some(fg),
            // Solid red so corners are unambiguously "background"
            // pixels with no anti-alias smear.
            background: Some(crate::Background::Color("#ff0000".to_string())),
            foreground_padding: Some(0.25),
        };
        // Drive render directly via sync_ios_icons — uses the same
        // composite path that runs at every output size.
        let outs = sync_ios_icons(Some(&icon), &out_dir).unwrap().unwrap();
        let img = image::open(&outs.marketing_1024).unwrap().to_rgba8();
        // Corner pixel must be pure red (background, no foreground
        // there because of the padding ring).
        let corner = img.get_pixel(0, 0);
        assert_eq!(
            corner.0[0], 255,
            "corner R must be 255 (red background), got {corner:?}",
        );
        assert_eq!(corner.0[1], 0, "corner G must be 0, got {corner:?}");
        assert_eq!(corner.0[2], 0, "corner B must be 0, got {corner:?}");
        // Also sample a pixel that should be foreground (the SVG
        // circle is centered at 32/64 with radius 28; at 25%
        // padding the foreground fills the central 50% of a 1024
        // canvas, so center pixel hits the blue circle).
        let center = img.get_pixel(512, 512);
        assert_ne!(
            center.0[0], 255,
            "center pixel should be foreground (blue circle), not background red — got {center:?}",
        );
    }

    #[test]
    fn cache_hit_skips_work_when_inputs_unchanged() {
        // Second call against the same input set must NOT rewrite
        // the output files. We detect "rewrite" by capturing the
        // mtime of one file before the second call and verifying
        // it's identical after.
        let tmp = tempfile::tempdir().unwrap();
        let svg = write_sample(tmp.path(), "icon.svg", SAMPLE_SVG);
        let out = tmp.path().join("out");
        let icon = block_with_source(svg);

        let first = sync_web_icons(Some(&icon), &out).unwrap().unwrap();
        let mtime_before = fs::metadata(&first.favicon_ico).unwrap().modified().unwrap();
        // Sleep past the filesystem mtime granularity so a rewrite
        // would actually shift the timestamp. macOS HFS+ is 1s.
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let second = sync_web_icons(Some(&icon), &out).unwrap().unwrap();
        let mtime_after = fs::metadata(&second.favicon_ico).unwrap().modified().unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "cache hit must skip re-rasterization (mtime drifted)",
        );
    }

    #[test]
    fn cache_invalidates_when_source_bytes_change() {
        let tmp = tempfile::tempdir().unwrap();
        let svg_path = write_sample(tmp.path(), "icon.svg", SAMPLE_SVG);
        let out = tmp.path().join("out");
        let icon = block_with_source(svg_path.clone());

        sync_web_icons(Some(&icon), &out).unwrap().unwrap();
        let before_bytes = fs::read(&out.join("favicon-512.png")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));

        // Rewrite the SVG with a different fill color. Same path —
        // content hash should still bust the cache.
        let changed = SAMPLE_SVG.replace("#1e90ff", "#ff0000");
        fs::write(&svg_path, changed).unwrap();
        sync_web_icons(Some(&icon), &out).unwrap().unwrap();

        let after_bytes = fs::read(&out.join("favicon-512.png")).unwrap();
        assert_ne!(
            before_bytes, after_bytes,
            "changing source bytes must invalidate cache → fresh PNG",
        );
    }

    #[test]
    fn cache_invalidates_when_padding_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let fg = write_sample(tmp.path(), "fg.svg", SAMPLE_SVG);
        let out = tmp.path().join("out");

        let icon_a = IconBlock {
            source: None,
            foreground: Some(fg.clone()),
            background: Some(crate::Background::Color("#000000".to_string())),
            foreground_padding: Some(0.1),
        };
        sync_ios_icons(Some(&icon_a), &out).unwrap().unwrap();
        let before = fs::read(out.join("AppIcon-1024.png")).unwrap();

        let icon_b = IconBlock {
            foreground_padding: Some(0.3),
            ..icon_a
        };
        sync_ios_icons(Some(&icon_b), &out).unwrap().unwrap();
        let after = fs::read(out.join("AppIcon-1024.png")).unwrap();

        assert_ne!(
            before, after,
            "changing foreground_padding must invalidate cache",
        );
    }

    #[test]
    fn cache_invalidates_when_an_output_is_deleted() {
        // Defense against partial-cleanup poisoning: if any output
        // listed in the cache no longer exists, regen everything.
        let tmp = tempfile::tempdir().unwrap();
        let svg = write_sample(tmp.path(), "icon.svg", SAMPLE_SVG);
        let out = tmp.path().join("out");
        let icon = block_with_source(svg);

        sync_web_icons(Some(&icon), &out).unwrap().unwrap();
        fs::remove_file(out.join("favicon-192.png")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let outs = sync_web_icons(Some(&icon), &out).unwrap().unwrap();
        assert!(
            outs.favicon_192.is_file(),
            "deleted output must be re-rendered on next sync",
        );
    }

    #[test]
    fn sync_android_icons_emits_adaptive_when_foreground_and_background_set() {
        let tmp = tempfile::tempdir().unwrap();
        let fg = write_sample(tmp.path(), "fg.svg", SAMPLE_SVG);
        let out = tmp.path().join("android");

        let icon = IconBlock {
            source: None,
            foreground: Some(fg),
            background: Some(crate::Background::Color("#00ff00".to_string())),
            foreground_padding: None,
        };
        let outs = sync_android_icons(Some(&icon), &out).unwrap().unwrap();

        // Flat + adaptive both produced.
        assert_eq!(outs.launcher_pngs.len(), ANDROID_DPI_SIZES.len());
        let adaptive = outs.adaptive.as_ref().expect("adaptive set");
        assert_eq!(adaptive.foreground_pngs.len(), ANDROID_ADAPTIVE_SIZES.len());
        assert_eq!(adaptive.background_pngs.len(), ANDROID_ADAPTIVE_SIZES.len());

        // XML lives in mipmap-anydpi-v26/ with both square + round
        // variants referencing the same drawables.
        let xml = fs::read_to_string(out.join("mipmap-anydpi-v26/ic_launcher.xml")).unwrap();
        assert!(xml.contains("@mipmap/ic_launcher_foreground"));
        assert!(xml.contains("@mipmap/ic_launcher_background"));
        assert!(out.join("mipmap-anydpi-v26/ic_launcher_round.xml").is_file());

        // Background layer at mdpi is 108×108 pure green (no
        // foreground composited in — the system masks foreground
        // over it at runtime).
        let bg_mdpi = image::open(out.join("mipmap-mdpi/ic_launcher_background.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!((bg_mdpi.width(), bg_mdpi.height()), (108, 108));
        let corner = bg_mdpi.get_pixel(0, 0);
        assert_eq!(corner.0, [0, 255, 0, 255]);
        let center = bg_mdpi.get_pixel(54, 54);
        assert_eq!(
            center.0, [0, 255, 0, 255],
            "background layer must be the fill ONLY (no foreground composited)",
        );

        // Foreground layer at mdpi is 108×108 with transparent
        // padding (~21px on each side) — corner pixel must be
        // fully transparent.
        let fg_mdpi = image::open(out.join("mipmap-mdpi/ic_launcher_foreground.png"))
            .unwrap()
            .to_rgba8();
        let fg_corner = fg_mdpi.get_pixel(0, 0);
        assert_eq!(
            fg_corner.0[3], 0,
            "foreground corner must be transparent (safe-zone padding), got {fg_corner:?}",
        );
    }

    #[test]
    fn sync_android_icons_no_adaptive_when_source_only() {
        // The legacy path: only `source` is set, no foreground +
        // background pair. Adaptive emission should be skipped
        // entirely — flat PNGs only.
        let tmp = tempfile::tempdir().unwrap();
        let svg = write_sample(tmp.path(), "icon.svg", SAMPLE_SVG);
        let out = tmp.path().join("android");
        let icon = block_with_source(svg);

        let outs = sync_android_icons(Some(&icon), &out).unwrap().unwrap();
        assert!(outs.adaptive.is_none());
        assert!(!out.join("mipmap-anydpi-v26").exists());
    }

    #[test]
    fn sync_android_icons_emits_every_dpi_bucket() {
        let tmp = tempfile::tempdir().unwrap();
        let svg = write_sample(tmp.path(), "icon.svg", SAMPLE_SVG);
        let out = tmp.path().join("android");

        let icon = block_with_source(svg);
        let outs = sync_android_icons(Some(&icon), &out).unwrap().unwrap();

        assert_eq!(outs.launcher_pngs.len(), ANDROID_DPI_SIZES.len());
        assert_eq!(outs.round_pngs.len(), ANDROID_DPI_SIZES.len());

        // mdpi (48) and xxxhdpi (192) bracket the density range.
        let mdpi = image::open(out.join("mipmap-mdpi/ic_launcher.png")).unwrap();
        assert_eq!((mdpi.width(), mdpi.height()), (48, 48));
        let xxxhdpi = image::open(out.join("mipmap-xxxhdpi/ic_launcher.png")).unwrap();
        assert_eq!((xxxhdpi.width(), xxxhdpi.height()), (192, 192));

        // The round variant ships at every dpi too.
        assert!(out.join("mipmap-hdpi/ic_launcher_round.png").is_file());
    }
}
