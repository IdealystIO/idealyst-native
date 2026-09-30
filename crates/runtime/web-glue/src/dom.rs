//! Typed DOM handles: the classes the framework reaches the browser
//! through, with checked casts ([`crate::cast`]).
//!
//! The hierarchy mirrors the DOM's (and web-sys's): `HtmlInputElement`
//! derefs to `HtmlElement`, which derefs to `Element`, `Node`,
//! `EventTarget`. Only the members the framework calls are bound — this is
//! a surface that grows with each port, not a generated copy of WebIDL.
//!
//! Every binding is one `import!` snippet; handles come back as fresh slab
//! slots (`G.add`), `null`/`undefined` results come back as slot 0 and
//! surface as `None`.

use crate::cast::JsCast;
use crate::{string, Closure, JsValue};

crate::js_class! {
    /// `EventTarget`.
    pub struct EventTarget = "EventTarget";
    /// `Node`.
    pub struct Node: EventTarget = "Node";
    /// `Element`.
    pub struct Element: Node, EventTarget = "Element";
    /// `HTMLElement`.
    pub struct HtmlElement: Element, Node, EventTarget = "HTMLElement";
    /// `HTMLInputElement`.
    pub struct HtmlInputElement: HtmlElement, Element, Node, EventTarget = "HTMLInputElement";
    /// `HTMLTextAreaElement`.
    pub struct HtmlTextAreaElement: HtmlElement, Element, Node, EventTarget = "HTMLTextAreaElement";
    /// `Text` (a text node).
    pub struct Text: Node, EventTarget = "Text";
    /// `Document`.
    pub struct Document: Node, EventTarget = "Document";
    /// `Window`.
    pub struct Window: EventTarget = "Window";
    /// `MediaQueryList`.
    pub struct MediaQueryList: EventTarget = "MediaQueryList";

    /// `Event`.
    pub struct Event = "Event";
    /// `UIEvent`.
    pub struct UiEvent: Event = "UIEvent";
    /// `MouseEvent`.
    pub struct MouseEvent: UiEvent, Event = "MouseEvent";
    /// `PointerEvent`.
    pub struct PointerEvent: MouseEvent, UiEvent, Event = "PointerEvent";
    /// `WheelEvent`.
    pub struct WheelEvent: MouseEvent, UiEvent, Event = "WheelEvent";
    /// `DragEvent`.
    pub struct DragEvent: MouseEvent, UiEvent, Event = "DragEvent";
    /// `KeyboardEvent`.
    pub struct KeyboardEvent: UiEvent, Event = "KeyboardEvent";
    /// `FocusEvent`.
    pub struct FocusEvent: UiEvent, Event = "FocusEvent";

    /// `DataTransfer`.
    pub struct DataTransfer = "DataTransfer";
    /// `FileList`.
    pub struct FileList = "FileList";
    /// `Blob`.
    pub struct Blob = "Blob";
    /// `File`.
    pub struct File: Blob = "File";
}

/// Slot 0 (`null` / `undefined` mapped by the snippet) → `None`.
fn opt<T: JsCast>(idx: u32) -> Option<T> {
    (idx != 0).then(|| T::unchecked_from_js(unsafe { JsValue::from_raw(idx) }))
}

fn owned<T: JsCast>(idx: u32) -> T {
    T::unchecked_from_js(unsafe { JsValue::from_raw(idx) })
}

crate::import! {
    // ---- globals ---------------------------------------------------------
    fn js_window() -> u32 = "() => typeof window === 'undefined' ? 0 : G.add(window)";
    fn js_document(w: u32) -> u32 = "(w) => G.add(G.get(w).document)";
    fn js_body(d: u32) -> u32 = "(d) => { const b = G.get(d).body; return b == null ? 0 : G.add(b); }";

    // ---- timers / frames --------------------------------------------------
    fn js_raf(w: u32, f: u32) -> i32 = "(w, f) => G.get(w).requestAnimationFrame(G.get(f))";
    fn js_cancel_raf(w: u32, h: i32) = "(w, h) => { G.get(w).cancelAnimationFrame(h); }";
    fn js_set_timeout(w: u32, f: u32, ms: i32) -> i32 = "(w, f, ms) => G.get(w).setTimeout(G.get(f), ms)";
    fn js_clear_timeout(w: u32, h: i32) = "(w, h) => { G.get(w).clearTimeout(h); }";
    fn js_perf_now() -> f64 = "() => performance.now()";
    fn js_date_now() -> f64 = "() => Date.now()";
    fn js_tz_offset() -> f64 = "() => new Date().getTimezoneOffset()";
    fn js_inner_width(w: u32) -> f64 = "(w) => G.get(w).innerWidth";
    fn js_inner_height(w: u32) -> f64 = "(w) => G.get(w).innerHeight";
    fn js_match_media(w: u32, p: usize, l: usize) -> u32 =
        "(w, p, l) => { const m = G.get(w).matchMedia(G.str(p, l)); return m == null ? 0 : G.add(m); }";
    fn js_mql_matches(m: u32) -> u32 = "(m) => G.get(m).matches ? 1 : 0";

    // ---- EventTarget -------------------------------------------------------
    // flags: 1 capture, 2 passive specified, 4 passive value, 8 once.
    fn js_add_listener(t: u32, p: usize, l: usize, f: u32, flags: u32) =
        "(t, p, l, f, fl) => { G.get(t).addEventListener(G.str(p, l), G.get(f), \
           (fl & 2) ? { capture: !!(fl & 1), passive: !!(fl & 4), once: !!(fl & 8) } \
                    : { capture: !!(fl & 1), once: !!(fl & 8) }); }";
    fn js_remove_listener(t: u32, p: usize, l: usize, f: u32, capture: u32) =
        "(t, p, l, f, c) => { G.get(t).removeEventListener(G.str(p, l), G.get(f), c !== 0); }";

    // ---- Node / Element ------------------------------------------------------
    fn js_text_content(n: u32, out: usize) -> u32 =
        "(n, o) => { const t = G.get(n).textContent; if (t == null) return 0; G.retStr(t, o); return 1; }";
    fn js_parent_element(n: u32) -> u32 = "(n) => { const p = G.get(n).parentElement; return p == null ? 0 : G.add(p); }";
    fn js_is_same_node(a: u32, b: u32) -> u32 = "(a, b) => G.get(a) === G.get(b) ? 1 : 0";
    fn js_contains(a: u32, b: u32) -> u32 = "(a, b) => G.get(a).contains(G.get(b)) ? 1 : 0";
    fn js_tag_name(e: u32, out: usize) = "(e, o) => G.retStr(G.get(e).tagName, o)";
    #[catch]
    fn js_closest(e: u32, p: usize, l: usize) -> u32 =
        "(e, p, l) => { const c = G.get(e).closest(G.str(p, l)); return c == null ? 0 : G.add(c); }";
    fn js_class_name(e: u32, out: usize) =
        "(e, o) => { const c = G.get(e).className; G.retStr(typeof c === 'string' ? c : (c && c.baseVal) || '', o); }";
    fn js_get_attribute(e: u32, p: usize, l: usize, out: usize) -> u32 =
        "(e, p, l, o) => { const v = G.get(e).getAttribute(G.str(p, l)); if (v == null) return 0; G.retStr(v, o); return 1; }";

    // ---- Event -----------------------------------------------------------------
    fn js_ev_type(e: u32, out: usize) = "(e, o) => G.retStr(G.get(e).type, o)";
    fn js_ev_target(e: u32) -> u32 = "(e) => { const t = G.get(e).target; return t == null ? 0 : G.add(t); }";
    fn js_ev_current_target(e: u32) -> u32 = "(e) => { const t = G.get(e).currentTarget; return t == null ? 0 : G.add(t); }";
    fn js_ev_prevent_default(e: u32) = "(e) => { G.get(e).preventDefault(); }";
    fn js_ev_stop_propagation(e: u32) = "(e) => { G.get(e).stopPropagation(); }";
    fn js_ev_stop_immediate(e: u32) = "(e) => { G.get(e).stopImmediatePropagation(); }";
    fn js_ev_default_prevented(e: u32) -> u32 = "(e) => G.get(e).defaultPrevented ? 1 : 0";
    fn js_ev_time_stamp(e: u32) -> f64 = "(e) => G.get(e).timeStamp";
    fn js_ev_num(e: u32, p: usize, l: usize) -> f64 = "(e, p, l) => +G.get(e)[G.str(p, l)]";
    fn js_ev_flag(e: u32, p: usize, l: usize) -> u32 = "(e, p, l) => G.get(e)[G.str(p, l)] ? 1 : 0";
    fn js_ev_str(e: u32, p: usize, l: usize, out: usize) =
        "(e, p, l, o) => { const v = G.get(e)[G.str(p, l)]; G.retStr(v == null ? '' : String(v), o); }";
    fn js_ev_obj(e: u32, p: usize, l: usize) -> u32 =
        "(e, p, l) => { const v = G.get(e)[G.str(p, l)]; return v == null ? 0 : G.add(v); }";

    // ---- FileList ----------------------------------------------------------------
    fn js_dt_types(d: u32, out: usize) =
        "(d, o) => G.retStr(Array.from(G.get(d).types).join('\\n'), o)";
    fn js_file_item(f: u32, i: u32) -> u32 = "(f, i) => { const v = G.get(f).item(i); return v == null ? 0 : G.add(v); }";
}

/// The global `window`, if this is a window context.
pub fn window() -> Option<Window> {
    opt(unsafe { js_window() })
}

impl Window {
    pub fn document(&self) -> Document {
        owned(unsafe { js_document(self.0.raw()) })
    }

    /// `requestAnimationFrame(f)` → the frame handle.
    pub fn request_animation_frame(&self, f: &Closure) -> i32 {
        unsafe { js_raf(self.0.raw(), f.as_js().raw()) }
    }

    pub fn cancel_animation_frame(&self, handle: i32) {
        unsafe { js_cancel_raf(self.0.raw(), handle) }
    }

    /// `setTimeout(f, ms)` → the timer handle.
    pub fn set_timeout(&self, f: &Closure, ms: i32) -> i32 {
        unsafe { js_set_timeout(self.0.raw(), f.as_js().raw(), ms) }
    }

    pub fn clear_timeout(&self, handle: i32) {
        unsafe { js_clear_timeout(self.0.raw(), handle) }
    }

    pub fn inner_width(&self) -> f64 {
        unsafe { js_inner_width(self.0.raw()) }
    }

    pub fn inner_height(&self) -> f64 {
        unsafe { js_inner_height(self.0.raw()) }
    }

    pub fn match_media(&self, query: &str) -> Option<MediaQueryList> {
        let (p, l) = string::abi(query);
        opt(unsafe { js_match_media(self.0.raw(), p, l) })
    }
}

impl Document {
    pub fn body(&self) -> Option<HtmlElement> {
        opt(unsafe { js_body(self.0.raw()) })
    }
}

impl MediaQueryList {
    pub fn matches(&self) -> bool {
        unsafe { js_mql_matches(self.0.raw()) != 0 }
    }
}

/// `performance.now()` in milliseconds.
pub fn performance_now() -> f64 {
    unsafe { js_perf_now() }
}

/// `Date.now()` in milliseconds since the epoch.
pub fn date_now() -> f64 {
    unsafe { js_date_now() }
}

/// `new Date().getTimezoneOffset()` — minutes, UTC minus local.
pub fn timezone_offset_minutes() -> f64 {
    unsafe { js_tz_offset() }
}

/// `addEventListener` options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListenerOptions {
    pub capture: bool,
    /// `None` leaves `passive` to the browser's default for the event.
    pub passive: Option<bool>,
    pub once: bool,
}

impl ListenerOptions {
    pub const CAPTURE: ListenerOptions = ListenerOptions { capture: true, passive: None, once: false };

    fn flags(self) -> u32 {
        let mut f = 0;
        if self.capture {
            f |= 1;
        }
        if let Some(p) = self.passive {
            f |= 2;
            if p {
                f |= 4;
            }
        }
        if self.once {
            f |= 8;
        }
        f
    }
}

impl EventTarget {
    /// `addEventListener(ty, f, options)`. The target holds a strong
    /// reference to `f`'s JS function; `f` itself (the Rust side) must be
    /// kept alive by the caller for as long as the listener should run —
    /// see [`Listener`] for the owner that detaches on drop.
    pub fn add_event_listener(&self, ty: &str, f: &Closure, options: ListenerOptions) {
        let (p, l) = string::abi(ty);
        unsafe { js_add_listener(self.0.raw(), p, l, f.as_js().raw(), options.flags()) }
    }

    /// `removeEventListener(ty, f, capture)`.
    pub fn remove_event_listener(&self, ty: &str, f: &Closure, capture: bool) {
        let (p, l) = string::abi(ty);
        unsafe { js_remove_listener(self.0.raw(), p, l, f.as_js().raw(), capture as u32) }
    }
}

/// A listener that owns its closure and DETACHES before the closure drops.
///
/// Dropping a [`Closure`] revokes its JS function, but the target still
/// holds it; a later dispatch then throws "called after its Rust owner
/// dropped it". Removing the listener first makes the dead function
/// unreachable instead — the invariant `backend_web::TrackedListener`
/// documents (a focused `<input>` fires `blur` during its own removal,
/// after the node's teardown already ran).
pub struct Listener {
    target: EventTarget,
    ty: &'static str,
    capture: bool,
    closure: Closure,
}

impl Listener {
    /// Attach `f` to `target` for `ty`.
    pub fn new(
        target: EventTarget,
        ty: &'static str,
        options: ListenerOptions,
        f: impl FnMut(Event) + 'static,
    ) -> Listener {
        let mut f = f;
        let closure = Closure::new(move |v: JsValue| f(Event::unchecked_from_js(v)));
        target.add_event_listener(ty, &closure, options);
        Listener { target, ty, capture: options.capture, closure }
    }

    /// Like [`Listener::new`], but `f` may be re-entered (a `scroll`
    /// handler whose body re-fires `scroll`) — see [`Closure::new_fn`].
    pub fn new_fn(
        target: EventTarget,
        ty: &'static str,
        options: ListenerOptions,
        f: impl Fn(Event) + 'static,
    ) -> Listener {
        let closure = Closure::new_fn(move |v: JsValue| f(Event::unchecked_from_js(v)));
        target.add_event_listener(ty, &closure, options);
        Listener { target, ty, capture: options.capture, closure }
    }

    /// Hand the listener to its target for good: the target keeps the JS
    /// function alive, and the Rust closure is released when JS collects
    /// the function (with the target) — see
    /// [`Closure::into_js_value`]. For listeners that live exactly as long
    /// as their element and are never detached.
    pub fn into_target_owned(self) {
        let me = std::mem::ManuallyDrop::new(self);
        // SAFETY: `me` is never dropped; each field is read exactly once,
        // so both move out. Dropping `target` releases one slab slot.
        let (target, closure) = unsafe { (std::ptr::read(&me.target), std::ptr::read(&me.closure)) };
        drop(target);
        drop(closure.into_js_value());
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Runs before the fields drop, so the closure is still registered —
        // which is what `removeEventListener` needs to match.
        self.target.remove_event_listener(self.ty, &self.closure, self.capture);
    }
}

impl Node {
    /// `textContent`; `None` for a document or doctype.
    pub fn text_content(&self) -> Option<String> {
        let mut has = 0;
        let s = string::receive(|o| has = unsafe { js_text_content(self.0.raw(), o) });
        (has != 0).then_some(s)
    }

    pub fn parent_element(&self) -> Option<Element> {
        opt(unsafe { js_parent_element(self.0.raw()) })
    }

    /// Identity (`===`).
    pub fn is_same_node(&self, other: &Node) -> bool {
        unsafe { js_is_same_node(self.0.raw(), other.0.raw()) != 0 }
    }

    pub fn contains(&self, other: &Node) -> bool {
        unsafe { js_contains(self.0.raw(), other.0.raw()) != 0 }
    }
}

impl Element {
    pub fn tag_name(&self) -> String {
        string::receive(|o| unsafe { js_tag_name(self.0.raw(), o) })
    }

    /// The `class` attribute (an SVG element's `className.baseVal`).
    pub fn class_name(&self) -> String {
        string::receive(|o| unsafe { js_class_name(self.0.raw(), o) })
    }

    /// `closest(selector)` — `Err` for an invalid selector.
    pub fn closest(&self, selector: &str) -> Result<Option<Element>, crate::JsError> {
        let (p, l) = string::abi(selector);
        unsafe { js_closest(self.0.raw(), p, l) }.map(opt)
    }

    pub fn get_attribute(&self, name: &str) -> Option<String> {
        let (p, l) = string::abi(name);
        let mut has = 0;
        let s = string::receive(|o| has = unsafe { js_get_attribute(self.0.raw(), p, l, o) });
        (has != 0).then_some(s)
    }
}

fn num(v: &JsValue, key: &str) -> f64 {
    let (p, l) = string::abi(key);
    unsafe { js_ev_num(v.raw(), p, l) }
}

fn flag(v: &JsValue, key: &str) -> bool {
    let (p, l) = string::abi(key);
    unsafe { js_ev_flag(v.raw(), p, l) != 0 }
}

fn text(v: &JsValue, key: &str) -> String {
    let (p, l) = string::abi(key);
    string::receive(|o| unsafe { js_ev_str(v.raw(), p, l, o) })
}

fn obj<T: JsCast>(v: &JsValue, key: &str) -> Option<T> {
    let (p, l) = string::abi(key);
    opt(unsafe { js_ev_obj(v.raw(), p, l) })
}

impl Event {
    /// `type`.
    pub fn type_(&self) -> String {
        string::receive(|o| unsafe { js_ev_type(self.0.raw(), o) })
    }
    pub fn target(&self) -> Option<EventTarget> {
        opt(unsafe { js_ev_target(self.0.raw()) })
    }
    pub fn current_target(&self) -> Option<EventTarget> {
        opt(unsafe { js_ev_current_target(self.0.raw()) })
    }
    pub fn prevent_default(&self) {
        unsafe { js_ev_prevent_default(self.0.raw()) }
    }
    pub fn stop_propagation(&self) {
        unsafe { js_ev_stop_propagation(self.0.raw()) }
    }
    pub fn stop_immediate_propagation(&self) {
        unsafe { js_ev_stop_immediate(self.0.raw()) }
    }
    pub fn default_prevented(&self) -> bool {
        unsafe { js_ev_default_prevented(self.0.raw()) != 0 }
    }
    pub fn time_stamp(&self) -> f64 {
        unsafe { js_ev_time_stamp(self.0.raw()) }
    }
}

impl MouseEvent {
    pub fn client_x(&self) -> i32 {
        num(&self.0, "clientX") as i32
    }
    pub fn client_y(&self) -> i32 {
        num(&self.0, "clientY") as i32
    }
    pub fn button(&self) -> i16 {
        num(&self.0, "button") as i16
    }
    pub fn buttons(&self) -> u16 {
        num(&self.0, "buttons") as u16
    }
    pub fn ctrl_key(&self) -> bool {
        flag(&self.0, "ctrlKey")
    }
    pub fn shift_key(&self) -> bool {
        flag(&self.0, "shiftKey")
    }
    pub fn alt_key(&self) -> bool {
        flag(&self.0, "altKey")
    }
    pub fn meta_key(&self) -> bool {
        flag(&self.0, "metaKey")
    }
}

impl PointerEvent {
    pub fn pointer_id(&self) -> i32 {
        num(&self.0, "pointerId") as i32
    }
    pub fn pointer_type(&self) -> String {
        text(&self.0, "pointerType")
    }
    pub fn pressure(&self) -> f32 {
        num(&self.0, "pressure") as f32
    }
}

impl WheelEvent {
    pub fn delta_x(&self) -> f64 {
        num(&self.0, "deltaX")
    }
    pub fn delta_y(&self) -> f64 {
        num(&self.0, "deltaY")
    }
    pub fn delta_z(&self) -> f64 {
        num(&self.0, "deltaZ")
    }
    pub fn delta_mode(&self) -> u32 {
        num(&self.0, "deltaMode") as u32
    }
}

impl DragEvent {
    pub fn data_transfer(&self) -> Option<DataTransfer> {
        obj(&self.0, "dataTransfer")
    }
}

impl KeyboardEvent {
    pub fn key(&self) -> String {
        text(&self.0, "key")
    }
    pub fn code(&self) -> String {
        text(&self.0, "code")
    }
    pub fn ctrl_key(&self) -> bool {
        flag(&self.0, "ctrlKey")
    }
    pub fn shift_key(&self) -> bool {
        flag(&self.0, "shiftKey")
    }
    pub fn alt_key(&self) -> bool {
        flag(&self.0, "altKey")
    }
    pub fn meta_key(&self) -> bool {
        flag(&self.0, "metaKey")
    }
    pub fn repeat(&self) -> bool {
        flag(&self.0, "repeat")
    }
    pub fn is_composing(&self) -> bool {
        flag(&self.0, "isComposing")
    }
}

impl DataTransfer {
    pub fn files(&self) -> Option<FileList> {
        obj(&self.0, "files")
    }
    /// `types` — the drag's data formats (`"Files"` for an OS file drag).
    pub fn types(&self) -> Vec<String> {
        let joined = string::receive(|o| unsafe { js_dt_types(self.0.raw(), o) });
        if joined.is_empty() {
            Vec::new()
        } else {
            joined.split('\n').map(str::to_owned).collect()
        }
    }
}

impl FileList {
    pub fn length(&self) -> u32 {
        num(&self.0, "length") as u32
    }
    pub fn get(&self, index: u32) -> Option<File> {
        opt(unsafe { js_file_item(self.0.raw(), index) })
    }
}

impl Blob {
    pub fn size(&self) -> f64 {
        num(&self.0, "size")
    }
    /// `type` (the MIME type, `""` when unknown).
    pub fn type_(&self) -> String {
        text(&self.0, "type")
    }
}

impl File {
    pub fn name(&self) -> String {
        text(&self.0, "name")
    }
}
