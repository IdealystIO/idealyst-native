//! Regression: the dev-side recorder must implement
//! `Backend::create_virtualizer` — as a TRAIT method, not an inherent
//! one.
//!
//! A nav-era refactor left `create_virtualizer` /
//! `virtualizer_data_changed` as inherent methods on
//! `WireRecordingBackend`, which silently shadowed nothing: the walker
//! calls through the `Backend` trait, whose default body is
//! `unimplemented!()`. Net effect: any screen containing a `flat_list`
//! panicked the dev session thread the moment it was built under
//! `idealyst dev` (the website's Primitives page took the whole
//! sidecar session down). This mounts such a screen on the real
//! recorder and asserts the wire command comes out instead.

use runtime_core::primitives::flat_list::{fixed_size, flat_list};
use runtime_core::{signal, Element};

fn list_screen() -> Element {
    let data = signal(vec![
        (1u32, "Aria".to_string()),
        (2, "Bram".to_string()),
        (3, "Cleo".to_string()),
    ]);
    // `S` is a phantom sizing-strategy marker on `flat_list` — unused
    // at runtime, uninferable here, so pin it to `()`.
    let list = flat_list::<_, _, (), _>(
        data,
        |_idx, item: &(u32, String)| item.0 as u64,
        fixed_size(36.0),
        |_idx, item: &(u32, String)| Element::Text {
            source: runtime_core::TextSource::Static(item.1.clone()),
            style: None,
            ref_fill: None,
            accessibility: Default::default(),
            test_id: None,
        },
    );
    Element::View {
        children: vec![list.into()],
        style: None,
        ref_fill: None,
        safe_area_sides: runtime_core::SafeAreaSides::NONE,
        on_touch: None,
        on_wheel: None,
        preserves_focus: false,
        on_hover: None,
        on_file_drop: None,
        is_container: false,
        accessibility: Default::default(),
        test_id: None,
    }
}

#[test]
fn regression_flat_list_on_recorder_emits_create_virtualizer_instead_of_panicking() {
    let recorder = dev_server::WireRecordingBackend::new();
    let backend_rc = std::rc::Rc::new(std::cell::RefCell::new(recorder.clone()));
    // Before the fix this mount panicked:
    // "create_virtualizer not implemented for this backend".
    let _owner = runtime_core::mount(backend_rc, list_screen);

    let cmds = recorder.drain_commands();
    let create = cmds.iter().find_map(|c| match c {
        wire::Command::CreateVirtualizer { initial_keys, initial_size, .. } => {
            Some((initial_keys.clone(), initial_size.sizes.clone()))
        }
        _ => None,
    });
    let (keys, sizes) = create.expect("no CreateVirtualizer command in the recorded stream");
    assert_eq!(keys, vec![1, 2, 3], "initial keys should snapshot the data set");
    assert_eq!(sizes, vec![36.0, 36.0, 36.0], "initial sizes should snapshot fixed_size");
}
