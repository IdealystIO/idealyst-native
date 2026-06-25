//! Fallback share backend for targets with no uniform native share surface
//! (Windows, Linux, and any other non-mobile / non-web target).
//!
//! Windows has the Data Transfer Manager (`Windows.ApplicationModel.DataTransfer`)
//! and Linux has nothing universal (no portal for *outbound* share at the time
//! of writing). Rather than silently no-op — which would make a share button
//! look broken — we return [`ShareError::NotSupported`] so the caller can hide
//! or relabel the affordance on these targets. A real Windows backend is a
//! later layer; this is an honest "not here yet", not a degraded share.

use crate::{ShareContent, ShareError, ShareOutcome};

pub(crate) async fn share(_content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    Err(ShareError::NotSupported)
}
