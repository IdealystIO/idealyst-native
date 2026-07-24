//! Fallback share backend for targets with no uniform native share surface
//! (Windows and any other non-mobile / non-web / non-Linux target).
//!
//! Windows has the Data Transfer Manager
//! (`Windows.ApplicationModel.DataTransfer`) but no backend for it yet. Rather
//! than silently no-op — which would make a share button look broken — we
//! return [`ShareError::NotSupported`] so the caller can hide or relabel the
//! affordance on these targets. A real Windows backend is a later layer; this
//! is an honest "not here yet", not a degraded share.
//!
//! Linux is *no longer* a stub target: it drives the XDG desktop portal via
//! `ashpd` (see `linux.rs`).

use crate::{ShareContent, ShareError, ShareOutcome};

pub(crate) async fn share(_content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    Err(ShareError::NotSupported)
}
