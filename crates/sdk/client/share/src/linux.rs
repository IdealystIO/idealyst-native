//! Linux share via the desktop **portal** (`xdg-desktop-portal`), driven through
//! `ashpd` — the same portal posture `file-export` / `file-picker` take for
//! their Linux dialogs.
//!
//! ## Why two portals, not one
//!
//! There is no single "share sheet" / `ACTION_SEND`-style chooser portal on
//! Linux. The portal surface splits the outbound-share job across two
//! interfaces, so we route by what the [`ShareContent`] actually carries:
//!
//! - **`OpenURI` (`org.freedesktop.portal.OpenURI`)** — hands a *single* URI to
//!   a handler app. With `ask(true)` the portal shows the "Open With" app
//!   chooser, which is the closest thing to a share sheet for a bare link. It
//!   carries only one URI: no body text, no subject, no attachments.
//! - **`Email` (`org.freedesktop.portal.Email` / `ComposeEmail`)** — opens the
//!   user's mail composer pre-filled with a body, a subject, and file
//!   attachments. It's the only portal that can carry body text + a title +
//!   files together, so anything richer than a lone URL goes here.
//!
//! Routing (see [`plan`]): if the content has files, body text, or a title we
//! use `Email` (only it can carry them); otherwise — a lone URL — we use
//! `OpenURI`'s app chooser. `share()` is only ever called with non-empty
//! content (the empty guard lives in [`crate::share`]), so the lone-URL branch
//! always has a URL to open.
//!
//! ## Outcome mapping
//!
//! Both portal calls resolve to a `Request` whose `response()` is `Ok(())` when
//! the flow ran and `Err(Response(Cancelled))` when the user dismissed the
//! chooser / composer. We map those to [`ShareOutcome::Completed`] /
//! [`ShareOutcome::Dismissed`] respectively (best-effort, per `ShareOutcome`'s
//! own note — a mail client that reports success on *launch* rather than *send*
//! reads as `Completed`). Any other portal error is an honest
//! [`ShareError::Backend`].
//!
//! VERIFICATION: `plan()` routing is unit-tested below and runs headlessly. The
//! D-Bus portal round-trip (`send_uri` / `ComposeEmail`) resolves only at
//! runtime against a live `xdg-desktop-portal` session and is NOT exercised by
//! the unit tests — same posture as `file-export` / `file-picker`'s Linux
//! backends.

use std::os::fd::OwnedFd;
use std::path::PathBuf;

use ashpd::desktop::email::EmailRequest;
use ashpd::desktop::open_uri::OpenFileRequest;

use crate::{ShareContent, ShareError, ShareOutcome};

/// The portal call `share()` will make for a given [`ShareContent`]. Split out
/// from the async I/O so the routing decision is unit-testable without a live
/// portal session.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SharePlan {
    /// Hand a single URI to the `OpenURI` portal (app chooser via `ask(true)`).
    OpenUri(String),
    /// Compose via the `Email` portal: an optional body, an optional subject,
    /// and zero or more file attachments.
    Email {
        body: Option<String>,
        subject: Option<String>,
        files: Vec<PathBuf>,
    },
}

/// Decide which portal to drive for `content`.
///
/// `OpenURI` carries a single URI and nothing else, so it's reserved for the
/// bare-URL case. Anything with files, body text, or a title needs `Email`,
/// which is the only portal that can carry all three together — the URL, when
/// present alongside text, is appended to the body (`ACTION_SEND` carries one
/// text blob; we mirror that here).
fn plan(content: &ShareContent) -> SharePlan {
    let has_body_or_meta = content.text.is_some() || content.title.is_some();

    if !content.files.is_empty() || has_body_or_meta {
        // text + url joined by a newline when both are present, mirroring the
        // Android backend's single-blob EXTRA_TEXT.
        let body = match (&content.text, &content.url) {
            (Some(t), Some(u)) => Some(format!("{t}\n{u}")),
            (Some(t), None) => Some(t.clone()),
            (None, Some(u)) => Some(u.clone()),
            (None, None) => None,
        };
        SharePlan::Email {
            body,
            subject: content.title.clone(),
            files: content.files.clone(),
        }
    } else {
        // Non-empty content with no files, text, or title must be a lone URL
        // (the empty guard in `crate::share` rules out the all-`None` case).
        SharePlan::OpenUri(content.url.clone().unwrap_or_default())
    }
}

pub(crate) async fn share(content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    match plan(content) {
        SharePlan::OpenUri(uri) => open_uri(&uri).await,
        SharePlan::Email {
            body,
            subject,
            files,
        } => compose_email(body.as_deref(), subject.as_deref(), &files).await,
    }
}

/// Drive the `OpenURI` portal with `ask(true)` so the compositor shows its
/// "Open With" app chooser for the link.
async fn open_uri(uri: &str) -> Result<ShareOutcome, ShareError> {
    let url = ashpd::url::Url::parse(uri)
        .map_err(|e| ShareError::Backend(format!("invalid url {uri:?}: {e}")))?;

    let request = OpenFileRequest::default()
        .ask(true)
        .send_uri(&url)
        .await
        .map_err(|e| ShareError::Backend(format!("OpenURI portal request: {e}")))?;

    outcome_from(request.response())
}

/// Drive the `Email` portal's `ComposeEmail`, attaching each file by fd.
async fn compose_email(
    body: Option<&str>,
    subject: Option<&str>,
    files: &[PathBuf],
) -> Result<ShareOutcome, ShareError> {
    let mut req = EmailRequest::default();
    if let Some(b) = body {
        req = req.body(b);
    }
    if let Some(s) = subject {
        req = req.subject(s);
    }
    for path in files {
        // The portal takes an fd per attachment (it can't read a raw path from
        // inside a sandbox). A file we can't open is a real failure, not a
        // silent skip — surface it rather than fake a partial share.
        let file = std::fs::File::open(path)
            .map_err(|e| ShareError::Backend(format!("cannot open {}: {e}", path.display())))?;
        req = req.attach(OwnedFd::from(file));
    }

    let request = req
        .send()
        .await
        .map_err(|e| ShareError::Backend(format!("Email portal request: {e}")))?;

    outcome_from(request.response())
}

/// Map a portal `Request::response()` to a [`ShareOutcome`] / [`ShareError`].
/// A `Cancelled` response is the user dismissing the chooser/composer — an
/// outcome, not an error.
fn outcome_from(response: Result<(), ashpd::Error>) -> Result<ShareOutcome, ShareError> {
    match response {
        Ok(()) => Ok(ShareOutcome::Completed),
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
            Ok(ShareOutcome::Dismissed)
        }
        Err(e) => Err(ShareError::Backend(format!("portal response: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Routing (`plan`) — exercised headlessly; no portal session needed. --

    #[test]
    fn lone_url_routes_to_open_uri() {
        let plan = plan(&ShareContent::url("https://idealyst.dev"));
        assert_eq!(plan, SharePlan::OpenUri("https://idealyst.dev".into()));
    }

    #[test]
    fn text_only_routes_to_email_body() {
        let plan = plan(&ShareContent::text("hello"));
        assert_eq!(
            plan,
            SharePlan::Email {
                body: Some("hello".into()),
                subject: None,
                files: vec![],
            }
        );
    }

    #[test]
    fn text_and_url_join_into_email_body() {
        let plan = plan(&ShareContent::text("look").with_url("https://example.com"));
        assert_eq!(
            plan,
            SharePlan::Email {
                body: Some("look\nhttps://example.com".into()),
                subject: None,
                files: vec![],
            }
        );
    }

    #[test]
    fn title_forces_email_even_for_a_url() {
        // A title can only be carried by the Email portal (subject line), so a
        // url + title must NOT take the OpenURI branch.
        let plan = plan(&ShareContent::url("https://example.com").with_title("Subject"));
        assert_eq!(
            plan,
            SharePlan::Email {
                body: Some("https://example.com".into()),
                subject: Some("Subject".into()),
                files: vec![],
            }
        );
    }

    #[test]
    fn files_route_to_email_with_url_in_body() {
        let plan = plan(
            &ShareContent::url("https://example.com").with_file("/tmp/a.txt"),
        );
        assert_eq!(
            plan,
            SharePlan::Email {
                body: Some("https://example.com".into()),
                subject: None,
                files: vec![PathBuf::from("/tmp/a.txt")],
            }
        );
    }

    #[test]
    fn files_only_route_to_email_with_no_body() {
        let plan = plan(&ShareContent::files(["/tmp/a.txt", "/tmp/b.txt"]));
        assert_eq!(
            plan,
            SharePlan::Email {
                body: None,
                subject: None,
                files: vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")],
            }
        );
    }

    /// A missing attachment is an honest `Backend` error, not a faked success.
    /// This drives `compose_email` far enough to hit the file-open failure
    /// *before* any D-Bus traffic, so it runs headlessly.
    #[tokio::test]
    async fn missing_attachment_is_backend_error() {
        let err = compose_email(None, None, &[PathBuf::from("/nonexistent/does-not-exist.bin")])
            .await
            .unwrap_err();
        match err {
            ShareError::Backend(msg) => assert!(msg.contains("does-not-exist.bin"), "got: {msg}"),
            other => panic!("expected Backend error, got {other:?}"),
        }
    }
}
