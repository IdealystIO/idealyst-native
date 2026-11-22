//! The words this crate puts in a native file dialog the app did not
//! pass.

use std::sync::{Arc, RwLock};

/// A file dialog's own words: the Linux portal's title, the filter
/// names Linux and Windows list. English until the app installs its own
/// with [`install_picker_labels`]. macOS, iOS, Android and the web name
/// their dialogs and filters themselves.
#[derive(Clone)]
pub struct PickerLabels {
    /// The Linux portal dialog's title.
    pub title: String,
    /// The Linux filter that holds every type the request allows.
    pub files: String,
    /// The Windows filter that matches every file.
    pub all_files: String,
    /// The Windows filter for one requested type, from its lower-cased
    /// MIME type (`"image/png"`, `"image/*"`). `None` leaves that type out
    /// of the list (the "All files" filter still matches it).
    pub type_name: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl PickerLabels {
    /// The English words.
    pub fn english() -> Self {
        Self {
            title: "Open File".into(),
            files: "Files".into(),
            all_files: "All files".into(),
            type_name: Arc::new(|mime| english_type_name(mime).map(str::to_string)),
        }
    }
}

impl Default for PickerLabels {
    fn default() -> Self {
        Self::english()
    }
}

/// The English name of each type the Windows filter table knows.
fn english_type_name(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/*" => "Images",
        "video/*" => "Videos",
        "audio/*" => "Audio",
        "text/*" | "text/plain" => "Text",
        "application/pdf" => "PDF",
        "image/png" => "PNG image",
        "image/jpeg" => "JPEG image",
        "image/gif" => "GIF image",
        "image/heic" => "HEIC image",
        "image/webp" => "WebP image",
        "video/mp4" => "MP4 video",
        "video/quicktime" => "QuickTime video",
        "audio/mpeg" => "MP3 audio",
        "text/csv" => "CSV",
        "text/html" => "HTML",
        "text/markdown" => "Markdown",
        "application/json" => "JSON",
        "application/zip" => "ZIP archive",
        _ => return None,
    })
}

// Process-wide rather than per-thread: a backend may open its dialog on a
// worker thread.
static INSTALLED: RwLock<Option<Arc<PickerLabels>>> = RwLock::new(None);

/// Make `labels` the words every file dialog shows from now on.
pub fn install_picker_labels(labels: PickerLabels) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(labels));
}

/// The installed words, or [`PickerLabels::english`].
pub fn picker_labels() -> Arc<PickerLabels> {
    INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(PickerLabels::english()))
}
