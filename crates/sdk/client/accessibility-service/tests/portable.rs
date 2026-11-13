//! Off Android every call fails with `Error::Unsupported` instead of
//! panicking, and `is_enabled` is `false` — the contract that lets an app
//! call the SDK from shared code.

#![cfg(not(target_os = "android"))]

use accessibility_service::*;

fn unsupported<T: std::fmt::Debug>(r: Result<T>) {
    match r {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("accessibility_service::"), "{msg}"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn every_call_is_unsupported_off_android() {
    assert!(!is_enabled());
    unsupported(open_settings());
    unsupported(global_action(GlobalAction::Back));
    unsupported(set_focused_text("hello"));
    unsupported(type_text("hello"));
    unsupported(press_key(Key::Enter));
    unsupported(pollster::block_on(screenshot(ImageFormat::Rgba)));
    unsupported(pollster::block_on(screenshot(ImageFormat::Jpeg { quality: 70 })));
    unsupported(pollster::block_on(tap(10.0, 10.0)));
    unsupported(pollster::block_on(long_press(10.0, 10.0, 600)));
    unsupported(pollster::block_on(swipe((10.0, 10.0), (10.0, 400.0), 300)));
    let q = Query::new().text("Start now");
    unsupported(pollster::block_on(find(&q)));
    unsupported(pollster::block_on(wait_for(&q, std::time::Duration::from_secs(1))));
    unsupported(pollster::block_on(windows()));
}

#[test]
fn bad_arguments_are_rejected_before_the_platform_on_every_target() {
    let invalid = |r: Result<()>| assert!(matches!(r, Err(Error::InvalidArgument(_))), "{r:?}");
    invalid(pollster::block_on(tap(-1.0, 10.0)));
    invalid(pollster::block_on(long_press(10.0, 10.0, 0)));
    invalid(pollster::block_on(swipe((0.0, 0.0), (f32::NAN, 1.0), 300)));
    invalid(pollster::block_on(swipe((0.0, 0.0), (1.0, 1.0), 60_001)));
    assert!(matches!(
        pollster::block_on(screenshot(ImageFormat::Jpeg { quality: 101 })),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn an_enabled_listener_is_kept_and_never_called_off_android() {
    let sub = on_enabled_change(|_| panic!("no platform reports changes here"));
    drop(sub);
}

#[test]
fn a_window_listener_is_kept_and_never_called_off_android() {
    let sub = on_window_change(|_| panic!("no platform reports changes here"));
    drop(sub);
}
