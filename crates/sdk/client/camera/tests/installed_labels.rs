//! An app installs its own names for unnamed cameras once, process-wide.

use camera::{camera_labels, install_camera_labels, CameraLabels};

#[test]
fn installed_camera_labels_are_read_back() {
    assert_eq!(*camera_labels(), CameraLabels::english());
    let fr = CameraLabels { front: "Caméra avant".into(), back: "Caméra arrière".into(), camera: "Caméra".into() };
    install_camera_labels(fr.clone());
    assert_eq!(*camera_labels(), fr);
}
