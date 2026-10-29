//! `configure` sets the image base, so a root-relative `image()` src loads
//! from the configured server on every platform. Its own test binary: both
//! the client config and the image base are process-wide.

use runtime_shared::image_src::{base_url, with_base};

/// REGRESSION: a root-relative `src` was rejected even after
/// `server::configure`, and the suggested fix (prefixing
/// `server::base_url()`) left it relative on same-origin web, where the
/// base is `""`.
#[test]
fn regression_configure_sets_the_image_base() {
    server::configure(server::ClientConfig::new("https://api.example.com/"));
    assert_eq!(base_url().as_deref(), Some("https://api.example.com"));
    assert_eq!(with_base("/uploads/x.png"), "https://api.example.com/uploads/x.png");

    // Same-origin web: no image base; the browser resolves against the page.
    server::configure(server::ClientConfig::new(""));
    assert_eq!(base_url(), None);
    assert_eq!(with_base("/uploads/x.png"), "/uploads/x.png");
}
