//! Fallback backend for targets without a save UI. Reports
//! [`ExportError::Unsupported`] so author code degrades predictably.

use crate::{ExportError, SaveOutcome, SaveRequest};

pub(crate) async fn save(_request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    Err(ExportError::Unsupported)
}
