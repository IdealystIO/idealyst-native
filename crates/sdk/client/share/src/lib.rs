//! Hand content to the system **share sheet** so the user can send it to
//! another app — the outbound counterpart to [`file-picker`](https://docs.rs/file-picker).
//!
//! Where `file-picker` brings content *in* from another app, `share` sends it
//! *out*: build a [`ShareContent`] (some text, a URL, and/or file references),
//! call [`share`], and the OS shows its own share UI — the iOS/macOS share
//! sheet, the Android chooser, or the browser's Web Share dialog. The user
//! either picks a target app ([`ShareOutcome::Completed`]) or dismisses it
//! ([`ShareOutcome::Dismissed`]).
//!
//! ```ignore
//! use share::{share, ShareContent, ShareOutcome};
//!
//! # async fn demo() -> Result<(), share::ShareError> {
//! let outcome = share(
//!     ShareContent::text("Look at this!").url("https://idealyst.dev"),
//! )
//! .await?;
//!
//! match outcome {
//!     ShareOutcome::Completed => { /* the user sent it somewhere */ }
//!     ShareOutcome::Dismissed => { /* the user cancelled */ }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # No permission required
//!
//! Every backend is user-initiated UI — bringing up the share sheet and
//! picking a target is what does the sending. So this SDK needs no permission
//! on any platform and declares no capability.
//!
//! # Per-platform mechanism
//!
//! The platform-agnostic surface ([`ShareContent`], [`ShareOutcome`],
//! [`ShareError`], [`share`]) lives here; one cfg-gated backend compiles per
//! target:
//!
//! - **iOS** — `UIActivityViewController` over the activity items, presented
//!   from the top view controller; the completion handler maps to the outcome.
//! - **macOS** — `NSSharingServicePicker`, shown relative to the key window.
//! - **Android** — `Intent.ACTION_SEND` (or `ACTION_SEND_MULTIPLE` for files)
//!   wrapped in `Intent.createChooser`, started on the current Activity.
//! - **web** — `navigator.share(...)` (the Web Share API); requires a user
//!   gesture and a secure context, else [`ShareError::NotSupported`].
//! - **Windows / Linux** — [`ShareError::NotSupported`] (no universal native
//!   share surface to target uniformly).

#![deny(missing_docs)]

use std::path::PathBuf;

#[doc(hidden)]
mod recipes;

// ---------------------------------------------------------------------------
// Backend selector. Exactly one compiles per target; each supplies an `imp`
// module with `async fn share(&ShareContent) -> Result<ShareOutcome, ShareError>`.
// Desktop (Windows/Linux) and any other target fall through to the `stub`,
// which returns `NotSupported` — there is no uniform native share surface
// there, and silently degrading would hide that.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
#[path = "android.rs"]
mod imp;

#[cfg(all(any(target_os = "ios", target_os = "macos"), not(target_arch = "wasm32")))]
#[path = "apple.rs"]
mod imp;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
#[path = "stub.rs"]
mod imp;

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// The content to hand to the share sheet: any combination of a text body, a
/// URL, and file references, with an optional title/subject.
///
/// Build it with the ergonomic constructors ([`text`](Self::text),
/// [`url`](Self::url), [`files`](Self::files)) and chain the builder methods to
/// add more. Sharing a [`ShareContent`] with nothing set is a
/// [`ShareError::NothingToShare`].
///
/// How each field is consumed is platform-dependent (some targets take only
/// text, some only a URL, some merge them) — that's the OS share sheet's job,
/// not this crate's. We pass everything set; the chosen target app picks what
/// it understands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShareContent {
    /// A plain-text body to share.
    pub text: Option<String>,
    /// A URL to share (kept distinct from `text` because some targets treat a
    /// URL specially — a link preview, a "copy link" affordance, etc.).
    pub url: Option<String>,
    /// File references to share. On platforms that can't attach files (web's
    /// `navigator.share` without `files`, or where a content URI isn't
    /// available) these are best-effort — see the crate docs.
    pub files: Vec<PathBuf>,
    /// An optional title / subject line. Used as the email subject on targets
    /// that have one (e.g. Mail); ignored by targets that don't.
    pub title: Option<String>,
}

impl ShareContent {
    /// An empty content builder. Prefer the typed constructors
    /// ([`text`](Self::text) / [`url`](Self::url) / [`files`](Self::files)).
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from a text body.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// Start from a URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            url: Some(url.into()),
            ..Self::default()
        }
    }

    /// Start from a set of file references.
    pub fn files(files: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        Self {
            files: files.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Set (or replace) the text body. Chainable.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Set (or replace) the URL. Chainable.
    ///
    /// Named `with_url` rather than `url` because [`url`](Self::url) is the
    /// constructor; this is the builder-chain setter you reach for after
    /// starting from `ShareContent::text(...)`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Add a file reference. Chainable.
    #[must_use]
    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.files.push(file.into());
        self
    }

    /// Set the title / subject. Chainable.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// True when there's nothing to share (no text, no URL, no files). The
    /// title alone isn't shareable content — it's metadata for a body.
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.url.is_none() && self.files.is_empty()
    }
}

/// The result of a [`share`]: the user either sent the content somewhere or
/// dismissed the sheet.
///
/// **Best-effort mapping.** Not every platform reports the distinction
/// reliably. iOS's `UIActivityViewController` and Android's chooser report
/// completion/cancellation; the Web Share API resolves on send and rejects with
/// `AbortError` on cancel. Where a platform can't tell us (e.g. macOS's
/// `NSSharingServicePicker` reports per-service delegates we don't subscribe
/// to), we report [`Completed`](Self::Completed) once the picker has been
/// shown — treat `Completed` as "the share UI ran", not a hard guarantee the
/// user committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShareOutcome {
    /// The user picked a target (or the platform can't distinguish and the
    /// share UI ran to completion).
    Completed,
    /// The user dismissed the share sheet without picking a target.
    Dismissed,
}

/// What can go wrong presenting the share sheet.
///
/// The user *dismissing* the sheet is **not** an error — that's
/// [`ShareOutcome::Dismissed`]. These variants are genuine failures.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShareError {
    /// The platform reported a failure presenting or running the share sheet;
    /// the message carries the backend's reason.
    Backend(String),
    /// Sharing isn't available on this target (desktop without a native share
    /// surface, or web without the Web Share API / outside a secure context /
    /// without a user gesture).
    NotSupported,
    /// The [`ShareContent`] had nothing to share (no text, URL, or files).
    NothingToShare,
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareError::Backend(msg) => write!(f, "share sheet error: {msg}"),
            ShareError::NotSupported => {
                write!(f, "the system share sheet is not available on this platform")
            }
            ShareError::NothingToShare => write!(f, "nothing to share (no text, url, or files)"),
        }
    }
}

impl std::error::Error for ShareError {}

/// Present the system share sheet for `content` and resolve once the user
/// either sends it ([`ShareOutcome::Completed`]) or dismisses it
/// ([`ShareOutcome::Dismissed`]).
///
/// Errors only on a genuine failure: nothing to share
/// ([`ShareError::NothingToShare`]), an unsupported target
/// ([`ShareError::NotSupported`]), or the platform reporting a failure
/// ([`ShareError::Backend`]).
///
/// Must be called in response to a user gesture on web (the Web Share API
/// requires one); native platforms have no such constraint but presenting a
/// share sheet from a button press is the natural shape everywhere.
pub async fn share(content: ShareContent) -> Result<ShareOutcome, ShareError> {
    if content.is_empty() {
        return Err(ShareError::NothingToShare);
    }
    imp::share(&content).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_constructor_sets_only_text() {
        let c = ShareContent::text("hi");
        assert_eq!(c.text.as_deref(), Some("hi"));
        assert_eq!(c.url, None);
        assert!(c.files.is_empty());
        assert!(!c.is_empty());
    }

    #[test]
    fn url_constructor_sets_only_url() {
        let c = ShareContent::url("https://idealyst.dev");
        assert_eq!(c.url.as_deref(), Some("https://idealyst.dev"));
        assert_eq!(c.text, None);
        assert!(!c.is_empty());
    }

    #[test]
    fn files_constructor_collects_paths() {
        let c = ShareContent::files(["/tmp/a.txt", "/tmp/b.txt"]);
        assert_eq!(c.files.len(), 2);
        assert_eq!(c.files[0], PathBuf::from("/tmp/a.txt"));
        assert!(!c.is_empty());
    }

    #[test]
    fn builder_chain_accumulates() {
        let c = ShareContent::text("body")
            .with_url("https://example.com")
            .with_file("/tmp/x")
            .with_title("Subject");
        assert_eq!(c.text.as_deref(), Some("body"));
        assert_eq!(c.url.as_deref(), Some("https://example.com"));
        assert_eq!(c.files, vec![PathBuf::from("/tmp/x")]);
        assert_eq!(c.title.as_deref(), Some("Subject"));
    }

    #[test]
    fn empty_and_title_only_are_empty() {
        assert!(ShareContent::new().is_empty());
        // A title alone is metadata for a body, not shareable content.
        assert!(ShareContent::new().with_title("just a subject").is_empty());
    }

    /// `share` rejects empty content *before* touching any backend, so this
    /// runs identically on every host (no share UI is presented).
    #[tokio::test]
    async fn share_empty_errors_nothing_to_share() {
        let err = share(ShareContent::new()).await.unwrap_err();
        assert_eq!(err, ShareError::NothingToShare);
    }

    /// `share` of title-only content is likewise nothing-to-share — the empty
    /// guard keys off body content, not metadata.
    #[tokio::test]
    async fn share_title_only_errors_nothing_to_share() {
        let err = share(ShareContent::new().with_title("subject"))
            .await
            .unwrap_err();
        assert_eq!(err, ShareError::NothingToShare);
    }

    #[test]
    fn errors_display() {
        assert!(ShareError::NotSupported.to_string().contains("not available"));
        assert!(ShareError::NothingToShare.to_string().contains("nothing"));
        assert!(ShareError::Backend("boom".into())
            .to_string()
            .contains("boom"));
    }
}
