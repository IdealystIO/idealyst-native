//! Platform-agnostic shaping of the camera list every backend returns.

use crate::{CameraDevice, DeviceFacing};

/// The label for a camera the platform gives no name: Android names none,
/// and a browser may leave one blank. Repeats are numbered afterwards by
/// [`number_repeated_labels`].
#[cfg_attr(not(any(test, target_arch = "wasm32", target_os = "android")), allow(dead_code))]
pub(crate) fn unnamed_label(facing: DeviceFacing) -> &'static str {
    match facing {
        DeviceFacing::Front => "Front camera",
        DeviceFacing::Back => "Back camera",
        DeviceFacing::Unreported => "Camera",
    }
}

// CameraCharacteristics.LENS_FACING values, as the Android shim's
// `listCameras` reports them.
#[cfg(any(test, target_os = "android"))]
const LENS_FACING_FRONT: i32 = 0;
#[cfg(any(test, target_os = "android"))]
const LENS_FACING_BACK: i32 = 1;

/// Pair up the Android shim's `listCameras` output (`[id, lensFacing, …]`,
/// `lensFacing` a decimal `LENS_FACING`, `-1` when unreported) into devices.
/// Camera2 names no cameras, so each label comes from its facing.
#[cfg(any(test, target_os = "android"))]
pub(crate) fn from_camera2_list(flat: &[String]) -> Vec<CameraDevice> {
    flat.chunks_exact(2)
        .map(|pair| {
            let facing = match pair[1].parse::<i32>() {
                Ok(LENS_FACING_FRONT) => DeviceFacing::Front,
                Ok(LENS_FACING_BACK) => DeviceFacing::Back,
                _ => DeviceFacing::Unreported,
            };
            CameraDevice { id: pair[0].clone(), label: unnamed_label(facing).into(), facing }
        })
        .collect()
}

/// Give each repeated label a number ("USB Camera", "USB Camera 2", …) so a
/// picker never shows two identical rows — two of the same webcam model, or
/// the several back cameras an Android phone reports. The first keeps its
/// name; order is the platform's.
pub(crate) fn number_repeated_labels(mut devices: Vec<CameraDevice>) -> Vec<CameraDevice> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, label: &str) -> CameraDevice {
        CameraDevice { id: id.into(), label: label.into(), facing: DeviceFacing::Unreported }
    }

    #[test]
    fn camera2_list_pairs_ids_with_facing_labels() {
        // LENS_FACING: 1 = back, 0 = front, 2 = external, -1 = unreported.
        let flat: Vec<String> = ["0", "1", "1", "0", "5", "1", "7", "2", "9", "-1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = number_repeated_labels(from_camera2_list(&flat));
        let got: Vec<_> = out.iter().map(|d| (d.id.as_str(), d.label.as_str(), d.facing)).collect();
        assert_eq!(
            got,
            [
                ("0", "Back camera", DeviceFacing::Back),
                ("1", "Front camera", DeviceFacing::Front),
                ("5", "Back camera 2", DeviceFacing::Back),
                ("7", "Camera", DeviceFacing::Unreported),
                ("9", "Camera 2", DeviceFacing::Unreported),
            ]
        );
    }

    #[test]
    fn repeated_labels_are_numbered_in_order() {
        let out = number_repeated_labels(vec![
            dev("a", "USB Camera"),
            dev("b", "FaceTime HD Camera"),
            dev("c", "USB Camera"),
            dev("d", "USB Camera"),
        ]);
        let labels: Vec<_> = out.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, ["USB Camera", "FaceTime HD Camera", "USB Camera 2", "USB Camera 3"]);
        let ids: Vec<_> = out.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"], "ids and order are untouched");
    }
}
