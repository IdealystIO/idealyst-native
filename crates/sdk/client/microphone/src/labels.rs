//! The names this crate gives an input the platform leaves unnamed.

use std::sync::{Arc, RwLock};

/// What an input is called when the platform names none. Android names
/// built-in and wired inputs by kind (their product name is the phone's
/// model), and a browser can leave a label blank. English until the app
/// installs its own with [`install_mic_labels`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicLabels {
    /// The device's own microphone.
    pub built_in: String,
    /// A headset on the jack.
    pub wired_headset: String,
    /// An analog or digital line input.
    pub line_in: String,
    /// A dock's input.
    pub dock: String,
    /// A Bluetooth headset with no product name.
    pub bluetooth_headset: String,
    /// A USB input with no product name.
    pub usb_microphone: String,
    /// Any other input the platform leaves unnamed.
    pub microphone: String,
}

impl MicLabels {
    /// The English names.
    pub fn english() -> Self {
        Self {
            built_in: "Built-in microphone".into(),
            wired_headset: "Wired headset".into(),
            line_in: "Line in".into(),
            dock: "Dock".into(),
            bluetooth_headset: "Bluetooth headset".into(),
            usb_microphone: "USB microphone".into(),
            microphone: "Microphone".into(),
        }
    }
}

impl Default for MicLabels {
    fn default() -> Self {
        Self::english()
    }
}

// Process-wide rather than per-thread: a backend may list its inputs on a
// worker thread.
static INSTALLED: RwLock<Option<Arc<MicLabels>>> = RwLock::new(None);

/// Make `labels` the names [`Microphone::devices`](crate::Microphone::devices)
/// gives unnamed inputs from now on.
pub fn install_mic_labels(labels: MicLabels) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(labels));
}

/// The installed names, or [`MicLabels::english`].
pub fn mic_labels() -> Arc<MicLabels> {
    INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(MicLabels::english()))
}
