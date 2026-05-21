//! iOS implementation of the WebView SDK.
//!
//! Builds a `WKWebView` via raw `msg_send` (see note in Cargo.toml on
//! why we don't lean on `objc2-web-kit`). Navigates via `loadRequest:`,
//! subscribes to reactive URL changes through `Effect::new(...)`.
//!
//! Callbacks (`on_message`/`on_load`/`on_error`) require a custom
//! NSObject conforming to `WKNavigationDelegate` /
//! `WKScriptMessageHandler`. Doable via `objc2::declare_class!` but
//! pushed to a follow-up — v1 matches the Android impl's parity (URL
//! only). The framework's previous iOS `create_web_view` was
//! `unimplemented!()`, so even URL-only is a net addition rather than
//! a regression.

use crate::{WebViewOps, WebViewProps};
// `backend-ios-mobile`'s `[lib].name` is `backend_ios` — historical
// staticlib filename preserved across the package rename.
use backend_ios::{IosBackend, IosNode};
use framework_core::Effect;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::{CGRect, NSObject, NSString, NSURL, NSURLRequest};
use objc2_ui_kit::UIView;
use std::any::Any;
use std::rc::Rc;

pub(crate) static OPS: &dyn WebViewOps = &IosWebViewOps;

pub fn register(backend: &mut IosBackend) {
    backend.register_external::<WebViewProps, _>(|props, b| build_web_view(props, b));
}

fn build_web_view(props: &Rc<WebViewProps>, b: &mut IosBackend) -> IosNode {
    // Look up WKWebView at the Obj-C runtime level. Equivalent to
    // `class!(WKWebView)`; explicit `AnyClass::get` makes the failure
    // path readable (missing class means WebKit framework isn't
    // linked, which would be a project misconfiguration).
    let wk_class: &AnyClass = AnyClass::get("WKWebView")
        .expect("WKWebView class not found — is WebKit linked into the app?");
    let cfg_class: &AnyClass = AnyClass::get("WKWebViewConfiguration")
        .expect("WKWebViewConfiguration class not found");

    // CGRect::ZERO frame: Taffy resizes us once the parent attaches +
    // lays out. WKWebView measures fine starting at 0×0 because we
    // drive `frame` from the layout pass.
    let zero_rect: CGRect = CGRect {
        origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
        size: objc2_foundation::CGSize { width: 0.0, height: 0.0 },
    };

    // Build a default WKWebViewConfiguration via raw `msg_send` —
    // objc2 0.5's `msg_send_id![alloc, init]` macro path expects
    // `Allocated<T>` retain semantics which need typed class bindings
    // we don't have here. The manual `from_raw` route is equivalent
    // at the runtime layer: `alloc` returns +1 retained, `init` either
    // preserves it (success) or returns nil (failure); we wrap the
    // resulting pointer in `Retained` once at the end.
    let config: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![cfg_class, alloc];
        let inited: *mut AnyObject = msg_send![allocated, init];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("WKWebViewConfiguration init returned nil")
    };

    // alloc/init WKWebView with the configuration. Same +1-retain
    // pattern as above.
    let webview_any: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![wk_class, alloc];
        let inited: *mut AnyObject =
            msg_send![allocated, initWithFrame: zero_rect, configuration: &*config];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("WKWebView init returned nil")
    };
    // WKWebView : UIView : UIResponder : NSObject — same pointer at
    // every level. The cast is sound at runtime; Obj-C dispatch on the
    // result still reaches WKWebView's selectors.
    let webview_uiview: Retained<UIView> = unsafe { Retained::cast(webview_any) };

    b.register_external_view(&webview_uiview);

    // Reactive URL — initial fire navigates the just-created
    // WKWebView; subsequent fires re-navigate when signals captured
    // in `url()` change. Owned by the walker's active scope so the
    // returned `_effect` going out of scope is fine.
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
    // `loadRequest:` is a WKWebView method — Obj-C dispatch on the
    // receiver's real class reaches it even though `webview` is typed
    // as UIView here.
    let _: () = unsafe { msg_send![webview, loadRequest: &*request] };
}

// ============================================================================
// Imperative ops
// ============================================================================

struct IosWebViewOps;

impl WebViewOps for IosWebViewOps {
    fn reload(&self, node: &dyn Any) {
        let Some(IosNode::View(uiview)) = node.downcast_ref::<IosNode>() else {
            return;
        };
        // WKWebView's `reload` selector. Obj-C dispatches on the
        // receiver's real class so this reaches WKWebView's `reload`.
        let _: () = unsafe { msg_send![uiview, reload] };
    }

    // `post_message` + `execute_js` need a JS bridge: a
    // `WKScriptMessageHandler` conformer for `post_message`, and a
    // `WKWebView.evaluateJavaScript:completionHandler:` callback for
    // `execute_js` (which is async — sync trait signature can't be
    // honored without a thread-blocking shim). Both pushed to a
    // follow-up.
}
