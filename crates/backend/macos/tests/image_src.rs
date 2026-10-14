//! `image()` `src` loading against real AppKit + `NSURLSession`
//! (`docs/image-src.md`): the shared conformance vectors through the real
//! `NSImage` decoder, remote fetches from a local HTTP server (success,
//! non-2xx, a superseded slow response), an unparseable URL, a bundled
//! `asset!` file, and the event contract (same src re-applied is a no-op,
//! each distinct failure fires `on_error`, one `on_load` per load, one
//! replay for a late handler).
//!
//! `harness = false`: AppKit views want the main thread, and the fetch
//! result hops to the main queue, which only drains while the main run loop
//! turns. This binary's `main` IS the main thread and pumps it.
//!
//! ```sh
//! cargo test -p backend-macos --test image_src
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use backend_macos::{MacosBackend, MacosNode};
    use objc2_foundation::MainThreadMarker;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::image_src::vectors::{self, Expect, SYNC};
    use runtime_shared::{AssetId, AssetSource, AssetTag};
    use runtime_vocabulary::caps::{AssetOps, ImageOps};

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after: u8) -> i32;
    }

    /// Turn the main run loop (which drains the main dispatch queue the
    /// fetch result hops onto) for `secs`.
    fn pump_for(secs: f64) {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < end {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
    }

    fn pump_until(mut done: impl FnMut() -> bool, secs: f64) -> bool {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < end {
            if done() {
                return true;
            }
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
        done()
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Event {
        Load(f32, f32),
        Error,
    }

    /// An image node with both observers recording into one event log.
    fn image(backend: &mut MacosBackend, src: &str) -> (MacosNode, Rc<RefCell<Vec<Event>>>) {
        let node = backend.create_image(src, None, &AccessibilityProps::default());
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        backend.install_image_load_handler(
            &node,
            Rc::new(move |ev| l.borrow_mut().push(Event::Load(ev.width, ev.height))),
        );
        let l = log.clone();
        backend.install_image_error_handler(&node, Rc::new(move || l.borrow_mut().push(Event::Error)));
        (node, log)
    }

    fn png_1x1() -> Vec<u8> {
        runtime_shared::image_src::parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes
    }

    const SVG_3X2: &[u8] =
        b"<svg xmlns='http://www.w3.org/2000/svg' width='3' height='2'><rect width='3' height='2'/></svg>";

    /// A tiny HTTP/1.1 server on 127.0.0.1:
    /// - `/ok.png`: 200, the 1x1 PNG
    /// - `/missing.png`: 404 whose body is ALSO the 1x1 PNG (decodable)
    /// - `/slow.svg`: waits 800 ms, then 200 with a 3x2 SVG
    fn serve() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (status, ctype, body): (&str, &str, Vec<u8>) = match path.as_str() {
                        "/ok.png" => ("200 OK", "image/png", png_1x1()),
                        "/missing.png" => ("404 Not Found", "image/png", png_1x1()),
                        "/slow.svg" => {
                            std::thread::sleep(Duration::from_millis(800));
                            ("200 OK", "image/svg+xml", SVG_3X2.to_vec())
                        }
                        _ => ("404 Not Found", "text/plain", b"no".to_vec()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        port
    }

    /// The cross-backend conformance table through the real `NSImage`
    /// decoder and resvg path.
    fn sync_vectors_follow_the_contract(backend: &mut MacosBackend, _port: u16) {
        for &(src, expect) in SYNC {
            let (_node, log) = image(backend, src);
            let want = match expect {
                Expect::Loads { width, height } => vec![Event::Load(width as f32, height as f32)],
                Expect::Errors => vec![Event::Error],
            };
            assert_eq!(*log.borrow(), want, "{src:?}");
        }
    }

    fn remote_png_loads(backend: &mut MacosBackend, port: u16) {
        let (_node, log) = image(backend, &format!("http://127.0.0.1:{port}/ok.png"));
        assert!(pump_until(|| !log.borrow().is_empty(), 10.0), "fetch never completed");
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);
    }

    /// Before: only `data` was checked, so a 404 whose body decoded was
    /// drawn and fired `on_load`.
    fn regression_non_2xx_status_is_an_error(backend: &mut MacosBackend, port: u16) {
        let (_node, log) = image(backend, &format!("http://127.0.0.1:{port}/missing.png"));
        assert!(pump_until(|| !log.borrow().is_empty(), 10.0), "fetch never completed");
        assert_eq!(*log.borrow(), vec![Event::Error], "404 with a PNG body must be on_error");
    }

    /// Before: `NSURL URLWithString:` returning nil made the loader return
    /// silently: a blank view, no `on_error`.
    fn regression_unparseable_url_fires_on_error(backend: &mut MacosBackend, _port: u16) {
        let (_node, log) = image(backend, "https://exa mple.com/a b.png");
        assert_eq!(*log.borrow(), vec![Event::Error]);
    }

    /// Before: no generation. A slow response for the previous `src`
    /// landed after the newer one, replaced it and fired a second `on_load`.
    fn regression_stale_image_response_is_dropped(backend: &mut MacosBackend, port: u16) {
        let (node, log) = image(backend, &format!("http://127.0.0.1:{port}/slow.svg"));
        backend.update_image_src(&node, vectors::PNG_1X1_BASE64);
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);
        // The slow response arrives ~800 ms in; give it time to (not) land.
        pump_for(2.0);
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)], "the superseded 3x2 SVG must be dropped");
    }

    /// Before: `AssetSource::Bundled` was skipped, so `asset!` images fired
    /// `on_error`. An unbundled binary's `resourcePath` is its own directory,
    /// so the file goes in `<exe dir>/assets/`.
    fn regression_bundled_asset_image_loads(backend: &mut MacosBackend, _port: u16) {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join("assets/image_src_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dot.png"), png_1x1()).unwrap();

        let id = AssetId(0x1D_0001);
        backend.register_asset(id, AssetTag::Image, &AssetSource::Bundled { path: "image_src_test/dot.png" });
        let (_node, log) = image(backend, &format!("asset://{}", id.0));
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);

        let missing = AssetId(0x1D_0002);
        backend.register_asset(missing, AssetTag::Image, &AssetSource::Bundled { path: "image_src_test/nope.png" });
        let (_node, log) = image(backend, &format!("asset://{}", missing.0));
        assert_eq!(*log.borrow(), vec![Event::Error], "a file the build didn't ship is on_error");

        let escape = AssetId(0x1D_0003);
        backend.register_asset(escape, AssetTag::Image, &AssetSource::Bundled { path: "../image_src_test/dot.png" });
        let (_node, log) = image(backend, &format!("asset://{}", escape.0));
        assert_eq!(*log.borrow(), vec![Event::Error], "`..` must not leave the assets dir");
    }

    fn remote_asset_is_fetched(backend: &mut MacosBackend, port: u16) {
        let url: &'static str = Box::leak(format!("http://127.0.0.1:{port}/ok.png").into_boxed_str());
        let id = AssetId(0x1D_0004);
        backend.register_asset(id, AssetTag::Image, &AssetSource::Remote { url });
        let (_node, log) = image(backend, &format!("asset://{}", id.0));
        assert!(pump_until(|| !log.borrow().is_empty(), 10.0), "fetch never completed");
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);
    }

    /// Mount applies the src twice (create, then the reactive update with
    /// the same value): the second is a no-op, for a load AND a failure.
    fn same_src_reapplied_is_a_no_op(backend: &mut MacosBackend, port: u16) {
        let (node, log) = image(backend, vectors::PNG_1X1_BASE64);
        backend.update_image_src(&node, vectors::PNG_1X1_BASE64);
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);

        let (node, log) = image(backend, "/attachments/abc");
        backend.update_image_src(&node, "/attachments/abc");
        assert_eq!(*log.borrow(), vec![Event::Error]);

        let url = format!("http://127.0.0.1:{port}/ok.png");
        let (node, log) = image(backend, &url);
        backend.update_image_src(&node, &url);
        assert!(pump_until(|| !log.borrow().is_empty(), 10.0), "fetch never completed");
        pump_for(0.3);
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)], "one fetch, one on_load");
    }

    /// A different src supersedes the old one; if it fails too, `on_error`
    /// fires again (once per distinct failing src).
    fn each_distinct_failing_src_fires_on_error(backend: &mut MacosBackend, _port: u16) {
        let (node, log) = image(backend, "/a.png");
        backend.update_image_src(&node, "/b.png");
        assert_eq!(*log.borrow(), vec![Event::Error, Event::Error]);
    }

    /// A handler installed after the outcome is replayed it exactly once.
    fn late_handler_gets_exactly_one_replay(backend: &mut MacosBackend, _port: u16) {
        let node = backend.create_image(vectors::PNG_1X1_BASE64, None, &AccessibilityProps::default());
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        backend.install_image_load_handler(&node, Rc::new(move |ev| l.borrow_mut().push(Event::Load(ev.width, ev.height))));
        pump_for(0.2);
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);

        let node = backend.create_image("logo.png", None, &AccessibilityProps::default());
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        backend.install_image_error_handler(&node, Rc::new(move || l.borrow_mut().push(Event::Error)));
        pump_for(0.2);
        assert_eq!(*log.borrow(), vec![Event::Error]);
    }

    /// Web found `on_load` landing twice for one load (replay + live
    /// event). Here the replay read the recorded natural size, which still
    /// held the PREVIOUS image while a new src loaded: a handler installed
    /// in that window got the old 1x1 replayed, then the live 3x2.
    fn regression_handler_installed_mid_load_fires_on_load_once(backend: &mut MacosBackend, port: u16) {
        let node = backend.create_image(vectors::PNG_1X1_BASE64, None, &AccessibilityProps::default());
        backend.update_image_src(&node, &format!("http://127.0.0.1:{port}/slow.svg"));
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        backend.install_image_load_handler(&node, Rc::new(move |ev| l.borrow_mut().push(Event::Load(ev.width, ev.height))));
        assert!(log.borrow().is_empty(), "the previous src's image must not be replayed");
        assert!(pump_until(|| !log.borrow().is_empty(), 10.0), "fetch never completed");
        pump_for(0.3);
        assert_eq!(*log.borrow(), vec![Event::Load(3.0, 2.0)]);
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "image_src must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let mut backend = MacosBackend::new(mtm);
        let port = serve();
        let tests: [(&str, fn(&mut MacosBackend, u16)); 11] = [
            ("sync_vectors_follow_the_contract", sync_vectors_follow_the_contract),
            ("remote_png_loads", remote_png_loads),
            ("regression_non_2xx_status_is_an_error", regression_non_2xx_status_is_an_error),
            ("regression_unparseable_url_fires_on_error", regression_unparseable_url_fires_on_error),
            ("regression_stale_image_response_is_dropped", regression_stale_image_response_is_dropped),
            ("regression_bundled_asset_image_loads", regression_bundled_asset_image_loads),
            ("remote_asset_is_fetched", remote_asset_is_fetched),
            ("same_src_reapplied_is_a_no_op", same_src_reapplied_is_a_no_op),
            ("each_distinct_failing_src_fires_on_error", each_distinct_failing_src_fires_on_error),
            ("late_handler_gets_exactly_one_replay", late_handler_gets_exactly_one_replay),
            (
                "regression_handler_installed_mid_load_fires_on_load_once",
                regression_handler_installed_mid_load_fires_on_load_once,
            ),
        ];
        for (name, t) in tests {
            t(&mut backend, port);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
