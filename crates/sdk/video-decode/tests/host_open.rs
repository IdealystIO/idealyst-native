//! Host reproduction of the macOS decode path. Opens a real local video and
//! exercises `open()` + the transport getters (the struct-returning `msg_send`s)
//! so an ABI/selector bug surfaces here with a backtrace, off the GUI.
//!
//! Run: `VIDEO_DECODE_TEST_FILE=/path/clip.mp4 cargo test -p video-decode --test host_open -- --nocapture`

#![cfg(target_os = "macos")]

#[test]
fn open_local_video() {
    let path = std::env::var("VIDEO_DECODE_TEST_FILE")
        .unwrap_or_else(|_| "/Users/nicho/Desktop/sample.mp4".to_string());
    if !std::path::Path::new(&path).exists() {
        eprintln!("skipping: no test video at {path} (set VIDEO_DECODE_TEST_FILE)");
        return;
    }
    let url = format!("file://{path}");
    eprintln!("opening {url}");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let clip = rt
        .block_on(async {
            video_decode::VideoDecoder::new()
                .open(
                    video_decode::DecodeSource::Url(url),
                    video_decode::DecodeConfig { max_dimension: Some(512), ..Default::default() },
                )
                .await
        })
        .expect("open should succeed");

    eprintln!("natural_size = {:?}", clip.natural_size());
    eprintln!("has_audio    = {}", clip.audio().is_some());

    let t = clip.transport();
    eprintln!("duration  = {}", t.duration());
    eprintln!("position  = {}", t.position());
    eprintln!("playing   = {}", t.is_playing());
    t.play();
    t.set_muted(true);
    t.seek(1.0);
    eprintln!("after seek position = {}", t.position());
    t.pause();
    eprintln!("ok");
}

/// Exercises the FRAME PUMP (`copyPixelBufferForItemTime:` etc.) under a real
/// run loop — the path `open_local_video` can't reach (no run loop), and the
/// suspected in-app crash site.
#[test]
fn pull_first_frame() {
    let path = std::env::var("VIDEO_DECODE_TEST_FILE")
        .unwrap_or_else(|_| "/Users/nicho/Desktop/sample.mp4".to_string());
    if !std::path::Path::new(&path).exists() {
        eprintln!("skipping: no test video at {path}");
        return;
    }
    let url = format!("file://{path}");
    eprintln!("pump opening {url}");
    match video_decode::debug_pull_first_frame(&url, Some(512)) {
        Ok((w, h)) => eprintln!("PUMP OK: first frame {w}x{h}"),
        Err(e) => eprintln!("PUMP no frame: {e}"),
    }
}
