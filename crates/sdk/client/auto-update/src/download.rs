//! Shared, portable download step used by the native apply backends.
//!
//! Fetching the artifact is cross-platform (via the `net` client), so it lives
//! here rather than in any one platform seam. The one job beyond fetching is
//! the security-critical **digest check**: the bytes must hash to the SHA-256
//! the (already signature-verified) manifest promised, or we refuse them.
//! Compiled only on native targets — the web backend never downloads.

use crate::manifest::sha256_hex;
use crate::UpdateError;

/// Download `url` and confirm its SHA-256 matches `expected_sha256` (the signed
/// digest from the manifest). Returns the verified bytes, or
/// [`UpdateError::DigestMismatch`] if the download doesn't match — the last
/// line of defense against a swapped artifact behind a correct-looking URL.
pub(crate) async fn download_verified(
    url: &str,
    expected_sha256: &str,
) -> Result<Vec<u8>, UpdateError> {
    let bytes = net::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| UpdateError::Fetch(e.to_string()))?
        .error_for_status()
        .map_err(|e| UpdateError::Fetch(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| UpdateError::Fetch(e.to_string()))?;

    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(UpdateError::DigestMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }
    Ok(bytes)
}

/// The last path segment of a URL (the artifact filename), or `fallback` if the
/// URL has no usable segment. Used to name the staged download / pick the
/// installer type by extension.
pub(crate) fn filename_from_url(url: &str, fallback: &str) -> String {
    url.rsplit('/')
        .next()
        .map(|s| s.split(['?', '#']).next().unwrap_or(s))
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// A per-process scratch directory for staging an update, unique to this PID so
/// concurrent instances don't collide. Created on demand.
pub(crate) fn staging_dir() -> Result<std::path::PathBuf, UpdateError> {
    let dir = std::env::temp_dir().join(format!("idealyst-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| UpdateError::Install(e.to_string()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_extraction() {
        assert_eq!(filename_from_url("https://x.com/a/App-1.2.dmg", "f"), "App-1.2.dmg");
        assert_eq!(filename_from_url("https://x.com/App.zip?token=1", "f"), "App.zip");
        assert_eq!(filename_from_url("https://x.com/", "fallback"), "fallback");
    }
}
