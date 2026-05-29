//! Static-asset resolution for the web backend.
//!
//! Two roles:
//!
//! 1. **URL resolution** — every registered asset (font / image /
//!    audio / …) is reduced to a single URL string the browser can
//!    fetch, kept in `WebBackend::asset_urls` keyed by [`AssetId`].
//!    The image primitive (next PR) will look up the URL on render;
//!    fonts feed into the [`@font-face`] injection below.
//!
//! 2. **`@font-face` injection** — registering a [`Typeface`] inserts
//!    one CSS `@font-face` rule per face into the shared stylesheet,
//!    pointing at the per-face asset URL. The rule indices are
//!    recorded so `unregister_typeface` can reclaim them.
//!
//! Fonts are resolved to a **served-file URL**, never a blob: a
//! `Bundled`/`BundledEmbedded` font becomes the root-absolute URL
//! `/{path}` and the `@font-face` rule links it with `src: url(...)`.
//! This keeps the font out of the wasm download — the browser fetches
//! and HTTP-caches the `.ttf`/`.woff2` like any other static asset.
//! (For `BundledEmbedded` the carried `bytes` exist only for the
//! byte-consuming backends in the same build, e.g. the wgpu Simulator;
//! the web backend ignores them.)
//!
//! Only a bytes-only `AssetSource::Embedded { bytes, .. }` (from
//! `embed_asset!`, with no bundle path to link) is wrapped in a `Blob`
//! and handed to `URL.createObjectURL`, producing a `blob:` URL the
//! browser handles like any same-origin resource. That blob URL is
//! revoked on `unregister_asset` to free the underlying allocation.
//!
//! [`AssetId`]: runtime_core::AssetId
//! [`Typeface`]: runtime_core::Typeface
//! [`@font-face`]: https://developer.mozilla.org/en-US/docs/Web/CSS/@font-face

use runtime_core::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
use js_sys::{Array, Uint8Array};
use wasm_bindgen::JsValue;

use crate::WebBackend;

/// Path prefix under which `Bundled` assets are expected to live in
/// the deployed `dist/`. The CLI's `serve` / `build` step is
/// responsible for copying each declared asset into
/// `dist/{ASSET_ROUTE}/{path}`. Kept as a const so it's easy to find
/// when wiring the build step.
const ASSET_ROUTE: &str = "assets";

/// Browser MIME type for an asset's extension. Drives the `type`
/// option passed to `Blob` for `Embedded` sources — browsers use it
/// to decide whether to display, download, or sandbox the resource.
/// Unknown extensions fall back to `application/octet-stream`.
fn mime_for(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "ico" => "image/vnd.microsoft.icon",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Wrap `bytes` in a `Blob` of the given MIME type and return its
/// `blob:` URL. Returns `None` if `Blob` or `URL.createObjectURL`
/// rejects the call — extremely rare; the caller falls back to the
/// "broken image" path so the failure is visible rather than silent.
fn blob_url_for(bytes: &[u8], mime: &str) -> Option<String> {
    let chunk = Uint8Array::new_with_length(bytes.len() as u32);
    chunk.copy_from(bytes);
    let parts = Array::new();
    parts.push(&JsValue::from(chunk));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

impl WebBackend {
    pub(crate) fn impl_register_asset(
        &mut self,
        id: AssetId,
        kind: AssetTag,
        source: &AssetSource,
    ) {
        if self.asset_urls.contains_key(&id) {
            return;
        }
        let url = match source {
            // A font with a bundle path is *linked*, not embedded: emit
            // a root-absolute served-file URL so `@font-face` can
            // `src: url("/fonts/…")` it and the browser fetches +
            // HTTP-caches the file. Root-absolute (not `assets/…`)
            // because (a) the build stages the project's top-level font
            // dir verbatim and the dev server serves the project root,
            // and (b) a relative URL would break under the SPA router
            // when the document path isn't `/`. Any embedded `bytes`
            // (BundledEmbedded, present when a byte-consuming backend
            // shares the build) are ignored here.
            AssetSource::Bundled { path } | AssetSource::BundledEmbedded { path, .. }
                if kind == AssetTag::Font =>
            {
                format!("/{path}")
            }
            AssetSource::Bundled { path } | AssetSource::BundledEmbedded { path, .. } => {
                format!("{ASSET_ROUTE}/{path}")
            }
            AssetSource::Remote { url } => (*url).to_string(),
            AssetSource::Embedded { bytes, extension } => {
                let mime = mime_for(extension);
                match blob_url_for(bytes, mime) {
                    Some(url) => {
                        self.blob_asset_urls.insert(id);
                        url
                    }
                    None => {
                        web_sys::console::warn_1(
                            &format!(
                                "register_asset({id:?}): failed to mint blob URL for {} bytes",
                                bytes.len()
                            )
                            .into(),
                        );
                        return;
                    }
                }
            }
        };
        self.asset_urls.insert(id, url);
    }

    pub(crate) fn impl_unregister_asset(&mut self, id: AssetId, _kind: AssetTag) {
        if let Some(url) = self.asset_urls.remove(&id) {
            // Revoke object URLs for `Embedded` sources so the
            // browser frees the Blob's backing storage. Bundled /
            // Remote URLs are owned by the page/CDN — leave them
            // alone.
            if self.blob_asset_urls.remove(&id) {
                let _ = web_sys::Url::revoke_object_url(&url);
            }
        }
    }

    pub(crate) fn impl_register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        _fallback: SystemFallback,
    ) {
        // Dedupe — register_typeface can be called more than once
        // across hot reload cycles. Existing rules stay valid; bail
        // before we double-insert.
        if self.font_face_rule_indices.contains_key(&id) {
            return;
        }
        let mut rule_indices = Vec::with_capacity(faces.len());
        for face in faces {
            let Some(url) = self.asset_urls.get(&face.asset) else {
                // Asset wasn't registered first. This is a framework
                // contract violation — register_typeface promises the
                // per-face assets have been registered already. Log
                // and skip the face so the rest of the family still
                // works.
                web_sys::console::warn_1(
                    &format!(
                        "register_typeface({family_name}): face asset {:?} not registered; skipping",
                        face.asset
                    )
                    .into(),
                );
                continue;
            };
            // Shared with the SSR backend so a face resolves identically.
            let rule = css::font_face_css(family_name, face, url);
            if let Some(idx) = insert_at_rule(self, &rule) {
                rule_indices.push(idx);
            }
        }
        self.font_face_rule_indices.insert(id, rule_indices);
    }

    pub(crate) fn impl_unregister_typeface(&mut self, id: TypefaceId) {
        let Some(indices) = self.font_face_rule_indices.remove(&id) else {
            return;
        };
        // @font-face rules use the same recycle path as everything
        // else: stash the indices in `free_rule_indices` and let the
        // next `insert_rule` reuse them.
        for idx in indices {
            self.delete_rule(idx);
        }
    }
}

/// Insert an at-rule (`@font-face`, `@keyframes`, etc.) into the
/// shared `<style>` element. Same recycle behavior as the class-rule
/// `insert_rule` helper, except we don't prepend `.classname` — we
/// hand the full rule string to the CSSOM.
fn insert_at_rule(backend: &mut WebBackend, rule: &str) -> Option<u32> {
    let sheet = backend.sheet();
    if let Some(idx) = backend.free_rule_indices.pop() {
        let _ = sheet.delete_rule(idx);
        sheet.insert_rule_with_index(rule, idx).ok()
    } else {
        let end = sheet.css_rules().map(|r| r.length()).unwrap_or(0);
        sheet.insert_rule_with_index(rule, end).ok()
    }
}
