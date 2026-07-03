//! Linux save via the desktop **portal** (`xdg-desktop-portal`)
//! `FileChooser.SaveFile` — the sandbox-friendly, compositor-native save
//! dialog, driven through `ashpd`. Same portal posture `screen-recorder`'s
//! Linux capture takes.
//!
//! VERIFICATION: type-checked for `x86_64-unknown-linux-gnu`; the D-Bus portal
//! exchange resolves only at runtime against a running portal service.

use ashpd::desktop::file_chooser::SelectedFiles;

use crate::{ExportError, SaveOutcome, SaveRequest, Source};

pub(crate) async fn save(request: SaveRequest) -> Result<SaveOutcome, ExportError> {
    let bytes = match request.source {
        Source::Bytes(b) => b,
        Source::Path(p) => std::fs::read(&p).map_err(|e| ExportError::Io(e.to_string()))?,
    };

    let response = SelectedFiles::save_file()
        .title("Save File")
        .current_name(request.suggested_name.as_str())
        .send()
        .await
        .map_err(|e| ExportError::Backend(format!("portal request: {e}")))?
        .response();

    let files = match response {
        Ok(files) => files,
        // The user dismissing the dialog comes back as a cancelled response.
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
            return Ok(SaveOutcome::Cancelled)
        }
        Err(e) => return Err(ExportError::Backend(format!("portal response: {e}"))),
    };

    let uri = match files.uris().first() {
        Some(u) => u.clone(),
        None => return Ok(SaveOutcome::Cancelled),
    };
    let path = uri
        .to_file_path()
        .map_err(|_| ExportError::Backend(format!("portal returned non-file URI: {uri}")))?;

    std::fs::write(&path, &bytes).map_err(|e| ExportError::Io(e.to_string()))?;
    Ok(SaveOutcome::Saved {
        location: Some(path.display().to_string()),
    })
}
