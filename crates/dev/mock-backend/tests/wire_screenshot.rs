//! The headline: a *mocked* wire command stream → a rasterized PNG, no
//! device, no window. Proves the full chain
//!
//!   real walker → WireRecordingBackend → wire::codec → WireBackend →
//!     WgpuBackend → offscreen GPU render → PNG
//!
//! works end-to-end — i.e. Robot / MCP can screenshot the app even when
//! it's only mocked.
//!
//! Needs a wgpu adapter (Metal on macOS; lavapipe on a GPU-less Linux
//! box). If none is available the tests **skip** rather than fail.
//!
//! Run with: `cargo test -p mock-backend --features screenshot`

#![cfg(feature = "screenshot")]

use std::rc::Rc;

use mock_backend::{screenshot_app, screenshot_commands};
use runtime_core::{
    Color, Element, IntoStyleSource, Length, SafeAreaSides, StyleApplication, StyleRules,
    StyleSheet, Tokenized,
};
use wire::{Command, NodeId};

fn colored_fill(hex: &'static str) -> Element {
    let sheet = Rc::new(StyleSheet::r#static({
        let mut r = StyleRules::default();
        r.width = Some(Tokenized::Literal(Length::Percent(100.0)));
        r.height = Some(Tokenized::Literal(Length::Percent(100.0)));
        r
    }));
    let style = StyleApplication::new(sheet).override_background(Color(hex.to_string()));
    Element::View {
        children: vec![],
        style: Some(style.into_style_source()),
        ref_fill: None,
        safe_area_sides: SafeAreaSides::NONE,
        on_touch: None,
        on_wheel: None,
        on_hover: None,
        is_container: false,
        accessibility: Default::default(),
        // `test_id` is present: this crate pins runtime-core/robot.
        test_id: None,
    }
}

fn png_center_is_bluish(png: &[u8], w: u32, h: u32) -> bool {
    let img = image::load_from_memory(png).expect("decode PNG").to_rgba8();
    let px = img.get_pixel(w / 2, h / 2).0;
    px[3] > 200 && px[2] as u16 > 120 && px[2] > px[0] && px[2] > px[1]
}

/// `true` if a wgpu adapter exists; otherwise the test should skip.
fn adapter_available() -> bool {
    match screenshot_commands(8, 8, vec![]) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("[wire-screenshot test] skipping — no wgpu adapter: {e}");
            false
        }
    }
}

#[test]
fn mocked_app_renders_to_png() {
    if !adapter_available() {
        return;
    }
    let (w, h) = (96u32, 64u32);
    let png = screenshot_app(w, h, || colored_fill("#2244dd")).expect("screenshot");
    assert!(
        png.starts_with(&[0x89, b'P', b'N', b'G']),
        "must be a PNG"
    );
    assert!(
        png_center_is_bluish(&png, w, h),
        "the mocked app's blue fill must rasterize blue-dominant at center"
    );
}

/// The real Robot/MCP path: register the `screenshot` verb against a
/// recorder, then invoke it exactly as the MCP server would (a bridge
/// `{"cmd":"screenshot","args":{...}}` command), and decode the PNG
/// from the JSON response. This is the end-to-end "Robot/MCP can
/// screenshot the mocked app" hookup.
#[test]
fn screenshot_verb_over_robot_bridge_returns_png() {
    use std::cell::RefCell;
    use std::rc::Rc;

    if !adapter_available() {
        return;
    }

    // A "mocked session": an app rendered into a recorder. (The
    // dev-server holds exactly this in runtime-server mode.) Hold the
    // owner so the scene stays mounted.
    let (w, h) = (80u32, 56u32);
    let recorder = dev_server::WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = runtime_core::mount(backend_rc, || colored_fill("#2244dd"));

    // Hook up the verb (the dev-server/host does this once).
    mock_backend::register_screenshot_command(recorder, (w, h));

    // Drive it the way the MCP server's `robot_call("screenshot", ...)`
    // does — through the bridge dispatch.
    let resp = runtime_core::robot::bridge::invoke_command(
        "screenshot",
        &serde_json::json!({ "width": w, "height": h }),
    )
    .expect("screenshot verb must dispatch");

    let parsed: serde_json::Value = serde_json::from_str(&resp).expect("JSON response");
    assert_eq!(parsed["width"], w);
    assert_eq!(parsed["height"], h);
    let b64 = parsed["png_base64"].as_str().expect("png_base64 field");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("valid base64");

    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "decoded payload must be a PNG");
    assert!(
        png_center_is_bluish(&png, w, h),
        "the mocked app's blue fill must rasterize blue-dominant through the bridge"
    );

    runtime_core::robot::bridge::unregister_command("screenshot");
}

#[test]
fn raw_wire_command_stream_renders_to_png() {
    if !adapter_available() {
        return;
    }
    // A hand-built command stream — exactly the shape the dev-server
    // would broadcast. Proves we screenshot from the wire bytes, not
    // from a live app object.
    let (w, h) = (64u32, 64u32);
    let root = NodeId(1);
    let commands = vec![
        Command::CreateView {
            id: root,
            a11y: Default::default(),
        },
        Command::Finish { root },
    ];
    let png = screenshot_commands(w, h, commands).expect("screenshot from wire");
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "must be a PNG");
    // No style on the bare view, so we only assert it produced a
    // well-formed image of the right size (the renderer ran and read
    // back). Decode + dimension check is the guard.
    let img = image::load_from_memory(&png).expect("decode PNG");
    assert_eq!((img.width(), img.height()), (w, h));
}
