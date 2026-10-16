//! The Windows `Video` plays through Media Foundation, not the placeholder.
//!
//! ## The bug this pins
//!
//! `register` had no Windows arm, so every `Video` on Windows mounted the
//! External "not supported" placeholder, and its handle's ops were the
//! no-op `UnsupportedOps` (duration always 0).
//!
//! Checked on a real HWND: a mounted URL source is opened by the Media
//! Engine, whose metadata reaches the handle (a positive `duration()`) and
//! whose `seek` moves `position()`. Needs network access (the test plays
//! W3C's public trailer, as the sdk-gallery does) — the Windows VM has it.
//! The pixels (frames in the swapchain, the painted controls) are checked
//! by hand in `examples/sdk-gallery`.
#![cfg(windows)]

use std::rc::Rc;
use std::time::{Duration, Instant};

use backend_windows::{newcore, WindowsBackend};
use runtime_shared::Ref;
use runtime_vocabulary::glue::{self, IntoElement};
use video::{VideoBind, VideoHandle, VideoProps};
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE, WS_POPUP, WS_VISIBLE};

const CLIP: &str = "https://media.w3.org/2010/05/sintel/trailer.mp4";

#[test]
fn regression_windows_video_opens_the_url_in_media_foundation() {
    let host = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("video-test"), WS_POPUP | WS_VISIBLE, 0, 0, 640, 480, None, None, None, None)
    }
    .expect("host window");
    let backend = Rc::new(std::cell::RefCell::new(WindowsBackend::new(host)));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let handle: Ref<VideoHandle> = Ref::new();
    let h = handle.clone();
    let app = newcore::start(backend.clone(), video::register, move || {
        glue::view(vec![newcore::SceneElement::from(
            video::Video(VideoProps { source: video::url(CLIP), ..Default::default() }).bind(h.clone()),
        )])
        .into_element()
    });

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut duration = 0.0;
    while Instant::now() < deadline {
        duration = handle.with(|v| v.duration()).unwrap_or(0.0);
        if duration > 0.0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(duration > 30.0, "the trailer's duration reaches the handle (got {duration})");

    handle.with(|v| v.seek(12.0));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut position = 0.0;
    while Instant::now() < deadline {
        position = handle.with(|v| v.position()).unwrap_or(0.0);
        if (position - 12.0).abs() < 1.0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!((position - 12.0).abs() < 1.0, "seek moves the position (got {position})");
    app.stop();
}
