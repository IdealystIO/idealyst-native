//! Native `"screenshot"` bridge verb — captures the backend's **real
//! rendered surface** (what the device is actually drawing) and returns
//! it as base64 PNG.
//!
//! This is the on-device counterpart to the headless wgpu-replay
//! `"screenshot"` verb that `headless-screenshot` registers for mocked
//! dev-server sessions. Both answer the same `{"cmd":"screenshot"}`
//! request with the same `{"png_base64","width","height"}` payload, but
//! this one snapshots native widgets / fonts / the live view hierarchy
//! instead of re-rendering the scene model through wgpu.
//!
//! [`crate::mount`] registers this automatically when the backend reports
//! [`Backend::supports_screenshot`](crate::Backend::supports_screenshot)
//! — so a backend that can't capture natively (or a `MockBackend`) leaves
//! the replay verb untouched rather than shadowing it with an
//! "unsupported" handler.

use crate::backend::Screenshot;

/// The capture entry point the bridge handler drives. Boxed callback so
/// async backends share the signature; native backends fire it inline.
/// Mirrors [`Backend::capture_screenshot`](crate::Backend::capture_screenshot)
/// minus the `&self`, so [`crate::mount`] can hand us a closure that
/// borrows the backend without leaking the backend's generic type into
/// the bridge registry.
pub type CaptureFn = dyn Fn(Box<dyn FnOnce(Result<Screenshot, String>)>);

/// Register the live native `"screenshot"` verb on the Robot bridge.
///
/// Must be called on the bridge-poll (UI) thread, like every
/// [`bridge::register_command`](crate::robot::bridge::register_command) —
/// the handler runs there and the supplied `capture` typically reaches
/// into the UI-thread-owned backend.
pub fn register_native_screenshot(capture: impl Fn(Box<dyn FnOnce(Result<Screenshot, String>)>) + 'static) {
    crate::robot::bridge::register_command("screenshot", move |_args| {
        // Native backends invoke the callback synchronously, so stash the
        // delivered value and read it back after `capture` returns. If a
        // backend ever completes asynchronously it lands here as `None`
        // and we surface a clear error rather than blocking the UI thread.
        let slot: std::rc::Rc<std::cell::RefCell<Option<Result<Screenshot, String>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let sink = slot.clone();
        capture(Box::new(move |res| {
            *sink.borrow_mut() = Some(res);
        }));
        let captured = slot.borrow_mut().take().ok_or_else(|| {
            "capture_screenshot did not complete synchronously (async capture \
             is not yet supported over the bridge)"
                .to_string()
        })?;
        encode_screenshot_response(captured?)
    });
}

/// Encode a captured frame as the bridge's `ok` payload:
/// `{"png_base64": "...", "width": W, "height": H}`. Factored out so the
/// base64 + JSON contract is unit-testable without a backend or a live
/// bridge.
pub(crate) fn encode_screenshot_response(shot: Screenshot) -> Result<String, String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&shot.png);
    serde_json::to_string(&serde_json::json!({
        "png_base64": b64,
        "width": shot.width,
        "height": shot.height,
    }))
    .map_err(|e| format!("screenshot response encode failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::robot::bridge;

    /// Minimal valid 1×1 PNG (8-byte signature + IHDR + IDAT + IEND).
    /// Enough to assert the bytes survive the base64 round-trip with the
    /// PNG magic intact.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
        0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R', // IHDR len + type
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89,
    ];

    #[test]
    fn encode_response_round_trips_png_bytes() {
        let payload = encode_screenshot_response(Screenshot {
            png: TINY_PNG.to_vec(),
            width: 1,
            height: 1,
        })
        .expect("encode");
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["width"], 1);
        assert_eq!(v["height"], 1);
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(v["png_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, TINY_PNG);
        assert_eq!(&decoded[..8], &TINY_PNG[..8], "PNG signature preserved");
    }

    // Regression: the registered "screenshot" verb must drive the
    // supplied capture closure and return the encoded PNG synchronously,
    // exactly as the bridge poll loop invokes it. Before this feature the
    // verb didn't exist on native backends at all.
    #[test]
    fn registered_verb_captures_and_encodes() {
        register_native_screenshot(|done| {
            done(Ok(Screenshot {
                png: TINY_PNG.to_vec(),
                width: 1,
                height: 1,
            }));
        });
        let out = bridge::invoke_command("screenshot", &serde_json::json!({}))
            .expect("verb should succeed");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(v["png_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(&decoded[..8], &TINY_PNG[..8]);
        bridge::unregister_command("screenshot");
    }

    // A capture that fails (e.g. no host root yet) surfaces as the verb's
    // `err`, not a panic or a silent empty payload.
    #[test]
    fn capture_error_propagates_to_verb() {
        register_native_screenshot(|done| done(Err("no host root".into())));
        let err = bridge::invoke_command("screenshot", &serde_json::json!({}))
            .expect_err("verb should report the capture error");
        assert!(err.contains("no host root"), "got: {err}");
        bridge::unregister_command("screenshot");
    }

    // True end-to-end over a real TCP socket — the exact path an MCP
    // client / the capture.py helper exercises: bind an ephemeral bridge
    // port, connect, send `{"cmd":"screenshot"}`, and read the PNG back
    // off the wire. The in-process `invoke_command` tests above skip the
    // socket; this one doesn't.
    #[test]
    fn screenshot_verb_over_real_tcp_socket() {
        use base64::Engine as _;
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpStream;
        use std::sync::mpsc;
        use std::time::Duration;

        // Register on THIS thread: the custom-verb registry and the poll
        // loop are thread-local, and we pump the handle on this thread.
        register_native_screenshot(|done| {
            done(Ok(Screenshot {
                png: TINY_PNG.to_vec(),
                width: 1,
                height: 1,
            }));
        });

        let (handle, port) = bridge::start_on_port(0).expect("bind ephemeral bridge port");

        // Client runs on its own thread (the request blocks until the UI
        // thread — us — pumps the handle and replies).
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream
                .write_all(b"{\"id\":7,\"cmd\":\"screenshot\",\"args\":{}}\n")
                .expect("send request");
            let mut line = String::new();
            BufReader::new(stream)
                .read_line(&mut line)
                .expect("read response");
            let _ = tx.send(line);
        });

        // Pump the bridge until the client thread forwards the response.
        let mut response = None;
        for _ in 0..500 {
            handle.poll();
            if let Ok(line) = rx.recv_timeout(Duration::from_millis(10)) {
                response = Some(line);
                break;
            }
        }
        bridge::unregister_command("screenshot");

        let line = response.expect("bridge should reply over the socket");
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["id"], 7, "response echoes the request id");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(v["ok"]["png_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(&decoded[..8], &TINY_PNG[..8], "PNG arrives intact over the wire");
    }
}
