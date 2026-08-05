//! Page-level metadata — document title, description, Open Graph — the
//! "how this screen is represented when shared or indexed externally"
//! hints. SSR emits these as `<head>` tags for crawlers and link
//! unfurlers (which never run wasm, so they need real HTML).
//!
//! Author code calls [`set_page_metadata`] during a screen's render.
//! [`mount`](crate::mount) drains the value after the build and hands it
//! to [`Backend::set_page_metadata`](crate::Backend::set_page_metadata):
//! the SSR backend emits `<head>` tags; the web backend (future) sets
//! `document.title` + meta on the client; platforms with no document
//! concept no-op via the trait default. `title` maps naturally to the
//! nav-bar title, and the SEO fields map to iOS `NSUserActivity` /
//! Android App Indexing if/when that representation is added.

use std::cell::RefCell;

/// External-representation metadata for a screen. All fields optional;
/// unset fields emit no tag.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageMetadata {
    /// Document title — `<title>` on web, nav-bar title on native.
    pub title: Option<String>,
    /// `<meta name="description">` + `og:description`.
    pub description: Option<String>,
    /// `og:image` — the unfurl preview image URL.
    pub og_image: Option<String>,
    /// `<link rel="canonical">` — the canonical URL for this page.
    pub canonical_url: Option<String>,
}

thread_local! {
    static PENDING: RefCell<Option<PageMetadata>> = const { RefCell::new(None) };
}

/// Declare the current screen's page metadata. Call from within a
/// screen's render. Last write wins for the build pass.
pub fn set_page_metadata(meta: PageMetadata) {
    PENDING.with(|p| *p.borrow_mut() = Some(meta));
}

/// Drain the pending page metadata. Called by `mount` after the build
/// to forward it to the backend.
pub fn take_page_metadata() -> Option<PageMetadata> {
    PENDING.with(|p| p.borrow_mut().take())
}
