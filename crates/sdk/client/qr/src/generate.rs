//! Generating QR codes: [`QrMatrix::encode`] turns data into the module grid,
//! [`QrMatrix::to_svg`] writes it as a standalone SVG file, and (with the
//! `component` feature) the [`QrCode`](crate::QrCode) component draws it on
//! screen.
//!
//! Everything here is plain data and pure Rust — no UI, no platform code — so
//! it also runs in server code (a code in an email, a downloadable SVG).
//!
//! ```
//! use qr::{ErrorCorrection, QrMatrix, SvgOptions};
//!
//! let matrix = QrMatrix::encode("https://example.com", ErrorCorrection::Medium).unwrap();
//! assert_eq!(matrix.width(), 25); // version 2: 17 + 4 × 2 modules per side
//! let svg = matrix.to_svg(&SvgOptions::default());
//! assert!(svg.starts_with("<svg"));
//! ```

use std::fmt::Write as _;

/// How much of the code can be damaged or covered and still read. Higher
/// levels make a denser (larger-version) code for the same data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ErrorCorrection {
    /// Recovers about 7 % damage — the smallest code.
    Low,
    /// Recovers about 15 % — the usual choice for screens.
    #[default]
    Medium,
    /// Recovers about 25 %.
    Quartile,
    /// Recovers about 30 % — for print that may get scuffed, or a logo laid
    /// over the middle of the code.
    High,
}

impl ErrorCorrection {
    fn level(self) -> qrcode::EcLevel {
        match self {
            ErrorCorrection::Low => qrcode::EcLevel::L,
            ErrorCorrection::Medium => qrcode::EcLevel::M,
            ErrorCorrection::Quartile => qrcode::EcLevel::Q,
            ErrorCorrection::High => qrcode::EcLevel::H,
        }
    }
}

/// Why data couldn't be encoded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GenerateError {
    /// More data than the largest QR code (version 40) holds at this error
    /// correction level — about 2.3 KB of bytes at `Medium`.
    #[error("too much data for a QR code at this error correction level")]
    DataTooLong,
    /// The encoder rejected the input for another reason.
    #[error("the data could not be encoded as a QR code: {0}")]
    Unencodable(String),
}

/// The quiet zone the QR specification requires around a code, in modules.
/// Scanners use it to find the code's edges; a smaller one may not read.
pub const DEFAULT_QUIET_ZONE: usize = 4;

/// An encoded QR code: a square grid of dark and light modules, without the
/// quiet zone (renderers add it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrMatrix {
    width: usize,
    dark: Vec<bool>,
}

impl QrMatrix {
    /// Encode `data` (text or raw bytes) as a QR code. The encoder picks the
    /// smallest version and the most compact mode (numeric, alphanumeric,
    /// bytes) that fit.
    pub fn encode(data: impl AsRef<[u8]>, error_correction: ErrorCorrection) -> Result<QrMatrix, GenerateError> {
        let code = qrcode::QrCode::with_error_correction_level(data.as_ref(), error_correction.level())
            .map_err(|e| match e {
                qrcode::types::QrError::DataTooLong => GenerateError::DataTooLong,
                other => GenerateError::Unencodable(other.to_string()),
            })?;
        Ok(QrMatrix {
            width: code.width(),
            dark: code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect(),
        })
    }

    /// Modules per side (21 for version 1, up to 177 for version 40).
    pub fn width(&self) -> usize {
        self.width
    }

    /// Whether the module at column `x`, row `y` is dark. Out-of-range
    /// coordinates are light (they're quiet zone).
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.width && self.dark[y * self.width + x]
    }

    /// The dark modules as horizontal runs `(x, y, len)`: each row's
    /// consecutive dark modules merged into one rectangle. This is the shape
    /// both renderers draw — far fewer rectangles than one per module, and
    /// no hairline seams between neighbours in a row.
    pub fn dark_runs(&self) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        (0..self.width).flat_map(move |y| {
            let row = &self.dark[y * self.width..(y + 1) * self.width];
            let mut runs = Vec::new();
            let mut x = 0;
            while x < row.len() {
                if row[x] {
                    let start = x;
                    while x < row.len() && row[x] {
                        x += 1;
                    }
                    runs.push((start, y, x - start));
                } else {
                    x += 1;
                }
            }
            runs
        })
    }

    /// The code as a standalone SVG document — for saving, sharing, printing
    /// or `file-export`. One `<path>` of horizontal runs on an optional
    /// background rect, in a `viewBox` of whole modules (quiet zone included),
    /// with `shape-rendering="crispEdges"` so modules stay sharp at any scale.
    pub fn to_svg(&self, options: &SvgOptions) -> String {
        let q = options.quiet_zone;
        let side = self.width + 2 * q;
        let mut svg = String::with_capacity(64 + self.width * self.width * 4);
        let _ = write!(svg, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {side} {side}""#);
        if let Some(px) = options.size_px {
            let _ = write!(svg, r#" width="{px}" height="{px}""#);
        }
        svg.push_str(r#" shape-rendering="crispEdges">"#);
        if let Some(light) = &options.light {
            let _ = write!(svg, r#"<rect width="{side}" height="{side}" fill="{}"/>"#, attr(light));
        }
        let _ = write!(svg, r#"<path fill="{}" d=""#, attr(&options.dark));
        for (x, y, len) in self.dark_runs() {
            let _ = write!(svg, "M{} {}h{len}v1h-{len}z", x + q, y + q);
        }
        svg.push_str(r#""/></svg>"#);
        svg
    }
}

/// Options for [`QrMatrix::to_svg`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvgOptions {
    /// Light border around the code, in modules. Default
    /// [`DEFAULT_QUIET_ZONE`] (the specification's minimum).
    pub quiet_zone: usize,
    /// Fill for the dark modules — any SVG color (`#000`, `rgb(…)`, a name).
    pub dark: String,
    /// Background fill, or `None` for a transparent background. A code needs
    /// contrast behind it to scan, so keep one unless the code sits on a light
    /// surface anyway.
    pub light: Option<String>,
    /// Write explicit `width`/`height` attributes, in px. `None` leaves the
    /// SVG sized by whatever embeds it (the `viewBox` keeps it square).
    pub size_px: Option<u32>,
}

impl Default for SvgOptions {
    fn default() -> Self {
        SvgOptions {
            quiet_zone: DEFAULT_QUIET_ZONE,
            dark: "#000".to_string(),
            light: Some("#fff".to_string()),
            size_px: None,
        }
    }
}

/// Escape a value for a double-quoted XML attribute.
fn attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_picks_the_smallest_version_that_fits() {
        let small = QrMatrix::encode("hi", ErrorCorrection::Low).unwrap();
        assert_eq!(small.width(), 21);
        let bigger = QrMatrix::encode("x".repeat(200), ErrorCorrection::Medium).unwrap();
        assert!(bigger.width() > 21 && (bigger.width() - 17).is_multiple_of(4));
    }

    #[test]
    fn too_much_data_is_an_error_not_a_panic() {
        let err = QrMatrix::encode(vec![0u8; 4000], ErrorCorrection::High).unwrap_err();
        assert_eq!(err, GenerateError::DataTooLong);
    }

    /// The runs must cover exactly the dark modules — nothing dropped,
    /// nothing extra — or both renderers would draw a different code.
    #[test]
    fn dark_runs_cover_exactly_the_dark_modules() {
        let m = QrMatrix::encode("runs cover the grid", ErrorCorrection::Quartile).unwrap();
        let mut covered = vec![false; m.width() * m.width()];
        for (x, y, len) in m.dark_runs() {
            assert!(len > 0);
            for i in x..x + len {
                assert!(!covered[y * m.width() + i], "runs overlap");
                covered[y * m.width() + i] = true;
            }
            // Runs are maximal: the modules either side are light.
            assert!(!m.is_dark(x.wrapping_sub(1), y) || x == 0);
            assert!(!m.is_dark(x + len, y));
        }
        for y in 0..m.width() {
            for x in 0..m.width() {
                assert_eq!(covered[y * m.width() + x], m.is_dark(x, y), "({x},{y})");
            }
        }
    }

    #[test]
    fn svg_has_the_quiet_zone_in_its_viewbox_and_escapes_colors() {
        let m = QrMatrix::encode("svg", ErrorCorrection::Medium).unwrap();
        let svg = m.to_svg(&SvgOptions {
            quiet_zone: 2,
            dark: r#"x"><script>"#.to_string(),
            light: None,
            size_px: Some(120),
        });
        let side = m.width() + 4;
        assert!(svg.contains(&format!(r#"viewBox="0 0 {side} {side}""#)));
        assert!(svg.contains(r#"width="120" height="120""#));
        assert!(!svg.contains("<rect"), "no background when light is None");
        assert!(!svg.contains("<script>"), "colors are attribute-escaped: {svg}");
        assert!(svg.ends_with("</svg>"));
    }
}
