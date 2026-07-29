//! Old-core suite: pins the `Element::External` lowering of the
//! authored surface (the byte-moved `oldcore` module) so the dual-core
//! restructure can't drift the default build.
#![cfg(not(feature = "new-core"))]

use std::any::TypeId;

use runtime_core::Element;
use webview::prelude::*;

/// `WebView(..)` lowers to `Element::External` keyed by
/// `WebViewProps`'s TypeId (backend handlers dispatch on it) and starts
/// childless.
#[test]
fn webview_builds_external_keyed_by_web_view_props() {
    let el: Element = WebView(WebViewProps::default()).into();
    match el {
        Element::External {
            type_id,
            type_name,
            children,
            ..
        } => {
            assert_eq!(type_id, TypeId::of::<WebViewProps>());
            assert!(type_name.contains("WebViewProps"));
            assert!(children.is_empty(), "no children by default");
        }
        _ => panic!("WebView must lower to Element::External"),
    }
}

/// The url closure rides the type-erased payload intact.
#[test]
fn url_closure_reaches_the_payload() {
    let el: Element = WebView(WebViewProps {
        url: url("https://example.com"),
        ..Default::default()
    })
    .into();
    match el {
        Element::External { payload, .. } => {
            let props = payload
                .downcast_ref::<WebViewProps>()
                .expect("payload is WebViewProps");
            assert_eq!((props.url)(), "https://example.com");
        }
        _ => panic!("expected Element::External"),
    }
}

/// The `WebView` tag builds the same External through `ui!` — the
/// PascalCase struct-literal dispatch (`pub type WebView =
/// WebViewProps` + `impl BuildElement`), mirroring form's
/// `form_via_ui_macro`.
#[test]
fn web_view_via_ui_macro() {
    use runtime_core::ui;

    let el: Element = ui! {
        WebView(url = webview::url("https://example.com"))
    };

    match el {
        Element::External {
            type_id,
            type_name,
            children,
            payload,
            ..
        } => {
            assert_eq!(type_id, TypeId::of::<WebViewProps>());
            assert!(type_name.contains("WebViewProps"));
            assert!(children.is_empty(), "tag form takes no children");
            let props = payload
                .downcast_ref::<WebViewProps>()
                .expect("payload is WebViewProps");
            assert_eq!((props.url)(), "https://example.com");
        }
        _ => panic!("ui! WebView tag must lower to Element::External"),
    }
}
