//! Linux (GTK4) implementation of the SVG SDK — resvg rasterizer.
//!
//! Unlike the iOS / Android leaves (which walk a parsed `usvg::Tree`
//! into native vector primitives — `UIBezierPath` / `android.graphics.
//! Path`), GTK4 has no low-level per-node vector-draw surface that
//! maps cleanly onto the walker's `SvgPainter`. GTK renders through
//! GSK render nodes; the pragmatic, robust path on this backend is to
//! rasterize the SVG with `resvg` + `tiny-skia` into an RGBA buffer,
//! wrap it in a `gdk::MemoryTexture`, and hand that to a
//! `gtk::Picture` as its paintable. `Picture` is GTK's image widget:
//! it scales the texture into its allocation with `ContentFit::Contain`
//! (aspect-preserving), matching the `<svg preserveAspectRatio=
//! "xMidYMid meet">` default that the iOS leaf's `scaleAspectFit` also
//! honors. So the *mechanism* differs from mobile (raster vs. vector)
//! but the *observable output* converges — the CLAUDE.md §7 contract.
//!
//! # Crispness
//!
//! A raster texture is resolution-fixed, so we supersample: the SVG is
//! rasterized at `SUPERSAMPLE ×` its intrinsic size (capped at
//! [`MAX_DIM`]). `Picture` then downscales for typical display sizes,
//! which stays crisp; extreme upscales past the supersampled
//! resolution soften — acceptable for the icon/logo cases this SDK
//! targets, and the honest trade for GTK not exposing a retained
//! vector-node draw API the way UIKit's `drawRect:` does.
//!
//! # Reactive markup
//!
//! Same `effect!`-driven shape as every other leaf: the effect reads
//! `(props.markup)()`, re-parses + re-rasterizes on change, and swaps
//! the `Picture`'s paintable via `set_paintable`. `on_load` fires per
//! successful render, `on_error` per parse failure (resvg's error).
//!
//! # Fonts
//!
//! System fonts are loaded once at [`register`] time into a shared
//! `fontdb::Database` (via resvg's re-exported `usvg::fontdb`), so
//! `<text>` elements shape against installed fonts — matching the
//! iOS / Android `system-fonts` posture.

use crate::{SvgOps, SvgProps};
use backend_linux::{LinuxBackend, LinuxNode};
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use resvg::tiny_skia::Pixmap;
use resvg::usvg;
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) static OPS: &dyn SvgOps = &LinuxSvgOps;

/// Supersample factor: rasterize at N× the SVG's intrinsic size so
/// `Picture` has headroom before an upscale softens the image.
const SUPERSAMPLE: f32 = 2.0;
/// Hard ceiling on either raster dimension — guards against a
/// pathological SVG (huge viewBox) allocating a multi-hundred-MB
/// pixmap. 4096 covers 2× retina for any realistic on-screen size.
const MAX_DIM: f32 = 4096.0;

/// Register the Linux `Svg` external handler on `backend`. Call once
/// at app boot (inside the host's `register_extensions`) so `Svg`
/// elements lower to a real `gtk::Picture` instead of the framework's
/// "External not registered" placeholder.
/// Shared font database for every `Svg` mounted on this backend.
///
/// `load_system_fonts` walks fontconfig and is far too costly to run per
/// markup change, so it is built once and shared (Arc) into every parse.
/// Was a `register`-time local under the old External table; v2 mounts
/// per node, so the cache moves to a `OnceLock` rather than being rebuilt
/// on each mount.
fn fontdb() -> Arc<usvg::fontdb::Database> {
    static FONTDB: std::sync::OnceLock<Arc<usvg::fontdb::Database>> = std::sync::OnceLock::new();
    FONTDB
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Scene-registry entry point: build the GTK leaf for one `Svg` payload.
/// The `register_external`/`RegisterExternal` pair this used to go through
/// is gone in v2 — `lib.rs`'s `mount_svg_linux` calls this instead.
pub(crate) fn build(props: &Rc<SvgProps>, b: &mut LinuxBackend) -> LinuxNode {
    build_svg(props, b, fontdb())
}

// =========================================================================
// Build + reactive raster
// =========================================================================

fn build_svg(
    props: &Rc<SvgProps>,
    b: &mut LinuxBackend,
    fontdb: Arc<usvg::fontdb::Database>,
) -> LinuxNode {
    let picture = gtk4::Picture::new();
    // Contain = aspect-preserving fit, centered — matches the SVG
    // `xMidYMid meet` default and the iOS leaf's scaleAspectFit.
    picture.set_content_fit(gtk4::ContentFit::Contain);
    // Let the widget shrink below the texture's intrinsic size; Taffy
    // drives the final allocation from author styles, not the SVG's
    // natural pixel dimensions.
    picture.set_can_shrink(true);

    let node = b.register_external_view(picture.clone().upcast::<gtk4::Widget>());

    // Reactive markup: re-parse + re-rasterize on every change. A
    // parse failure leaves the previous paintable untouched (better
    // than blanking a working image on a transient bad string) and
    // routes to `on_error`.
    let picture_for_effect = picture.clone();
    let props_for_effect = props.clone();
    runtime_core::effect!({
        let markup = (props_for_effect.markup)();

        let mut opt = usvg::Options::default();
        opt.fontdb = fontdb.clone();

        match usvg::Tree::from_str(&markup, &opt) {
            Ok(tree) => {
                if let Some(texture) = rasterize(&tree) {
                    picture_for_effect.set_paintable(Some(&texture));
                }
                if let Some(cb) = &props_for_effect.on_load {
                    cb();
                }
            }
            Err(e) => {
                if let Some(cb) = &props_for_effect.on_error {
                    cb(format!("{e}"));
                }
            }
        }
    });

    node
}

/// Rasterize a parsed tree into a `gdk::MemoryTexture`.
///
/// Returns `None` only if the pixmap allocation fails (degenerate
/// size) — the caller keeps the prior paintable in that case.
fn rasterize(tree: &usvg::Tree) -> Option<gdk::MemoryTexture> {
    let size = tree.size();
    let w0 = size.width().max(1.0);
    let h0 = size.height().max(1.0);

    // Supersample, then clamp so neither dimension exceeds MAX_DIM.
    let mut scale = SUPERSAMPLE;
    let longest = w0.max(h0);
    if longest * scale > MAX_DIM {
        scale = MAX_DIM / longest;
    }
    let w = (w0 * scale).round().clamp(1.0, MAX_DIM) as u32;
    let h = (h0 * scale).round().clamp(1.0, MAX_DIM) as u32;

    let mut pixmap = Pixmap::new(w, h)?;
    // Map the SVG's user space onto the pixmap: separate x/y factors
    // absorb the rounding of w/h back to integer pixels.
    let transform = resvg::tiny_skia::Transform::from_scale(w as f32 / w0, h as f32 / h0);
    resvg::render(tree, transform, &mut pixmap.as_mut());

    // tiny-skia's pixmap is premultiplied RGBA in R,G,B,A byte order,
    // which is exactly `MemoryFormat::R8g8b8a8Premultiplied` — no
    // channel swizzle needed. `from_owned` takes the Vec by value so
    // the buffer lives as long as the texture references it.
    let bytes = glib::Bytes::from_owned(pixmap.data().to_vec());
    Some(gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8Premultiplied,
        &bytes,
        (w * 4) as usize,
    ))
}

// =========================================================================
// Imperative ops
// =========================================================================

struct LinuxSvgOps;

impl SvgOps for LinuxSvgOps {
    fn intrinsic_size(&self, _node: &dyn Any) -> Option<(f32, f32)> {
        // The `RefFill` closure hands the ops a type-erased
        // `LinuxNode`, whose `widget` field is `pub(crate)` to
        // `backend-linux` — this SDK crate can't reach it, and the
        // project rules forbid editing `backend-linux` to expose an
        // accessor. Without the widget we can't reach the `Picture`'s
        // paintable to read its intrinsic size, so this degrades to
        // the trait default (`None`) rather than guessing. Authors who
        // need the natural size can parse the same markup with `usvg`
        // and read `tree.size()` directly. If a widget accessor lands
        // on `LinuxNode`, wire this up via
        // `picture.paintable()?.intrinsic_width()/height()`.
        None
    }
}
