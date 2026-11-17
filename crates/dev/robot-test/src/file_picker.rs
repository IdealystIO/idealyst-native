//! `app.file_picker()`: hand the app files instead of driving the OS
//! picker — the `file-picker` SDK's `file_picker.*` controls, typed. The
//! app registers them once at boot (`file_picker::register_robot_controls()`).
//! Files are read here, on the test's machine, and sent over the bridge,
//! so this works with the app on a phone too.
//!
//! ```ignore
//! app.file_picker().queue_files(["tests/fixtures/template.pdf"]);
//! app.test_id("upload-template").click();   // the pick returns it, no dialog
//! app.file_picker().cancel_next();          // the next pick is dismissed
//! ```

use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::App;

/// The app's file picker, through the robot. From [`App::file_picker`].
pub struct FilePicker<'a> {
    pub(crate) app: &'a mut App,
}

impl FilePicker<'_> {
    fn queue(&mut self, files: Vec<Value>) -> &mut Self {
        self.app.call_function("file_picker.queue", json!({ "files": files }));
        self
    }

    /// The next pick returns these files from the test's disk (the mime
    /// type from each name's extension).
    pub fn queue_files<P: AsRef<Path>>(&mut self, paths: impl IntoIterator<Item = P>) -> &mut Self {
        let files = paths
            .into_iter()
            .map(|p| {
                let p = p.as_ref();
                let bytes = std::fs::read(p).unwrap_or_else(|e| panic!("file_picker: read {}: {e}", p.display()));
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                json!({ "name": name, "base64": base64::engine::general_purpose::STANDARD.encode(bytes) })
            })
            .collect();
        self.queue(files)
    }

    /// The next pick returns one file with these bytes.
    pub fn queue_bytes(&mut self, name: &str, mime: &str, bytes: &[u8]) -> &mut Self {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.queue(vec![json!({ "name": name, "mime": mime, "base64": b64 })])
    }

    /// The next pick is dismissed, as if the user cancelled.
    pub fn cancel_next(&mut self) -> &mut Self {
        self.app.call_function("file_picker.cancel_next", json!({}));
        self
    }

    /// The picks the app made, oldest first (`documents application/pdf`,
    /// `media images multiple`, …).
    pub fn requests(&mut self) -> Vec<String> {
        let v = self.app.call_function("file_picker.requests", json!({}));
        serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("file_picker.requests: {v}: {e}"))
    }

    /// Drop queued answers and the request log.
    pub fn clear(&mut self) -> &mut Self {
        self.app.call_function("file_picker.clear", json!({}));
        self
    }
}
