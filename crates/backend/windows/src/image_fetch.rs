//! `image()` `http(s)` loading and the UI-thread message pump that
//! delivers image events.
//!
//! ## Fetch
//!
//! WinHTTP, synchronous, on a small pool of worker threads
//! ([`FETCH_WORKERS`]) — the OS HTTP stack, so system proxy settings and
//! TLS roots apply and no TLS / async runtime is linked in. (The `net` SDK's
//! desktop transport is `reqwest` + tokio, which a backend should not drag
//! into every Windows app for image loads.) A non-2xx status, a transport
//! error, or a body over [`MAX_IMAGE_BYTES`] is a failure (`on_error`).
//!
//! ## Back to the UI thread
//!
//! GDI+ objects, the node map and author handlers all live on the UI
//! thread. A worker hands its result back by `PostMessageW`-ing a
//! heap-allocated [`FetchDone`] (raw pointer in `LPARAM`) to a hidden
//! message-only window ([`ImagePump`]) the backend owns; its window
//! procedure runs on the UI thread from the message pump, with no backend
//! borrow held. If the post fails (window gone) the worker frees the box.
//!
//! The same window also carries deferred event deliveries
//! ([`ImagePump::queue_delivery`]): `create_image` / `update_image_src` /
//! the handler installs run inside a backend `RefCell` borrow, and an
//! author handler fired there would re-enter the backend and abort with
//! `RefCell already borrowed` — so they post, and the pump fires. A posted
//! message is always asynchronous, independent of whether a scheduler is
//! installed.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Weak;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PeekMessageW, PostMessageW,
    RegisterClassW, SetWindowLongPtrW, GWLP_USERDATA, HWND_MESSAGE, MSG, PM_REMOVE, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_DESTROY, WM_NCDESTROY, WNDCLASSW,
};

use crate::image_policy::{split_http_url, status_is_success, Failure};
use crate::WindowsBackend;

/// Concurrent image downloads. A screen of remote thumbnails should not
/// queue behind one slow response, nor open a socket per image; 4 is in
/// the range browsers use per host (HTTP/1.1 allows 6).
const FETCH_WORKERS: usize = 4;

/// Largest response body accepted, in bytes. Guards the UI process from a
/// URL that streams forever; far above any image a UI shows.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// `WinHttpReadData` chunk size.
const READ_CHUNK: usize = 64 * 1024;

/// A finished fetch for node `node_id`'s load `generation`.
pub(crate) struct FetchDone {
    pub node_id: u64,
    pub generation: u64,
    pub result: Result<Vec<u8>, Failure>,
}

const WM_IMAGE_DELIVER: u32 = WM_APP + 0x51;
const WM_IMAGE_FETCHED: u32 = WM_APP + 0x52;

// =========================================================================
// Pump window
// =========================================================================

/// The backend's hidden message-only window. Destroyed with the backend.
pub(crate) struct ImagePump {
    hwnd: HWND,
}

impl ImagePump {
    /// Create the pump on the UI thread. `backend` is the weak self-ref the
    /// window procedure upgrades per message. `None` if window creation
    /// fails — loads still happen, events just can't be delivered.
    pub(crate) fn new(backend: Weak<RefCell<WindowsBackend>>) -> Option<ImagePump> {
        static CLASS: OnceLock<()> = OnceLock::new();
        let class = w!("IdealystImagePump");
        unsafe {
            let hinstance = GetModuleHandleW(None).ok()?;
            CLASS.get_or_init(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(pump_proc),
                    hInstance: hinstance.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance.into()),
                None,
            )
            .ok()?;
            let slot = Box::into_raw(Box::new(backend));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, slot as isize);
            Some(ImagePump { hwnd })
        }
    }

    /// Fire node `node_id`'s owed image event later, from the pump.
    pub(crate) fn queue_delivery(&self, node_id: u64) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_IMAGE_DELIVER, WPARAM(node_id as usize), LPARAM(0));
        }
    }

    /// Fetch `url` off the UI thread; the result returns through this pump.
    pub(crate) fn fetch(&self, node_id: u64, generation: u64, url: &str) {
        let job = Job { url: url.to_string(), node_id, generation, pump: self.hwnd.0 as isize };
        if pool().lock().map(|tx| tx.send(job)).is_err() {
            // The pool's workers are gone (a panicked worker poisoned the
            // lock) — report the load as failed rather than hang.
            post_done(
                self.hwnd.0 as isize,
                FetchDone { node_id, generation, result: Err(Failure::Network("fetch pool down".into())) },
            );
        }
    }
}

impl Drop for ImagePump {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn pump_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let slot = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Weak<RefCell<WindowsBackend>>;
    match msg {
        // Both arms re-post when the backend is mid-borrow: only possible
        // if a backend call runs a nested message loop (a modal dialog),
        // and the retry lands once that call returns.
        WM_IMAGE_DELIVER if !slot.is_null() => {
            if !WindowsBackend::deliver_image_events(&*slot, wparam.0 as u64) {
                let _ = PostMessageW(Some(hwnd), msg, wparam, lparam);
            }
            LRESULT(0)
        }
        WM_IMAGE_FETCHED => {
            let done = Box::from_raw(lparam.0 as *mut FetchDone);
            if !slot.is_null() {
                if let Err(done) = WindowsBackend::apply_image_fetch(&*slot, *done) {
                    post_done(hwnd.0 as isize, done);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // A destroyed window's queued messages are discarded — free the
            // results still in flight so their bytes don't leak.
            let mut m = MSG::default();
            while PeekMessageW(&mut m, Some(hwnd), WM_IMAGE_FETCHED, WM_IMAGE_FETCHED, PM_REMOVE).as_bool() {
                drop(Box::from_raw(m.lParam.0 as *mut FetchDone));
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            if !slot.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(slot as *mut Weak<RefCell<WindowsBackend>>));
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Hand a result to the pump window (any thread). Frees it if the window
/// is gone.
fn post_done(pump: isize, done: FetchDone) {
    let raw = Box::into_raw(Box::new(done));
    let posted = unsafe {
        PostMessageW(Some(HWND(pump as *mut c_void)), WM_IMAGE_FETCHED, WPARAM(0), LPARAM(raw as isize))
    };
    if posted.is_err() {
        drop(unsafe { Box::from_raw(raw) });
    }
}

// =========================================================================
// Worker pool
// =========================================================================

struct Job {
    url: String,
    node_id: u64,
    generation: u64,
    /// Pump HWND as `isize` (`HWND` is not `Send`).
    pump: isize,
}

/// The job sender, started on first use.
fn pool() -> &'static Mutex<Sender<Job>> {
    static POOL: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..FETCH_WORKERS {
            let rx = rx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("idealyst-image-fetch-{i}"))
                .spawn(move || worker(rx));
        }
        Mutex::new(tx)
    })
}

fn worker(rx: Arc<Mutex<Receiver<Job>>>) {
    let session = Session::open();
    loop {
        let job = match rx.lock() {
            Ok(rx) => match rx.recv() {
                Ok(job) => job,
                Err(_) => return,
            },
            Err(_) => return,
        };
        let result = match &session {
            Some(s) => s.get(&job.url),
            None => Err(Failure::Network("WinHttpOpen failed".into())),
        };
        post_done(job.pump, FetchDone { node_id: job.node_id, generation: job.generation, result });
    }
}

/// An owned WinHTTP handle, closed on drop.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void, what: &str) -> Result<Handle, Failure> {
        if raw.is_null() {
            Err(Failure::Network(format!("{what}: {}", windows::core::Error::from_thread())))
        } else {
            Ok(Handle(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// One WinHTTP session per worker thread.
struct Session(Handle);

impl Session {
    fn open() -> Option<Session> {
        let agent = w!("Idealyst");
        unsafe {
            // Automatic proxy (WinINet/IE settings + WPAD) needs Windows
            // 8.1; fall back to the WinHTTP default proxy config before it.
            let mut h = WinHttpOpen(agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
            if h.is_null() {
                h = WinHttpOpen(agent, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
            }
            Handle::new(h, "WinHttpOpen").ok().map(Session)
        }
    }

    /// GET `url`; the body on a 2xx response.
    fn get(&self, url: &str) -> Result<Vec<u8>, Failure> {
        let target =
            split_http_url(url).ok_or_else(|| Failure::Network("URL WinHTTP cannot request".into()))?;
        let host: Vec<u16> = target.host.encode_utf16().chain([0]).collect();
        let path: Vec<u16> = target.path.encode_utf16().chain([0]).collect();
        unsafe {
            let connect =
                Handle::new(WinHttpConnect(self.0 .0, PCWSTR(host.as_ptr()), target.port, 0), "connect")?;
            let flags = if target.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
            let request = Handle::new(
                WinHttpOpenRequest(
                    connect.0,
                    w!("GET"),
                    PCWSTR(path.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    flags,
                ),
                "open request",
            )?;
            let net = |e: windows::core::Error| Failure::Network(e.to_string());
            WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(net)?;
            WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(net)?;
            let mut status: u32 = 0;
            let mut len = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut c_void),
                &mut len,
                std::ptr::null_mut(),
            )
            .map_err(net)?;
            if !status_is_success(status) {
                return Err(Failure::HttpStatus(status));
            }
            let mut body = Vec::new();
            let mut chunk = vec![0u8; READ_CHUNK];
            loop {
                let mut read: u32 = 0;
                WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, READ_CHUNK as u32, &mut read)
                    .map_err(net)?;
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read as usize]);
                if body.len() > MAX_IMAGE_BYTES {
                    return Err(Failure::Network(format!("body over {MAX_IMAGE_BYTES} bytes")));
                }
            }
            Ok(body)
        }
    }
}

/// The real Win32 path end to end: a `WindowsBackend` with its image pump,
/// GDI+ decode, WinHTTP against a local server, and handler delivery
/// through the message loop. Runs on Windows (or under Wine); the
/// host-portable policy half is `tests/image_src.rs`.
#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, MSG, PM_REMOVE};

    use runtime_shared::image_src::vectors;
    use runtime_shared::primitives::image::ImageLoadEvent;

    use crate::WindowsBackend;

    /// Run the UI thread's message loop until `done()` or a timeout.
    fn pump_until(done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() && Instant::now() < deadline {
            let mut m = MSG::default();
            unsafe {
                while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&m);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Settle any trailing deliveries (to prove nothing fires twice).
    fn pump_for(d: Duration) {
        let end = Instant::now() + d;
        pump_until(|| Instant::now() >= end);
    }

    fn backend() -> Rc<RefCell<WindowsBackend>> {
        crate::ensure_gdiplus();
        let b = Rc::new(RefCell::new(WindowsBackend::new(HWND(std::ptr::null_mut()))));
        b.borrow_mut().set_self_ref(Rc::downgrade(&b));
        b
    }

    struct Counters {
        loads: Rc<RefCell<Vec<(f32, f32)>>>,
        errors: Rc<Cell<u32>>,
    }

    /// `create_image(src)` then install both handlers — the walker's order.
    fn mount(b: &Rc<RefCell<WindowsBackend>>, src: &str) -> (crate::WindowsNode, Counters) {
        let node = b.borrow_mut().create_image(src, None, &Default::default());
        let c = Counters { loads: Default::default(), errors: Default::default() };
        let loads = c.loads.clone();
        let errors = c.errors.clone();
        b.borrow_mut().install_image_load_handler(
            &node,
            Rc::new(move |e: &ImageLoadEvent| loads.borrow_mut().push((e.width, e.height))),
        );
        b.borrow_mut().install_image_error_handler(&node, Rc::new(move || errors.set(errors.get() + 1)));
        // The walker's mount-time src binding repeats create's src.
        b.borrow_mut().update_image_src(&node, src);
        (node, c)
    }

    /// A one-request-per-connection HTTP/1.0 server. Each request waits
    /// for a `release` message before answering, so a test can order the
    /// response against a src swap. Returns the base URL.
    fn server(routes: Vec<(&'static str, u16, Vec<u8>)>, release: mpsc::Receiver<()>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut req = Vec::new();
                let mut buf = [0u8; 1024];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let line = String::from_utf8_lossy(&req).lines().next().unwrap_or("").to_string();
                let path = line.split(' ').nth(1).unwrap_or("").to_string();
                if release.recv().is_err() {
                    return;
                }
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, st, b)| (*st, b.clone()))
                    .unwrap_or((404, Vec::new()));
                let head = format!(
                    "HTTP/1.0 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        base
    }

    fn png_1x1() -> Vec<u8> {
        backend_image_source::parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes
    }

    #[test]
    fn sync_sources_fire_their_handler_once_through_the_pump() {
        let b = backend();
        let (_, ok) = mount(&b, vectors::PNG_1X1_BASE64);
        let (_, svg) = mount(&b, vectors::SVG_2X1_PERCENT);
        let (_, bad) = mount(&b, "file:///C:/x.png");
        // Nothing fires synchronously inside the backend borrow.
        assert!(ok.loads.borrow().is_empty() && bad.errors.get() == 0);
        pump_until(|| !ok.loads.borrow().is_empty() && !svg.loads.borrow().is_empty() && bad.errors.get() > 0);
        pump_for(Duration::from_millis(100));
        assert_eq!(*ok.loads.borrow(), vec![(1.0, 1.0)]);
        assert_eq!(*svg.loads.borrow(), vec![(2.0, 1.0)]);
        assert_eq!((bad.errors.get(), bad.loads.borrow().len()), (1, 0));
        assert_eq!(ok.errors.get() + svg.errors.get(), 0);
    }

    #[test]
    fn http_fetch_loads_and_non_2xx_errors() {
        let (tx, rx) = mpsc::channel();
        let base = server(vec![("/a.png", 200, png_1x1()), ("/gone.png", 404, b"nope".to_vec())], rx);
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        let b = backend();
        let (_, ok) = mount(&b, &format!("{base}/a.png"));
        let (_, gone) = mount(&b, &format!("{base}/gone.png"));
        pump_until(|| !ok.loads.borrow().is_empty() && gone.errors.get() > 0);
        pump_for(Duration::from_millis(100));
        assert_eq!(*ok.loads.borrow(), vec![(1.0, 1.0)]);
        assert_eq!(ok.errors.get(), 0);
        assert_eq!((gone.errors.get(), gone.loads.borrow().len()), (1, 0));
    }

    #[test]
    fn slow_fetch_never_replaces_a_newer_src() {
        let (tx, rx) = mpsc::channel();
        let svg = backend_image_source::parse_data_uri(vectors::SVG_2X1_PERCENT).unwrap().bytes;
        let base = server(vec![("/slow.svg", 200, svg)], rx);
        let b = backend();
        let (node, c) = mount(&b, &format!("{base}/slow.svg"));
        // Supersede while the response is held, then let it through.
        b.borrow_mut().update_image_src(&node, vectors::PNG_1X1_BASE64);
        tx.send(()).unwrap();
        pump_until(|| !c.loads.borrow().is_empty());
        pump_for(Duration::from_millis(300));
        assert_eq!(*c.loads.borrow(), vec![(1.0, 1.0)], "the 2×1 response is stale");
        // ...and it must not replace what the node displays either.
        let shown = match &b.borrow().nodes[&node.id].kind {
            crate::NodeKind::Image(p) => p.source.as_ref().map(|s| s.natural()),
            _ => None,
        };
        assert_eq!(shown, Some((1.0, 1.0)), "node still shows the newer src");
    }
}
