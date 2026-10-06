//! QR codes on every target.
//!
//! - **Scanning** ([`scan`], feature `scan`): read codes out of any live
//!   `MediaStream` — a camera, a screen share — or a still image. The scanner
//!   consumes a stream the app already has; it never opens its own camera.
//! - **Generating** ([`generate`], feature `generate`): encode data into a
//!   [`QrMatrix`] and write it as a standalone SVG with [`QrMatrix::to_svg`].
//!   Pure Rust with no UI dependency, so it also runs server-side.
//! - **Showing** (feature `component`): the [`QrCode`] component draws a code
//!   as crisp vector rectangles in a `canvas`, identically on every renderer.
//!
//! All three are on by default; turn off what you don't use
//! (`default-features = false, features = ["component"]` for an app that only
//! shows codes skips the decoder and `offload`).
//!
//! ```ignore
//! use qr::{QrCode, QrScanner, ScanConfig};
//!
//! // Show one:
//! ui! { QrCode(data = "https://example.com".to_string(), size = Some(160.0)) }
//!
//! // Read one from the camera stream the app is already showing:
//! let scanner = QrScanner::new(&stream, ScanConfig::default());
//! let scan = scanner.next().await?;
//! ```

#![deny(missing_docs)]

#[cfg(feature = "scan")]
pub mod scan;
#[cfg(feature = "scan")]
pub use scan::{
    decode_luma8, decode_rgba8, Point, QrScanner, Scan, ScanConfig, ScanError, ScannedCode,
    DEFAULT_MAX_DIMENSION,
};

#[cfg(feature = "generate")]
pub mod generate;
#[cfg(feature = "generate")]
pub use generate::{ErrorCorrection, GenerateError, QrMatrix, SvgOptions, DEFAULT_QUIET_ZONE};

#[cfg(feature = "component")]
mod component;
#[cfg(feature = "component")]
pub use component::{QrCode, QrCodeProps};
