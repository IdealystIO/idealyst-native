//! Windows implementation of the WebView SDK — Microsoft Edge WebView2.
//!
//! A real embedded browser (the Edge/Chromium engine Windows 10/11 ship as the
//! WebView2 runtime), the analogue of the Linux WebKitGTK view and the
//! web `<iframe>`.
//!
//! # Hosting
//!
//! The Windows backend paints the scene itself, so the WebView2 controller
//! is parented to a child window of its own — `IdealystWebViewHost` —
//! which the backend frames like its native controls
//! (`register_external_view`: laid out by the flex tree, moved with
//! `SetWindowPos`). The host window's `WM_SIZE` resizes the controller.
//!
//! # The loader
//!
//! WebView2 starts through Microsoft's loader. Its static form links only
//! with the MSVC toolchain and Windows apps here are built with the GNU one,
//! so it is `WebView2Loader.dll`, which this crate declares as a runtime
//! file (`[package.metadata.idealyst.windows] runtime-files`) and the
//! Windows build copies next to the `.exe`. It is loaded at runtime with
//! `LoadLibraryW`, never imported: an app whose loader is missing still
//! starts, and the webview reports `on_error` (and logs why).
//!
//! # Lifecycle
//!
//! Environment and controller creation are asynchronous (COM completion
//! handlers on this thread, delivered through the message pump). Until the
//! controller is ready the latest URL is held and navigated to when it is.
//! Profile data lives under `%LOCALAPPDATA%\<app id>\WebView2` — the
//! default, beside the `.exe`, is unwritable once the app is installed under
//! Program Files.
//!
//! # Callbacks (parity with the other leaves)
//!
//! * `on_load` / `on_error` — `NavigationCompleted`, by `IsSuccess`.
//! * `on_message` — page → native. WebView2's channel is
//!   `window.chrome.webview.postMessage`; a script injected at document
//!   creation routes the page's `window.postMessage` through it, the same
//!   page-side contract as the Linux shim.
//! * `post_message` (native → page) arrives as a `message` event on
//!   `window`, as on the web leaf.
//! * `execute_js` stays the trait-default error: `ExecuteScript` is
//!   asynchronous, and the synchronous signature would mean blocking the UI
//!   thread (the Linux leaf makes the same call).
//!
//! Author callbacks run from WebView2's events — outside the framework's
//! dispatch sites — so each is followed by `newcore::schedule_flush()` to
//! commit the signal writes it made.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use backend_windows::{WindowsBackend, WindowsNode};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler,
    ICoreWebView2Environment,
};
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationCompletedEventHandler, WebMessageReceivedEventHandler,
};
use windows::core::{Interface, BOOL, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, RegisterClassW, CS_HREDRAW,
    CS_VREDRAW, WINDOW_EX_STYLE, WM_SIZE, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_VISIBLE,
};

use crate::{WebViewOps, WebViewProps};

pub(crate) static OPS: &dyn WebViewOps = &WindowsWebViewOps;

const HOST_CLASS: PCWSTR = windows::core::w!("IdealystWebViewHost");
const LOADER_DLL: &str = "WebView2Loader.dll";

/// Routes the page's `window.postMessage` to WebView2's channel (so
/// `on_message` fires) and replays native → page messages as `message`
/// events on `window` (the web leaf's contract). Installed at document
/// creation, before page scripts run.
const SHIM_JS: &str = r#"
(function () {
  if (window.__idealyst_pm_shim_installed || !window.chrome || !window.chrome.webview) return;
  window.__idealyst_pm_shim_installed = true;
  var original = window.postMessage;
  window.postMessage = function (msg, targetOrigin, transfer) {
    try {
      window.chrome.webview.postMessage(typeof msg === 'string' ? msg : JSON.stringify(msg));
    } catch (e) {}
    if (original) { try { return original.apply(window, arguments); } catch (e) {} }
  };
  window.chrome.webview.addEventListener('message', function (e) {
    window.dispatchEvent(new MessageEvent('message', { data: e.data }));
  });
})();
"#;

/// One mounted webview.
struct Leaf {
    host: HWND,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    /// The source to show once the controller is ready.
    pending: Option<String>,
}

thread_local! {
    /// node id → leaf, for the imperative ops.
    static LEAVES: RefCell<HashMap<u64, Rc<RefCell<Leaf>>>> = RefCell::new(HashMap::new());
    /// host window → leaf, for `WM_SIZE`.
    static HOSTS: RefCell<HashMap<isize, Weak<RefCell<Leaf>>>> = RefCell::new(HashMap::new());
}

/// Build the host window + WebView2 for one `WebView` payload.
pub(crate) fn build(props: &Rc<WebViewProps>, backend: &Rc<RefCell<WindowsBackend>>) -> WindowsNode {
    // WebView2 runs on a single-threaded apartment. Already initialized
    // (S_FALSE, or the STA OLE drag-drop set up) is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let parent = backend.borrow().host_hwnd();
    let host = create_host(parent);
    let node = backend.borrow_mut().register_external_view(host);
    let id = node.id();
    let leaf = Rc::new(RefCell::new(Leaf { host, controller: None, webview: None, pending: None }));
    LEAVES.with(|m| m.borrow_mut().insert(id, leaf.clone()));
    HOSTS.with(|m| m.borrow_mut().insert(host.0 as isize, Rc::downgrade(&leaf)));

    start(&leaf, props);

    let props_for_effect = props.clone();
    let leaf_for_effect = leaf.clone();
    runtime_world::effect(move || {
        let url = (props_for_effect.url)();
        navigate(&leaf_for_effect, url);
    });

    runtime_vocabulary::style_attach::on_teardown(move || {
        LEAVES.with(|m| m.borrow_mut().remove(&id));
        HOSTS.with(|m| m.borrow_mut().remove(&(host.0 as isize)));
        let controller = leaf.borrow_mut().controller.take();
        if let Some(c) = controller {
            let _ = unsafe { c.Close() };
        }
        let _ = unsafe { DestroyWindow(host) };
    });
    node
}

fn create_host(parent: HWND) -> HWND {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(host_proc),
                hInstance: instance.into(),
                lpszClassName: HOST_CLASS,
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            HOST_CLASS,
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .unwrap_or_default()
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_SIZE {
        if let Some(leaf) = HOSTS.with(|m| m.borrow().get(&(hwnd.0 as isize)).and_then(Weak::upgrade)) {
            fit(&leaf.borrow());
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Size the controller to its host window's client area.
fn fit(leaf: &Leaf) {
    let Some(controller) = &leaf.controller else { return };
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(leaf.host, &mut rc);
        let _ = controller.SetBounds(rc);
    }
}

fn navigate(leaf: &Rc<RefCell<Leaf>>, source: String) {
    let webview = leaf.borrow().webview.clone();
    match webview {
        Some(w) => show(&w, &source),
        None => leaf.borrow_mut().pending = Some(source),
    }
}

/// A string that looks like markup is shown as a document; anything else
/// is navigated to as a URL (the Linux leaf's rule).
fn show(webview: &ICoreWebView2, source: &str) {
    let s = HSTRING::from(source);
    let _ = unsafe {
        if source.trim_start().starts_with('<') {
            webview.NavigateToString(&s)
        } else {
            webview.Navigate(&s)
        }
    };
}

/// Run an author callback and commit the signal writes it made.
fn deliver(f: impl FnOnce()) {
    f();
    backend_windows::newcore::schedule_flush();
}

fn fail(props: &WebViewProps, why: &str) {
    eprintln!("[webview] {why}");
    if let Some(cb) = &props.on_error {
        deliver(|| cb());
    }
}

/// `CreateCoreWebView2EnvironmentWithOptions`, resolved from the loader DLL
/// beside the executable.
type CreateEnvironment = unsafe extern "system" fn(
    browser_folder: PCWSTR,
    user_data_folder: PCWSTR,
    options: *mut core::ffi::c_void,
    handler: *mut core::ffi::c_void,
) -> HRESULT;

fn loader() -> Result<CreateEnvironment, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or("cannot locate the executable's folder")?;
    let path = exe_dir.join(LOADER_DLL);
    unsafe {
        let module = LoadLibraryW(&HSTRING::from(path.as_os_str()))
            .map_err(|e| format!("{LOADER_DLL} not loaded from {} ({e}); it ships beside the .exe", exe_dir.display()))?;
        let f = GetProcAddress(module, windows::core::s!("CreateCoreWebView2EnvironmentWithOptions"))
            .ok_or_else(|| format!("{LOADER_DLL} has no CreateCoreWebView2EnvironmentWithOptions"))?;
        Ok(std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateEnvironment>(f))
    }
}

/// `%LOCALAPPDATA%\<app id>\WebView2`, or `None` (WebView2's default).
fn user_data_folder() -> Option<HSTRING> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    let dir = std::path::Path::new(&base).join(backend_windows::app_identity::current()).join("WebView2");
    Some(HSTRING::from(dir.as_os_str()))
}

/// Create the environment, then the controller, then wire the events.
fn start(leaf: &Rc<RefCell<Leaf>>, props: &Rc<WebViewProps>) {
    let create = match loader() {
        Ok(f) => f,
        Err(why) => return fail(props, &why),
    };
    let weak = Rc::downgrade(leaf);
    let props_env = props.clone();
    let handler: ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, environment| {
            let Some(leaf) = weak.upgrade() else { return Ok(()) };
            match (result, environment) {
                (Ok(()), Some(env)) => create_controller(&leaf, env, props_env),
                (Err(e), _) => fail(&props_env, &format!("WebView2 environment: {e}")),
                (Ok(()), None) => fail(&props_env, "WebView2 environment: none returned"),
            }
            Ok(())
        }));
    let folder = user_data_folder();
    let hr = unsafe {
        create(
            PCWSTR::null(),
            folder.as_ref().map(|f| PCWSTR(f.as_ptr())).unwrap_or(PCWSTR::null()),
            std::ptr::null_mut(),
            handler.as_raw(),
        )
    };
    if let Err(e) = hr.ok() {
        // No WebView2 runtime installed is the common cause.
        fail(props, &format!("WebView2 could not start ({e}) — is the WebView2 runtime installed?"));
    }
}

fn create_controller(leaf: &Rc<RefCell<Leaf>>, env: ICoreWebView2Environment, props: Rc<WebViewProps>) {
    let host = leaf.borrow().host;
    let weak = Rc::downgrade(leaf);
    let props_ctl = props.clone();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, controller| {
        let Some(leaf) = weak.upgrade() else {
            if let Some(c) = controller {
                let _ = unsafe { c.Close() };
            }
            return Ok(());
        };
        let controller = match (result, controller) {
            (Ok(()), Some(c)) => c,
            (Err(e), _) => {
                fail(&props_ctl, &format!("WebView2 controller: {e}"));
                return Ok(());
            }
            (Ok(()), None) => {
                fail(&props_ctl, "WebView2 controller: none returned");
                return Ok(());
            }
        };
        let webview = unsafe { controller.CoreWebView2()? };
        wire_events(&webview, &props_ctl)?;
        unsafe {
            webview.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(SHIM_JS), None)?;
            controller.SetIsVisible(true)?;
        }
        let pending = {
            let mut l = leaf.borrow_mut();
            l.controller = Some(controller);
            l.webview = Some(webview.clone());
            l.pending.take()
        };
        fit(&leaf.borrow());
        if let Some(source) = pending {
            show(&webview, &source);
        }
        Ok(())
    }));
    if let Err(e) = unsafe { env.CreateCoreWebView2Controller(host, &handler) } {
        fail(&props, &format!("WebView2 controller: {e}"));
    }
}

fn wire_events(webview: &ICoreWebView2, props: &Rc<WebViewProps>) -> windows::core::Result<()> {
    let mut token = 0i64;
    let p = props.clone();
    let on_nav = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut ok = BOOL(0);
        unsafe { args.IsSuccess(&mut ok)? };
        let cb = if ok.as_bool() { p.on_load.clone() } else { p.on_error.clone() };
        if let Some(cb) = cb {
            deliver(|| cb());
        }
        Ok(())
    }));
    unsafe { webview.add_NavigationCompleted(&on_nav, &mut token)? };

    let p = props.clone();
    let on_message = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
        let (Some(args), Some(cb)) = (args, p.on_message.clone()) else { return Ok(()) };
        let text = unsafe {
            let mut raw = PWSTR::null();
            // A string message (the shim always posts one); else its JSON.
            if args.TryGetWebMessageAsString(&mut raw).is_err() {
                args.WebMessageAsJson(&mut raw)?;
            }
            let s = raw.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw.0 as *const _));
            s
        };
        deliver(|| cb(text));
        Ok(())
    }));
    unsafe { webview.add_WebMessageReceived(&on_message, &mut token)? };
    Ok(())
}

struct WindowsWebViewOps;

impl WindowsWebViewOps {
    fn webview(node: &dyn std::any::Any) -> Option<ICoreWebView2> {
        let id = node.downcast_ref::<WindowsNode>()?.id();
        LEAVES.with(|m| m.borrow().get(&id).and_then(|l| l.borrow().webview.clone()))
    }
}

impl WebViewOps for WindowsWebViewOps {
    fn post_message(&self, node: &dyn std::any::Any, msg: &str) {
        if let Some(w) = Self::webview(node) {
            let _ = unsafe { w.PostWebMessageAsString(&HSTRING::from(msg)) };
        }
    }

    fn reload(&self, node: &dyn std::any::Any) {
        if let Some(w) = Self::webview(node) {
            let _ = unsafe { w.Reload() };
        }
    }
}
