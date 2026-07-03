//! Web backend — a no-op. On the web a page reload *is* the update: the next
//! visit fetches the current build. There is nothing for the app to download
//! or swap, so [`install_kind`] reports [`InstallKind::Web`] and the [`Updater`]
//! short-circuits to [`UpdateState::Unsupported`](crate::UpdateState::Unsupported)
//! without ever hitting [`apply`].

use crate::{InstallKind, PreparedUpdate, UpdateError};

pub(crate) fn install_kind() -> InstallKind {
    InstallKind::Web
}

pub(crate) async fn apply(_prepared: &PreparedUpdate) -> Result<(), UpdateError> {
    // Unreachable in practice — `Updater::check` never stages an update on web —
    // but keep the seam total.
    Err(UpdateError::Unsupported)
}

pub(crate) fn relaunch() -> Result<(), UpdateError> {
    Err(UpdateError::Unsupported)
}
