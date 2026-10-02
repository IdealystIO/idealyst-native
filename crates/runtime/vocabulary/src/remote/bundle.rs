//! The bundle half: encode an `Element` into a [`Node`], keeping its
//! closures in a callback table the host calls by id.
//!
//! Compiled into a remote bundle (`--cfg idealyst_stream_guest`) and into
//! the codec's in-process tests (`remote-loopback`). The table is
//! thread-local and REFCOUNTED: every time an id crosses, its count goes up,
//! and every [`release`] from the host takes it down — so a stylesheet that
//! crosses once per node it styles is one entry, and a resend can never race
//! the host releasing an earlier copy (see [`register_sheet`]).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{DynKind, Element};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{Action, SheetPart, StyleApplication, StyleRules, StyleSheet, VariantSet};
use runtime_world::Value;
use std::collections::HashMap;
use serde::Serialize;

use super::*;
use crate::prims::*;
use crate::style_attach::StyleProp;

/// A table entry: one closure (or held value) the host can reach by id.
enum Entry {
    /// `()` → `()`.
    Fire(Rc<dyn Fn()>),
    /// `()` → the getter's value, encoded by the getter.
    Get(Rc<dyn Fn() -> Vec<u8>>),
    /// `()` → `bool`.
    Changed(Rc<dyn Fn() -> bool>),
    /// `()` → [`Node`].
    Build(Rc<dyn Fn() -> Element>),
    /// `()` → `Vec<(WireKey, Cb)>`; each row's item is registered as an
    /// [`Entry::Item`].
    Items(Rc<dyn Fn() -> Vec<(runtime_scene::Key, Box<dyn Any>)>>),
    /// `(Cb)` → [`Node`]; consumes the item entry.
    Render(Rc<dyn Fn(Box<dyn Any>) -> Element>),
    /// A keyed row's item, waiting for `render` (or for the host to drop
    /// the row unrendered).
    Item(Option<Box<dyn Any>>),
    /// `(SheetPart, VariantSet)` → `StyleRules`.
    Sheet(Rc<StyleSheet>),
}

struct Slot {
    entry: Entry,
    refs: u32,
}

#[derive(Default)]
struct Table {
    slots: HashMap<Cb, Slot>,
    next: Cb,
    /// Live sheet entries by sheet address, so a sheet keeps one id.
    sheets: HashMap<*const StyleSheet, Cb>,
}

thread_local! {
    static TABLE: RefCell<Table> = RefCell::new(Table::default());
}

fn register(entry: Entry) -> Cb {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.next = t.next.checked_add(1).expect("remote codec: callback ids exhausted");
        let id = t.next;
        t.slots.insert(id, Slot { entry, refs: 1 });
        id
    })
}

/// Register `sheet`, or count one more crossing of its existing id. The
/// count is what makes reuse safe: the host releases once per crossing it
/// received, so an id that crosses again before the host's release of an
/// earlier copy is still live when that release lands.
fn register_sheet(sheet: &Rc<StyleSheet>) -> Cb {
    let ptr = Rc::as_ptr(sheet);
    let existing = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        let id = *t.sheets.get(&ptr)?;
        t.slots.get_mut(&id).expect("sheet index points at a live entry").refs += 1;
        Some(id)
    });
    existing.unwrap_or_else(|| {
        let id = register(Entry::Sheet(sheet.clone()));
        TABLE.with(|t| t.borrow_mut().sheets.insert(ptr, id));
        id
    })
}

/// Number of live callback entries — what the codec's tests check for
/// leaks.
pub fn live_callbacks() -> usize {
    TABLE.with(|t| t.borrow().slots.len())
}

/// The live callback entries, as `(id, kind, crossings)` — for leak
/// diagnostics.
pub fn live_callback_kinds() -> Vec<(Cb, &'static str, u32)> {
    TABLE.with(|t| {
        let mut v: Vec<_> = t
            .borrow()
            .slots
            .iter()
            .map(|(id, s)| {
                let kind = match &s.entry {
                    Entry::Fire(_) => "fire",
                    Entry::Get(_) => "get",
                    Entry::Changed(_) => "changed",
                    Entry::Build(_) => "build",
                    Entry::Items(_) => "items",
                    Entry::Render(_) => "render",
                    Entry::Item(_) => "item",
                    Entry::Sheet(_) => "sheet",
                };
                (*id, kind, s.refs)
            })
            .collect();
        v.sort();
        v
    })
}

/// The host dropped one copy of `cb`.
pub fn release(cb: Cb) {
    // Out of the table first, dropped after: dropping a closure may drop
    // signals and run cleanups, which may re-enter the table.
    let dropped = TABLE.try_with(|t| {
        let mut t = t.borrow_mut();
        let slot = t.slots.get_mut(&cb)?;
        slot.refs -= 1;
        if slot.refs > 0 {
            return None;
        }
        let slot = t.slots.remove(&cb).expect("present");
        if let Entry::Sheet(sheet) = &slot.entry {
            t.sheets.remove(&Rc::as_ptr(sheet));
        }
        Some(slot)
    });
    drop(dropped);
}

/// Run callback `cb` with `args`, returning its encoded reply. Panics on an
/// unknown id or malformed arguments — the host and the bundle disagree
/// about the protocol, which is a bug, not a recoverable state.
pub fn invoke(cb: Cb, args: &[u8]) -> Vec<u8> {
    enum Call {
        Fire(Rc<dyn Fn()>),
        Get(Rc<dyn Fn() -> Vec<u8>>),
        Changed(Rc<dyn Fn() -> bool>),
        Build(Rc<dyn Fn() -> Element>),
        Items(Rc<dyn Fn() -> Vec<(runtime_scene::Key, Box<dyn Any>)>>),
        Render(Rc<dyn Fn(Box<dyn Any>) -> Element>),
        Sheet(Rc<StyleSheet>),
    }
    // Clone the closure out and release the table before running it: every
    // callback may encode more nodes (registering entries) or drop some.
    let call = TABLE.with(|t| {
        let t = t.borrow();
        let slot = t.slots.get(&cb).unwrap_or_else(|| panic!("remote codec: the host called unknown callback {cb}"));
        match &slot.entry {
            Entry::Fire(f) => Call::Fire(f.clone()),
            Entry::Get(f) => Call::Get(f.clone()),
            Entry::Changed(f) => Call::Changed(f.clone()),
            Entry::Build(f) => Call::Build(f.clone()),
            Entry::Items(f) => Call::Items(f.clone()),
            Entry::Render(f) => Call::Render(f.clone()),
            Entry::Sheet(s) => Call::Sheet(s.clone()),
            Entry::Item(_) => panic!("remote codec: callback {cb} is a keyed item, not a callable"),
        }
    });
    let bad_args = |e: postcard::Error| -> ! { panic!("remote codec: arguments for callback {cb} do not decode: {e}") };
    match call {
        Call::Fire(f) => {
            f();
            Vec::new()
        }
        Call::Get(f) => f(),
        Call::Changed(f) => to_bytes(&f()),
        Call::Build(f) => to_bytes(&encode(f())),
        Call::Items(f) => {
            let rows: Vec<(WireKey, Cb)> =
                f().into_iter().map(|(key, item)| (key.into(), register(Entry::Item(Some(item))))).collect();
            to_bytes(&rows)
        }
        Call::Render(f) => {
            let item_id: Cb = from_bytes(args).unwrap_or_else(|e| bad_args(e));
            let item = TABLE.with(|t| match t.borrow_mut().slots.get_mut(&item_id).map(|s| &mut s.entry) {
                Some(Entry::Item(item)) => item.take(),
                _ => None,
            });
            let item = item.unwrap_or_else(|| panic!("remote codec: render of keyed item {item_id}, which is gone"));
            to_bytes(&encode(f(item)))
        }
        Call::Sheet(sheet) => {
            let (part, variants): (SheetPart, VariantSet) = from_bytes(args).unwrap_or_else(|e| bad_args(e));
            let rules = sheet
                .eval_part(&part, &variants)
                .unwrap_or_else(|| panic!("remote codec: the host asked sheet {cb} for {part:?}, which it does not have"));
            to_bytes(&rules)
        }
    }
}

// ---------------------------------------------------------------------------
// App components, by name
// ---------------------------------------------------------------------------

/// An app component used from a bundle: what [`import`] builds. Never
/// mounted — only ever encoded into a [`Node::Import`].
pub struct ImportPrim {
    pub name: String,
    pub props: Vec<u8>,
}

/// Use the app's component `name` here, with these props and children. The
/// app must have registered it (`remote::host::register_import`) with props
/// that decode from what `props` serializes to.
pub fn import(name: &str, props: &impl Serialize, children: Vec<Element>) -> Element {
    runtime_scene::item(ImportPrim { name: name.to_owned(), props: to_bytes(props) }, children)
}

// ---------------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------------

#[track_caller]
fn refuse(what: &str, field: &str) -> ! {
    panic!(
        "remote component: `{what}`'s `{field}` can't cross to the host yet — the remote codec \
         does not carry it (runtime-vocabulary src/remote)"
    )
}

/// Encode `element` for the host, registering its closures.
pub fn encode(element: Element) -> Node {
    match element {
        Element::Item { data, children, .. } => encode_item(data, children),
        Element::Fragment(children) => Node::Fragment(encode_all(children)),
        Element::Dyn(spec) => {
            let (kind, retire) = spec.into_parts();
            if retire.is_some() {
                refuse("dyn hole", "retire hook");
            }
            match kind {
                DynKind::Plain(build) => Node::Dyn { build: register(Entry::Build(Rc::from(build))) },
                DynKind::Guarded { changed, build } => Node::Guarded {
                    changed: register(Entry::Changed(Rc::from(changed))),
                    build: register(Entry::Build(Rc::from(build))),
                },
            }
        }
        Element::Keyed { items, render } => Node::Keyed {
            items: register(Entry::Items(Rc::from(items))),
            render: register(Entry::Render(Rc::from(render))),
        },
        Element::Owned { element, owned } => Node::Owned {
            scope: runtime_world::remote_guest::release_scope(owned),
            element: Box::new(encode(*element)),
        },
        Element::Many { .. } => refuse("repeat", "multi-node payload"),
    }
}

fn encode_all(children: Vec<Element>) -> Vec<Node> {
    children.into_iter().map(encode).collect()
}

fn encode_item(data: Box<dyn Any>, children: Vec<Element>) -> Node {
    let data = match data.downcast::<ImportPrim>() {
        Ok(import) => {
            let ImportPrim { name, props } = *import;
            return Node::Import { name, props, children: encode_all(children) };
        }
        Err(data) => data,
    };
    if let Some(cell) = data.downcast_ref::<PrimCell<ViewPrim>>() {
        let p = cell.take();
        if p.on_touch.is_some() {
            refuse("view", "on_touch");
        }
        if p.on_wheel.is_some() {
            refuse("view", "on_wheel");
        }
        if p.on_hover.is_some() {
            refuse("view", "on_hover");
        }
        if p.on_file_drop.is_some() {
            refuse("view", "on_file_drop");
        }
        if p.ref_fill.is_some() {
            refuse("view", "ref");
        }
        return Node::View {
            common: common(p.test_id, p.style, p.a11y),
            safe_area: p.safe_area.0,
            preserves_focus: p.preserves_focus,
            is_container: p.is_container,
            children: encode_all(children),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<PressablePrim>>() {
        let p = cell.take();
        if p.ref_fill.is_some() {
            refuse("pressable", "ref");
        }
        return Node::Pressable {
            common: common(p.test_id, p.style, p.a11y),
            on_press: register(Entry::Fire(p.on_press)),
            disabled: p.disabled.map(val),
            preserves_focus: p.preserves_focus,
            children: encode_all(children),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<TextPrim>>() {
        let p = cell.take();
        if p.ref_fill.is_some() {
            refuse("text", "ref");
        }
        let content = match p.content {
            TextSourceProp::Value(v) => val(v),
            // The f-string fast path exists for JS-binding backends (web),
            // where a remote component is never used; everywhere else the
            // text handler runs the same `compute_fallback`.
            TextSourceProp::JsBinding(b) => getter(b.compute_fallback),
            TextSourceProp::Runs(_) => refuse("text", "styled runs"),
        };
        return Node::Text { common: common(p.test_id, p.style, p.a11y), content };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<ButtonPrim>>() {
        let p = cell.take();
        if p.ref_fill.is_some() {
            refuse("button", "ref");
        }
        if p.leading_icon.is_some() || p.trailing_icon.is_some() {
            refuse("button", "icons");
        }
        return Node::Button {
            common: common(p.test_id, p.style, p.a11y),
            label: val(p.label),
            on_press: action(p.on_press),
            disabled: p.disabled.map(val),
        };
    }
    let name = crate::remote::crossing((*data).type_id()).map_or_else(
        || runtime_scene::payload_type_name((*data).type_id()).unwrap_or("an unknown payload"),
        Crossing::name,
    );
    panic!(
        "remote component: primitive `{name}` can't cross to the host — the remote codec does not \
         carry it (see runtime-vocabulary src/remote `crossing`)"
    )
}

fn common(test_id: Option<&'static str>, style: Option<StyleProp>, a11y: AccessibilityProps) -> Common {
    Common { test_id: test_id.map(str::to_owned), style: style.map(style_prop), a11y: wire_a11y(a11y) }
}

fn getter<T: Serialize + 'static>(f: Rc<dyn Fn() -> T>) -> Val<T> {
    Val::Dyn(register(Entry::Get(Rc::new(move || to_bytes(&f())))))
}

fn val<T: Serialize + 'static>(v: Value<T>) -> Val<T> {
    match v {
        Value::Const(v) => Val::Const(v),
        Value::Dyn(f) => getter(Rc::from(f)),
    }
}

fn action(a: Action) -> WireAction {
    WireAction {
        fire: register(Entry::Fire(a.fire)),
        method: a.method.to_owned(),
        inputs: a.inputs,
        initial: runtime_shared::__serde_json::to_string(&a.initial)
            .unwrap_or_else(|e| panic!("remote codec: action `{}`'s initial values: {e}", a.method)),
        output: a.output,
    }
}

fn wire_a11y(a: AccessibilityProps) -> Option<A11y> {
    if a.is_default() {
        return None;
    }
    Some(A11y {
        label: a.label,
        hint: a.hint,
        role: a.role,
        traits: a.traits.bits(),
        hidden: a.hidden,
        live_region: a.live_region,
        actions: a.actions.into_iter().map(|act| (act.name, register(Entry::Fire(act.handler)))).collect(),
        identifier: a.identifier,
    })
}

fn style_prop(s: StyleProp) -> Style {
    match s {
        StyleProp::Static(rules) => Style::Rules(Rc::unwrap_or_clone(rules)),
        StyleProp::Dynamic(f) => {
            Style::Dynamic(register(Entry::Get(Rc::new(move || to_bytes::<StyleRules>(&f())))))
        }
        StyleProp::Sheet(app) => Style::Sheet(wire_app(*app)),
        StyleProp::SheetDynamic(f) => sheet_dynamic(Rc::from(f)),
        // The signal-class fast path is a web binding; its `compute` is the
        // exact fallback every other backend runs.
        StyleProp::SignalClass(spec) => sheet_dynamic(spec.compute),
        StyleProp::Preminted { .. } | StyleProp::PremintedDynamic { .. } => refuse(
            "style",
            "preminted class (premint is a web build step; a bundle's styles are never preminted)",
        ),
    }
}

fn sheet_dynamic(f: Rc<dyn Fn() -> StyleApplication>) -> Style {
    // The reply encodes an `App`, registering its sheet — inside the call,
    // so the sheet's crossing count is taken per reply the host receives.
    Style::SheetDynamic(register(Entry::Get(Rc::new(move || to_bytes(&wire_app(f()))))))
}

fn wire_app(app: StyleApplication) -> App {
    let computed = app.computed().map(|c| {
        let compute = c.compute.clone();
        (c.key.clone(), register(Entry::Get(Rc::new(move || to_bytes(&compute())))))
    });
    App {
        sheet: SheetRef { id: register_sheet(&app.sheet), shape: app.sheet.shape() },
        overrides: app.has_overrides().then(|| app.overrides.clone()),
        inline: app.inline().map(|r| (**r).clone()),
        computed,
        variants: app.variants,
    }
}
