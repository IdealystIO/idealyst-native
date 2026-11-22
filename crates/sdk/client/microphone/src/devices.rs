//! Platform-agnostic shaping of the input list every backend returns.

use crate::MicDevice;

/// Give each repeated label a number ("USB Audio", "USB Audio 2", …) so a
/// picker never shows two identical rows — two of the same headset model, or
/// the several built-in microphones an Android phone reports. The first keeps
/// its name; order is the platform's.
pub(crate) fn number_repeated_labels(mut devices: Vec<MicDevice>) -> Vec<MicDevice> {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for d in &mut devices {
        match seen.iter_mut().find(|(label, _)| *label == d.label) {
            Some((_, n)) => {
                *n += 1;
                d.label = format!("{} {}", d.label, n);
            }
            None => seen.push((d.label.clone(), 1)),
        }
    }
    devices
}

/// cpal identifies an input only by its name, and two identical devices share
/// one. Ids are therefore the names, with a repeat suffixed by its ordinal
/// (`"USB Audio"`, `"USB Audio#2"`) so each id names exactly one input. The
/// backend lists and opens through this same mapping, so an id from
/// `devices()` reopens the same input while the device set is unchanged.
#[cfg(any(test, not(any(target_arch = "wasm32", target_os = "android"))))]
pub(crate) fn ids_for_names(names: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        let ordinal = names[..i].iter().filter(|n| *n == name).count() + 1;
        out.push(if ordinal == 1 { name.clone() } else { format!("{name}#{ordinal}") });
    }
    out
}

// `AudioDeviceInfo.TYPE_*` values for the inputs an `AudioRecord` from
// `AudioSource.MIC` can capture (stable platform constants).
#[cfg(any(test, target_os = "android"))]
mod android_type {
    pub const WIRED_HEADSET: i32 = 3;
    pub const LINE_ANALOG: i32 = 5;
    pub const LINE_DIGITAL: i32 = 6;
    pub const BLUETOOTH_SCO: i32 = 7;
    pub const USB_DEVICE: i32 = 11;
    pub const USB_ACCESSORY: i32 = 12;
    pub const DOCK: i32 = 13;
    pub const BUILTIN_MIC: i32 = 15;
    pub const USB_HEADSET: i32 = 22;
    pub const BLE_HEADSET: i32 = 26;
}

/// The picker label for an Android input of `AudioDeviceInfo` type `kind`, or
/// `None` for inputs a microphone app can't record from (the telephony
/// uplink, FM / TV tuners, remote submix, echo reference). Built-in and wired
/// inputs are named by kind: their `getProductName()` is the phone's model.
/// Bluetooth and USB inputs use the product name, which names the accessory.
#[cfg(any(test, target_os = "android"))]
pub(crate) fn android_input_label(labels: &crate::MicLabels, kind: i32, product: &str) -> Option<String> {
    use android_type::*;
    let named = |fallback: &str| {
        let p = product.trim();
        Some(if p.is_empty() { fallback.to_string() } else { p.to_string() })
    };
    match kind {
        BUILTIN_MIC => Some(labels.built_in.clone()),
        WIRED_HEADSET => Some(labels.wired_headset.clone()),
        LINE_ANALOG | LINE_DIGITAL => Some(labels.line_in.clone()),
        DOCK => Some(labels.dock.clone()),
        BLUETOOTH_SCO | BLE_HEADSET => named(&labels.bluetooth_headset),
        USB_DEVICE | USB_ACCESSORY | USB_HEADSET => named(&labels.usb_microphone),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, label: &str) -> MicDevice {
        MicDevice { id: id.into(), label: label.into() }
    }

    #[test]
    fn repeated_labels_are_numbered_in_order() {
        let out = number_repeated_labels(vec![
            dev("a", "USB Audio"),
            dev("b", "MacBook Pro Microphone"),
            dev("c", "USB Audio"),
        ]);
        let labels: Vec<_> = out.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, ["USB Audio", "MacBook Pro Microphone", "USB Audio 2"]);
    }

    #[test]
    fn identical_cpal_names_get_distinct_ids() {
        let names: Vec<String> = ["USB Audio", "Mic", "USB Audio", "USB Audio"].iter().map(|s| s.to_string()).collect();
        assert_eq!(ids_for_names(&names), ["USB Audio", "Mic", "USB Audio#2", "USB Audio#3"]);
    }

    #[test]
    fn android_inputs_are_labelled_by_kind_or_product() {
        let en = crate::MicLabels::english();
        assert_eq!(android_input_label(&en, 15, "Pixel 8").as_deref(), Some("Built-in microphone"));
        assert_eq!(android_input_label(&en, 3, "Pixel 8").as_deref(), Some("Wired headset"));
        assert_eq!(android_input_label(&en, 7, "WH-1000XM5").as_deref(), Some("WH-1000XM5"));
        assert_eq!(android_input_label(&en, 7, " ").as_deref(), Some("Bluetooth headset"));
        assert_eq!(android_input_label(&en, 22, "").as_deref(), Some("USB microphone"));
        // Telephony uplink (18), FM tuner (16), remote submix (25): not microphones.
        for kind in [16, 18, 25, 28] {
            assert_eq!(android_input_label(&en, kind, "x"), None, "type {kind} is not listed");
        }
    }

    /// A user of the framework could not translate the names the SDK
    /// gives unnamed inputs. They come from the installed labels.
    #[test]
    fn regression_unnamed_inputs_take_the_installed_names() {
        let fr = crate::MicLabels {
            built_in: "Micro intégré".into(),
            bluetooth_headset: "Casque Bluetooth".into(),
            ..crate::MicLabels::english()
        };
        assert_eq!(android_input_label(&fr, 15, "Pixel 8").as_deref(), Some("Micro intégré"));
        assert_eq!(android_input_label(&fr, 7, "").as_deref(), Some("Casque Bluetooth"));
        assert_eq!(android_input_label(&fr, 7, "WH-1000XM5").as_deref(), Some("WH-1000XM5"), "a product name stays");
    }
}
