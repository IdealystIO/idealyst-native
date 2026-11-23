//! The words the platform shows for a hold.

use std::sync::{Arc, RwLock};

/// The words a desktop can show beside the app while the display is held on
/// (KDE's power applet, `powercfg /requests`, `pmset -g assertions`).
/// English until the app installs its own with [`install_keep_awake_labels`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepAwakeLabels {
    /// Why the display is being kept on.
    pub reason: String,
}

impl KeepAwakeLabels {
    /// The English words.
    pub fn english() -> Self {
        Self { reason: "Keeping the screen on".into() }
    }
}

impl Default for KeepAwakeLabels {
    fn default() -> Self {
        Self::english()
    }
}

// Process-wide rather than per-thread: Linux asks from a worker thread.
static INSTALLED: RwLock<Option<Arc<KeepAwakeLabels>>> = RwLock::new(None);

/// Make `labels` the words given to the platform from the next time the
/// display is held on.
pub fn install_keep_awake_labels(labels: KeepAwakeLabels) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(labels));
}

/// The installed words, or [`KeepAwakeLabels::english`].
pub fn keep_awake_labels() -> Arc<KeepAwakeLabels> {
    INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(KeepAwakeLabels::english()))
}
