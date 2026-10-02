//! The host half: turn a bundle's [`Node`] into an `Element` of real prims
//! whose closures call the bundle back through a [`Link`].
//!
//! Every decoded closure holds a [`CbRef`]: dropping the last one releases
//! the id, and the bundle frees what it held. The tree is realized by the
//! app's own registry afterwards; nothing downstream knows it is remote.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::{Mutex, OnceLock};

use runtime_scene::{dyn_element, dyn_guarded, Element};
use runtime_shared::accessibility::{AccessibilityAction, AccessibilityProps, AccessibilityTraits};
use runtime_shared::{Action, SafeAreaSides, SheetPart, StyleApplication, StyleRules, StyleSheet, VariantSet};
use runtime_world::Value;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::*;
use crate::prims::*;
use crate::style_attach::StyleProp;

/// How the host reaches one bundle's callback table. In-process for the
/// codec's tests; over wasm in `stream-host`.
pub trait Link: 'static {
    /// Run callback `cb` with `args`; its encoded reply.
    fn call(&self, cb: Cb, args: &[u8]) -> Vec<u8>;
    /// The host dropped one copy of `cb`.
    fn release(&self, cb: Cb);
}

/// Why a remote tree did not decode.
#[derive(Debug, PartialEq)]
pub enum DecodeError {
    /// The bytes are not a [`Node`].
    Malformed(String),
    /// The bundle uses an app component the app does not export — a bundle
    /// built against a newer app than the one running it.
    MissingImport(String),
    /// The app's component `name` rejected the props the bundle sent.
    BadProps { name: String, reason: String },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Malformed(e) => write!(f, "remote tree does not decode: {e}"),
            DecodeError::MissingImport(n) => {
                write!(f, "the remote component uses app component `{n}`, which this app does not export")
            }
            DecodeError::BadProps { name, reason } => {
                write!(f, "the remote component's props for app component `{name}` do not decode: {reason}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// One bundle, as the decoded tree reaches it.
struct Conn {
    link: Rc<dyn Link>,
    /// Proxy sheets by bundle sheet id, so every node styled by one bundle
    /// sheet shares one host sheet (and its variant cache).
    sheets: RefCell<HashMap<Cb, Weak<StyleSheet>>>,
}

/// One received copy of a callback id; released on drop.
struct CbRef {
    id: Cb,
    conn: Rc<Conn>,
}

impl Drop for CbRef {
    fn drop(&mut self) {
        self.conn.link.release(self.id);
    }
}

impl CbRef {
    fn call(&self, args: &[u8]) -> Vec<u8> {
        self.conn.link.call(self.id, args)
    }

    /// Call and decode the reply. A reply that does not decode is the two
    /// sides disagreeing about the protocol: a bug, so it panics.
    fn get<T: DeserializeOwned>(&self, args: &[u8]) -> T {
        let bytes = self.call(args);
        from_bytes(&bytes).unwrap_or_else(|e| panic!("remote codec: callback {}'s reply does not decode: {e}", self.id))
    }
}

/// Decode a bundle's tree. Errors in the tree itself are reported here;
/// a subtree a `Dyn` hole or keyed row builds LATER is decoded when the
/// host realizes it, and an error there panics with the same message — by
/// then there is no caller to hand a `Result` to.
pub fn decode(link: Rc<dyn Link>, bytes: &[u8]) -> Result<Element, DecodeError> {
    let node: Node = from_bytes(bytes).map_err(|e| DecodeError::Malformed(e.to_string()))?;
    let conn = Rc::new(Conn { link, sheets: RefCell::new(HashMap::new()) });
    build(&conn, node)
}

fn subtree(conn: &Rc<Conn>, bytes: &[u8]) -> Element {
    let node: Node = from_bytes(bytes).unwrap_or_else(|e| panic!("remote codec: a subtree does not decode: {e}"));
    build(conn, node).unwrap_or_else(|e| panic!("remote component: {e}"))
}

fn cb(conn: &Rc<Conn>, id: Cb) -> Rc<CbRef> {
    Rc::new(CbRef { id, conn: conn.clone() })
}

fn build(conn: &Rc<Conn>, node: Node) -> Result<Element, DecodeError> {
    Ok(match node {
        Node::View { common, safe_area, preserves_focus, is_container, children } => {
            let (test_id, style, a11y) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(ViewPrim {
                    test_id,
                    style,
                    safe_area: SafeAreaSides(safe_area),
                    on_touch: None,
                    on_wheel: None,
                    on_hover: None,
                    on_file_drop: None,
                    preserves_focus,
                    is_container,
                    a11y,
                    ref_fill: None,
                }),
                build_all(conn, children)?,
            )
        }
        Node::Pressable { common, on_press, disabled, preserves_focus, children } => {
            let (test_id, style, a11y) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(PressablePrim {
                    test_id,
                    on_press: fire(conn, on_press),
                    disabled: disabled.map(|v| value(conn, v)),
                    preserves_focus,
                    style,
                    a11y,
                    ref_fill: None,
                }),
                build_all(conn, children)?,
            )
        }
        Node::Text { common, content } => {
            let (test_id, style, a11y) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(TextPrim {
                    test_id,
                    content: TextSourceProp::Value(value(conn, content)),
                    style,
                    a11y,
                    ref_fill: None,
                }),
                Vec::new(),
            )
        }
        Node::Button { common, label, on_press, disabled } => {
            let (test_id, style, a11y) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(ButtonPrim {
                    test_id,
                    label: value(conn, label),
                    on_press: action(conn, on_press),
                    leading_icon: None,
                    trailing_icon: None,
                    disabled: disabled.map(|v| value(conn, v)),
                    style,
                    a11y,
                    ref_fill: None,
                }),
                Vec::new(),
            )
        }
        Node::Fragment(children) => runtime_scene::fragment(build_all(conn, children)?),
        Node::Dyn { build } => {
            let (c, b) = (conn.clone(), cb(conn, build));
            dyn_element(move || subtree(&c, &b.call(&[])))
        }
        Node::Guarded { changed, build } => {
            let changed = cb(conn, changed);
            let (c, b) = (conn.clone(), cb(conn, build));
            dyn_guarded(move || changed.get::<bool>(&[]), move || subtree(&c, &b.call(&[])))
        }
        Node::Keyed { items, render } => keyed(conn, cb(conn, items), cb(conn, render)),
        Node::Owned { scope, element } => {
            runtime_scene::owned(build(conn, *element)?, runtime_world::remote::claim_scope(scope))
        }
        Node::Import { name, props, children } => {
            let children = build_all(conn, children)?;
            let f = IMPORTS
                .with(|m| m.borrow().get(name.as_str()).cloned())
                .ok_or_else(|| DecodeError::MissingImport(name.clone()))?;
            f(&props, children).map_err(|reason| DecodeError::BadProps { name, reason })?
        }
    })
}

fn build_all(conn: &Rc<Conn>, nodes: Vec<Node>) -> Result<Vec<Element>, DecodeError> {
    nodes.into_iter().map(|n| build(conn, n)).collect()
}

/// A keyed row's item as the scene holds it: the bundle keeps the real
/// item. `render` moves the item out of the bundle's entry but leaves the
/// entry, so this copy's release on drop always balances — for a rendered
/// row it frees the emptied entry, for a row the scene dropped unrendered
/// it frees the item itself.
struct ItemRef(CbRef);

fn keyed(conn: &Rc<Conn>, items: Rc<CbRef>, render: Rc<CbRef>) -> Element {
    let c = conn.clone();
    let rc = conn.clone();
    Element::Keyed {
        items: Box::new(move || {
            let rows: Vec<(WireKey, Cb)> = items.get(&[]);
            rows.into_iter()
                .map(|(key, id)| {
                    let item = ItemRef(CbRef { id, conn: c.clone() });
                    (key.into(), Box::new(item) as Box<dyn std::any::Any>)
                })
                .collect()
        }),
        render: Box::new(move |item| {
            let item = item.downcast::<ItemRef>().expect("remote codec: keyed render got a foreign item");
            subtree(&rc, &render.call(&to_bytes(&item.0.id)))
        }),
    }
}

fn fire(conn: &Rc<Conn>, id: Cb) -> Rc<dyn Fn()> {
    let r = cb(conn, id);
    Rc::new(move || {
        r.call(&[]);
    })
}

fn value<T: DeserializeOwned + 'static>(conn: &Rc<Conn>, v: Val<T>) -> Value<T> {
    match v {
        Val::Const(v) => Value::Const(v),
        Val::Dyn(id) => {
            let r = cb(conn, id);
            Value::Dyn(Box::new(move || r.get::<T>(&[])))
        }
    }
}

fn action(conn: &Rc<Conn>, a: WireAction) -> Action {
    Action {
        method: intern(&a.method),
        inputs: a.inputs,
        initial: runtime_shared::__serde_json::from_str(&a.initial)
            .unwrap_or_else(|e| panic!("remote codec: action `{}`'s initial values: {e}", a.method)),
        output: a.output,
        fire: fire(conn, a.fire),
    }
}

fn common(conn: &Rc<Conn>, c: Common) -> (Option<&'static str>, Option<StyleProp>, AccessibilityProps) {
    (c.test_id.as_deref().map(intern), c.style.map(|s| style(conn, s)), c.a11y.map_or_else(Default::default, |a| a11y(conn, a)))
}

fn a11y(conn: &Rc<Conn>, a: A11y) -> AccessibilityProps {
    AccessibilityProps {
        label: a.label,
        hint: a.hint,
        role: a.role,
        traits: AccessibilityTraits::from_bits_retain(a.traits),
        hidden: a.hidden,
        live_region: a.live_region,
        actions: a.actions.into_iter().map(|(name, id)| AccessibilityAction { name, handler: fire(conn, id) }).collect(),
        identifier: a.identifier,
    }
}

fn style(conn: &Rc<Conn>, s: Style) -> StyleProp {
    match s {
        Style::Rules(rules) => StyleProp::Static(Rc::new(rules)),
        Style::Dynamic(id) => {
            let r = cb(conn, id);
            StyleProp::Dynamic(Box::new(move || Rc::new(r.get::<StyleRules>(&[]))))
        }
        Style::Sheet(app) => StyleProp::Sheet(Box::new(application(conn, app))),
        Style::SheetDynamic(id) => {
            let (c, r) = (conn.clone(), cb(conn, id));
            StyleProp::SheetDynamic(Box::new(move || application(&c, r.get::<App>(&[]))))
        }
    }
}

fn application(conn: &Rc<Conn>, app: App) -> StyleApplication {
    let mut out = StyleApplication::new(sheet(conn, app.sheet));
    out.variants = app.variants;
    if let Some(rules) = app.overrides {
        out = out.with_overrides(rules);
    }
    if let Some(rules) = app.inline {
        out = out.with_inline(rules);
    }
    if let Some((key, id)) = app.computed {
        let r = cb(conn, id);
        out = out.with_computed(key, move || r.get::<StyleRules>(&[]));
    }
    out
}

/// The host proxy for bundle sheet `r.id`: the live one if the host still
/// holds it (this crossing's copy of the id is released at once — the
/// proxy already holds one), else a new proxy that owns this copy.
fn sheet(conn: &Rc<Conn>, r: SheetRef) -> Rc<StyleSheet> {
    if let Some(live) = conn.sheets.borrow().get(&r.id).and_then(Weak::upgrade) {
        conn.link.release(r.id);
        return live;
    }
    let eval = cb(conn, r.id);
    let sheet = Rc::new(StyleSheet::from_shape(
        &r.shape,
        Rc::new(move |part: &SheetPart, variants: &VariantSet| {
            eval.get::<StyleRules>(&to_bytes(&(part, variants)))
        }),
    ));
    conn.sheets.borrow_mut().insert(r.id, Rc::downgrade(&sheet));
    sheet
}

/// `test_id` and an action's `method` are `&'static str` on the prims:
/// interned, one leak per distinct string for the process's life.
fn intern(s: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&n) = names.get(s) {
        return n;
    }
    let n: &'static str = Box::leak(s.to_owned().into_boxed_str());
    names.insert(n);
    n
}

// ---------------------------------------------------------------------------
// App components exported to bundles
// ---------------------------------------------------------------------------

type ImportFn = Rc<dyn Fn(&[u8], Vec<Element>) -> Result<Element, String>>;

thread_local! {
    static IMPORTS: RefCell<HashMap<String, ImportFn>> = RefCell::new(HashMap::new());
}

/// Export app component `name` to bundles: a bundle's
/// `remote::bundle::import(name, &props, children)` builds `render(props,
/// children)` here. Props cross serialized, so `P` must decode from what
/// the bundle's props type serializes to — in practice one shared type.
pub fn register_import<P: DeserializeOwned + 'static>(
    name: &str,
    render: impl Fn(P, Vec<Element>) -> Element + 'static,
) {
    let f: ImportFn = Rc::new(move |bytes, children| {
        let props: P = from_bytes(bytes).map_err(|e| e.to_string())?;
        Ok(render(props, children))
    });
    IMPORTS.with(|m| m.borrow_mut().insert(name.to_owned(), f));
}

/// The names of the app components exported to bundles.
pub fn exported_imports() -> Vec<String> {
    IMPORTS.with(|m| m.borrow().keys().cloned().collect())
}

/// Encode a value the way the codec does — for a host building props to
/// hand a bundle's mount export.
pub fn encode_value<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    to_bytes(v)
}
