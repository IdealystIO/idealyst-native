//! Native launch-screen assets.
//!
//! One [`SplashSpec`] (a fill color plus an optional centered logo, for
//! light and dark appearances) becomes:
//!
//! - **iOS**: a `LaunchBackground` colorset and a `LaunchLogo` imageset in
//!   an `Assets.xcassets`, referenced from `UILaunchScreen`
//!   (`UIColorName` / `UIImageName`). Both carry a dark appearance.
//! - **Android**: a `Theme.Idealyst.Splash` style for the launching
//!   Activity. API 31+ draws `windowSplashScreenBackground` +
//!   `windowSplashScreenAnimatedIcon`; older releases draw the theme's
//!   `windowBackground` (a layer-list of the fill and the logo). Dark mode
//!   comes from `-night` resource qualifiers.
//!
//! The logo is drawn at [`SPLASH_LOGO_PT`] points/dp on every platform so
//! the launch screens match.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::render;

/// One appearance: `"#RRGGBB"` fill plus an optional logo source.
#[derive(Debug, Clone, Copy)]
pub struct SplashLook<'a> {
    pub background: &'a str,
    pub logo: Option<&'a Path>,
}

#[derive(Debug, Clone, Copy)]
pub struct SplashSpec<'a> {
    pub light: SplashLook<'a>,
    pub dark: SplashLook<'a>,
}

impl SplashSpec<'_> {
    fn has_logo(&self) -> bool {
        self.light.logo.is_some() || self.dark.logo.is_some()
    }
}

/// Logo box edge in points (iOS) / dp (Android). Android 12+ masks the
/// splash icon (a 288dp canvas when it has no icon background) to a
/// 192dp circle; the largest square inside that circle is
/// 192/√2 ≈ 135.8dp, so a 135dp logo is never clipped. iOS and pre-12
/// Android use the same box so all launch screens match.
pub const SPLASH_LOGO_PT: u32 = 135;
/// Android 12+ splash icon canvas (no icon background), in dp.
const ANDROID_SPLASH_ICON_CANVAS_DP: u32 = 288;

/// Asset-catalog names `UILaunchScreen` references.
pub const IOS_SPLASH_COLOR_NAME: &str = "LaunchBackground";
pub const IOS_SPLASH_IMAGE_NAME: &str = "LaunchLogo";

/// The Activity theme carrying the launch screen. The generated Activity
/// switches to its normal theme in `onCreate`.
pub const ANDROID_SPLASH_THEME: &str = "@style/Theme.Idealyst.Splash";

const IOS_SCALES: &[u32] = &[1, 2, 3];
/// Android density buckets as (qualifier, scale × 2), so hdpi's 1.5× stays
/// integral.
const ANDROID_DENSITIES: &[(&str, u32)] = &[
    ("mdpi", 2),
    ("hdpi", 3),
    ("xhdpi", 4),
    ("xxhdpi", 6),
    ("xxxhdpi", 8),
];

/// What [`sync_ios_splash_catalog`] wrote, for the `UILaunchScreen` dict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IosSplashAssets {
    pub color_name: &'static str,
    pub image_name: Option<&'static str>,
}

/// Write the launch-screen colorset (and imageset, when a logo is set)
/// into `xcassets` (created if missing). `actool` compiles them into
/// `Assets.car`; the Info.plist's `UILaunchScreen` names them.
pub fn sync_ios_splash_catalog(spec: &SplashSpec<'_>, xcassets: &Path) -> Result<IosSplashAssets> {
    write_catalog_root(xcassets)?;

    let colorset = xcassets.join(format!("{IOS_SPLASH_COLOR_NAME}.colorset"));
    fs::create_dir_all(&colorset).with_context(|| format!("create {}", colorset.display()))?;
    let contents = serde_json::json!({
        "colors": [
            { "idiom": "universal", "color": ios_color(spec.light.background)? },
            {
                "idiom": "universal",
                "appearances": [{ "appearance": "luminosity", "value": "dark" }],
                "color": ios_color(spec.dark.background)?,
            },
        ],
        "info": { "author": "idealyst", "version": 1 },
    });
    write_json(&colorset.join("Contents.json"), &contents)?;

    let imageset = xcassets.join(format!("{IOS_SPLASH_IMAGE_NAME}.imageset"));
    if !spec.has_logo() {
        // A stale imageset from an earlier build would still compile in.
        let _ = fs::remove_dir_all(&imageset);
        return Ok(IosSplashAssets { color_name: IOS_SPLASH_COLOR_NAME, image_name: None });
    }
    fs::create_dir_all(&imageset).with_context(|| format!("create {}", imageset.display()))?;
    let mut images = Vec::new();
    for (suffix, look, appearance) in [("", spec.light, None), ("-dark", spec.dark, Some("dark"))] {
        for &scale in IOS_SCALES {
            let filename = format!("{IOS_SPLASH_IMAGE_NAME}{suffix}@{scale}x.png");
            write_logo(look.logo, SPLASH_LOGO_PT * scale, &imageset.join(&filename))?;
            let mut entry = serde_json::json!({
                "idiom": "universal",
                "filename": filename,
                "scale": format!("{scale}x"),
            });
            if let Some(value) = appearance {
                entry["appearances"] =
                    serde_json::json!([{ "appearance": "luminosity", "value": value }]);
            }
            images.push(entry);
        }
    }
    write_json(
        &imageset.join("Contents.json"),
        &serde_json::json!({ "images": images, "info": { "author": "idealyst", "version": 1 } }),
    )?;
    Ok(IosSplashAssets {
        color_name: IOS_SPLASH_COLOR_NAME,
        image_name: Some(IOS_SPLASH_IMAGE_NAME),
    })
}

/// Write the launch-screen resources (theme, colors, drawables) into the
/// Android `res_dir`. The caller compiles it with `aapt2` and sets the
/// main Activity's `android:theme` to [`ANDROID_SPLASH_THEME`].
pub fn sync_android_splash_res(spec: &SplashSpec<'_>, res_dir: &Path) -> Result<()> {
    // Rebuilt from scratch so a removed logo or dark override leaves no
    // stale drawable behind.
    let _ = fs::remove_dir_all(res_dir);

    write_res(
        res_dir,
        "values",
        "idealyst_splash.xml",
        &format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="idealyst_splash_background">{light}</color>
    <style name="Theme.Idealyst.Splash" parent="@android:style/Theme.DeviceDefault.Light.NoActionBar">
        <item name="android:windowBackground">@drawable/idealyst_splash_window</item>
    </style>
</resources>
"#,
            light = spec.light.background,
        ),
    )?;
    write_res(
        res_dir,
        "values-night",
        "idealyst_splash.xml",
        &format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="idealyst_splash_background">{dark}</color>
</resources>
"#,
            dark = spec.dark.background,
        ),
    )?;
    // API 31+ ignores `windowBackground` for the launch screen and draws
    // the SplashScreen attributes instead. Unset, `windowSplashScreen
    // AnimatedIcon` falls back to the launcher icon, which iOS would not
    // show, so the no-logo case sets a transparent drawable.
    write_res(
        res_dir,
        "values-v31",
        "idealyst_splash.xml",
        r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
    <style name="Theme.Idealyst.Splash" parent="@android:style/Theme.DeviceDefault.Light.NoActionBar">
        <item name="android:windowBackground">@drawable/idealyst_splash_window</item>
        <item name="android:windowSplashScreenBackground">@color/idealyst_splash_background</item>
        <item name="android:windowSplashScreenAnimatedIcon">@drawable/idealyst_splash_icon</item>
    </style>
</resources>
"#,
    )?;

    if !spec.has_logo() {
        write_res(
            res_dir,
            "drawable",
            "idealyst_splash_window.xml",
            r#"<?xml version="1.0" encoding="utf-8"?>
<layer-list xmlns:android="http://schemas.android.com/apk/res/android">
    <item android:drawable="@color/idealyst_splash_background"/>
</layer-list>
"#,
        )?;
        write_res(
            res_dir,
            "drawable",
            "idealyst_splash_icon.xml",
            r#"<?xml version="1.0" encoding="utf-8"?>
<color xmlns:android="http://schemas.android.com/apk/res/android" android:color="@android:color/transparent"/>
"#,
        )?;
        return Ok(());
    }

    // Pre-31: the logo bitmap draws at its intrinsic size, which the
    // per-density PNGs make SPLASH_LOGO_PT dp.
    write_res(
        res_dir,
        "drawable",
        "idealyst_splash_window.xml",
        r#"<?xml version="1.0" encoding="utf-8"?>
<layer-list xmlns:android="http://schemas.android.com/apk/res/android">
    <item android:drawable="@color/idealyst_splash_background"/>
    <item>
        <bitmap android:gravity="center" android:src="@drawable/idealyst_splash_logo"/>
    </item>
</layer-list>
"#,
    )?;
    // API 31+: the system resizes the icon drawable to its own icon box
    // (the 288dp canvas), so the logo's margin is a FRACTION of that box,
    // not a dp inset: a fixed `76.5dp` inset left a ~17dp logo on an API
    // 34 Pixel. The fraction keeps the logo at SPLASH_LOGO_PT/288 of the
    // canvas, inside the circular mask.
    let inset_pct = (ANDROID_SPLASH_ICON_CANVAS_DP - SPLASH_LOGO_PT) as f32 / 2.0
        / ANDROID_SPLASH_ICON_CANVAS_DP as f32
        * 100.0;
    write_res(
        res_dir,
        "drawable",
        "idealyst_splash_icon.xml",
        &format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<inset xmlns:android="http://schemas.android.com/apk/res/android"
    android:drawable="@drawable/idealyst_splash_logo"
    android:inset="{inset_pct}%"/>
"#
        ),
    )?;

    let dark_differs = spec.dark.logo != spec.light.logo;
    for &(density, scale_x2) in ANDROID_DENSITIES {
        let px = (SPLASH_LOGO_PT * scale_x2).div_ceil(2);
        let dir = res_dir.join(format!("drawable-{density}"));
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        write_logo(spec.light.logo, px, &dir.join("idealyst_splash_logo.png"))?;
        if dark_differs {
            let dir = res_dir.join(format!("drawable-night-{density}"));
            fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
            write_logo(spec.dark.logo, px, &dir.join("idealyst_splash_logo.png"))?;
        }
    }
    Ok(())
}

/// The logo fitted into a `px` square, or a transparent square when this
/// appearance has none (the other appearance does).
fn write_logo(logo: Option<&Path>, px: u32, out: &Path) -> Result<()> {
    let png = match logo {
        Some(path) => render::render_contained_png(path, px)?,
        None => render::render_transparent_png(px)?,
    };
    fs::write(out, png).with_context(|| format!("write {}", out.display()))
}

/// `"#RRGGBB"` → an asset-catalog sRGB color object.
fn ios_color(hex: &str) -> Result<serde_json::Value> {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    anyhow::ensure!(
        digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()),
        "splash color {hex:?} must be \"#RRGGBB\""
    );
    Ok(serde_json::json!({
        "color-space": "srgb",
        "components": {
            "red": format!("0x{}", &digits[0..2]),
            "green": format!("0x{}", &digits[2..4]),
            "blue": format!("0x{}", &digits[4..6]),
            "alpha": "1.000",
        },
    }))
}

fn write_catalog_root(xcassets: &Path) -> Result<()> {
    fs::create_dir_all(xcassets).with_context(|| format!("create {}", xcassets.display()))?;
    write_json(
        &xcassets.join("Contents.json"),
        &serde_json::json!({ "info": { "author": "idealyst", "version": 1 } }),
    )
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("serialize asset catalog JSON")?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn write_res(res_dir: &Path, sub: &str, file: &str, body: &str) -> Result<()> {
    let dir = res_dir.join(sub);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    fs::write(dir.join(file), body).with_context(|| format!("write {}/{file}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 50" width="200" height="50"><rect width="200" height="50" fill="#ff0000"/></svg>"##;

    fn spec<'a>(logo: Option<&'a Path>, dark_logo: Option<&'a Path>) -> SplashSpec<'a> {
        SplashSpec {
            light: SplashLook { background: "#1A1A2E", logo },
            dark: SplashLook { background: "#000000", logo: dark_logo },
        }
    }

    fn png_size(path: &Path) -> (u32, u32) {
        let img = image::open(path).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn ios_catalog_carries_light_and_dark_color_and_logo() {
        let tmp = tempfile::tempdir().unwrap();
        let logo = tmp.path().join("logo.svg");
        fs::write(&logo, WIDE_SVG).unwrap();
        let xcassets = tmp.path().join("Assets.xcassets");

        let out = sync_ios_splash_catalog(&spec(Some(&logo), Some(&logo)), &xcassets).unwrap();
        assert_eq!(out.image_name, Some(IOS_SPLASH_IMAGE_NAME));

        let colors: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(xcassets.join("LaunchBackground.colorset/Contents.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(colors["colors"][0]["color"]["components"]["blue"], "0x2E");
        assert_eq!(colors["colors"][1]["appearances"][0]["value"], "dark");
        assert_eq!(colors["colors"][1]["color"]["components"]["red"], "0x00");

        let imageset = xcassets.join("LaunchLogo.imageset");
        assert_eq!(png_size(&imageset.join("LaunchLogo@3x.png")), (405, 405));
        assert!(imageset.join("LaunchLogo-dark@2x.png").is_file());
    }

    #[test]
    fn ios_catalog_without_logo_drops_a_stale_imageset() {
        let tmp = tempfile::tempdir().unwrap();
        let logo = tmp.path().join("logo.svg");
        fs::write(&logo, WIDE_SVG).unwrap();
        let xcassets = tmp.path().join("Assets.xcassets");
        sync_ios_splash_catalog(&spec(Some(&logo), Some(&logo)), &xcassets).unwrap();

        let out = sync_ios_splash_catalog(&spec(None, None), &xcassets).unwrap();
        assert_eq!(out.image_name, None);
        assert!(!xcassets.join("LaunchLogo.imageset").exists());
    }

    #[test]
    fn android_res_has_night_color_v31_theme_and_dense_logos() {
        let tmp = tempfile::tempdir().unwrap();
        let logo = tmp.path().join("logo.svg");
        fs::write(&logo, WIDE_SVG).unwrap();
        let res = tmp.path().join("res");

        sync_android_splash_res(&spec(Some(&logo), Some(&logo)), &res).unwrap();
        let values = fs::read_to_string(res.join("values/idealyst_splash.xml")).unwrap();
        assert!(values.contains(">#1A1A2E<"));
        let night = fs::read_to_string(res.join("values-night/idealyst_splash.xml")).unwrap();
        assert!(night.contains(">#000000<"));
        let v31 = fs::read_to_string(res.join("values-v31/idealyst_splash.xml")).unwrap();
        assert!(v31.contains("windowSplashScreenAnimatedIcon"));
        let icon = fs::read_to_string(res.join("drawable/idealyst_splash_icon.xml")).unwrap();
        assert!(icon.contains("android:inset=\"26.5625%\""), "{icon}");
        assert_eq!(png_size(&res.join("drawable-hdpi/idealyst_splash_logo.png")), (203, 203));
        assert_eq!(png_size(&res.join("drawable-xxxhdpi/idealyst_splash_logo.png")), (540, 540));
        // Same logo in both appearances → no night drawables.
        assert!(!res.join("drawable-night-mdpi").exists());
    }

    #[test]
    fn regression_android_no_logo_hides_the_launcher_icon_on_api_31() {
        // Unset, API 31+ draws the launcher icon on the splash; iOS shows
        // only the fill, so the icon must be explicitly transparent.
        let tmp = tempfile::tempdir().unwrap();
        let res = tmp.path().join("res");
        sync_android_splash_res(&spec(None, None), &res).unwrap();
        let icon = fs::read_to_string(res.join("drawable/idealyst_splash_icon.xml")).unwrap();
        assert!(icon.contains("@android:color/transparent"));
        assert!(!res.join("drawable-mdpi").exists());
    }

    #[test]
    fn wide_raster_logo_is_fitted_not_stretched() {
        let tmp = tempfile::tempdir().unwrap();
        let logo = tmp.path().join("wide.png");
        image::RgbaImage::from_pixel(200, 50, image::Rgba([255, 0, 0, 255]))
            .save(&logo)
            .unwrap();
        let out = tmp.path().join("out.png");
        write_logo(Some(&logo), 100, &out).unwrap();
        let img = image::open(&out).unwrap().to_rgba8();
        // Fitted 100×25, centered: rows above and below are transparent.
        assert_eq!(img.get_pixel(50, 10).0[3], 0);
        assert_eq!(img.get_pixel(50, 50).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(50, 90).0[3], 0);
    }
}
