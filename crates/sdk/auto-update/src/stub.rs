//! Fallback backend for targets with no self-update path (Android, and any
//! other target not covered by a dedicated backend). Reports
//! [`InstallKind::Unknown`] so the [`Updater`] resolves to
//! [`UpdateState::Unsupported`](crate::UpdateState::Unsupported) and author
//! code degrades predictably — on mobile, updates come from the app store.

use crate::{InstallKind, PreparedUpdate, UpdateError};

pub(crate) fn install_kind() -> InstallKind {
    InstallKind::Unknown
}

pub(crate) async fn apply(_prepared: &PreparedUpdate) -> Result<(), UpdateError> {
    Err(UpdateError::Unsupported)
}

pub(crate) fn relaunch() -> Result<(), UpdateError> {
    Err(UpdateError::Unsupported)
}
