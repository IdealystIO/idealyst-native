//! Checked casts (`JsCast::is_type_of` / `dyn_into` / `dyn_ref`) for every
//! class web-glue declares, in a real browser.
//!
//! Each `js_class!` type gets its own `instanceof` import naming its
//! constructor (see the `js_class!` docs in `src/cast.rs`). These tests pin
//! that every one of those generated checkers recognises its class and the
//! ancestors it declares, rejects what it should, and — the regression the
//! per-class import fixes — that a cast does not ship and decode the class
//! name on every call.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_glue::dom::*;
use web_glue::js::{Array, ArrayBuffer, Function, Map, Object, Promise, Set, Uint32Array, Uint8Array};
use web_glue::worker::Worker;
use web_glue::{JsCast, JsValue};

wasm_bindgen_test_configure!(run_in_browser);

/// `new Function(body)` — a JS helper for the test's own probes.
fn js_fn(args: &str, body: &str) -> JsValue {
    let mut ctor_args: Vec<JsValue> = args.split(',').filter(|a| !a.is_empty()).map(JsValue::from_str).collect();
    ctor_args.push(JsValue::from_str(body));
    let refs: Vec<&JsValue> = ctor_args.iter().collect();
    JsValue::global().get("Function").unwrap().construct(&refs).expect("new Function")
}

/// An object whose prototype is `globalThis[class].prototype`: `instanceof`
/// walks the prototype chain, so this is an instance of the class and of
/// every class it really extends, without having to construct one (most
/// DOM classes have no public constructor).
fn instance_by_proto(class: &str) -> JsValue {
    js_fn("c", "const C = globalThis[c]; return C ? Object.create(C.prototype) : undefined;")
        .call(&JsValue::NULL, &[&JsValue::from_str(class)])
        .unwrap()
}

/// Every class the test checks, in `js_class!` syntax: name, declared
/// ancestors, JS constructor. Compared against the crate's sources by
/// `the_checked_list_is_every_declared_class`, so a newly declared class
/// that is missing here fails the suite.
macro_rules! classes {
    ($($name:ident $(: $($anc:ident),+)? = $class:literal;)*) => {
        const CHECKED: &[(&str, &str)] = &[$((stringify!($name), $class)),*];

        fn check_all() {
            let not_an_object = JsValue::from_f64(5.0);
            let plain = js_fn("", "return {};").call(&JsValue::NULL, &[]).unwrap();
            $(
                let v = instance_by_proto($class);
                assert!(!v.is_undefined(), "{} is not a global in this browser", $class);
                assert!($name::is_type_of(&v), "{}::is_type_of rejects an instance of {}", stringify!($name), $class);
                assert!(v.dyn_ref::<$name>().is_some(), "dyn_ref::<{}>", stringify!($name));
                $($(
                    assert!(
                        $anc::is_type_of(&v),
                        "{} declares ancestor {}, but an instance of {} is not one",
                        stringify!($name), stringify!($anc), $class,
                    );
                )+)?
                assert!(!$name::is_type_of(&not_an_object), "{} accepts a number", stringify!($name));
                if $class != "Object" {
                    assert!(!$name::is_type_of(&plain), "{} accepts a plain object", stringify!($name));
                }
                assert!(v.clone().dyn_into::<$name>().is_ok());
            )*
        }
    };
}

classes! {
    EventTarget = "EventTarget";
    Node: EventTarget = "Node";
    Element: Node, EventTarget = "Element";
    HtmlElement: Element, Node, EventTarget = "HTMLElement";
    HtmlInputElement: HtmlElement, Element, Node, EventTarget = "HTMLInputElement";
    HtmlTextAreaElement: HtmlElement, Element, Node, EventTarget = "HTMLTextAreaElement";
    Text: Node, EventTarget = "Text";
    Document: Node, EventTarget = "Document";
    Window: EventTarget = "Window";
    MediaQueryList: EventTarget = "MediaQueryList";
    DocumentFragment: Node, EventTarget = "DocumentFragment";
    HtmlHeadElement: HtmlElement, Element, Node, EventTarget = "HTMLHeadElement";
    HtmlStyleElement: HtmlElement, Element, Node, EventTarget = "HTMLStyleElement";
    HtmlCanvasElement: HtmlElement, Element, Node, EventTarget = "HTMLCanvasElement";
    HtmlAnchorElement: HtmlElement, Element, Node, EventTarget = "HTMLAnchorElement";
    HtmlImageElement: HtmlElement, Element, Node, EventTarget = "HTMLImageElement";
    HtmlIFrameElement: HtmlElement, Element, Node, EventTarget = "HTMLIFrameElement";
    HtmlOptionElement: HtmlElement, Element, Node, EventTarget = "HTMLOptionElement";
    SvgElement: Element, Node, EventTarget = "SVGElement";
    NodeList = "NodeList";
    HtmlCollection = "HTMLCollection";
    DomTokenList = "DOMTokenList";
    DomRectReadOnly = "DOMRectReadOnly";
    DomRect: DomRectReadOnly = "DOMRect";
    CssStyleDeclaration = "CSSStyleDeclaration";
    StyleSheet = "StyleSheet";
    CssStyleSheet: StyleSheet = "CSSStyleSheet";
    CssRuleList = "CSSRuleList";
    CssRule = "CSSRule";
    CssStyleRule: CssRule = "CSSStyleRule";
    CssMediaRule: CssRule = "CSSMediaRule";
    History = "History";
    Location = "Location";
    Navigator = "Navigator";
    ResizeObserver = "ResizeObserver";
    ResizeObserverEntry = "ResizeObserverEntry";
    FontFace = "FontFace";
    FontFaceSet: EventTarget = "FontFaceSet";
    WebSocket: EventTarget = "WebSocket";
    MessageEvent: Event = "MessageEvent";
    CloseEvent: Event = "CloseEvent";
    Response = "Response";
    Performance = "Performance";
    XmlSerializer = "XMLSerializer";
    XmlHttpRequest: EventTarget = "XMLHttpRequest";
    CanvasRenderingContext2d = "CanvasRenderingContext2D";
    Event = "Event";
    UiEvent: Event = "UIEvent";
    MouseEvent: UiEvent, Event = "MouseEvent";
    PointerEvent: MouseEvent, UiEvent, Event = "PointerEvent";
    WheelEvent: MouseEvent, UiEvent, Event = "WheelEvent";
    DragEvent: MouseEvent, UiEvent, Event = "DragEvent";
    KeyboardEvent: UiEvent, Event = "KeyboardEvent";
    FocusEvent: UiEvent, Event = "FocusEvent";
    DataTransfer = "DataTransfer";
    FileList = "FileList";
    Blob = "Blob";
    File: Blob = "File";
    MediaStream: EventTarget = "MediaStream";
    MediaStreamTrack: EventTarget = "MediaStreamTrack";
    Object = "Object";
    Array: Object = "Array";
    Function: Object = "Function";
    Promise: Object = "Promise";
    ArrayBuffer: Object = "ArrayBuffer";
    Uint8Array: Object = "Uint8Array";
    Uint32Array: Object = "Uint32Array";
    Set: Object = "Set";
    Map: Object = "Map";
    Worker: EventTarget = "Worker";
}

/// `(struct name, JS class)` of every `js_class!` entry in `src`.
fn declared(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("crate::js_class!") {
            inside = true;
        } else if inside && t == "}" {
            inside = false;
        } else if inside && t.starts_with("pub struct ") {
            let rest = &t["pub struct ".len()..];
            let name = rest.split(|c: char| c == ':' || c == ' ').next().unwrap().to_string();
            let class = rest.split('"').nth(1).expect("class string").to_string();
            out.push((name, class));
        }
    }
    out
}

#[wasm_bindgen_test]
fn the_checked_list_is_every_declared_class() {
    let mut want: Vec<(String, String)> = [
        include_str!("../src/dom.rs"),
        include_str!("../src/js.rs"),
        include_str!("../src/worker.rs"),
    ]
    .iter()
    .flat_map(|s| declared(s))
    .collect();
    let mut got: Vec<(String, String)> = CHECKED.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect();
    want.sort();
    got.sort();
    assert!(!want.is_empty(), "found no js_class! entries — has the macro's spelling changed?");
    assert_eq!(got, want, "tests/web_cast.rs must check every class web-glue declares");
}

#[wasm_bindgen_test]
fn every_declared_class_checker_accepts_its_class_and_ancestors() {
    check_all();
}

#[wasm_bindgen_test]
fn checkers_see_real_objects_and_reject_siblings() {
    let doc = window().unwrap().document().unwrap();
    let input: JsValue = doc.create_element("input").unwrap().into();
    assert!(input.is_instance_of::<HtmlInputElement>());
    assert!(input.is_instance_of::<HtmlElement>() && input.is_instance_of::<Element>());
    assert!(input.is_instance_of::<Node>() && input.is_instance_of::<EventTarget>());
    assert!(!input.is_instance_of::<HtmlTextAreaElement>());
    assert!(!input.is_instance_of::<Text>());
    let text: JsValue = doc.create_text_node("x").into();
    assert!(text.is_instance_of::<Text>() && text.is_instance_of::<Node>());
    assert!(!text.is_instance_of::<Element>());
    let d: JsValue = doc.clone().into();
    assert!(d.is_instance_of::<Document>() && !d.is_instance_of::<Element>());
    // A failed `dyn_into` hands the same value back.
    let back = text.clone().dyn_into::<Element>().unwrap_err();
    assert!(back.strict_eq(&text));
}

/// Regression: every checked cast used to pass the class name as a string
/// and `TextDecoder.decode` it in JS before the `instanceof` — 67 k
/// decodes per benchmark rebuild, which took backend-web's batch decode
/// from 7 ms to 31 ms. The runtime's decoder is a `TextDecoder`, whose
/// `decode` is looked up on the prototype at call time, so wrapping the
/// prototype method counts every decode the glue does.
#[wasm_bindgen_test]
fn regression_checked_cast_decodes_no_string_per_call() {
    let doc = window().unwrap().document().unwrap();
    let el: JsValue = doc.create_element("div").unwrap().into();
    let install = js_fn(
        "",
        "const P = TextDecoder.prototype; const orig = P.decode; globalThis.__castDecodes = 0; \
         P.decode = function (...a) { globalThis.__castDecodes++; return orig.apply(this, a); }; \
         return () => { P.decode = orig; return globalThis.__castDecodes; };",
    );
    let restore = install.call(&JsValue::NULL, &[]).unwrap();
    let mut hits = 0;
    for _ in 0..1000 {
        hits += el.dyn_ref::<HtmlElement>().is_some() as u32;
        hits += el.dyn_ref::<HtmlInputElement>().is_some() as u32;
    }
    let decodes = restore.call(&JsValue::NULL, &[]).unwrap().as_f64().unwrap();
    assert_eq!(hits, 1000);
    assert_eq!(decodes, 0.0, "2000 checked casts decoded {decodes} strings");
}
