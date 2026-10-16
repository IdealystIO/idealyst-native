//! Golden-file substrate for the backend parity suites.
//!
//! Each consumer renders a scene with the current implementation and
//! compares the observable output against a committed reference file:
//! byte-exact SSR/email HTML, cell-exact terminal grids, pixel-exact CPU
//! framebuffers (lossless RGBA8 PNG), byte-exact Roku command streams,
//! and wire-snapshot JSON.
//!
//! ```ignore
//! let goldens = Goldens::new(env!("CARGO_MANIFEST_DIR"));
//!
//! let out = render(scene());
//! goldens.check_text("kitchen_sink.html", &out);
//! ```
//!
//! # Contract
//!
//! `check_*` always compares. `freeze_*` writes **only** under
//! `IDEALYST_FREEZE_GOLDENS=1` and is a no-op otherwise, so a normal
//! `cargo test` run is a golden comparison and nothing else.
//!
//! The backend references are fixed: they were captured from an
//! independent reference renderer that no longer exists, so no backend
//! test calls `freeze_*` and they cannot be regenerated (dev-tui's panel
//! goldens are the one freezable corpus). Rewriting
//! one by hand (or by adding a freeze call) re-baselines it against the
//! current implementation's own output — review that diff as the
//! substance of the change, and never do it to turn a red test green.
//!
//! # Normalization
//!
//! Nothing here normalizes beyond one trailing newline on text files.
//! Normalization (sanctioned divergences, non-deterministic ids) is the
//! *consumer's* job and must be applied to BOTH the reference and the
//! actual before comparing — freeze the normalized form if the raw form
//! is unstable, and say so in the consumer's `tests/goldens/README.md`.

use std::path::{Path, PathBuf};

/// The env var that turns `freeze_*` from a no-op into a write.
pub const FREEZE_ENV: &str = "IDEALYST_FREEZE_GOLDENS";

/// True when this process was asked to (re)write frozen artifacts.
pub fn is_freezing() -> bool {
    std::env::var_os(FREEZE_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// A crate's frozen-artifact directory (`<manifest>/tests/goldens`).
#[derive(Clone, Debug)]
pub struct Goldens {
    dir: PathBuf,
}

impl Goldens {
    /// Anchor at `<manifest_dir>/tests/goldens`. Pass
    /// `env!("CARGO_MANIFEST_DIR")`.
    pub fn new(manifest_dir: &str) -> Self {
        Self {
            dir: Path::new(manifest_dir).join("tests").join("goldens"),
        }
    }

    /// Anchor at an arbitrary directory (for suites with more than one
    /// corpus, e.g. `tests/goldens/ssg`).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Nested sub-corpus under this one.
    pub fn sub(&self, name: &str) -> Self {
        Self {
            dir: self.dir.join(name),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn ensure_dir(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create golden dir {}: {e}", parent.display()));
        }
    }

    // -------------------------------------------------------------------
    // Freeze (writes only under IDEALYST_FREEZE_GOLDENS=1)
    // -------------------------------------------------------------------

    /// Freeze a text artifact (HTML, CSS, a grid dump, a JSON stream).
    ///
    /// A trailing newline is normalized in so the files are
    /// diff-friendly; [`check_text`](Self::check_text) applies the same
    /// normalization to the actual, so the comparison stays exact on
    /// everything else.
    pub fn freeze_text(&self, name: &str, text: &str) {
        if !is_freezing() {
            return;
        }
        let path = self.path(name);
        self.ensure_dir(&path);
        std::fs::write(&path, normalize_trailing_newline(text))
            .unwrap_or_else(|e| panic!("write golden {}: {e}", path.display()));
    }

    /// Freeze an opaque binary artifact.
    pub fn freeze_bytes(&self, name: &str, bytes: &[u8]) {
        if !is_freezing() {
            return;
        }
        let path = self.path(name);
        self.ensure_dir(&path);
        std::fs::write(&path, bytes)
            .unwrap_or_else(|e| panic!("write golden {}: {e}", path.display()));
    }

    /// Freeze a framebuffer as a lossless RGBA8 PNG (`name` should end
    /// in `.png`). Alpha is preserved, so the decode round trip is
    /// byte-exact against the source buffer.
    pub fn freeze_rgba(&self, name: &str, width: u32, height: u32, rgba: &[u8]) {
        if !is_freezing() {
            return;
        }
        assert_eq!(
            rgba.len(),
            (width as usize) * (height as usize) * 4,
            "golden {name}: RGBA buffer is not {width}x{height}x4"
        );
        let path = self.path(name);
        self.ensure_dir(&path);
        let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())
            .expect("RgbaImage::from_raw with a validated length");
        img.save_with_format(&path, image::ImageFormat::Png)
            .unwrap_or_else(|e| panic!("write PNG golden {}: {e}", path.display()));
        // Paranoia: prove the round trip is lossless at freeze time, so
        // a codec surprise can never be baked into the corpus.
        let back = read_rgba(&path);
        assert!(
            back == rgba,
            "golden {name}: PNG round trip was not byte-exact — refusing to \
             freeze a lossy artifact"
        );
    }

    // -------------------------------------------------------------------
    // Check (always compares)
    // -------------------------------------------------------------------

    /// Compare `actual` against the frozen text artifact.
    pub fn check_text(&self, name: &str, actual: &str) {
        let path = self.path(name);
        let expected = self.read_text(&path, name);
        let actual = normalize_trailing_newline(actual);
        if expected == actual {
            return;
        }
        let at = expected
            .bytes()
            .zip(actual.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| expected.len().min(actual.len()));
        let lo = at.saturating_sub(80);
        panic!(
            "{}\n\
             \n\
             First divergence at byte {at}.\n\
             frozen (len {}): …{}…\n\
             actual (len {}): …{}…\n\
             \n--- frozen ({}) ---\n{expected}\n--- actual ---\n{actual}",
            mismatch_preamble(name, &path),
            expected.len(),
            &expected[lo..(at + 80).min(expected.len())],
            actual.len(),
            &actual[lo..(at + 80).min(actual.len())],
            path.display(),
        );
    }

    /// Compare `actual` against the frozen binary artifact.
    pub fn check_bytes(&self, name: &str, actual: &[u8]) {
        let path = self.path(name);
        let expected = std::fs::read(&path).unwrap_or_else(|_| missing(name, &path));
        if expected == actual {
            return;
        }
        let at = expected
            .iter()
            .zip(actual.iter())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| expected.len().min(actual.len()));
        panic!(
            "{}\n\nFirst divergent byte at offset {at} (frozen len {}, actual len {}).",
            mismatch_preamble(name, &path),
            expected.len(),
            actual.len(),
        );
    }

    /// Compare a framebuffer against the frozen PNG, pixel-exact, with
    /// the first divergent pixel's coordinates + channels on failure.
    pub fn check_rgba(&self, name: &str, width: u32, height: u32, actual: &[u8]) {
        let path = self.path(name);
        if !path.exists() {
            missing::<()>(name, &path);
        }
        let expected = read_rgba(&path);
        assert_eq!(
            expected.len(),
            (width as usize) * (height as usize) * 4,
            "{}\n\nframe size: frozen PNG is not {width}x{height}",
            mismatch_preamble(name, &path),
        );
        assert_eq!(
            actual.len(),
            expected.len(),
            "{}\n\nframe size: actual buffer is {} bytes, frozen is {}",
            mismatch_preamble(name, &path),
            actual.len(),
            expected.len(),
        );
        for (i, (a, b)) in expected
            .chunks_exact(4)
            .zip(actual.chunks_exact(4))
            .enumerate()
        {
            if a != b {
                let x = i as u32 % width;
                let y = i as u32 / width;
                panic!(
                    "{}\n\nPixel divergence at ({x},{y}): frozen {a:?} vs actual {b:?}",
                    mismatch_preamble(name, &path),
                );
            }
        }
    }

    fn read_text(&self, path: &Path, name: &str) -> String {
        std::fs::read_to_string(path)
            .map(|s| normalize_trailing_newline(&s))
            .unwrap_or_else(|_| missing(name, path))
    }
}

/// Line endings are left alone; only a single trailing newline is
/// normalized so the committed files end in one and the comparison
/// doesn't hinge on it.
fn normalize_trailing_newline(s: &str) -> String {
    let mut out = s.trim_end_matches('\n').to_string();
    out.push('\n');
    out
}

fn missing<T>(name: &str, path: &Path) -> T {
    panic!(
        "missing frozen parity artifact `{name}` at {}\n\
         \n\
         This artifact is the OLD core's frozen output and is the only\n\
         reference this test has. The old core is deleted, so it cannot\n\
         be re-derived — restore the file from git rather than\n\
         re-freezing (which would only record the current output).\n\
         \n\
         See the crate's tests/goldens/README.md.",
        path.display(),
    )
}

fn mismatch_preamble(name: &str, path: &Path) -> String {
    format!(
        "FROZEN-PARITY MISMATCH for `{name}` ({}).\n\
         \n\
         The frozen artifact is the OLD core's output, captured before the\n\
         old core was deleted. The runtime now produces something else.\n\
         \n\
         Unless the difference is a divergence already sanctioned in\n\
         docs/migrating-to-runtime-v2.md (\"What is guaranteed\") AND handled\n\
         by this suite's normalization, this is a BUG — fix the code, not\n\
         the artifact. Do NOT widen a normalization and do NOT re-freeze to\n\
         make this pass.",
        path.display(),
    )
}

fn read_rgba(path: &Path) -> Vec<u8> {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("decode PNG golden {}: {e}", path.display()));
    img.to_rgba8().into_raw()
}
