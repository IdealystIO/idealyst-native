//! Runtime font registration into the GTK/Pango font map.
//!
//! Native `face!` assets arrive as embedded TTF bytes
//! ([`AssetSource::Embedded`] / [`AssetSource::BundledEmbedded`]). GTK
//! resolves fonts through Pango's font map (fontconfig-backed on
//! Linux), which loads from files, so we spill the embedded bytes to a
//! temp file and hand the path to `PangoFontMap::add_font_file`. Once
//! added, a `GtkLabel` requesting `family: "Inter"` resolves to the
//! embedded face — no system install needed.
//!
//! The temp file must outlive the process's use of the font (Pango
//! memory-maps it lazily), so the backend retains the returned path for
//! its lifetime.

use std::io::Write;
use std::path::PathBuf;

use gtk4::pango;
use gtk4::pango::prelude::*;
use runtime_core::assets::AssetSource;

/// Extract embedded font bytes + extension from an [`AssetSource`], if
/// this source carries bytes (native `face!` emits `Embedded` or
/// `BundledEmbedded`; `Bundled`/`Remote` are web/path-only and have no
/// bytes for us to load).
pub fn embedded_bytes(source: &AssetSource) -> Option<(&'static [u8], &'static str)> {
    match source {
        AssetSource::Embedded { bytes, extension } => Some((bytes, extension)),
        AssetSource::BundledEmbedded {
            bytes, extension, ..
        } => Some((bytes, extension)),
        AssetSource::Bundled { .. } | AssetSource::Remote { .. } => None,
    }
}

/// Write `bytes` to a per-process temp file and register it with
/// `font_map`. Returns the temp path (retain it — Pango reads the file
/// lazily). `key` uniquifies the filename so two faces don't collide.
pub fn add_font(
    font_map: &pango::FontMap,
    key: u64,
    bytes: &[u8],
    extension: &str,
) -> Result<PathBuf, String> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "idealyst-font-{}-{}.{}",
        std::process::id(),
        key,
        extension
    ));
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    font_map
        .add_font_file(&path)
        .map_err(|e| e.to_string())?;
    Ok(path)
}
