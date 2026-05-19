//! WebView primitive — embedded native web view.
//!
//! Web: a non-sandboxed `<iframe>` (matches RN's wide-open default;
//! authors can layer their own sandbox attributes via raw style if
//! needed).
//! iOS: `WKWebView`.
//! Android: `android.webkit.WebView`.
//!
//! Builder methods cover the message channel + lifecycle slots:
//! `.on_message(...)`, `.on_load(...)`, `.on_error(...)`. The
//! handle (filled via `bind(ref)`) exposes `post_message`,
//! `reload`, and `execute_js`. Backends that can't service a slot
//! ignore the callback / no-op the method.

use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

#[derive(Clone)]
pub struct WebViewHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn WebViewOps,
}

impl WebViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn WebViewOps) -> Self {
        Self { node, ops }
    }

    /// Send a message to the embedded content. Web maps to
    /// `iframe.contentWindow.postMessage(msg, "*")`; iOS/Android
    /// route through their native message channels. The payload is
    /// an opaque string — callers usually JSON-stringify a
    /// structured object on this side and parse on the other.
    pub fn post_message(&self, msg: &str) {
        self.ops.post_message(&*self.node, msg);
    }

    /// Soft-reload the embedded content. On web, re-sets the
    /// iframe's `src` to its current value, which the browser
    /// handles as a navigation.
    pub fn reload(&self) {
        self.ops.reload(&*self.node);
    }

    /// Synchronously evaluate JS inside the embedded content's
    /// global scope. Returns the result as a JSON string
    /// (`JSON.stringify(value)` of whatever the expression
    /// evaluated to). Returns `Err` if the embedded content is
    /// cross-origin, the call throws, or the backend doesn't
    /// support synchronous evaluation.
    ///
    /// Common shape: `.execute_js("window.setRows(1000)")` to
    /// invoke a function the embedded content has exposed on its
    /// global.
    pub fn execute_js(&self, code: &str) -> Result<String, String> {
        self.ops.execute_js(&*self.node, code)
    }
}

pub trait WebViewOps {
    fn post_message(&self, _node: &dyn Any, _msg: &str) {}
    fn reload(&self, _node: &dyn Any) {}
    fn execute_js(&self, _node: &dyn Any, _code: &str) -> Result<String, String> {
        Err("execute_js not supported by this backend".into())
    }
}

pub trait IntoWebViewUrl {
    fn into_web_view_url(self) -> Box<dyn Fn() -> String>;
}

impl IntoWebViewUrl for &str {
    fn into_web_view_url(self) -> Box<dyn Fn() -> String> {
        let s = self.to_string();
        Box::new(move || s.clone())
    }
}

impl IntoWebViewUrl for String {
    fn into_web_view_url(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self.clone())
    }
}

impl<F> IntoWebViewUrl for F
where
    F: Fn() -> String + 'static,
{
    fn into_web_view_url(self) -> Box<dyn Fn() -> String> {
        Box::new(self)
    }
}

pub fn web_view<U: IntoWebViewUrl>(url: U) -> Bound<WebViewHandle> {
    Bound::new(Primitive::WebView {
        url: url.into_web_view_url(),
        on_message: None,
        on_load: None,
        on_error: None,
        style: None,
        ref_fill: None,
    })
}

impl Bound<WebViewHandle> {
    /// Receive each `postMessage` from the embedded content.
    /// Callback gets the message payload as a string — typically a
    /// JSON document the embedded content stringified before
    /// posting.
    pub fn on_message<F: Fn(String) + 'static>(mut self, f: F) -> Self {
        if let Primitive::WebView { on_message, .. } = &mut self.primitive {
            *on_message = Some(Box::new(f));
        }
        self
    }

    /// Fired when the embedded content finishes loading. On web
    /// this is the iframe's `load` event.
    pub fn on_load<F: Fn() + 'static>(mut self, f: F) -> Self {
        if let Primitive::WebView { on_load, .. } = &mut self.primitive {
            *on_load = Some(Box::new(f));
        }
        self
    }

    /// Fired when the embedded content fails to load. On web this
    /// is the iframe's `error` event (which only covers a narrow
    /// set of failure modes — network errors inside the iframe's
    /// content don't bubble up here).
    pub fn on_error<F: Fn() + 'static>(mut self, f: F) -> Self {
        if let Primitive::WebView { on_error, .. } = &mut self.primitive {
            *on_error = Some(Box::new(f));
        }
        self
    }

    pub fn bind(mut self, r: Ref<WebViewHandle>) -> Self {
        if let Primitive::WebView { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::WebView(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
