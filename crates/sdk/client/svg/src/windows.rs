//! Windows implementation of the SVG SDK — the backend's own SVG image.
//!
//! `backend-windows` already draws SVG for `image()` (the `docs/image-src.md`
//! contract: SVG on every backend, rasterized by resvg at the size it is
//! displayed and the screen's pixel density, re-rasterized when either
//! changes). So an `Svg` here is one image node whose `src` is the markup as
//! a `data:image/svg+xml,` URI: crispness across DPI changes and object-fit
//! come from the image path rather than from a second rasterizer in this
//! crate. The observable result matches the other leaves — aspect-fit
//! (`Contain`, the image default and the SVG `xMidYMid meet` default),
//! sharp at any size it is displayed at.
//!
//! The markup is also parsed here, with usvg, so `on_error` carries the
//! parser's message (the image path reports only that decoding failed) and
//! the handle can answer `intrinsic_size`.
//!
//! Gap: the image decoder is built without a font stack, so SVG `<text>` is
//! not drawn (the image contract states this for native); iOS, Android and
//! Linux shape it against system fonts.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use backend_windows::{WindowsBackend, WindowsNode};
use resvg::usvg;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::caps::ImageOps;

use crate::{SvgOps, SvgProps};

pub(crate) static OPS: &dyn SvgOps = &WindowsSvgOps;

runtime_tls::thread_local! {
    /// Intrinsic size of each mounted `Svg`, by node id, from its last
    /// successful parse. The handle reads it; teardown removes it.
    static INTRINSIC: RefCell<HashMap<u64, (f32, f32)>> = RefCell::new(HashMap::new());
}

/// Build the image node for one `Svg` payload and bind its markup.
pub(crate) fn build(props: &Rc<SvgProps>, backend: &Rc<RefCell<WindowsBackend>>) -> WindowsNode {
    // Created with the current markup (read untracked: the effect below is
    // what subscribes), so the node never holds an empty `src` — the image
    // path rejects one with a dev warning.
    let initial = data_uri(&runtime_world::untrack(|| (props.markup)()));
    let node = backend.borrow_mut().create_image(&initial, None, &AccessibilityProps::default());
    let id = node.id();

    // Reactive markup. A parse failure keeps the image that was showing
    // (a transient bad string should not blank a working image) and routes
    // the parser's message to `on_error`. Re-pointing the node at the same
    // URI is free: the backend ignores a `src` it already shows.
    let props = props.clone();
    let backend_for_effect = backend.clone();
    let node_for_effect = node.clone();
    runtime_world::effect(move || {
        let markup = (props.markup)();
        match usvg::Tree::from_str(&markup, &usvg::Options::default()) {
            Ok(tree) => {
                let size = tree.size();
                INTRINSIC.with(|m| m.borrow_mut().insert(id, (size.width(), size.height())));
                backend_for_effect.borrow_mut().update_image_src(&node_for_effect, &data_uri(&markup));
                if let Some(cb) = &props.on_load {
                    cb();
                }
            }
            Err(e) => {
                if let Some(cb) = &props.on_error {
                    cb(e.to_string());
                }
            }
        }
    });
    runtime_vocabulary::style_attach::on_teardown(move || {
        INTRINSIC.with(|m| m.borrow_mut().remove(&id));
    });
    node
}

/// `data:image/svg+xml,` + the markup, percent-encoded (every byte outside
/// RFC 3986's unreserved set), the form `docs/image-src.md` lists.
fn data_uri(markup: &str) -> String {
    let mut out = String::with_capacity(19 + markup.len() * 3);
    out.push_str("data:image/svg+xml,");
    for b in markup.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

struct WindowsSvgOps;

impl SvgOps for WindowsSvgOps {
    fn intrinsic_size(&self, node: &dyn std::any::Any) -> Option<(f32, f32)> {
        let id = node.downcast_ref::<WindowsNode>()?.id();
        INTRINSIC.with(|m| m.borrow().get(&id).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URI must survive `image_policy`'s percent-decoding byte-exact,
    /// including the characters SVG markup is made of (`<`, `"`, `#`, `%`,
    /// spaces, non-ASCII).
    #[test]
    fn data_uri_percent_encodes_everything_outside_unreserved() {
        let uri = data_uri(r##"<svg a="#f00">é 50%</svg>"##);
        assert_eq!(uri, "data:image/svg+xml,%3Csvg%20a%3D%22%23f00%22%3E%C3%A9%2050%25%3C%2Fsvg%3E");
    }
}
