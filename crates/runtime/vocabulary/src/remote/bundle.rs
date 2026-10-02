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
    /// Arguments → reply, both encoded by the closure (an app component's
    /// callback prop).
    Call(Rc<dyn Fn(&[u8]) -> Vec<u8>>),
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
                    Entry::Call(_) => "call",
                };
                (*id, kind, s.refs)
            })
            .collect();
        v.sort();
        v
    })
}

/// Run `f` on held item / snapshot `id` (an [`Entry::Item`]) without
/// consuming it: taken out for the call, so `f` may use the table.
fn with_item<R>(id: Cb, f: impl FnOnce(&dyn Any) -> R) -> Option<R> {
    let item = TABLE.with(|t| match t.borrow_mut().slots.get_mut(&id).map(|s| &mut s.entry) {
        Some(Entry::Item(item)) => item.take(),
        _ => None,
    })?;
    let r = f(&*item);
    TABLE.with(|t| {
        if let Some(Entry::Item(slot)) = t.borrow_mut().slots.get_mut(&id).map(|s| &mut s.entry) {
            *slot = Some(item);
        }
    });
    Some(r)
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
        Call(Rc<dyn Fn(&[u8]) -> Vec<u8>>),
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
            Entry::Call(f) => Call::Call(f.clone()),
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
        Call::Call(f) => f(args),
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
        Element::Many { data } => match data.downcast_ref::<PrimCell<RepeatPrim>>() {
            Some(cell) => {
                let RepeatPrim { count, row_builder } = cell.take();
                Node::Repeat { count, row: handler(move |i: &usize| encode(row_builder(*i))) }
            }
            None => refuse("a multi-node primitive", "payload"),
        },
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
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::handles::ViewHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::View {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            safe_area: p.safe_area.0,
            preserves_focus: p.preserves_focus,
            is_container: p.is_container,
            on_touch: p.on_touch.map(|f| handler(move |e| f(e))),
            on_wheel: p.on_wheel.map(|f| handler(move |e| f(e))),
            on_hover: p.on_hover.map(|f| handler(move |h: &bool| f(*h))),
            on_file_drop: p.on_file_drop.map(|f| handler(move |e: &WireFileDrop| f(&file_drop(e)))),
            children: encode_all(children),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<PressablePrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::handles::PressableHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::Pressable {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            on_press: register(Entry::Fire(p.on_press)),
            disabled: p.disabled.map(val),
            preserves_focus: p.preserves_focus,
            children: encode_all(children),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<TextPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::handles::TextHandle::new(n, &super::handles::REMOTE_OPS));
        let content = match p.content {
            TextSourceProp::Value(v) => TextContent::Value(val(v)),
            // The f-string fast path exists for JS-binding backends (web),
            // where a remote component is never used; everywhere else the
            // text handler runs the same `compute_fallback`.
            TextSourceProp::JsBinding(b) => TextContent::Value(getter(b.compute_fallback)),
            TextSourceProp::Runs(runs) => TextContent::Runs(runs),
        };
        return Node::Text { common: common(p.test_id, p.style, p.a11y).with_fill(fill), content };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<ButtonPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::handles::ButtonHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::Button {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            label: val(p.label),
            on_press: action(p.on_press),
            leading_icon: p.leading_icon.map(WireIcon::from),
            trailing_icon: p.trailing_icon.map(WireIcon::from),
            disabled: p.disabled.map(val),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<ImagePrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::image::ImageHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::Image {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            src: val(p.src),
            alt: val(p.alt),
            on_load: p.on_load.map(|f| handler(move |e| f(e))),
            on_error: p.on_error.map(|f| handler(move |_: &()| f())),
            asset: p.asset.map(|a| WireAsset { id: a.id.0, source: wire_asset_source(a.source) }),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<IconPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::icon::IconHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::Icon {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            data: val_map(p.data, WireIcon::from),
            color: p.color.map(val),
            stroke: p.stroke.map(val),
            draw_in: p.draw_in,
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<LinkPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::link::LinkHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::Link {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            url: val(p.url),
            // The route's name; its typed params cross as the url, which
            // the app's navigator parses (`ParamsFromUrl`).
            route: p.route_link.map(|r| r.name.to_owned()),
            external: p.external,
            on_activate: p.on_activate.map(|f| register(Entry::Fire(f))),
            children: encode_all(children),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<TogglePrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::toggle::ToggleHandle::new(n, &super::handles::REMOTE_OPS));
        let f = p.on_change;
        return Node::Toggle {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            value: val(p.value),
            on_change: handler(move |v: &bool| f(*v)),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<SliderPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::slider::SliderHandle::new(n, &super::handles::REMOTE_OPS));
        let f = p.on_change;
        return Node::Slider {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            value: val(p.value),
            on_change: handler(move |v: &f32| f(*v)),
            min: p.min,
            max: p.max,
            step: p.step,
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<ActivityIndicatorPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::activity_indicator::ActivityIndicatorHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::ActivityIndicator { common: common(p.test_id, p.style, p.a11y).with_fill(fill), size: val(p.size), color: p.color };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<TextInputPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::text_input::TextInputHandle::new(n, &super::handles::REMOTE_OPS));
        let f = p.on_change;
        return Node::TextInput {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            value: val(p.value),
            on_change: handler(move |v: &String| f(v.clone())),
            on_key_down: p.on_key_down.map(|f| handler(move |e| f(e))),
            on_blur: p.on_blur.map(|f| handler(move |_: &()| f())),
            on_focus: p.on_focus.map(|f| handler(move |v: &bool| f(*v))),
            placeholder: val(p.placeholder),
            secure: val(p.secure),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<TextAreaPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::text_area::TextAreaHandle::new(n, &super::handles::REMOTE_OPS));
        let f = p.on_change;
        return Node::TextArea {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            value: val(p.value),
            on_change: handler(move |v: &String| f(v.clone())),
            on_key_down: p.on_key_down.map(|f| handler(move |e| f(e))),
            placeholder: p.placeholder,
            wrap: p.wrap,
            min_rows: p.min_rows,
            max_rows: p.max_rows,
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<ScrollViewPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::scroll_view::ScrollViewHandle::new(n, &super::handles::REMOTE_OPS));
        return Node::ScrollView {
            common: common(p.test_id, p.style, p.a11y).with_fill(fill),
            horizontal: p.horizontal,
            on_scroll: p.on_scroll.map(|f| handler(move |&(x, y): &(f32, f32)| f(x, y))),
            on_end_reached: p.on_end_reached.map(|f| register(Entry::Fire(f))),
            end_reached_threshold: p.end_reached_threshold,
            safe_area: p.safe_area.map(|s| s.0),
            bounces: p.bounces,
            always_bounce: p.always_bounce,
            children: encode_all(children),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<PresencePrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::presence::PresenceHandle::new(n, &super::handles::REMOTE_OPS));
        let child: Rc<dyn Fn() -> Element> = Rc::from(p.child);
        return Node::Presence {
            test_id: p.test_id.map(str::to_owned),
            a11y: wire_a11y(p.a11y).map(Box::new),
            fill,
            child: register(Entry::Build(child)),
            present: register(Entry::Changed(p.present)),
            enter: p.enter,
            exit: p.exit,
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<PortalPrim>>() {
        use runtime_shared::primitives::portal::PortalTarget;
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::portal::PortalHandle::new(n, &super::handles::REMOTE_OPS));
        let target = match p.target {
            PortalTarget::Viewport(v) => WirePortalTarget::Viewport(v),
            PortalTarget::Named(n) => WirePortalTarget::Named(n.to_owned()),
            PortalTarget::Anchor { target, side, align, offset } => {
                WirePortalTarget::Anchor { rect: handler(move |_: &()| target.rect()), side, align, offset }
            }
        };
        return Node::Portal {
            target,
            fill,
            on_dismiss: p.on_dismiss.map(|f| register(Entry::Fire(f))),
            trap_focus: p.trap_focus,
            style: p.style.map(|s| Box::new(style_prop(s))),
            a11y: wire_a11y(p.a11y).map(Box::new),
            children: encode_all(children),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<VirtualizerPrim>>() {
        use runtime_shared::primitives::virtualizer::ItemSize;
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::virtualizer::VirtualizerHandle::new(n, &super::handles::REMOTE_OPS));
        let (count, key, render) = (p.item_count, p.item_key, p.render_item);
        let (measured, size) = match p.item_size {
            ItemSize::Known(f) => (false, f),
            ItemSize::Measured(f) => (true, f),
        };
        return Node::Virtualizer {
            common: common(None, p.style, p.a11y).with_fill(fill),
            item_count: handler(move |_: &()| count()),
            item_key: handler(move |i: &usize| key(*i)),
            measured,
            item_size: handler(move |i: &usize| size(*i)),
            render_item: handler(move |i: &usize| encode(render(*i))),
            item_diff: p.item_diff.map(|d| {
                let (capture, differs) = (d.capture, d.differs);
                (
                    handler(move |i: &usize| capture(*i).map(|snap| register(Entry::Item(Some(snap))))),
                    handler(move |&(snap, i): &(Cb, usize)| {
                        with_item(snap, |s| differs(s, i))
                            .unwrap_or_else(|| panic!("remote codec: diff against snapshot {snap}, which is gone"))
                    }),
                )
            }),
            overscan: p.overscan,
            layout: p.layout,
            on_scroll: p.on_scroll.map(|f| handler(move |&(x, y): &(f32, f32)| f(x, y))),
            on_end_reached: p.on_end_reached.map(|f| register(Entry::Fire(f))),
            end_reached_threshold: p.end_reached_threshold,
            safe_area: p.safe_area.map(|s| s.0),
        };
    }
    if let Some(cell) = data.downcast_ref::<PrimCell<VirtualGridPrim>>() {
        let p = cell.take();
        let fill = super::handles::fill(p.ref_fill, |n| runtime_shared::primitives::virtual_grid::VirtualGridHandle::new(n, &super::handles::REMOTE_OPS));
        let (cols, rows, cw, rh, key, render) =
            (p.col_count, p.row_count, p.col_width, p.row_height, p.cell_key, p.render_cell);
        return Node::VirtualGrid {
            common: common(None, p.style, p.a11y).with_fill(fill),
            col_count: handler(move |_: &()| cols()),
            row_count: handler(move |_: &()| rows()),
            col_width: handler(move |i: &usize| cw(*i)),
            row_height: handler(move |i: &usize| rh(*i)),
            cell_key: handler(move |&(r, c): &(usize, usize)| key(r, c)),
            render_cell: handler(move |&(r, c): &(usize, usize)| encode(render(r, c))),
            overscan: p.overscan,
            on_scroll: p.on_scroll.map(|f| handler(move |&(x, y): &(f32, f32)| f(x, y))),
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
    Common {
        test_id: test_id.map(str::to_owned),
        style: style.map(|s| Box::new(style_prop(s))),
        a11y: wire_a11y(a11y).map(Box::new),
        fill: None,
    }
}

fn getter<T: Serialize + 'static>(f: Rc<dyn Fn() -> T>) -> Val<T> {
    Val::Dyn(register(Entry::Get(Rc::new(move || to_bytes(&f())))))
}

/// An event handler: the host calls it with the encoded event `A` and
/// decodes its encoded reply `R`.
fn handler<A, R>(f: impl Fn(&A) -> R + 'static) -> Cb
where
    A: serde::de::DeserializeOwned,
    R: Serialize,
{
    register(Entry::Call(Rc::new(move |args: &[u8]| {
        let event: A = from_bytes(args).unwrap_or_else(|e| panic!("remote codec: an event does not decode: {e}"));
        to_bytes(&f(&event))
    })))
}

/// A `Value<T>` crossing as `W` (for a `T` that can't serialize as is).
fn val_map<T: 'static, W: Serialize + 'static>(v: Value<T>, map: fn(T) -> W) -> Val<W> {
    match v {
        Value::Const(v) => Val::Const(map(v)),
        Value::Dyn(f) => Val::Dyn(register(Entry::Get(Rc::new(move || to_bytes(&map(f())))))),
    }
}

fn wire_asset_source(s: runtime_shared::assets::AssetSource) -> WireAssetSource {
    use runtime_shared::assets::AssetSource;
    match s {
        AssetSource::Embedded { bytes, extension } => {
            WireAssetSource::Embedded { bytes: bytes.to_vec(), extension: extension.to_owned() }
        }
        AssetSource::Bundled { path } => WireAssetSource::Bundled { path: path.to_owned() },
        AssetSource::BundledEmbedded { path, bytes, extension } => WireAssetSource::BundledEmbedded {
            path: path.to_owned(),
            bytes: bytes.to_vec(),
            extension: extension.to_owned(),
        },
        AssetSource::Remote { url } => WireAssetSource::Remote { url: url.to_owned() },
    }
}

/// A file drop as the bundle's handler sees it. Native files carry a
/// path; the web-only opaque `source` never crosses.
fn file_drop(e: &WireFileDrop) -> runtime_shared::file_drop::FileDropEvent {
    use runtime_shared::file_drop::{DroppedFile, FileDropEvent, FileDropPhase};
    let phase = match &e.phase {
        WireDropPhase::Entered => FileDropPhase::Entered,
        WireDropPhase::Exited => FileDropPhase::Exited,
        WireDropPhase::Dropped(files) => FileDropPhase::Dropped(
            files
                .iter()
                .map(|f| DroppedFile {
                    name: f.name.clone(),
                    mime: f.mime.clone(),
                    size: f.size,
                    path: f.path.clone(),
                    source: None,
                })
                .collect(),
        ),
    };
    FileDropEvent { phase, position: e.position }
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

/// The bundle-side body of a `#[component(remote)]` mount export
/// (macro-emitted): decode the props the app wrote (`len` bytes in the
/// argument buffer), build the component, and reply with its encoded tree.
/// The props' imports belong to the scope that crosses as the tree's root,
/// so unmounting on the app side releases them.
#[cfg(idealyst_stream_guest)]
#[doc(hidden)]
pub fn __mount(len: u32, build: impl FnOnce(&mut &[u8]) -> Element) -> i64 {
    let args = super::wasm::take_args(len);
    let tree = runtime_scene::component_scope(|| build(&mut &args[..]));
    super::wasm::reply(to_bytes(&encode(tree)))
}

// ---------------------------------------------------------------------------
// App components (imports) — what `crate::__remote_import!` and the
// `ImportArg` impls use
// ---------------------------------------------------------------------------

/// Use the app's component `name` (its `#[component]` registration key)
/// with `props` encoded by its props' `ImportArg`.
#[doc(hidden)]
pub fn import_component(name: &'static str, props: Vec<u8>) -> Element {
    runtime_scene::item(ImportPrim { name: name.to_owned(), props }, Vec::new())
}

/// A getter the app can call (a live `Reactive` prop).
#[doc(hidden)]
pub fn register_getter(f: impl Fn() -> Vec<u8> + 'static) -> Cb {
    register(Entry::Get(Rc::new(f)))
}

/// A callback the app can call with encoded arguments.
#[doc(hidden)]
pub fn register_call(f: Rc<dyn Fn(&[u8]) -> Vec<u8>>) -> Cb {
    register(Entry::Call(f))
}

/// A stylesheet, as it crosses (one entry per sheet, refcounted).
#[doc(hidden)]
pub fn sheet_ref(sheet: &Rc<StyleSheet>) -> SheetRef {
    SheetRef { id: register_sheet(sheet), shape: sheet.shape() }
}

// ---------------------------------------------------------------------------
// `#[host_fn]` calls from a bundle (what its bundle stub calls)
// ---------------------------------------------------------------------------

/// Call a SYNC host function: `import` hands the encoded arguments to the
/// app, which runs the function and writes its encoded reply into this
/// bundle's argument buffer (`idealyst_ui_alloc`), returning its length.
#[cfg(idealyst_stream_guest)]
#[doc(hidden)]
pub fn host_fn_sync(args: &[u8], import: impl FnOnce(*const u8, u32) -> i64) -> Vec<u8> {
    let len = import(args.as_ptr(), args.len() as u32);
    super::wasm::take_args(len as u32)
}

/// What an ASYNC `#[host_fn]` returns in a bundle: a future that resolves
/// when the app's function does. The app runs the real future on its own
/// executor and, when it completes, calls back into this bundle with the
/// encoded result (a one-shot callback, released by the app after the
/// call). Drive it like the native future —
/// `spawn_then(take_photo(opts), move |photo| …)`; the bundle's executor
/// (`remote::wasm`) polls it.
///
/// If the bundle is stopped (it panicked) before the result arrives, the
/// app never calls back and the future simply never resolves.
#[cfg(idealyst_stream_guest)]
pub struct HostFuture<T> {
    name: &'static str,
    state: Rc<RefCell<HostReply>>,
    decode: fn(&[u8]) -> Option<T>,
}

#[cfg(idealyst_stream_guest)]
#[derive(Default)]
struct HostReply {
    bytes: Option<Vec<u8>>,
    waker: Option<std::task::Waker>,
}

#[cfg(idealyst_stream_guest)]
impl<T> HostFuture<T> {
    #[doc(hidden)]
    pub fn start(
        name: &'static str,
        args: Vec<u8>,
        decode: fn(&[u8]) -> Option<T>,
        import: impl FnOnce(*const u8, u32, u32),
    ) -> Self {
        let state = Rc::new(RefCell::new(HostReply::default()));
        let filled = state.clone();
        let then = register_call(Rc::new(move |reply: &[u8]| {
            // Taken out before waking: the wake may poll this future at once.
            let waker = {
                let mut s = filled.borrow_mut();
                s.bytes = Some(reply.to_vec());
                s.waker.take()
            };
            if let Some(w) = waker {
                w.wake();
            }
            Vec::new()
        }));
        import(args.as_ptr(), args.len() as u32, then);
        HostFuture { name, state, decode }
    }
}

#[cfg(idealyst_stream_guest)]
impl<T> std::future::Future for HostFuture<T> {
    type Output = T;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<T> {
        let mut s = self.state.borrow_mut();
        match s.bytes.take() {
            Some(bytes) => std::task::Poll::Ready((self.decode)(&bytes).unwrap_or_else(|| {
                panic!(
                    "host_fn `{}`: the result does not decode — the load-time schema check should have refused this bundle",
                    self.name
                )
            })),
            None => {
                s.waker = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}
