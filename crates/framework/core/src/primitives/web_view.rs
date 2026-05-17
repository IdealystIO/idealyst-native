//! WebView primitive — embedded native web view.
//!
//! Web: a non-sandboxed `<iframe>` (matches RN's wide-open default;
//! authors can layer their own sandbox attributes via raw style if
//! needed).
//! iOS: `WKWebView`.
//! Android: `android.webkit.WebView`.
//!
//! URL-only in v1. No JS bridge / message channel. The handle is a
//! placeholder for future methods like `reload()`, `go_back()`,
//! `go_forward()`, `execute_js(...)`.

use crate::{Bound, Primitive, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

#[derive(Clone)]
pub struct WebViewHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn WebViewOps,
}

impl WebViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn WebViewOps) -> Self {
        Self { node, ops }
    }
}

pub trait WebViewOps {
    // Reserved for reload(), go_back(), go_forward(), execute_js().
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
        style: None,
        ref_fill: None,
    })
}

impl Bound<WebViewHandle> {
    pub fn bind(mut self, r: Ref<WebViewHandle>) -> Self {
        if let Primitive::WebView { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::WebView(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
