//! `Element::Image` — a `gtk::Picture` whose paintable holds the decoded
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
//! - **local path** (`/tmp/logo.png`, `assets/x.jpg`) → `set_filename`
//!   (GdkPixbuf decodes PNG/JPG/GIF/BMP/… synchronously).
//! - **`data:` URI** (`data:image/png;base64,…`) → base64-decode the
//!   payload and build a `gdk::Texture` from the raw bytes.
//! - **`http(s)://` URL** → `gio::File::for_uri` + `set_file`. GTK loads
//!   the paintable through GIO/GVfs. NOTE: this depends on a GVfs http
//!   backend being present and reads through GIO rather than a dedicated
//!   streaming async decode — a remote image on a host without GVfs
//!   simply stays blank rather than erroring. A fully async
//!   `NSURLSession`-style loader (see the macOS backend) is the
//!   documented follow-on; `set_file` is GTK's built-in equivalent and
//!   is non-blocking on the main loop for the GVfs-backed case.
//!
//! `content-fit` (the `ObjectFit`) is honored in `apply_style`
//! (`super::apply_style` routes an image node here) — a `src` swap keeps
//! the fit, matching the other backends.

use gtk4::{gdk, gio, glib};

use runtime_shared::ObjectFit;

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

/// Build the `gtk::Picture` for an image node. Default fit is `Contain`
/// (the framework-wide `ObjectFit` default); `apply_style` overrides it
/// when the node's `object_fit` is set. `set_can_shrink(true)` lets the
/// picture size *down* to a box smaller than the bitmap (GTK defaults to
/// only shrinking, but we set it explicitly so intent is visible).
pub(crate) fn build_picture(src: &str, alt: Option<&str>) -> gtk4::Picture {
    let pic = gtk4::Picture::new();
    pic.set_content_fit(gtk4::ContentFit::Contain);
    pic.set_can_shrink(true);
    if let Some(a) = alt {
        pic.set_alternative_text(Some(a));
    }
    set_source(&pic, src);
    pic
}

/// Point a `gtk::Picture` at `src`, decoding by source kind. An empty
/// `src` clears the paintable (an image whose reactive source resolved to
/// nothing renders blank rather than keeping the stale bitmap).
pub(crate) fn set_source(pic: &gtk4::Picture, src: &str) {
    if src.is_empty() {
        pic.set_paintable(gdk::Paintable::NONE);
        return;
    }
    if let Some(rest) = src.strip_prefix("data:") {
        match decode_data_uri(rest) {
            Some(bytes) => {
                // `glib::Bytes` copies the slice; the decoded Vec can drop.
                let gbytes = glib::Bytes::from(&bytes[..]);
                match gdk::Texture::from_bytes(&gbytes) {
                    Ok(tex) => pic.set_paintable(Some(&tex)),
                    // Undecodable payload → blank, don't panic.
                    Err(_) => pic.set_paintable(gdk::Paintable::NONE),
                }
            }
            None => pic.set_paintable(gdk::Paintable::NONE),
        }
        return;
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        // GTK loads the paintable through GIO/GVfs (see module doc for the
        // async caveat). `for_uri` never blocks here; the fetch is driven
        // by GTK's media file machinery.
        let file = gio::File::for_uri(src);
        pic.set_file(Some(&file));
        return;
    }
    // Anything else is a local filesystem path (absolute or relative).
    pic.set_filename(Some(std::path::Path::new(src)));
}

// =========================================================================
// data: URI base64 decode
//
// Pure logic (no GTK) so it's unit-testable without a display. Handles the
// standard `data:[<mime>][;base64],<payload>` shape. Only base64 payloads
// build a texture; a non-base64 (percent-encoded) data URI returns None
// (rare for images, and GdkPixbuf can't decode raw text bytes anyway).
// =========================================================================

/// Decode the part of a data URI *after* the `data:` prefix. Returns the
/// raw bytes for a `;base64,` payload, or `None` for a malformed / non-
/// base64 URI.
pub(crate) fn decode_data_uri(rest: &str) -> Option<Vec<u8>> {
    let comma = rest.find(',')?;
    let (meta, payload) = rest.split_at(comma);
    let payload = &payload[1..]; // skip the comma
    if !meta
        .rsplit(';')
        .next()
        .map(|s| s.eq_ignore_ascii_case("base64"))
        .unwrap_or(false)
    {
        // Not a base64 data URI — nothing decodable to a bitmap.
        return None;
    }
    base64_decode(payload.trim())
}

/// Minimal RFC 4648 base64 decoder (standard alphabet, `=` padding,
/// whitespace tolerated). Hand-rolled to avoid pulling a base64 crate
/// into the backend for one call site. Returns `None` on any invalid
/// character or a truncated final quantum.
pub(crate) fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut quad = [0u8; 4];
    let mut n = 0usize; // symbols collected in the current quantum
    let mut pads = 0usize;
    for &c in input.as_bytes() {
        if c == b'\n' || c == b'\r' || c == b' ' || c == b'\t' {
            continue;
        }
        if c == b'=' {
            pads += 1;
            quad[n] = 0;
            n += 1;
        } else {
            if pads > 0 {
                return None; // data after padding — malformed
            }
            quad[n] = val(c)?;
            n += 1;
        }
        if n == 4 {
            out.push((quad[0] << 2) | (quad[1] >> 4));
            if pads < 2 {
                out.push((quad[1] << 4) | (quad[2] >> 2));
            }
            if pads < 1 {
                out.push((quad[2] << 6) | quad[3]);
            }
            n = 0;
            pads = 0;
        }
    }
    // A well-formed base64 string is a whole number of quanta.
    if n != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests — no GTK context, so they run under
    //! `cargo test -p backend-linux --lib` without a display.
    use super::*;

    #[test]
    fn base64_decodes_ascii_payloads() {
        assert_eq!(base64_decode("").unwrap(), b"");
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        // Padding-driven tail lengths.
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode("TQ==").unwrap(), b"M");
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn base64_tolerates_embedded_whitespace() {
        // MIME base64 wraps at 76 cols — newlines must be ignored.
        assert_eq!(base64_decode("Zm9v\nYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("TWE =").unwrap(), b"Ma");
    }

    #[test]
    fn base64_rejects_malformed_input() {
        assert!(base64_decode("TWF").is_none()); // truncated quantum
        assert!(base64_decode("****").is_none()); // invalid alphabet
        assert!(base64_decode("TW=E").is_none()); // data after padding
    }

    #[test]
    fn data_uri_extracts_base64_payload() {
        // "Man" → TWFu, with a mime + ;base64.
        let bytes = decode_data_uri("image/png;base64,TWFu").unwrap();
        assert_eq!(bytes, b"Man");
        // No mime, just ;base64.
        assert_eq!(decode_data_uri(";base64,TWE=").unwrap(), b"Ma");
    }

    #[test]
    fn data_uri_rejects_non_base64() {
        // Percent-encoded (text) data URIs aren't decodable to a bitmap.
        assert!(decode_data_uri("text/plain,hello").is_none());
        // No comma at all.
        assert!(decode_data_uri("image/png;base64").is_none());
    }

    #[test]
    fn object_fit_maps_one_to_one() {
        assert_eq!(map_fit(ObjectFit::Fill), gtk4::ContentFit::Fill);
        assert_eq!(map_fit(ObjectFit::Contain), gtk4::ContentFit::Contain);
        assert_eq!(map_fit(ObjectFit::Cover), gtk4::ContentFit::Cover);
    }
}
