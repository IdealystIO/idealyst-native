//! iOS implementation of the WebView SDK.
//!
//! Builds a `WKWebView` via raw `msg_send` (see note in Cargo.toml on
//! why we don't lean on `objc2-web-kit`). Navigates via `loadRequest:`,
//! subscribes to reactive URL changes through `Effect::new(...)`.
//!
//! # Callbacks (v2)
//!
//! `on_load` / `on_error` / `on_message` are wired through a custom
//! `IdealystWebViewDelegate` NSObject subclass that informally conforms
//! to:
//!   * `WKNavigationDelegate` — `webView:didFinishNavigation:` fires
//!     `on_load`; `webView:didFailNavigation:withError:` and
//!     `webView:didFailProvisionalNavigation:withError:` both fire
//!     `on_error` (WebKit fires the provisional variant for the common
//!     "couldn't reach the host" case, the non-provisional variant for
//!     mid-load failures).
//!   * `WKScriptMessageHandler` —
//!     `userContentController:didReceiveScriptMessage:` fires
//!     `on_message`. Obj-C protocols are selector-presence checked, so
//!     no explicit conformance declaration is needed.
//!
//! ## JS contract (parity with the web leaf)
//!
//! The web leaf listens for `window.message` events from
//! `iframe.contentWindow`; here, there's no iframe. To keep the page-
//! side API symmetrical:
//!
//!   * **Page → native**: page code calls `window.postMessage(msg, "*")`.
//!     We inject a `WKUserScript` at document-start that wraps
//!     `window.postMessage` so it ALSO fires
//!     `window.webkit.messageHandlers.idealyst.postMessage(msg)` (the
//!     channel our delegate listens on). The original `postMessage`
//!     behavior is preserved.
//!
//!   * **Native → page**: `WebViewHandle::post_message(msg)` calls
//!     `evaluateJavaScript:` to dispatch a synthetic `MessageEvent` on
//!     the page's `window`, so page authors can write
//!     `window.addEventListener('message', e => ...)` on iOS just like
//!     on web.
//!
//! `execute_js` remains the trait default error (WKWebView's
//! `evaluateJavaScript:completionHandler:` is async-only and can't
//! honor the sync trait signature without a thread-blocking shim).

use crate::{WebViewOps, WebViewProps};
use backend_ios::{IosBackend, IosNode};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{
    CGRect, MainThreadMarker, NSObject, NSString, NSURL, NSURLRequest,
};
use objc2_ui_kit::UIView;
use runtime_core::Effect;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) static OPS: &dyn WebViewOps = &IosWebViewOps;

pub fn register(backend: &mut IosBackend) {
    backend.register_external::<WebViewProps, _>(|props, b| build_web_view(props, b));
}

// =========================================================================
// Delegate class — bridges WKNavigationDelegate + WKScriptMessageHandler
// to the Rust closures stored on `WebViewProps`.
// =========================================================================

#[allow(dead_code)]
pub(crate) struct WebViewDelegateIvars {
    on_load: RefCell<Option<Rc<dyn Fn()>>>,
    on_error: RefCell<Option<Rc<dyn Fn()>>>,
    on_message: RefCell<Option<Rc<dyn Fn(String)>>>,
}

declare_class!(
    pub(crate) struct WebViewDelegate;

    unsafe impl ClassType for WebViewDelegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystWebViewDelegate";
    }

    impl DeclaredClass for WebViewDelegate {
        type Ivars = WebViewDelegateIvars;
    }

    unsafe impl NSObjectProtocol for WebViewDelegate {}

    unsafe impl WebViewDelegate {
        // ---- WKNavigationDelegate ----------------------------------------

        #[method(webView:didFinishNavigation:)]
        fn did_finish_navigation(&self, _webview: &NSObject, _navigation: &NSObject) {
            if let Some(cb) = self.ivars().on_load.borrow().as_ref() {
                cb();
            }
        }

        #[method(webView:didFailNavigation:withError:)]
        fn did_fail_navigation(
            &self,
            _webview: &NSObject,
            _navigation: &NSObject,
            _error: &NSObject,
        ) {
            if let Some(cb) = self.ivars().on_error.borrow().as_ref() {
                cb();
            }
        }

        #[method(webView:didFailProvisionalNavigation:withError:)]
        fn did_fail_provisional_navigation(
            &self,
            _webview: &NSObject,
            _navigation: &NSObject,
            _error: &NSObject,
        ) {
            if let Some(cb) = self.ivars().on_error.borrow().as_ref() {
                cb();
            }
        }

        // ---- WKScriptMessageHandler --------------------------------------

        #[method(userContentController:didReceiveScriptMessage:)]
        fn did_receive_script_message(
            &self,
            _ucc: &NSObject,
            message: &NSObject,
        ) {
            let Some(cb) = self.ivars().on_message.borrow().as_ref().cloned() else {
                return;
            };
            // `message.body` is `id` — could be NSString, NSNumber, or
            // an NSDictionary if the page passed a structured object.
            // We coerce to NSString via `description` for the common
            // case (page sent a JSON-stringified payload, which is the
            // pattern the web docs recommend).
            let body: *mut AnyObject = unsafe { msg_send![message, body] };
            if body.is_null() {
                return;
            }
            // Test isKindOfClass: NSString. If yes, use it directly;
            // otherwise call `description` to get a string form.
            let ns_string_class: &AnyClass = match AnyClass::get("NSString") {
                Some(c) => c,
                None => return,
            };
            let is_string: bool =
                unsafe { msg_send![body, isKindOfClass: ns_string_class] };
            let payload: String = if is_string {
                let ns: &NSString = unsafe { &*body.cast::<NSString>() };
                ns.to_string()
            } else {
                let desc: *mut AnyObject = unsafe { msg_send![body, description] };
                if desc.is_null() {
                    return;
                }
                let ns: &NSString = unsafe { &*desc.cast::<NSString>() };
                ns.to_string()
            };
            cb(payload);
        }
    }
);

impl WebViewDelegate {
    fn new(
        mtm: MainThreadMarker,
        on_load: Option<Rc<dyn Fn()>>,
        on_error: Option<Rc<dyn Fn()>>,
        on_message: Option<Rc<dyn Fn(String)>>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(WebViewDelegateIvars {
            on_load: RefCell::new(on_load),
            on_error: RefCell::new(on_error),
            on_message: RefCell::new(on_message),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// WKUserScript JS shim — installed at document-start so the page's
// `window.postMessage` rebroadcasts to the script-message channel.
// =========================================================================

const POST_MESSAGE_SHIM_JS: &str = r#"
(function () {
  if (window.__idealyst_pm_shim_installed) return;
  window.__idealyst_pm_shim_installed = true;
  var original = window.postMessage;
  window.postMessage = function (msg, targetOrigin, transfer) {
    try {
      var payload = (typeof msg === 'string') ? msg : JSON.stringify(msg);
      window.webkit.messageHandlers.idealyst.postMessage(payload);
    } catch (e) {
      // Channel not available (handler not added, removed, or
      // cross-origin) — fall through to native postMessage so
      // existing same-window listeners keep firing.
    }
    return original.apply(this, arguments);
  };
})();
"#;

// =========================================================================
// View builder
// =========================================================================

fn build_web_view(props: &Rc<WebViewProps>, b: &mut IosBackend) -> IosNode {
    let mtm = b.mtm();

    let wk_class: &AnyClass = AnyClass::get("WKWebView")
        .expect("WKWebView class not found — is WebKit linked into the app?");
    let cfg_class: &AnyClass = AnyClass::get("WKWebViewConfiguration")
        .expect("WKWebViewConfiguration class not found");
    let ucc_class: &AnyClass = AnyClass::get("WKUserContentController")
        .expect("WKUserContentController class not found");
    let user_script_class: &AnyClass = AnyClass::get("WKUserScript")
        .expect("WKUserScript class not found");

    let zero_rect: CGRect = CGRect {
        origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
        size: objc2_foundation::CGSize { width: 0.0, height: 0.0 },
    };

    // Build the user-content controller, install the JS shim + the
    // script message handler. The handler name is `idealyst` so the
    // shim above can reach it via `window.webkit.messageHandlers.idealyst`.
    let ucc: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![ucc_class, alloc];
        let inited: *mut AnyObject = msg_send![allocated, init];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("WKUserContentController init returned nil")
    };

    // Inject the postMessage shim at document-start. injectionTime = 0
    // is `WKUserScriptInjectionTimeAtDocumentStart`; forMainFrameOnly =
    // YES so iframes inside the page don't get the wrap installed
    // (which would loop their own postMessage calls back to native).
    let shim_source = NSString::from_str(POST_MESSAGE_SHIM_JS);
    let user_script: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![user_script_class, alloc];
        let inited: *mut AnyObject = msg_send![
            allocated,
            initWithSource: &*shim_source,
            injectionTime: 0_i64,
            forMainFrameOnly: true,
        ];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("WKUserScript init returned nil")
    };
    let _: () = unsafe { msg_send![&*ucc, addUserScript: &*user_script] };

    // Build the delegate. Holding it via `Retained` keeps it alive as
    // long as the WKWebView retains it (which it does for both the
    // navigationDelegate and the script message handler).
    let delegate = WebViewDelegate::new(
        mtm,
        props.on_load.clone(),
        props.on_error.clone(),
        props.on_message.clone(),
    );

    // Register the script-message channel. WKWebView retains the
    // handler via the UCC, so the `delegate` Retained can be dropped
    // at end of scope without dropping the underlying object.
    let handler_name = NSString::from_str("idealyst");
    let _: () = unsafe {
        msg_send![
            &*ucc,
            addScriptMessageHandler: &*delegate,
            name: &*handler_name,
        ]
    };

    // Build the configuration, install the UCC.
    let config: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![cfg_class, alloc];
        let inited: *mut AnyObject = msg_send![allocated, init];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("WKWebViewConfiguration init returned nil")
    };
    let _: () = unsafe { msg_send![&*config, setUserContentController: &*ucc] };

    // Now build the WKWebView.
    let webview_any: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![wk_class, alloc];
        let inited: *mut AnyObject =
            msg_send![allocated, initWithFrame: zero_rect, configuration: &*config];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("WKWebView init returned nil")
    };
    let webview_uiview: Retained<UIView> = unsafe { Retained::cast(webview_any) };

    // Wire the navigation delegate. WKWebView holds it weakly per
    // Apple's docs, so we have to keep the `Retained` alive elsewhere
    // (see the ivar stash on `WebViewHandle` via the IosNode's
    // retained UIView — but the UIView doesn't natively own arbitrary
    // Objective-C strong refs). The simplest reliable owner is the
    // user-content controller: it already retains the delegate via
    // `addScriptMessageHandler:name:`, so as long as the UCC is alive
    // (it's retained by the config, which is retained by WKWebView),
    // the delegate is alive too.
    let _: () = unsafe { msg_send![&*webview_uiview, setNavigationDelegate: &*delegate] };

    b.register_external_view(&webview_uiview);

    // Reactive URL.
    let webview_for_url = webview_uiview.clone();
    let props_clone = props.clone();
    let _effect = Effect::new(move || {
        let url = (props_clone.url)();
        load_url(&webview_for_url, &url);
    });

    IosNode::View(webview_uiview)
}

fn load_url(webview: &UIView, url_str: &str) {
    let ns_url_str = NSString::from_str(url_str);
    let Some(url) = (unsafe { NSURL::URLWithString(&ns_url_str) }) else {
        return;
    };
    let request = unsafe { NSURLRequest::requestWithURL(&url) };
    let _: () = unsafe { msg_send![webview, loadRequest: &*request] };
}

// =========================================================================
// Imperative ops
// =========================================================================

struct IosWebViewOps;

impl WebViewOps for IosWebViewOps {
    fn reload(&self, node: &dyn Any) {
        let Some(IosNode::View(uiview)) = node.downcast_ref::<IosNode>() else {
            return;
        };
        let _: () = unsafe { msg_send![uiview, reload] };
    }

    /// Native → page postMessage. Dispatches a synthetic `MessageEvent`
    /// inside the page so authors can use `window.addEventListener(
    /// 'message', ...)` the same way they would on the web leaf
    /// (where `iframe.contentWindow.postMessage` reaches the iframe's
    /// `message` listeners).
    fn post_message(&self, node: &dyn Any, msg: &str) {
        let Some(IosNode::View(uiview)) = node.downcast_ref::<IosNode>() else {
            return;
        };
        let escaped = escape_for_js_string(msg);
        let js = format!(
            "window.dispatchEvent(new MessageEvent('message', \
                 {{ data: \"{}\", source: window }}));",
            escaped
        );
        let ns_js = NSString::from_str(&js);
        // `evaluateJavaScript:completionHandler:` accepts a nil
        // completion handler (cast as a null block pointer). We pass
        // nil since we're fire-and-forget for outbound posts.
        let _: () = unsafe {
            msg_send![
                uiview,
                evaluateJavaScript: &*ns_js,
                completionHandler: std::ptr::null::<AnyObject>(),
            ]
        };
    }

    // `execute_js` stays as the trait default Err — WKWebView's
    // `evaluateJavaScript:completionHandler:` is callback-only and
    // can't be made synchronous without thread-blocking, which we
    // refuse on the UI thread.
}

/// Escape a Rust string for embedding inside a JS double-quoted string
/// literal. Handles the four characters that would otherwise break the
/// literal: backslash, double-quote, newline, carriage return. Does
/// NOT handle the full JSON-string escape table — callers that need
/// arbitrary structured payloads should JSON.stringify on the page
/// side rather than relying on this for safety.
fn escape_for_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}
