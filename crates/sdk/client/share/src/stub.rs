//! Fallback share backend for targets with no native share surface (not web,
//! Android, Apple, Linux or Windows). Rather than silently no-op — which
//! would make a share button look broken — it returns
//! [`ShareError::NotSupported`] so the caller can hide or relabel the
//! affordance.

use crate::{ShareContent, ShareError, ShareOutcome};

pub(crate) async fn share(_content: &ShareContent) -> Result<ShareOutcome, ShareError> {
    Err(ShareError::NotSupported)
}
