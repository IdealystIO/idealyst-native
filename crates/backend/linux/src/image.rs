//! `ImagePrim` — a `gtk::Picture` whose paintable holds the decoded
//! bitmap, scaled by `content-fit`.
//!
//! ## Why `gtk::Picture` (not `gtk::Image`)
//!
//! `gtk::Image` is icon-sized (it snaps to fixed icon dimensions and
//! never scales a large bitmap to a box). `gtk::Picture` is the widget
//! for arbitrary-size images and — crucially — exposes `content-fit`
//! (`Fill` / `Contain` / `Cover` / `ScaleDown`), the exact
//! aspect-fit / aspect-fill / stretch modes `ObjectFit` needs. This
//! mirrors CSS `object-fit`, UIKit `contentMode`, AppKit
//! `contentsGravity`, and Android `ScaleType` on the other backends —
//! one author style, uniform output (CLAUDE.md §7).
//!
//! | [`ObjectFit`]       | `gtk::ContentFit` |
//! | ------------------- | ----------------- |
//! | `Fill`              | `Fill`            |
//! | `Contain` (default) | `Contain`         |
//! | `Cover`             | `Cover`           |
//!
//! ## Sources
//!
//! `src` follows the cross-backend contract in `docs/image-src.md`; the
//! routing, decoding and fetching live in the GTK-free [`crate::image_load`]
//! (tested without a display). This module only turns a decoded image into a
//! GDK paintable:
//!
//! - **raster** (PNG/JPEG/GIF/WebP, decoded by the `image` crate) → a
//!   `gdk::MemoryTexture`.
//! - **SVG** (parsed by `resvg`) → [`SvgPaintable`], which rasterizes at the
//!   size the picture actually draws it, times the widget's scale factor, so
//!   a logo scaled up stays sharp.
//!
//! ## `on_load` / `on_error`
//!
//! `install_image_load_handler` / `install_image_error_handler` are
//! DEFAULTED no-ops on the capability trait; without overriding both an
//! author's `Image(on_error = ...)` fallback never runs (e.g. a remote fetch
//! fails for lack of a GVfs http backend and nothing tells the app).
//!
//! `data:` URIs and assets resolve SYNCHRONOUSLY inside
//! `create_image` / `update_image_src`; `http(s)` resolves on a later
//! main-loop turn (`LinuxBackend::finish_remote_image`), guarded by a
//! per-node load generation so a slow earlier response never replaces a newer
//! `src`.
//!
//! Handlers are installed by the vocabulary AFTER `create_image` returns, so a
//! synchronous outcome has to be REMEMBERED and replayed on install — the
//! trait requires `on_load` to fire immediately for an already-decoded
//! image. [`LinuxBackend::image_nodes`](crate::LinuxBackend) holds that
//! per-node state.
//!
//! Author handlers are invoked with NO backend borrow held: they write
//! signals, signal writes run effects synchronously, and effects re-enter
//! the backend. Firing one through a live `RefCell` borrow aborts the
//! process with `RefCell already borrowed` — the same hazard documented on
//! `set_viewport_size` and `on_scroll`.

use std::cell::{Cell, RefCell};

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene};

use backend_image_source::SvgImage;
use runtime_shared::ObjectFit;

use crate::image_load::Decoded;

/// Map the framework's [`ObjectFit`] to GTK's `content-fit`. `ScaleDown`
/// has no `ObjectFit` peer (it's "contain, but never upscale"), so it's
/// intentionally unused — the framework's three fits map 1:1.
pub(crate) fn map_fit(fit: ObjectFit) -> gtk4::ContentFit {
    match fit {
        ObjectFit::Fill => gtk4::ContentFit::Fill,
        ObjectFit::Contain => gtk4::ContentFit::Contain,
        ObjectFit::Cover => gtk4::ContentFit::Cover,
    }
}

/// Build the (empty) `gtk::Picture` for an image node; the backend loads
/// its `src` once the node has an id. Default fit is `Contain` (the
/// framework-wide `ObjectFit` default); `apply_style` overrides it when the
/// node's `object_fit` is set. `set_can_shrink(true)` lets the picture size
/// *down* to a box smaller than the bitmap.
pub(crate) fn build_picture(alt: Option<&str>) -> gtk4::Picture {
    let pic = gtk4::Picture::new();
    pic.set_content_fit(gtk4::ContentFit::Contain);
    pic.set_can_shrink(true);
    if let Some(a) = alt {
        pic.set_alternative_text(Some(a));
    }
    // An SVG is rasterized for the display it is on; moving the window to a
    // display with a different scale must re-rasterize, not upscale.
    pic.connect_scale_factor_notify(|p| {
        if let Some(svg) = p.paintable().and_downcast::<SvgPaintable>() {
            svg.set_screen_scale(p.scale_factor() as f64);
        }
    });
    pic
}

/// What a load attempt resolved to. `Pending` means a remote fetch is in
/// flight and the outcome arrives on a later main-loop turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ImageOutcome {
    /// Decoded, with the natural size (px for a raster, the intrinsic size
    /// for SVG).
    Loaded { width: f32, height: f32 },
    /// Rejected `src`, bad bytes, missing asset, transport or HTTP error.
    Failed,
    /// Remote fetch in flight.
    Pending,
}

/// Put `decoded` on screen and report its natural size.
pub(crate) fn show(pic: &gtk4::Picture, decoded: Decoded) -> ImageOutcome {
    let (width, height) = decoded.natural_size();
    match decoded {
        Decoded::Raster(r) => {
            if r.width == 0 || r.height == 0 {
                return clear(pic);
            }
            let stride = r.width as usize * 4;
            let tex = gdk::MemoryTexture::new(
                r.width as i32,
                r.height as i32,
                gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_owned(r.rgba),
                stride,
            );
            pic.set_paintable(Some(&tex));
        }
        Decoded::Svg(svg) => {
            let paintable = SvgPaintable::new(svg, pic.scale_factor() as f64);
            pic.set_paintable(Some(&paintable));
        }
    }
    ImageOutcome::Loaded { width, height }
}

/// Blank the picture: a failed load must not keep showing the previous
/// `src`'s bitmap.
pub(crate) fn clear(pic: &gtk4::Picture) -> ImageOutcome {
    pic.set_paintable(gdk::Paintable::NONE);
    ImageOutcome::Failed
}

// =========================================================================
// SvgPaintable — an SVG drawn at its displayed size.
//
// `gtk::Picture` calls `snapshot(width, height)` with the size it draws the
// paintable at (after `content-fit`), and measures through the intrinsic
// size. So the paintable reports the SVG's intrinsic size (what `on_load`
// and Taffy's measure see) and rasterizes in `snapshot` at the drawn size ×
// screen scale, caching the last raster by pixel size so a steady frame
// re-uses it. Same rule as the Apple/Android backends
// (`SvgImage::raster_scale`): enough pixels that no axis is upscaled.
// =========================================================================

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SvgPaintable {
        pub svg: RefCell<Option<SvgImage>>,
        /// Device pixels per logical px (the picture's scale factor).
        pub screen_scale: Cell<f64>,
        /// Last raster, keyed by its pixel size.
        pub cache: RefCell<Option<((u32, u32), gdk::Texture)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SvgPaintable {
        const NAME: &'static str = "IdealystSvgPaintable";
        type Type = super::SvgPaintable;
        type ParentType = glib::Object;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for SvgPaintable {}

    impl SvgPaintable {
        fn intrinsic(&self) -> (f64, f64) {
            self.svg.borrow().as_ref().map(|s| s.intrinsic_size()).unwrap_or((0.0, 0.0))
        }
    }

    impl PaintableImpl for SvgPaintable {
        fn flags(&self) -> gdk::PaintableFlags {
            // The intrinsic size never changes; the CONTENTS can (a
            // scale-factor change re-rasterizes), so only SIZE is static.
            gdk::PaintableFlags::SIZE
        }

        fn intrinsic_width(&self) -> i32 {
            self.intrinsic().0.round() as i32
        }

        fn intrinsic_height(&self) -> i32 {
            self.intrinsic().1.round() as i32
        }

        fn intrinsic_aspect_ratio(&self) -> f64 {
            let (w, h) = self.intrinsic();
            if w > 0.0 && h > 0.0 {
                w / h
            } else {
                0.0
            }
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let svg = self.svg.borrow();
            let Some(svg) = svg.as_ref() else { return };
            let scale = svg.raster_scale((width, height), self.screen_scale.get());
            let (iw, ih) = svg.intrinsic_size();
            // `SvgImage::rasterize` sizes its pixmap exactly like this.
            let key = (
                (iw * scale).round().max(1.0) as u32,
                (ih * scale).round().max(1.0) as u32,
            );
            let cached = self.cache.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, t)| t.clone());
            let tex = match cached {
                Some(t) => t,
                None => {
                    let Some(r) = svg.rasterize(scale) else { return };
                    let tex: gdk::Texture = gdk::MemoryTexture::new(
                        r.width_px as i32,
                        r.height_px as i32,
                        gdk::MemoryFormat::R8g8b8a8Premultiplied,
                        &glib::Bytes::from_owned(r.rgba_premultiplied),
                        r.width_px as usize * 4,
                    )
                    .upcast();
                    *self.cache.borrow_mut() = Some((key, tex.clone()));
                    tex
                }
            };
            if let Some(snap) = snapshot.downcast_ref::<gtk4::Snapshot>() {
                snap.append_texture(
                    &tex,
                    &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
                );
            }
        }
    }
}

glib::wrapper! {
    pub struct SvgPaintable(ObjectSubclass<imp::SvgPaintable>)
        @implements gdk::Paintable;
}

impl SvgPaintable {
    pub(crate) fn new(svg: SvgImage, screen_scale: f64) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().svg.borrow_mut() = Some(svg);
        obj.imp().screen_scale.set(screen_scale.max(1.0));
        obj
    }

    fn set_screen_scale(&self, scale: f64) {
        let scale = scale.max(1.0);
        if self.imp().screen_scale.replace(scale) != scale {
            self.invalidate_contents();
        }
    }

    /// The pixel size of the last raster drawn, if any (tests).
    #[cfg(test)]
    pub(crate) fn last_raster_px(&self) -> Option<(u32, u32)> {
        self.imp().cache.borrow().as_ref().map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests — no GTK context, so they run under
    //! `cargo test -p backend-linux --lib` without a display.
    use super::*;

    #[test]
    fn object_fit_maps_one_to_one() {
        assert_eq!(map_fit(ObjectFit::Fill), gtk4::ContentFit::Fill);
        assert_eq!(map_fit(ObjectFit::Contain), gtk4::ContentFit::Contain);
        assert_eq!(map_fit(ObjectFit::Cover), gtk4::ContentFit::Cover);
    }
}
