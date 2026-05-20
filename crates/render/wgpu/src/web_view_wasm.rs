//! Wasm32 stub for the WebView primitive.
//!
//! The native `web_view.rs` runs a Blitz worker thread that
//! fetches a URL, paints the document into an RGBA buffer, and
//! the renderer's pre-pass uploads that buffer to a wgpu texture.
//! Blitz's network provider depends on `reqwest` + `tokio`,
//! neither of which compile to wasm32; on the web target this
//! module exposes a stub with the same shape so the rest of
//! `render-wgpu` (node-kind variant, walk arm, scrub helpers)
//! keeps building.
//!
//! Visually, the wasm path mounts an `<iframe>` directly via
//! [`crate::dom_overlay::DomOverlay`]: every frame the renderer
//! drains a request list of (node, url, screen rect) and the
//! host shell creates / repositions a sibling DOM element above
//! the canvas. The wgpu side paints nothing for the WebView
//! itself — the iframe sits in front of the canvas at the right
//! rect; `pointer-events: none` on the wrapper means clicks
//! outside the iframe fall back through to the framework's
//! pointer pipeline.
//!
//! `navigate(...)` round-trips through `url.borrow_mut()`; the
//! overlay reads the latest value on its next sync and updates
//! the iframe's `src` attribute when it changes.

use std::cell::RefCell;

/// One painted page — kept around for API parity with the native
/// module. On wasm the renderer never reads it; the iframe is
/// composited by the browser, not via wgpu.
pub struct PaintedPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct WebView {
    url: RefCell<String>,
}

impl WebView {
    pub fn spawn(url: String, _width: u32, _height: u32) -> Self {
        Self {
            url: RefCell::new(url),
        }
    }

    pub fn url(&self) -> String {
        self.url.borrow().clone()
    }

    pub fn navigate(&self, url: String) {
        *self.url.borrow_mut() = url;
    }

    pub fn shutdown(&self) {}
}
