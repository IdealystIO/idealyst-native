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
use runtime_shared::primitives::key::KeyOutcome;
use runtime_shared::primitives::text_input::BlurOutcome;
use runtime_shared::touch::TouchResponse;
use runtime_shared::{Action, SafeAreaSides, SheetPart, StyleApplication, StyleRules, StyleSheet, VariantSet};
use runtime_world::Value;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::handles::Held;
use super::*;
use crate::prims::*;
use crate::style_attach::StyleProp;

/// How the host reaches one bundle's callback table. In-process for the
/// codec's tests; over wasm in `stream-host`.
pub trait Link: 'static {
    /// Run callback `cb` with `args`; its encoded reply. `None` when the
    /// bundle can no longer be called — it panicked (trapped) and was
    /// POISONED, or it is gone. Every reply site then falls back to a safe
    /// value (see [`CbRef`]) instead of panicking the app: the bundle's
    /// loader replaces its components with the panic message on the next
    /// flush, so the fallbacks only bridge the calls already in progress.
    fn call(&self, cb: Cb, args: &[u8]) -> Option<Vec<u8>>;
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

impl Drop for Conn {
    /// The tree is gone: so are the handles the app held for it, whether
    /// or not the bundle released them (it may keep a handle in a `Ref`
    /// slot that outlives the tree, or have been stopped).
    fn drop(&mut self) {
        let _ = LIVE_TREES.try_with(|n| n.set(n.get() - 1));
        super::handles::purge_dead();
    }
}

/// One received copy of a callback id; released on drop.
struct CbRef {
    id: Cb,
    conn: Rc<Conn>,
    /// The last reply to an argument-less call: what a getter answers once
    /// its bundle can't be called (poisoned) — its last real value, rather
    /// than a made-up one. Argument-less only: a reply to one set of
    /// arguments says nothing about another.
    last: RefCell<Option<Vec<u8>>>,
}

impl Drop for CbRef {
    fn drop(&mut self) {
        self.conn.link.release(self.id);
    }
}

impl CbRef {
    fn new(id: Cb, conn: Rc<Conn>) -> CbRef {
        CbRef { id, conn, last: RefCell::new(None) }
    }

    /// `None` when the bundle can't be called (see [`Link::call`]).
    fn call(&self, args: &[u8]) -> Option<Vec<u8>> {
        self.conn.link.call(self.id, args)
    }

    /// Call and decode the reply; for an argument-less call to a bundle that
    /// can't be called, the last reply. `None` only when there is neither.
    /// A reply that does not decode is the two sides disagreeing about the
    /// protocol: a bug, so it panics.
    fn get<T: DeserializeOwned>(&self, args: &[u8]) -> Option<T> {
        self.get_bytes(args).map(|b| {
            from_bytes(&b).unwrap_or_else(|e| panic!("remote codec: callback {}'s reply does not decode: {e}", self.id))
        })
    }

    /// [`get`](Self::get), undecoded: for replies their reader decodes
    /// itself (a prop's own `ImportArg`).
    fn get_bytes(&self, args: &[u8]) -> Option<Vec<u8>> {
        match self.call(args) {
            Some(bytes) => {
                if args.is_empty() {
                    *self.last.borrow_mut() = Some(bytes.clone());
                }
                Some(bytes)
            }
            None if args.is_empty() => self.last.borrow().clone(),
            None => None,
        }
    }
}

/// What a `Dyn` hole or keyed row shows when its bundle can't be called:
/// nothing (the loader is about to replace the whole component).
fn nothing() -> Element {
    runtime_scene::fragment(Vec::new())
}

/// Decode a bundle's tree. Errors in the tree itself are reported here;
/// a subtree a `Dyn` hole or keyed row builds LATER is decoded when the
/// host realizes it, and an error there panics with the same message — by
/// then there is no caller to hand a `Result` to.
pub fn decode(link: Rc<dyn Link>, bytes: &[u8]) -> Result<Element, DecodeError> {
    let node: Node = from_bytes(bytes).map_err(|e| DecodeError::Malformed(e.to_string()))?;
    let conn = Rc::new(Conn { link, sheets: RefCell::new(HashMap::new()) });
    LIVE_TREES.with(|n| n.set(n.get() + 1));
    let element = build(&conn, node)?;
    // The mounted tree owns its connection: the handles the app holds for
    // the bundle (`handles`) live exactly as long as the tree, even one
    // with no callbacks to keep the connection alive (a view with a ref).
    // Unmounting it also drops the handles the app holds for it — not
    // waiting for the connection to drop, which a held handle can keep alive
    // (a navigator's handle reaches its screens' callbacks, and through them
    // this connection: a cycle through the handle table).
    let ((), owned) = runtime_world::collect_owned(|| {
        runtime_world::on_scope_drop(move || {
            let tree: std::rc::Rc<dyn std::any::Any> = conn;
            super::handles::purge_tree(&std::rc::Rc::downgrade(&tree));
            drop(tree);
        })
    });
    Ok(runtime_scene::owned(element, owned))
}

thread_local! {
    static LIVE_TREES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Decoded trees whose connection is still alive — for leak checks: `0`
/// once every remote tree is unmounted and nothing holds its callbacks.
pub fn live_trees() -> usize {
    LIVE_TREES.with(|n| n.get())
}

fn subtree(conn: &Rc<Conn>, bytes: &[u8]) -> Element {
    let node: Node = from_bytes(bytes).unwrap_or_else(|e| panic!("remote codec: a subtree does not decode: {e}"));
    build(conn, node).unwrap_or_else(|e| panic!("remote component: {e}"))
}

fn cb(conn: &Rc<Conn>, id: Cb) -> Rc<CbRef> {
    Rc::new(CbRef::new(id, conn.clone()))
}

fn build(conn: &Rc<Conn>, node: Node) -> Result<Element, DecodeError> {
    Ok(match node {
        Node::View {
            common,
            safe_area,
            preserves_focus,
            is_container,
            on_touch,
            on_wheel,
            on_hover,
            on_file_drop,
            children,
        } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(ViewPrim {
                    test_id,
                    style,
                    safe_area: SafeAreaSides(safe_area),
                    on_touch: on_touch.map(|id| Rc::new(handler(conn, id, TouchResponse::default)) as _),
                    on_wheel: on_wheel.map(|id| Rc::new(handler(conn, id, TouchResponse::default)) as _),
                    on_hover: on_hover.map(|id| {
                        let h = handler::<bool, ()>(conn, id, || ());
                        Rc::new(move |v: bool| h(&v)) as _
                    }),
                    on_file_drop: on_file_drop.map(|id| {
                        let h = handler::<WireFileDrop, TouchResponse>(conn, id, TouchResponse::default);
                        Rc::new(move |e: &runtime_shared::file_drop::FileDropEvent| h(&wire_file_drop(e))) as _
                    }),
                    preserves_focus,
                    is_container,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::View),
                }),
                build_all(conn, children)?,
            )
        }
        Node::Pressable { common, on_press, disabled, preserves_focus, children } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(PressablePrim {
                    test_id,
                    on_press: fire(conn, on_press),
                    disabled: disabled.map(|v| value(conn, v)),
                    preserves_focus,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Pressable),
                }),
                build_all(conn, children)?,
            )
        }
        Node::Text { common, content } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(TextPrim {
                    test_id,
                    content: match content {
                        TextContent::Value(v) => TextSourceProp::Value(value(conn, v)),
                        TextContent::Runs(runs) => TextSourceProp::Runs(runs),
                    },
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Text),
                }),
                Vec::new(),
            )
        }
        Node::Button { common, label, on_press, leading_icon, trailing_icon, disabled } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(ButtonPrim {
                    test_id,
                    label: value(conn, label),
                    on_press: action(conn, on_press),
                    leading_icon: leading_icon.map(intern_icon),
                    trailing_icon: trailing_icon.map(intern_icon),
                    disabled: disabled.map(|v| value(conn, v)),
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Button),
                }),
                Vec::new(),
            )
        }
        Node::Image { common, src, alt, on_load, on_error, asset } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(ImagePrim {
                    test_id,
                    src: value(conn, src),
                    alt: value(conn, alt),
                    on_load: on_load.map(|id| Rc::new(handler::<_, ()>(conn, id, || ())) as _),
                    on_error: on_error.map(|id| {
                        let h = handler::<(), ()>(conn, id, || ());
                        Rc::new(move || h(&())) as _
                    }),
                    asset: asset.map(|a| {
                        runtime_shared::assets::Asset::new(runtime_shared::assets::AssetId(a.id), asset_source(a.source))
                    }),
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Image),
                }),
                Vec::new(),
            )
        }
        Node::Icon { common, data, color, stroke, draw_in } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(IconPrim {
                    test_id,
                    data: value_with(conn, data, intern_icon, blank_icon),
                    color: color.map(|v| value_with(conn, v, |c| c, || runtime_shared::Color(String::new()))),
                    stroke: stroke.map(|v| value(conn, v)),
                    draw_in,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Icon),
                }),
                Vec::new(),
            )
        }
        Node::Link { common, url, route, external, on_activate, children } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(LinkPrim {
                    test_id,
                    url: value(conn, url),
                    external,
                    on_activate: on_activate.map(|id| fire(conn, id)),
                    route_link: route.map(|name| RouteLink {
                        name: intern(&name),
                        make_params: Rc::new(|| Box::new(ParamsFromUrl) as Box<dyn std::any::Any>),
                    }),
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Link),
                }),
                build_all(conn, children)?,
            )
        }
        Node::Toggle { common, value: v, on_change } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            let h = handler::<bool, ()>(conn, on_change, || ());
            runtime_scene::item(
                PrimCell::new(TogglePrim {
                    test_id,
                    value: value(conn, v),
                    on_change: Rc::new(move |b: bool| h(&b)),
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Toggle),
                }),
                Vec::new(),
            )
        }
        Node::Slider { common, value: v, on_change, min, max, step } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            let h = handler::<f32, ()>(conn, on_change, || ());
            runtime_scene::item(
                PrimCell::new(SliderPrim {
                    test_id,
                    value: value(conn, v),
                    on_change: Rc::new(move |x: f32| h(&x)),
                    min,
                    max,
                    step,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Slider),
                }),
                Vec::new(),
            )
        }
        Node::ActivityIndicator { common, size, color } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(ActivityIndicatorPrim {
                    test_id,
                    size: value_with(conn, size, |s| s, || {
                        runtime_shared::primitives::activity_indicator::ActivityIndicatorSize::Small
                    }),
                    color,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::ActivityIndicator),
                }),
                Vec::new(),
            )
        }
        Node::TextInput { common, value: v, on_change, on_key_down, on_blur, on_focus, placeholder, secure } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            let change = handler::<String, ()>(conn, on_change, || ());
            runtime_scene::item(
                PrimCell::new(TextInputPrim {
                    test_id,
                    value: value(conn, v),
                    on_change: Rc::new(move |t: String| change(&t)),
                    on_key_down: on_key_down.map(|id| Rc::new(handler(conn, id, || KeyOutcome::Default)) as _),
                    on_blur: on_blur.map(|id| {
                        let h = handler::<(), BlurOutcome>(conn, id, || BlurOutcome::Allow);
                        Rc::new(move || h(&())) as _
                    }),
                    on_focus: on_focus.map(|id| {
                        let h = handler::<bool, ()>(conn, id, || ());
                        Rc::new(move |f: bool| h(&f)) as _
                    }),
                    placeholder: value(conn, placeholder),
                    secure: value(conn, secure),
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::TextInput),
                }),
                Vec::new(),
            )
        }
        Node::TextArea { common, value: v, on_change, on_key_down, placeholder, wrap, min_rows, max_rows } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            let change = handler::<String, ()>(conn, on_change, || ());
            runtime_scene::item(
                PrimCell::new(TextAreaPrim {
                    test_id,
                    value: value(conn, v),
                    on_change: Rc::new(move |t: String| change(&t)),
                    on_key_down: on_key_down.map(|id| Rc::new(handler(conn, id, || KeyOutcome::Default)) as _),
                    placeholder,
                    wrap,
                    min_rows,
                    max_rows,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::TextArea),
                }),
                Vec::new(),
            )
        }
        Node::ScrollView {
            common,
            horizontal,
            on_scroll,
            on_end_reached,
            end_reached_threshold,
            safe_area,
            bounces,
            always_bounce,
            children,
        } => {
            let (test_id, style, a11y, fill) = self::common(conn, common);
            runtime_scene::item(
                PrimCell::new(ScrollViewPrim {
                    test_id,
                    horizontal,
                    on_scroll: on_scroll.map(|id| {
                        let h = handler::<(f32, f32), ()>(conn, id, || ());
                        Rc::new(move |x: f32, y: f32| h(&(x, y))) as _
                    }),
                    on_end_reached: on_end_reached.map(|id| fire(conn, id)),
                    end_reached_threshold,
                    safe_area: safe_area.map(SafeAreaSides),
                    bounces,
                    always_bounce,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::ScrollView),
                }),
                build_all(conn, children)?,
            )
        }
        Node::Repeat { count, row } => {
            let row = render_with::<usize>(conn, row);
            runtime_scene::many(PrimCell::new(RepeatPrim { count, row_builder: Box::new(move |i| row(&i)) }))
        }
        Node::Presence { test_id, a11y: a, fill, child, present, enter, exit } => {
            let (c, child) = (conn.clone(), cb(conn, child));
            let present = cb(conn, present);
            runtime_scene::item(
                PrimCell::new(PresencePrim {
                    test_id: test_id.as_deref().map(intern),
                    child: Box::new(move || child.call(&[]).map_or_else(nothing, |bytes| subtree(&c, &bytes))),
                    present: Rc::new(move || present.get::<bool>(&[]).unwrap_or(false)),
                    enter,
                    exit,
                    a11y: a.map(|a| a11y(conn, *a)).unwrap_or_default(),
                    ref_fill: fill_handle(conn, fill, Held::Presence),
                }),
                Vec::new(),
            )
        }
        Node::Portal { target, fill, on_dismiss, trap_focus, style: st, a11y: a, children } => {
            use runtime_shared::primitives::portal::PortalTarget;
            runtime_scene::item(
                PrimCell::new(PortalPrim {
                    target: match target {
                        WirePortalTarget::Viewport(v) => PortalTarget::Viewport(v),
                        WirePortalTarget::Named(n) => PortalTarget::Named(intern(&n)),
                        WirePortalTarget::Anchor { rect, side, align, offset } => {
                            let rect = handler::<(), Option<runtime_shared::primitives::portal::ViewportRect>>(conn, rect, || None);
                            PortalTarget::Anchor {
                                target: runtime_shared::primitives::portal::AnchorTarget::from_fn(move || rect(&())),
                                side,
                                align,
                                offset,
                            }
                        }
                    },
                    on_dismiss: on_dismiss.map(|id| fire(conn, id)),
                    trap_focus,
                    style: st.map(|st| style(conn, *st)),
                    a11y: a.map(|a| a11y(conn, *a)).unwrap_or_default(),
                    ref_fill: fill_handle(conn, fill, Held::Portal),
                }),
                build_all(conn, children)?,
            )
        }
        Node::Virtualizer {
            common,
            item_count,
            item_key,
            measured,
            item_size,
            render_item,
            item_diff,
            overscan,
            layout,
            on_scroll,
            on_end_reached,
            end_reached_threshold,
            safe_area,
        } => {
            use runtime_shared::primitives::virtualizer::{ItemDiff, ItemSize};
            let (_, style, a11y, fill) = self::common(conn, common);
            let count = handler::<(), usize>(conn, item_count, || 0);
            let key = handler::<usize, u64>(conn, item_key, || 0);
            let size: Rc<dyn Fn(usize) -> f32> = {
                let h = handler::<usize, f32>(conn, item_size, || 0.0);
                Rc::new(move |i| h(&i))
            };
            let render = render_with::<usize>(conn, render_item);
            runtime_scene::item(
                PrimCell::new(VirtualizerPrim {
                    item_count: Box::new(move || count(&())),
                    item_key: Box::new(move |i| key(&i)),
                    item_size: if measured { ItemSize::Measured(size) } else { ItemSize::Known(size) },
                    render_item: Rc::new(move |i| render(&i)),
                    item_diff: item_diff.map(|(capture, differs)| {
                        let c = conn.clone();
                        let capture = handler::<usize, Option<Cb>>(conn, capture, || None);
                        let differs = handler::<(Cb, usize), bool>(conn, differs, || false);
                        ItemDiff {
                            capture: Rc::new(move |i| {
                                capture(&i).map(|id| Box::new(ItemRef(CbRef::new(id, c.clone()))) as Box<dyn std::any::Any>)
                            }),
                            differs: Rc::new(move |snap, i| match snap.downcast_ref::<ItemRef>() {
                                Some(snap) => differs(&(snap.0.id, i)),
                                None => true,
                            }),
                        }
                    }),
                    overscan,
                    layout,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::Virtualizer),
                    on_scroll: on_scroll.map(|id| {
                        let h = handler::<(f32, f32), ()>(conn, id, || ());
                        Rc::new(move |x: f32, y: f32| h(&(x, y))) as _
                    }),
                    on_end_reached: on_end_reached.map(|id| fire(conn, id)),
                    end_reached_threshold,
                    safe_area: safe_area.map(SafeAreaSides),
                }),
                Vec::new(),
            )
        }
        Node::VirtualGrid { common, col_count, row_count, col_width, row_height, cell_key, render_cell, overscan, on_scroll } => {
            let (_, style, a11y, fill) = self::common(conn, common);
            let cols = handler::<(), usize>(conn, col_count, || 0);
            let rows = handler::<(), usize>(conn, row_count, || 0);
            let cw = handler::<usize, f32>(conn, col_width, || 0.0);
            let rh = handler::<usize, f32>(conn, row_height, || 0.0);
            let key = handler::<(usize, usize), u64>(conn, cell_key, || 0);
            let render = render_with::<(usize, usize)>(conn, render_cell);
            runtime_scene::item(
                PrimCell::new(VirtualGridPrim {
                    col_count: Box::new(move || cols(&())),
                    row_count: Box::new(move || rows(&())),
                    col_width: Rc::new(move |i| cw(&i)),
                    row_height: Rc::new(move |i| rh(&i)),
                    cell_key: Rc::new(move |r, c| key(&(r, c))),
                    render_cell: Rc::new(move |r, c| render(&(r, c))),
                    overscan,
                    style,
                    a11y,
                    ref_fill: fill_handle(conn, fill, Held::VirtualGrid),
                    on_scroll: on_scroll.map(|id| {
                        let h = handler::<(f32, f32), ()>(conn, id, || ());
                        Rc::new(move |x: f32, y: f32| h(&(x, y))) as _
                    }),
                }),
                Vec::new(),
            )
        }
        Node::StackNavigator { config, layout, retention, style: st, a11y: a, on_handle, nav_label } => {
            let layout = layout.map(|id| {
                let render = render_with::<Option<WireStackNav>>(conn, id);
                Rc::new(move || {
                    // The app's `StackNav` (provided by the navigator around
                    // this call), exported for the bundle's layout to read.
                    let mut keep: Vec<Box<dyn std::any::Any>> = Vec::new();
                    let nav = runtime_world::inject::<StackNav>().map(|n| export_stack_nav(&n, &mut keep));
                    keeping(render(&nav), keep)
                }) as Rc<dyn Fn() -> Element>
            });
            runtime_scene::item(
                PrimCell::new(StackNavigatorPrim {
                    config: nav_config(conn, config),
                    layout,
                    retention,
                    style: st.map(|s| style(conn, *s)),
                    a11y: a.map(|a| a11y(conn, *a)).unwrap_or_default(),
                    on_handle: fill_handle(conn, on_handle, Held::Nav),
                    nav_label: nav_label.as_deref().map(intern),
                }),
                Vec::new(),
            )
        }
        Node::SwapNavigator { config, layout, mount_policy, select_args, style: st, a11y: a, on_handle, nav_label } => {
            let layout = layout.map(|id| {
                let render = render_with::<Option<WireSwapNav>>(conn, id);
                Rc::new(move || {
                    let mut keep: Vec<Box<dyn std::any::Any>> = Vec::new();
                    let nav = runtime_world::inject::<SwapNav>().map(|n| export_swap_nav(&n, &mut keep));
                    keeping(render(&nav), keep)
                }) as Rc<dyn Fn() -> Element>
            });
            let select_args = select_args
                .into_iter()
                .map(|(route, id)| {
                    let (c, h) = (conn.clone(), handler::<(), Option<(String, Cb)>>(conn, id, || None));
                    let args: SelectArgs = Rc::new(move || {
                        h(&()).map(|(url, item)| (url, Box::new(ItemRef(CbRef::new(item, c.clone()))) as Box<dyn std::any::Any>))
                    });
                    (intern(&route), args)
                })
                .collect();
            runtime_scene::item(
                PrimCell::new(SwapNavigatorPrim {
                    config: nav_config(conn, config),
                    layout,
                    mount_policy,
                    select_args,
                    style: st.map(|s| style(conn, *s)),
                    a11y: a.map(|a| a11y(conn, *a)).unwrap_or_default(),
                    on_handle: fill_handle(conn, on_handle, Held::Nav),
                    nav_label: nav_label.as_deref().map(intern),
                }),
                Vec::new(),
            )
        }
        Node::NavigatorOutlet { style: st, a11y: a } => runtime_scene::item(
            PrimCell::new(NavigatorOutletPrim {
                style: st.map(|s| style(conn, *s)),
                a11y: a.map(|a| a11y(conn, *a)).unwrap_or_default(),
            }),
            Vec::new(),
        ),
        Node::Fragment(children) => runtime_scene::fragment(build_all(conn, children)?),
        Node::Dyn { build } => {
            let (c, b) = (conn.clone(), cb(conn, build));
            dyn_element(move || b.call(&[]).map_or_else(nothing, |bytes| subtree(&c, &bytes)))
        }
        Node::Guarded { changed, build } => {
            let changed = cb(conn, changed);
            let (c, b) = (conn.clone(), cb(conn, build));
            dyn_guarded(
                move || changed.get::<bool>(&[]).unwrap_or(false),
                move || b.call(&[]).map_or_else(nothing, |bytes| subtree(&c, &bytes)),
            )
        }
        Node::Keyed { items, render } => keyed(conn, cb(conn, items), cb(conn, render)),
        Node::Owned { scope, element } => {
            runtime_scene::owned(build(conn, *element)?, runtime_world::remote::claim_scope(scope))
        }
        Node::Import { name, props, children } => {
            let children = build_all(conn, children)?;
            match IMPORTS.with(|m| m.borrow().get(name.as_str()).cloned()) {
                Some(f) => f(&props, children).map_err(|reason| DecodeError::BadProps { name, reason })?,
                // Every component not marked `remote` is assumed to be in
                // this binary: its `#[component]` registered it.
                None => {
                    let app = app_component(&name).ok_or_else(|| DecodeError::MissingImport(name.clone()))?;
                    let cx = ImportCx(conn.clone());
                    let mut input = &props[..];
                    (app.build)(&mut input, &cx).map_err(|reason| DecodeError::BadProps { name, reason })?
                }
            }
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
            let rows: Vec<(WireKey, Cb)> = items.get(&[]).unwrap_or_default();
            rows.into_iter()
                .map(|(key, id)| {
                    let item = ItemRef(CbRef::new(id, c.clone()));
                    (key.into(), Box::new(item) as Box<dyn std::any::Any>)
                })
                .collect()
        }),
        render: Box::new(move |item| {
            let item = item.downcast::<ItemRef>().expect("remote codec: keyed render got a foreign item");
            render.call(&to_bytes(&item.0.id)).map_or_else(nothing, |bytes| subtree(&rc, &bytes))
        }),
    }
}

fn fire(conn: &Rc<Conn>, id: Cb) -> Rc<dyn Fn()> {
    let r = cb(conn, id);
    Rc::new(move || {
        r.call(&[]);
    })
}

fn value<T: DeserializeOwned + Default + 'static>(conn: &Rc<Conn>, v: Val<T>) -> Value<T> {
    value_with(conn, v, |v| v, T::default)
}

/// A `Value<T>` that crossed as `W`; `fallback` when a stopped bundle's
/// getter has no last value.
fn value_with<W: DeserializeOwned + 'static, T: 'static>(
    conn: &Rc<Conn>,
    v: Val<W>,
    map: fn(W) -> T,
    fallback: fn() -> T,
) -> Value<T> {
    match v {
        Val::Const(v) => Value::Const(map(v)),
        Val::Dyn(id) => {
            let r = cb(conn, id);
            Value::Dyn(Box::new(move || r.get::<W>(&[]).map_or_else(fallback, map)))
        }
    }
}

/// A bundle callback that takes `A` and replies a [`Node`], as a builder:
/// a stopped bundle's builds render nothing.
fn render_with<A: Serialize + 'static>(conn: &Rc<Conn>, id: Cb) -> impl Fn(&A) -> Element {
    let (c, r) = (conn.clone(), cb(conn, id));
    move |a: &A| r.call(&to_bytes(a)).map_or_else(nothing, |bytes| subtree(&c, &bytes))
}

/// An event handler that runs in the bundle: the event crosses encoded,
/// the reply comes back encoded; `fallback` is the reply once the bundle
/// can't be called (it was stopped) — the platform's default behaviour.
fn handler<A: Serialize + 'static, R: DeserializeOwned + 'static>(
    conn: &Rc<Conn>,
    id: Cb,
    fallback: fn() -> R,
) -> impl Fn(&A) -> R {
    let r = cb(conn, id);
    move |event: &A| {
        r.call(&to_bytes(event)).map_or_else(fallback, |bytes| {
            from_bytes(&bytes).unwrap_or_else(|e| panic!("remote codec: a handler's reply does not decode: {e}"))
        })
    }
}

fn wire_file_drop(e: &runtime_shared::file_drop::FileDropEvent) -> WireFileDrop {
    use runtime_shared::file_drop::FileDropPhase;
    let phase = match &e.phase {
        FileDropPhase::Entered => WireDropPhase::Entered,
        FileDropPhase::Exited => WireDropPhase::Exited,
        FileDropPhase::Dropped(files) => WireDropPhase::Dropped(
            files
                .iter()
                .map(|f| WireDroppedFile { name: f.name.clone(), mime: f.mime.clone(), size: f.size, path: f.path.clone() })
                .collect(),
        ),
        // `FileDropPhase` is non-exhaustive: a phase added later reaches the
        // bundle as "left", the safe reading for a drag it doesn't know.
        _ => WireDropPhase::Exited,
    };
    WireFileDrop { phase, position: e.position }
}

/// The prims hold icon and asset data as `&'static`: decoded ones are
/// interned, one leak per DISTINCT icon / byte blob for the process's
/// life — bounded by what bundles actually use, like [`intern`].
fn blank_icon() -> runtime_shared::primitives::icon::IconData {
    runtime_shared::primitives::icon::IconData {
        view_box: (0, 0),
        paths: &[],
        fill_rule: runtime_shared::primitives::icon::FillRule::NonZero,
        filled: false,
    }
}

fn intern_bytes(b: Vec<u8>) -> &'static [u8] {
    static BLOBS: OnceLock<Mutex<HashSet<&'static [u8]>>> = OnceLock::new();
    let mut blobs = BLOBS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&x) = blobs.get(&b[..]) {
        return x;
    }
    let x: &'static [u8] = Box::leak(b.into_boxed_slice());
    blobs.insert(x);
    x
}

fn asset_source(s: WireAssetSource) -> runtime_shared::assets::AssetSource {
    use runtime_shared::assets::AssetSource;
    match s {
        WireAssetSource::Embedded { bytes, extension } => {
            AssetSource::Embedded { bytes: intern_bytes(bytes), extension: intern(&extension) }
        }
        WireAssetSource::Bundled { path } => AssetSource::Bundled { path: intern(&path) },
        WireAssetSource::BundledEmbedded { path, bytes, extension } => AssetSource::BundledEmbedded {
            path: intern(&path),
            bytes: intern_bytes(bytes),
            extension: intern(&extension),
        },
        WireAssetSource::Remote { url } => AssetSource::Remote { url: intern(&url) },
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

fn common(conn: &Rc<Conn>, c: Common) -> (Option<&'static str>, Option<StyleProp>, AccessibilityProps, Option<Cb>) {
    (
        c.test_id.as_deref().map(intern),
        c.style.map(|s| style(conn, *s)),
        c.a11y.map_or_else(Default::default, |a| a11y(conn, *a)),
        c.fill,
    )
}

/// A prim's `ref_fill`, decoded: when the backend fills the ref, hold the
/// real handle for the bundle (`handles::hold`) and send the bundle its id.
/// The bundle's fill callback is released with this closure — after the
/// call, or unrun if the prim never mounts.
fn fill_handle<H: 'static>(
    conn: &Rc<Conn>,
    fill: Option<Cb>,
    wrap: fn(H) -> super::handles::Held,
) -> Option<Box<dyn FnOnce(H)>> {
    let (c, r) = (Rc::downgrade(conn), cb(conn, fill?));
    Some(Box::new(move |h: H| {
        let tree: std::rc::Weak<dyn std::any::Any> = c;
        let id = super::handles::hold(wrap(h), tree);
        r.call(&to_bytes(&id));
    }))
}

/// Callback `id` of the tree `conn` (a handle table entry's tree).
pub(crate) fn callback_for(conn: &Rc<dyn std::any::Any>, id: Cb) -> CallbackRef {
    let conn = conn.clone().downcast::<Conn>().unwrap_or_else(|_| panic!("remote codec: a handle's tree is not a Conn"));
    CallbackRef(cb(&conn, id))
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
            StyleProp::Dynamic(Box::new(move || Rc::new(r.get::<StyleRules>(&[]).unwrap_or_default())))
        }
        Style::Sheet(app) => StyleProp::Sheet(Box::new(application(conn, app))),
        Style::SheetDynamic(id) => {
            let (c, r) = (conn.clone(), cb(conn, id));
            StyleProp::SheetDynamic(Box::new(move || match r.get::<App>(&[]) {
                Some(app) => application(&c, app),
                None => StyleApplication::new(Rc::new(StyleSheet::new(|_| StyleRules::default()))),
            }))
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
        out = out.with_computed(key, move || r.get::<StyleRules>(&[]).unwrap_or_default());
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
            eval.get::<StyleRules>(&to_bytes(&(part, variants))).unwrap_or_default()
        }),
    ));
    conn.sheets.borrow_mut().insert(r.id, Rc::downgrade(&sheet));
    sheet
}

/// A route name, interned (`NavCommand` / `RouteLink` names are
/// `&'static str`).
pub(crate) fn intern_name(s: &str) -> &'static str {
    intern(s)
}

/// `test_id` and an action's `method` are `&'static str` on the prims:
/// interned, one leak per distinct string for the process's life.
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

// ---------------------------------------------------------------------------
// `#[component(remote)]` on the app side
// ---------------------------------------------------------------------------

/// What a remote component's props keep alive for as long as it is mounted
/// (the export guards of signals it was handed).
pub type Keep = Vec<Box<dyn std::any::Any>>;

/// Where `#[component(remote)]` components come from: the app installs one
/// (`stream_host::remote::install`) before mounting any.
pub trait Loader: 'static {
    /// A number that changes when the bundle is replaced. Read TRACKED by
    /// every mounted remote component, so a reload remounts them.
    fn generation(&self) -> u64;
    /// Mount component `component` (its `#[component(remote)]` fn name) with
    /// its encoded props. An `Err` is shown in the component's place.
    fn mount(&self, component: &str, args: &[u8]) -> Result<Element, String>;
}

thread_local! {
    static LOADER: RefCell<Option<Rc<dyn Loader>>> = const { RefCell::new(None) };
}

/// Install the app's remote component loader.
pub fn install_loader(loader: Rc<dyn Loader>) {
    LOADER.with(|l| *l.borrow_mut() = Some(loader));
    export_contexts();
}

thread_local! {
    static EXPORTED_CONTEXTS: RefCell<Option<Vec<Box<dyn std::any::Any>>>> = const { RefCell::new(None) };
}

/// Offer every `#[remote_context]` type to bundles (once per thread; the
/// remote loader's install does it). Undeclared context stays invisible.
pub fn export_contexts() {
    EXPORTED_CONTEXTS.with(|e| {
        let mut e = e.borrow_mut();
        if e.is_none() {
            *e = Some(super::REMOTE_CONTEXTS.iter().map(|c| (c.export)()).collect());
        }
    });
}

/// The app-side body of a `#[component(remote)]` component (macro-emitted):
/// send the props, then mount the component from the installed loader —
/// again whenever the loader's bundle is replaced.
#[doc(hidden)]
pub fn __mount_remote(component: &'static str, send: impl FnOnce(&mut Vec<u8>, &mut Keep)) -> Element {
    let mut args = Vec::new();
    let mut keep = Keep::new();
    send(&mut args, &mut keep);
    // The exports live exactly as long as this component's scope.
    runtime_world::on_scope_drop(move || drop(keep));
    let loader = LOADER.with(|l| l.borrow().clone()).unwrap_or_else(|| {
        panic!(
            "remote component `{component}` mounted, but this app installed no remote loader \
             (`stream_host::remote::install`)"
        )
    });
    let select = loader.clone();
    runtime_scene::dyn_keyed(
        move || select.generation(),
        move |_| match loader.mount(component, &args) {
            Ok(element) => element,
            Err(msg) => crate::builders::text().content(format!("⚠ remote component `{component}`: {msg}")).build(),
        },
    )
}

// ---------------------------------------------------------------------------
// The app's components, as bundles import them
// ---------------------------------------------------------------------------

/// One app component bundles may use: every `#[component]` not marked
/// `remote` registers one in a native app build with `remote` on
/// (`crate::__remote_app_component!`).
pub struct AppComponent {
    /// `module_path::Name` — what the bundle's stub asks for.
    pub name: &'static str,
    /// Decode the props (`ImportArg`) and build the component.
    pub build: fn(&mut &[u8], &ImportCx) -> Result<Element, String>,
}

/// Filled at link time: no startup work, and nothing in apps without
/// `remote`.
#[linkme::distributed_slice]
pub static APP_COMPONENTS: [AppComponent];

fn app_component(name: &str) -> Option<&'static AppComponent> {
    thread_local! {
        static BY_NAME: HashMap<&'static str, &'static AppComponent> =
            APP_COMPONENTS.iter().map(|c| (c.name, c)).collect();
    }
    BY_NAME.with(|m| m.get(name).copied())
}

/// The names of the app components bundles may import.
pub fn app_component_names() -> Vec<&'static str> {
    APP_COMPONENTS.iter().map(|c| c.name).collect()
}

/// What an imported component's props decode against: the bundle they came
/// from.
#[derive(Clone)]
pub struct ImportCx(Rc<Conn>);

impl ImportCx {
    pub(crate) fn build(&self, node: Node) -> Result<Element, String> {
        build(&self.0, node).map_err(|e| e.to_string())
    }
    pub(crate) fn callback(&self, id: Cb) -> CallbackRef {
        CallbackRef(cb(&self.0, id))
    }
    pub(crate) fn sheet(&self, r: SheetRef) -> Rc<StyleSheet> {
        sheet(&self.0, r)
    }
    /// Hold `held` for this bundle's tree (it dies with the tree); its id.
    pub(crate) fn hold(&self, held: super::handles::Held) -> u32 {
        let tree: std::rc::Weak<dyn std::any::Any> = Rc::downgrade(&self.0) as std::rc::Weak<Conn>;
        super::handles::hold(held, tree)
    }
}

/// A bundle callback a decoded prop holds.
pub(crate) struct CallbackRef(Rc<CbRef>);

impl CallbackRef {
    /// `None` when the bundle can't be called (poisoned or gone).
    pub(crate) fn call(&self, args: &[u8]) -> Option<Vec<u8>> {
        self.0.call(args)
    }
    /// See `CbRef::get_bytes`.
    pub(crate) fn get_bytes(&self, args: &[u8]) -> Option<Vec<u8>> {
        self.0.get_bytes(args)
    }
}

// ---------------------------------------------------------------------------
// Values the app defines natively, crossing by key
// ---------------------------------------------------------------------------

/// One value a bundle may name by key (`__remote_key!`): a tone, a
/// variant — something the app's code defines, with behavior, so it can't
/// cross as data. The bundle sends the key; the app rebuilds its own.
pub struct KeyedEntry {
    pub ty: fn() -> std::any::TypeId,
    pub key: fn() -> &'static str,
    pub make: fn() -> Box<dyn std::any::Any>,
}

/// Every `__remote_key!` in the app. Filled at link time.
#[linkme::distributed_slice]
pub static KEYED: [KeyedEntry];

/// The app's `T` registered under `key`, if any.
pub fn by_key<T: 'static>(key: &str) -> Option<T> {
    type Make = fn() -> Box<dyn std::any::Any>;
    thread_local! {
        static BY_KEY: HashMap<(std::any::TypeId, &'static str), Make> =
            KEYED.iter().map(|e| (((e.ty)(), (e.key)()), e.make)).collect();
    }
    let make = BY_KEY.with(|m| m.get(&(std::any::TypeId::of::<T>(), key)).copied())?;
    Some(*make().downcast::<T>().expect("a keyed entry builds the type it is registered under"))
}

// ---------------------------------------------------------------------------
// Navigators a bundle defines
// ---------------------------------------------------------------------------

/// A bundle screen's chrome options, held bundle-side.
struct OptionsRef(CbRef);

/// The bundle's id for chrome options it built (`nav_codecs::chrome`).
pub(crate) fn options_id(options: &dyn std::any::Any) -> Option<Cb> {
    options.downcast_ref::<OptionsRef>().map(|o| o.0.id)
}

/// A bundle navigator's screens: their typed params are items held in the
/// bundle (`ItemRef` here), made by its `from_segments` and consumed by its
/// `build`.
fn nav_config(conn: &Rc<Conn>, w: WireNavConfig) -> NavConfig {
    let mut screens = HashMap::new();
    for e in w.screens {
        let (c1, c2) = (conn.clone(), conn.clone());
        let build = cb(conn, e.build);
        let from = handler::<HashMap<String, String>, Option<Cb>>(conn, e.from_segments, || None);
        let name = intern(&e.name);
        screens.insert(
            name,
            NavScreenEntry {
                path: intern(&e.path),
                order: e.order,
                build: Rc::new(move |params: Box<dyn std::any::Any>| {
                    // The item (if any) is released after the call, which
                    // consumed it bundle-side.
                    let (wire, item) = match params.downcast::<ItemRef>() {
                        Ok(item) => (WireParams::Item(item.0.id), Some(item)),
                        Err(params) if params.is::<()>() => (WireParams::Unit, None),
                        Err(_) => panic!("remote navigator: screen `{name}` was given params the bundle didn't make"),
                    };
                    let reply = build.call(&to_bytes(&wire));
                    drop(item);
                    match reply {
                        Some(bytes) => {
                            let ws: WireScreen = from_bytes(&bytes)
                                .unwrap_or_else(|e| panic!("remote codec: a screen does not decode: {e}"));
                            Screen {
                                element: self::build(&c1, ws.element).unwrap_or_else(|e| panic!("remote component: {e}")),
                                options: ws.options.map(|id| Rc::new(OptionsRef(CbRef::new(id, c1.clone()))) as Rc<dyn std::any::Any>),
                            }
                        }
                        None => Screen::new(nothing()),
                    }
                }),
                from_segments: Rc::new(move |segs: &HashMap<String, String>| {
                    from(segs).map(|id| Box::new(ItemRef(CbRef::new(id, c2.clone()))) as Box<dyn std::any::Any>)
                }),
            },
        );
    }
    NavConfig { initial: intern(&w.initial), initial_path: intern(&w.initial_path), screens }
}

/// `element`, owning `keep` (export guards) for as long as it is mounted.
fn keeping(element: Element, keep: Vec<Box<dyn std::any::Any>>) -> Element {
    let ((), owned) = runtime_world::collect_owned(|| runtime_world::on_scope_drop(move || drop(keep)));
    runtime_scene::owned(element, owned)
}

fn export_handle<T: PartialEq + 'static>(
    s: runtime_world::Signal<T>,
    codec: runtime_world::remote::Codec<T>,
    keep: &mut Vec<Box<dyn std::any::Any>>,
) -> SignalHandle {
    let (h, guard) = runtime_world::remote::export_signal(s, codec);
    keep.push(Box::new(guard));
    h
}

fn export_call(f: Rc<dyn Fn(&[u8]) -> Vec<u8>>, keep: &mut Vec<Box<dyn std::any::Any>>) -> u32 {
    let (id, guard) = super::handles::hold_scoped(Held::Call(f));
    keep.push(Box::new(guard));
    id
}

fn export_stack_nav(n: &StackNav, keep: &mut Vec<Box<dyn std::any::Any>>) -> WireStackNav {
    let pop = n.pop.clone();
    WireStackNav {
        active_route: export_handle(n.active_route, nav_codecs::route(), keep),
        active_path: export_handle(n.active_path, signal_codec::<String>(), keep),
        query: export_handle(n.query, nav_codecs::query(), keep),
        depth: export_handle(n.depth, signal_codec::<usize>(), keep),
        can_go_back: export_handle(n.can_go_back, signal_codec::<bool>(), keep),
        screen_chrome: export_handle(n.screen_chrome, nav_codecs::chrome(), keep),
        pop: export_call(
            Rc::new(move |_: &[u8]| {
                pop();
                Vec::new()
            }),
            keep,
        ),
    }
}

fn export_swap_nav(n: &SwapNav, keep: &mut Vec<Box<dyn std::any::Any>>) -> WireSwapNav {
    let select = n.on_select.clone();
    WireSwapNav {
        active_route: export_handle(n.active_route, nav_codecs::route(), keep),
        active_path: export_handle(n.active_path, signal_codec::<String>(), keep),
        query: export_handle(n.query, nav_codecs::query(), keep),
        on_select: export_call(
            Rc::new(move |args: &[u8]| {
                let route: String =
                    from_bytes(args).unwrap_or_else(|e| panic!("remote codec: a route name does not decode: {e}"));
                select(intern(&route));
                Vec::new()
            }),
            keep,
        ),
    }
}
