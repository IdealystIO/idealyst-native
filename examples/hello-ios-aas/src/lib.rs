//! iOS-side AAS (Application-as-a-Server) client.
//!
//! The Swift host calls `ios_main(root_view, server_url_utf8)` once
//! from `viewDidLoad`. We build an `IosBackend`, wrap it in an
//! `AasClient<IosBackend>`, and spawn a background thread that runs
//! the blocking WebSocket transport. Incoming wire commands are
//! shipped onto the main thread via `dispatch_async_f` (UIKit is
//! main-thread-only), where they're applied against the
//! `AasClient` to drive the actual UI.
//!
//! The same AAS dev-server that drives the web demo also drives
//! this — there's only one application running, and it lives on
//! the server. Open the web demo and this iOS app at the same time
//! and you'll see the counter stay in sync across both.

#![cfg(target_os = "ios")]

use backend_ios::IosBackend;
use dev_client::{AasClient, OutboundSender};
use wire::{AppToDev, DevToApp};
use objc2::rc::Retained;
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::UIView;
use std::cell::RefCell;
use std::ffi::{c_char, CStr};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

thread_local! {
    /// The `AasClient<IosBackend>` lives on the main thread for the
    /// life of the app. Held across calls so the dispatch_async
    /// drain handler can reach it.
    static WIRE: RefCell<Option<Rc<RefCell<AasClient<IosBackend>>>>> = const { RefCell::new(None) };

    /// Inbound DevToApp messages from the WebSocket thread. The
    /// main-thread drain function pulls them off and applies them
    /// through the wire.
    static INBOUND: RefCell<Option<mpsc::Receiver<DevToApp>>> = const { RefCell::new(None) };
}

/// C-exported entry point called by the Swift host.
///
/// # Safety
/// - Must be called on the main thread.
/// - `root_view` must be a non-null, valid `UIView *`.
/// - `url_utf8` may be null (defaults to `ws://127.0.0.1:9001`) or a
///   non-null pointer to a NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn ios_main(root_view: *mut std::ffi::c_void, url_utf8: *const c_char) {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("RUST PANIC: {}", info);
    }));

    // SAFETY: contract requires main-thread invocation.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    let url = if url_utf8.is_null() {
        "ws://127.0.0.1:9001".to_string()
    } else {
        unsafe { CStr::from_ptr(url_utf8) }
            .to_string_lossy()
            .into_owned()
    };
    eprintln!("[ios-aas] connecting to {}", url);

    // Take a strong ref on the host UIView.
    let view: Retained<UIView> = unsafe {
        Retained::retain(root_view as *mut UIView)
            .expect("ios_main: root_view must be non-null")
    };

    // Build the platform backend.
    let mut backend = IosBackend::new(mtm);
    backend.set_host_root(view);

    // AAS wrapper. No `framework_core::render` here — the dev server
    // runs the walker; this side only consumes wire commands.
    let outbound = OutboundSender::new();
    let wire = Rc::new(RefCell::new(AasClient::new(backend, outbound.clone())));

    // Bridge channels. The sender side moves to the WS thread; the
    // receiver side stays on the main thread for the drain pass.
    let (inbound_tx, inbound_rx) = mpsc::channel::<DevToApp>();
    let (outbound_tx, outbound_rx) = mpsc::channel::<AppToDev>();
    outbound.set(outbound_tx);

    INBOUND.with(|slot| *slot.borrow_mut() = Some(inbound_rx));
    WIRE.with(|slot| *slot.borrow_mut() = Some(wire));

    // Spawn the WebSocket worker.
    std::thread::spawn(move || ws_thread(url, inbound_tx, outbound_rx));

    // Start the main-thread drain timer.
    start_main_thread_drain_timer();
}

/// Background thread: maintains a connection to the dev server and
/// shuttles frames between the socket and the main-thread channels.
/// Reconnects on disconnect with a short backoff.
fn ws_thread(
    url: String,
    inbound_tx: mpsc::Sender<DevToApp>,
    outbound_rx: mpsc::Receiver<AppToDev>,
) {
    loop {
        let (mut ws, _) = match tungstenite::connect(&url) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[ios-aas] connect failed: {} — retrying in 1s", e);
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
            let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
        }

        // Greet.
        let hello = AppToDev::Hello {
            app_name: "hello-ios-aas".into(),
            color_scheme: wire::WireColorScheme::Auto,
            initial_url: None,
        };
        let _ = ws_send(&mut ws, &hello);
        eprintln!("[ios-aas] connected");

        let _ = run_ws_session(&mut ws, &inbound_tx, &outbound_rx);
        eprintln!("[ios-aas] disconnected; reconnecting in 500ms");
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn ws_send<S>(
    ws: &mut tungstenite::WebSocket<S>,
    msg: &AppToDev,
) -> Result<(), tungstenite::Error>
where
    S: std::io::Read + std::io::Write,
{
    let bytes = serde_json::to_vec(msg).expect("encode AppToDev");
    ws.send(tungstenite::Message::Binary(bytes.into()))
}

fn run_ws_session<S>(
    ws: &mut tungstenite::WebSocket<S>,
    inbound_tx: &mpsc::Sender<DevToApp>,
    outbound_rx: &mpsc::Receiver<AppToDev>,
) -> Result<(), tungstenite::Error>
where
    S: std::io::Read + std::io::Write,
{
    use std::io::ErrorKind;
    use tungstenite::Message;
    loop {
        match ws.read() {
            Ok(Message::Binary(b)) => match serde_json::from_slice::<DevToApp>(&b) {
                Ok(msg) => {
                    if inbound_tx.send(msg).is_err() {
                        return Ok(());
                    }
                }
                Err(e) => eprintln!("[ios-aas] decode error: {}", e),
            },
            Ok(Message::Text(t)) => match serde_json::from_str::<DevToApp>(t.as_str()) {
                Ok(msg) => {
                    if inbound_tx.send(msg).is_err() {
                        return Ok(());
                    }
                }
                Err(e) => eprintln!("[ios-aas] decode error: {}", e),
            },
            Ok(Message::Close(_)) => return Ok(()),
            Ok(Message::Ping(p)) => {
                let _ = ws.send(Message::Pong(p));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(
                tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed,
            ) => return Ok(()),
            Err(e) => return Err(e),
        }
        while let Ok(msg) = outbound_rx.try_recv() {
            ws_send(ws, &msg)?;
        }
    }
}

/// Periodic main-thread drain. Same `dispatch_async_f` pattern as
/// `hello-ios`'s robot poll timer: a background thread sleeps for
/// ~16ms, then dispatches a closure onto the main run loop that
/// pops pending DevToApp messages and applies them through the
/// wire.
fn start_main_thread_drain_timer() {
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }
    extern "C" fn do_drain(_ctx: *mut std::ffi::c_void) {
        drain_inbound_on_main();
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(16));
        unsafe {
            dispatch_async_f(
                &_dispatch_main_q as *const _ as *const std::ffi::c_void,
                std::ptr::null_mut(),
                do_drain,
            );
        }
    });
}

/// Runs on the main thread. Pulls pending DevToApp messages off the
/// inbound channel and applies them through the wire.
fn drain_inbound_on_main() {
    let msgs: Vec<DevToApp> = INBOUND.with(|slot| {
        let mut out = Vec::new();
        if let Some(rx) = slot.borrow().as_ref() {
            while let Ok(msg) = rx.try_recv() {
                out.push(msg);
            }
        }
        out
    });
    if msgs.is_empty() {
        return;
    }
    WIRE.with(|slot| {
        let Some(wire) = slot.borrow().clone() else {
            return;
        };
        let mut wire = wire.borrow_mut();
        for msg in msgs {
            apply_dev_msg(&mut wire, msg);
        }
    });
}

fn apply_dev_msg(wire: &mut AasClient<IosBackend>, msg: DevToApp) {
    match msg {
        DevToApp::Hello { .. } => {}
        DevToApp::Commands(cmds) => {
            if let Err(e) = wire.apply_batch(cmds) {
                eprintln!("[ios-aas] replay error: {:?}", e);
            }
            // Re-run the layout pass. In framework mode, the iOS
            // backend's `schedule_layout_pass()` defers this via
            // `IOS_BACKEND_SELF`; in AAS mode we own the backend
            // directly (no Weak to register), so we drive layout
            // ourselves at the end of each batch.
            wire.backend_mut().run_layout();
        }
        DevToApp::Rebuilding => eprintln!("[ios-aas] dev rebuilding…"),
        DevToApp::Error { message } => eprintln!("[ios-aas] dev error: {}", message),
        DevToApp::ThemeChanged { .. } => {}
    }
}

/// Tear down the active mount. Called by the Swift host from
/// `applicationWillTerminate` (or wherever the app shuts down).
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {
    WIRE.with(|slot| slot.borrow_mut().take());
    INBOUND.with(|slot| slot.borrow_mut().take());
}
