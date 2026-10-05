//! `ref`s across the boundary: a bundle holding a handle to a node the APP
//! mounted (`text_input(..).on_handle(|h| h.focus())`).
//!
//! A handle is a node plus a static ops table (`TextInputHandle::new(node,
//! ops)`). So a bundle gets the SAME handle type as native code, built with
//! [`RemoteOps`] — one type implementing every primitive's ops trait, each
//! method sending a [`HandleCall`] to the app — and a node that is only the
//! app's id for the real handle ([`RemoteNode`]). Author code doesn't
//! change.
//!
//! The app keeps the real handle in a table ([`handle_call`] dispatches to
//! it) from the moment the backend fills the ref until the bundle drops its
//! copy (the node's `Drop` sends [`HandleCall::Release`]). Entries also die
//! with the tree that made them (`purge_dead`, when its connection drops) —
//! a stopped bundle never releases, and a bundle may keep a handle in a
//! `Ref` slot that outlives the tree; neither may keep native nodes alive.
//!
//! Transport: a bundle→app call. Over wasm it is the `idealyst_ui.handle_call`
//! import (`stream-host` defines it); in-process (`remote-loopback`) a direct
//! call.

use serde::{Deserialize, Serialize};

#[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
pub use bundle_side::*;
#[cfg(not(idealyst_stream_guest))]
pub use host_side::*;

use runtime_shared::animation::AnimProp;
use runtime_shared::Easing;

/// One handle method, as it crosses. Each reply is postcard-encoded: the
/// method's return value, `()` when it has none.
#[derive(Serialize, Deserialize, Debug)]
pub enum HandleCall {
    /// `AnchorableHandle::rect` (view, pressable, button) → `ViewportRect`.
    Rect,
    /// → `Option<ViewportRect>`.
    Frame,
    AbsoluteFrame,
    SetAnimatedF32 { prop: AnimProp, value: f32 },
    SetAnimatedColor { prop: AnimProp, value: [f32; 4] },
    /// → `bool`.
    InstallKeyframes { prop: AnimProp, keyframes: Vec<(f32, f32)>, duration_ms: u32, repeat_forever: bool, autoreverse: bool },
    /// `callback` takes `(f32, f32)`. → the subscription's id (`u32`).
    SubscribeLayout { callback: super::Cb },
    Unsubscribe(u32),
    Click,
    Activate,
    AnimateStroke { from: f32, to: f32, duration_ms: u32, easing: Easing },
    SetStrokeProgress(f32),
    Focus,
    Blur,
    SelectAll,
    InsertText(String),
    ScrollTo { x: f32, y: f32 },
    ScrollToIndex(usize),
    ScrollToCell { col: usize, row: usize },
    /// → `(f32, f32)`.
    ScrollOffset,
    /// The bundle dropped its handle.
    Release,
    /// A navigation command on a navigator handle (`NavHandle`).
    Nav(WireNav),
    /// Call an app closure handed to the bundle (`Held::Call`) with encoded
    /// arguments; → its encoded reply.
    Invoke(Vec<u8>),
}

/// A `NavCommand` as it crosses. Typed params never do: the receiving
/// navigator rebuilds them from `url` (`ParamsFromUrl`).
#[derive(Serialize, Deserialize, Debug)]
pub enum WireNav {
    Push { name: String, url: String, query: String },
    Replace { name: String, url: String, query: String },
    Reset { name: String, url: String, query: String },
    Select { name: String, url: String, query: String },
    Pop,
}

// ---------------------------------------------------------------------------
// Bundle side
// ---------------------------------------------------------------------------

#[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
mod bundle_side {
    use std::any::Any;
    use std::cell::RefCell;
    use std::rc::Rc;

    use runtime_shared::animation::AnimProp;
    use runtime_shared::handles::*;
    use runtime_shared::primitives::activity_indicator::ActivityIndicatorOps;
    use runtime_shared::primitives::icon::IconOps;
    use runtime_shared::primitives::image::ImageOps;
    use runtime_shared::primitives::link::LinkOps;
    use runtime_shared::primitives::portal::{PortalOps, ViewportRect};
    use runtime_shared::primitives::presence::PresenceOps;
    use runtime_shared::primitives::scroll_view::ScrollViewOps;
    use runtime_shared::primitives::slider::SliderOps;
    use runtime_shared::primitives::text_area::TextAreaOps;
    use runtime_shared::primitives::text_input::TextInputOps;
    use runtime_shared::primitives::toggle::ToggleOps;
    use runtime_shared::primitives::virtual_grid::VirtualGridOps;
    use runtime_shared::primitives::virtualizer::VirtualizerOps;
    use runtime_shared::Easing;
    use serde::de::DeserializeOwned;

    use super::HandleCall;
    use crate::remote::{from_bytes, to_bytes, Cb};

    /// A handle's node, in a bundle: the app's id for the real handle.
    /// Dropping the last copy releases the app's entry.
    pub struct RemoteNode(pub u32);

    impl Drop for RemoteNode {
        fn drop(&mut self) {
            // During thread teardown the app side may already be gone;
            // `try_send` makes that a no-op.
            try_send(self.0, &HandleCall::Release);
        }
    }

    /// Every primitive's ops trait, forwarded to the app (see the module
    /// docs). One static: a handle's ops is `&'static dyn …Ops`.
    pub struct RemoteOps;

    pub static REMOTE_OPS: RemoteOps = RemoteOps;

    fn id(node: &dyn Any) -> u32 {
        node.downcast_ref::<RemoteNode>()
            .unwrap_or_else(|| panic!("remote codec: a bundle handle whose node is not a RemoteNode"))
            .0
    }

    fn call<R: DeserializeOwned>(node: &dyn Any, c: HandleCall) -> Option<R> {
        let reply = try_send(id(node), &c)?;
        Some(from_bytes(&reply).unwrap_or_else(|e| panic!("remote codec: a handle reply does not decode: {e}")))
    }

    fn send(node: &dyn Any, c: HandleCall) {
        try_send(id(node), &c);
    }

    /// Send `c` for handle `id`; `None` when the app is unreachable (thread
    /// teardown).
    fn try_send(id: u32, c: &HandleCall) -> Option<Vec<u8>> {
        transport(id, &to_bytes(c))
    }

    #[cfg(idealyst_stream_guest)]
    fn transport(id: u32, args: &[u8]) -> Option<Vec<u8>> {
        #[link(wasm_import_module = "idealyst_ui")]
        extern "C" {
            /// The app runs the call and writes its reply into this
            /// bundle's argument buffer (`idealyst_ui_alloc`); its length.
            fn handle_call(id: u32, args_ptr: *const u8, args_len: u32) -> i64;
        }
        // SAFETY: the app copies `args_len` bytes at `args_ptr` during the call.
        let len = unsafe { handle_call(id, args.as_ptr(), args.len() as u32) };
        Some(crate::remote::wasm::take_args(len as u32))
    }

    #[cfg(not(idealyst_stream_guest))]
    fn transport(id: u32, args: &[u8]) -> Option<Vec<u8>> {
        super::host_side::try_handle_call(id, args)
    }

    impl ViewOps for RemoteOps {
        fn rect(&self, node: &dyn Any) -> ViewportRect {
            call(node, HandleCall::Rect).unwrap_or_default()
        }
        fn frame(&self, node: &dyn Any) -> Option<ViewportRect> {
            call(node, HandleCall::Frame).flatten()
        }
        fn absolute_frame(&self, node: &dyn Any) -> Option<ViewportRect> {
            call(node, HandleCall::AbsoluteFrame).flatten()
        }
        fn set_animated_f32(&self, node: &dyn Any, prop: AnimProp, value: f32) {
            send(node, HandleCall::SetAnimatedF32 { prop, value })
        }
        fn set_animated_color(&self, node: &dyn Any, prop: AnimProp, value: [f32; 4]) {
            send(node, HandleCall::SetAnimatedColor { prop, value })
        }
        fn install_keyframe_animation(
            &self,
            node: &dyn Any,
            prop: AnimProp,
            keyframes: &[(f32, f32)],
            duration_ms: u32,
            repeat_forever: bool,
            autoreverse: bool,
        ) -> bool {
            let keyframes = keyframes.to_vec();
            call(node, HandleCall::InstallKeyframes { prop, keyframes, duration_ms, repeat_forever, autoreverse })
                .unwrap_or(false)
        }
        fn subscribe_layout(&self, node: &dyn Any, callback: Box<dyn Fn(f32, f32)>) -> LayoutSubscription {
            // The app holds the callback (and releases it with the
            // subscription); the bundle holds the subscription.
            let cb: Cb = crate::remote::bundle::register_call(Rc::new(move |args: &[u8]| {
                let (w, h): (f32, f32) =
                    from_bytes(args).unwrap_or_else(|e| panic!("remote codec: a layout size does not decode: {e}"));
                callback(w, h);
                Vec::new()
            }));
            let handle = id(node);
            match call::<u32>(node, HandleCall::SubscribeLayout { callback: cb }) {
                Some(sub) => LayoutSubscription::new(move || {
                    try_send(handle, &HandleCall::Unsubscribe(sub));
                }),
                None => LayoutSubscription::noop(),
            }
        }
    }

    impl PressableOps for RemoteOps {
        fn click(&self, node: &dyn Any) {
            send(node, HandleCall::Click)
        }
        fn rect(&self, node: &dyn Any) -> ViewportRect {
            call(node, HandleCall::Rect).unwrap_or_default()
        }
    }

    impl ButtonOps for RemoteOps {
        fn click(&self, node: &dyn Any) {
            send(node, HandleCall::Click)
        }
        fn rect(&self, node: &dyn Any) -> ViewportRect {
            call(node, HandleCall::Rect).unwrap_or_default()
        }
    }

    impl TextOps for RemoteOps {
        fn set_animated_color(&self, node: &dyn Any, prop: AnimProp, value: [f32; 4]) {
            send(node, HandleCall::SetAnimatedColor { prop, value })
        }
    }

    impl IconOps for RemoteOps {
        fn animate_stroke(&self, node: &dyn Any, from: f32, to: f32, duration_ms: u32, easing: Easing) {
            send(node, HandleCall::AnimateStroke { from, to, duration_ms, easing })
        }
        fn set_stroke_progress(&self, node: &dyn Any, progress: f32) {
            send(node, HandleCall::SetStrokeProgress(progress))
        }
    }

    impl LinkOps for RemoteOps {
        fn activate(&self, node: &dyn Any) {
            send(node, HandleCall::Activate)
        }
    }

    impl TextInputOps for RemoteOps {
        fn focus(&self, node: &dyn Any) {
            send(node, HandleCall::Focus)
        }
        fn blur(&self, node: &dyn Any) {
            send(node, HandleCall::Blur)
        }
        fn select_all(&self, node: &dyn Any) {
            send(node, HandleCall::SelectAll)
        }
        fn insert_text(&self, node: &dyn Any, text: &str) {
            send(node, HandleCall::InsertText(text.to_owned()))
        }
    }

    impl TextAreaOps for RemoteOps {
        fn focus(&self, node: &dyn Any) {
            send(node, HandleCall::Focus)
        }
        fn blur(&self, node: &dyn Any) {
            send(node, HandleCall::Blur)
        }
        fn select_all(&self, node: &dyn Any) {
            send(node, HandleCall::SelectAll)
        }
        fn insert_text(&self, node: &dyn Any, text: &str) {
            send(node, HandleCall::InsertText(text.to_owned()))
        }
    }

    impl ScrollViewOps for RemoteOps {
        fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
            send(node, HandleCall::ScrollTo { x, y })
        }
    }

    impl VirtualizerOps for RemoteOps {
        fn scroll_to_index(&self, node: &dyn Any, index: usize) {
            send(node, HandleCall::ScrollToIndex(index))
        }
        fn scroll_offset(&self, node: &dyn Any) -> (f32, f32) {
            call(node, HandleCall::ScrollOffset).unwrap_or_default()
        }
        fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
            send(node, HandleCall::ScrollTo { x, y })
        }
    }

    impl VirtualGridOps for RemoteOps {
        fn scroll_to_cell(&self, node: &dyn Any, col: usize, row: usize) {
            send(node, HandleCall::ScrollToCell { col, row })
        }
        fn scroll_offset(&self, node: &dyn Any) -> (f32, f32) {
            call(node, HandleCall::ScrollOffset).unwrap_or_default()
        }
        fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
            send(node, HandleCall::ScrollTo { x, y })
        }
    }

    impl ImageOps for RemoteOps {}
    impl ToggleOps for RemoteOps {}
    impl SliderOps for RemoteOps {}
    impl ActivityIndicatorOps for RemoteOps {}
    impl PortalOps for RemoteOps {}
    impl PresenceOps for RemoteOps {}

    thread_local! {
        /// The navigator handles this bundle received from the app, by
        /// identity — so handing one back (to an app component) sends the
        /// app's id, not a new proxy.
        static NAV_IDS: RefCell<std::collections::HashMap<*const (), u32>> = RefCell::new(Default::default());
    }

    /// The bundle's `NavHandle` for the app's navigator handle `id`: its
    /// commands cross as [`WireNav`]. Dropping its last copy releases the
    /// app's entry.
    pub fn nav_proxy(id: u32) -> crate::prims::NavHandle {
        use runtime_shared::primitives::navigator::NavCommand;
        let node = Rc::new(RemoteNode(id));
        let handle = crate::prims::NavHandle::new(Rc::new(move |cmd: NavCommand| {
            let wire = match cmd {
                NavCommand::Push { name, url, query, .. } => {
                    super::WireNav::Push { name: name.into(), url, query: query.to_query_string() }
                }
                NavCommand::Replace { name, url, query, .. } => {
                    super::WireNav::Replace { name: name.into(), url, query: query.to_query_string() }
                }
                NavCommand::Reset { name, url, query, .. } => {
                    super::WireNav::Reset { name: name.into(), url, query: query.to_query_string() }
                }
                NavCommand::Select { name, url, query, .. } => {
                    super::WireNav::Select { name: name.into(), url, query: query.to_query_string() }
                }
                NavCommand::Pop => super::WireNav::Pop,
                _ => panic!("remote component: a custom navigation command can't cross to the app"),
            };
            try_send(node.0, &HandleCall::Nav(wire));
        }));
        NAV_IDS.with(|m| m.borrow_mut().insert(handle.identity(), id));
        handle
    }

    /// A bundle closure calling app closure `id` (`Held::Call`) with
    /// encoded arguments. Dropping its last copy releases the app's entry.
    pub fn app_call(id: u32) -> Rc<dyn Fn(&[u8]) -> Vec<u8>> {
        let node = Rc::new(RemoteNode(id));
        Rc::new(move |args: &[u8]| try_send(node.0, &HandleCall::Invoke(args.to_vec())).unwrap_or_default())
    }

    /// The app's id for `handle`, if it is one this bundle received.
    pub fn nav_id(handle: &crate::prims::NavHandle) -> Option<u32> {
        NAV_IDS.with(|m| m.borrow().get(&handle.identity()).copied())
    }

    /// A prim's `ref_fill`, as it crosses: a one-shot callback the app calls
    /// with the id of the real handle it now holds. `make` builds the
    /// bundle's handle (`|n| TextInputHandle::new(n, &REMOTE_OPS)`).
    pub fn fill<H: 'static>(ref_fill: Option<Box<dyn FnOnce(H)>>, make: fn(Rc<dyn Any>) -> H) -> Option<Cb> {
        let once = RefCell::new(Some(ref_fill?));
        Some(crate::remote::bundle::register_call(Rc::new(move |args: &[u8]| {
            let id: u32 = from_bytes(args).unwrap_or_else(|e| panic!("remote codec: a handle id does not decode: {e}"));
            if let Some(f) = once.borrow_mut().take() {
                f(make(Rc::new(RemoteNode(id))));
            }
            Vec::new()
        })))
    }
}

// ---------------------------------------------------------------------------
// App side
// ---------------------------------------------------------------------------

#[cfg(not(idealyst_stream_guest))]
mod host_side {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::{Rc, Weak};

    use runtime_shared::handles::*;
    use runtime_shared::primitives::activity_indicator::ActivityIndicatorHandle;
    use runtime_shared::primitives::icon::IconHandle;
    use runtime_shared::primitives::image::ImageHandle;
    use runtime_shared::primitives::link::LinkHandle;
    use runtime_shared::primitives::portal::{AnchorableHandle, PortalHandle};
    use runtime_shared::primitives::presence::PresenceHandle;
    use runtime_shared::primitives::scroll_view::ScrollViewHandle;
    use runtime_shared::primitives::slider::SliderHandle;
    use runtime_shared::primitives::text_area::TextAreaHandle;
    use runtime_shared::primitives::text_input::TextInputHandle;
    use runtime_shared::primitives::toggle::ToggleHandle;
    use runtime_shared::primitives::virtual_grid::VirtualGridHandle;
    use runtime_shared::primitives::virtualizer::VirtualizerHandle;

    use super::HandleCall;
    use crate::remote::{from_bytes, to_bytes};

    /// A real handle the app holds for a bundle.
    #[derive(Clone)]
    pub enum Held {
        View(ViewHandle),
        Pressable(PressableHandle),
        Text(TextHandle),
        Button(ButtonHandle),
        Image(ImageHandle),
        Icon(IconHandle),
        Link(LinkHandle),
        Toggle(ToggleHandle),
        Slider(SliderHandle),
        ActivityIndicator(ActivityIndicatorHandle),
        TextInput(TextInputHandle),
        TextArea(TextAreaHandle),
        ScrollView(ScrollViewHandle),
        Portal(PortalHandle),
        Presence(PresenceHandle),
        Virtualizer(VirtualizerHandle),
        VirtualGrid(VirtualGridHandle),
        Nav(crate::prims::NavHandle),
        /// An app `Ref<H>` of a navigator handle, read when the bundle
        /// uses it (as native code reads its `Ref` at call time — a screen
        /// is built before its navigator fills the ref). `original` is the
        /// `Ref<H>` itself, so handing it back to an app component returns
        /// the same ref.
        NavRef { get: Rc<dyn Fn() -> Option<crate::prims::NavHandle>>, original: Rc<dyn std::any::Any> },
        /// An app closure a bundle may call (a navigator's `pop`).
        Call(Rc<dyn Fn(&[u8]) -> Result<Vec<u8>, String>>),
        /// An app `Ref` to a node handle, filled by an app component the
        /// bundle handed it to (`bind_to`); read when the bundle uses it.
        NodeRef(Rc<dyn Fn() -> Option<Held>>),
    }

    struct Entry {
        held: Held,
        /// The decoded tree's connection: the entry dies with it. `None`
        /// for a handle the app handed to a bundle (a prop): that entry
        /// lives as long as its [`HoldGuard`].
        tree: Option<Weak<dyn std::any::Any>>,
        /// Layout subscriptions this handle made, by id.
        subs: HashMap<u32, LayoutSubscription>,
    }

    thread_local! {
        static HANDLES: RefCell<HashMap<u32, Entry>> = RefCell::new(HashMap::new());
        static NEXT: Cell<u32> = const { Cell::new(0) };
    }

    /// Hold `held` for the bundle whose decoded tree is `tree`; its id.
    pub(crate) fn hold(held: Held, tree: Weak<dyn std::any::Any>) -> u32 {
        insert(held, Some(tree))
    }

    fn insert(held: Held, tree: Option<Weak<dyn std::any::Any>>) -> u32 {
        let id = NEXT.with(|n| {
            n.set(n.get().checked_add(1).expect("remote codec: handle ids exhausted"));
            n.get()
        });
        HANDLES.with(|h| h.borrow_mut().insert(id, Entry { held, tree, subs: HashMap::new() }));
        id
    }

    /// Hold `held` for a bundle until the guard drops — a handle the app
    /// passes as a prop (the mount keeps the guard).
    pub fn hold_scoped(held: Held) -> (u32, HoldGuard) {
        let id = insert(held, None);
        (id, HoldGuard(id))
    }

    /// Ends a [`hold_scoped`] entry.
    pub struct HoldGuard(u32);

    impl Drop for HoldGuard {
        fn drop(&mut self) {
            let gone = HANDLES.try_with(|h| h.borrow_mut().remove(&self.0)).ok().flatten();
            drop(gone);
        }
    }

    /// The navigator handle the app holds under `id` (for a `Ref`, what it
    /// holds now).
    pub fn held_nav(id: u32) -> Option<crate::prims::NavHandle> {
        let held = HANDLES.with(|h| h.borrow().get(&id).map(|e| e.held.clone()))?;
        match held {
            Held::Nav(n) => Some(n),
            Held::NavRef { get, .. } => get(),
            _ => None,
        }
    }

    /// The app `Ref` held under `id`, if that entry is one.
    pub fn held_nav_ref(id: u32) -> Option<Rc<dyn std::any::Any>> {
        HANDLES.with(|h| match h.borrow().get(&id).map(|e| &e.held) {
            Some(Held::NavRef { original, .. }) => Some(original.clone()),
            _ => None,
        })
    }

    /// Drop the entries `tree` made (it unmounted).
    pub(crate) fn purge_tree(tree: &Weak<dyn std::any::Any>) {
        let gone: Vec<Entry> = HANDLES
            .try_with(|h| {
                let mut h = h.borrow_mut();
                let ids: Vec<u32> = h
                    .iter()
                    .filter(|(_, e)| e.tree.as_ref().is_some_and(|t| Weak::ptr_eq(t, tree)))
                    .map(|(id, _)| *id)
                    .collect();
                ids.into_iter().filter_map(|id| h.remove(&id)).collect()
            })
            .unwrap_or_default();
        drop(gone);
    }

    /// Drop the entries whose tree is gone (`host::Conn`'s drop).
    pub(crate) fn purge_dead() {
        let dead: Vec<Entry> = HANDLES
            .try_with(|h| {
                let mut h = h.borrow_mut();
                let ids: Vec<u32> = h
                    .iter()
                    .filter(|(_, e)| e.tree.as_ref().is_some_and(|t| t.strong_count() == 0))
                    .map(|(id, _)| *id)
                    .collect();
                ids.into_iter().filter_map(|id| h.remove(&id)).collect()
            })
            .unwrap_or_default();
        // Dropped outside the borrow: a handle's drop may run backend code.
        drop(dead);
    }

    /// Handles the app currently holds for bundles (for leak checks).
    pub fn held_handles() -> usize {
        HANDLES.with(|h| h.borrow().len())
    }

    /// A bundle's handle call, in one process. `None` during thread
    /// teardown; a malformed call panics (there is no bundle to stop).
    pub fn try_handle_call(id: u32, args: &[u8]) -> Option<Vec<u8>> {
        HANDLES.try_with(|_| ()).ok()?;
        Some(handle_call(id, args).unwrap_or_else(|e| panic!("{e}")))
    }

    /// Run a bundle's handle call against the real handle `id`; the encoded
    /// reply. A call on a handle the app no longer holds (its tree was torn
    /// down) is a no-op answering the method's default. `Err` for a call
    /// that does not decode, or a method the handle doesn't have: the
    /// bundle and the app disagree, and the bundle is stopped.
    pub fn handle_call(id: u32, args: &[u8]) -> Result<Vec<u8>, String> {
        let call: HandleCall =
            from_bytes(args).map_err(|e| format!("remote codec: a handle call does not decode: {e}"))?;
        match call {
            HandleCall::Release => {
                let gone = HANDLES.with(|h| h.borrow_mut().remove(&id));
                drop(gone);
                return Ok(Vec::new());
            }
            HandleCall::Unsubscribe(sub) => {
                let gone = HANDLES.with(|h| h.borrow_mut().get_mut(&id).and_then(|e| e.subs.remove(&sub)));
                drop(gone);
                return Ok(Vec::new());
            }
            _ => {}
        }
        // Cloned out: a handle method may re-enter (and drop entries).
        let held = HANDLES.with(|h| h.borrow().get(&id).map(|e| e.held.clone()));
        let Some(mut held) = held else { return Ok(default_reply(&call)) };
        // A ref an app component fills: what it holds now, if anything.
        if let Held::NodeRef(get) = &held {
            match get() {
                Some(now) => held = now,
                None => return Ok(default_reply(&call)),
            }
        }
        use Held as H;
        use HandleCall as C;
        Ok(match (held, call) {
            (H::View(h), C::Rect) => to_bytes(&h.rect()),
            (H::Pressable(h), C::Rect) => to_bytes(&h.rect()),
            (H::Button(h), C::Rect) => to_bytes(&h.rect()),
            (H::View(h), C::Frame) => to_bytes(&h.frame()),
            (H::View(h), C::AbsoluteFrame) => to_bytes(&h.absolute_frame()),
            (H::View(h), C::SetAnimatedF32 { prop, value }) => unit(h.set_animated_f32(prop, value)),
            (H::View(h), C::SetAnimatedColor { prop, value }) => unit(h.set_animated_color(prop, value)),
            (H::Text(h), C::SetAnimatedColor { prop, value }) => unit(h.set_animated_color(prop, value)),
            (H::View(h), C::InstallKeyframes { prop, keyframes, duration_ms, repeat_forever, autoreverse }) => {
                to_bytes(&h.install_keyframe_animation(prop, &keyframes, duration_ms, repeat_forever, autoreverse))
            }
            (H::View(h), C::SubscribeLayout { callback }) => {
                let tree = HANDLES.with(|t| t.borrow().get(&id).and_then(|e| e.tree.clone()));
                let Some(conn) = tree.and_then(|t| t.upgrade()) else { return Ok(to_bytes(&0u32)) };
                let cb = crate::remote::host::callback_for(&conn, callback);
                let sub = h.on_layout(move |w, hh| {
                    cb.call(&to_bytes(&(w, hh)));
                });
                let sub_id = NEXT.with(|n| {
                    n.set(n.get() + 1);
                    n.get()
                });
                let rejected = HANDLES.with(|t| match t.borrow_mut().get_mut(&id) {
                    Some(e) => {
                        e.subs.insert(sub_id, sub);
                        None
                    }
                    None => Some(sub),
                });
                drop(rejected);
                to_bytes(&sub_id)
            }
            (H::Pressable(h), C::Click) => unit(h.click()),
            (H::Button(h), C::Click) => unit(h.click()),
            (H::Link(h), C::Activate) => unit(h.activate()),
            (H::Icon(h), C::AnimateStroke { from, to, duration_ms, easing }) => {
                unit(h.animate_stroke(from, to, duration_ms, easing))
            }
            (H::Icon(h), C::SetStrokeProgress(p)) => unit(h.set_stroke_progress(p)),
            (H::TextInput(h), C::Focus) => unit(h.focus()),
            (H::TextInput(h), C::Blur) => unit(h.blur()),
            (H::TextInput(h), C::SelectAll) => unit(h.select_all()),
            (H::TextInput(h), C::InsertText(t)) => unit(h.insert_text(&t)),
            (H::TextArea(h), C::Focus) => unit(h.focus()),
            (H::TextArea(h), C::Blur) => unit(h.blur()),
            (H::TextArea(h), C::SelectAll) => unit(h.select_all()),
            (H::TextArea(h), C::InsertText(t)) => unit(h.insert_text(&t)),
            (H::ScrollView(h), C::ScrollTo { x, y }) => unit(h.scroll_to(x, y)),
            (H::Virtualizer(h), C::ScrollTo { x, y }) => unit(h.scroll_to(x, y)),
            (H::VirtualGrid(h), C::ScrollTo { x, y }) => unit(h.scroll_to(x, y)),
            (H::Virtualizer(h), C::ScrollToIndex(i)) => unit(h.scroll_to_index(i)),
            (H::VirtualGrid(h), C::ScrollToCell { col, row }) => unit(h.scroll_to_cell(col, row)),
            (H::Virtualizer(h), C::ScrollOffset) => to_bytes(&h.scroll_offset()),
            (H::VirtualGrid(h), C::ScrollOffset) => to_bytes(&h.scroll_offset()),
            (H::Nav(nav), C::Nav(cmd)) => unit(nav.dispatch(nav_command(cmd))),
            (H::Call(f), C::Invoke(args)) => f(&args)?,
            // An unfilled ref drops the command, as native code's
            // `if let Some(h) = nav.get()` would.
            (H::NavRef { get, .. }, C::Nav(cmd)) => unit(if let Some(nav) = get() {
                nav.dispatch(nav_command(cmd))
            }),
            (_, call) => {
                return Err(format!("remote codec: handle {id} has no method for {call:?} — the bundle and the app disagree"))
            }
        })
    }

    /// A remote component's navigation command: its params are rebuilt
    /// from the url by the receiving navigator.
    fn nav_command(w: super::WireNav) -> runtime_shared::primitives::navigator::NavCommand {
        use runtime_shared::primitives::navigator::{NavCommand, QueryParams};
        use super::WireNav as W;
        let params = || Box::new(crate::prims::ParamsFromUrl) as Box<dyn std::any::Any>;
        let name = |n: &str| crate::remote::host::intern_name(n);
        match w {
            W::Push { name: n, url, query } => {
                NavCommand::Push { name: name(&n), url, params: params(), query: QueryParams::parse(&query) }
            }
            W::Replace { name: n, url, query } => {
                NavCommand::Replace { name: name(&n), url, params: params(), query: QueryParams::parse(&query) }
            }
            W::Reset { name: n, url, query } => {
                NavCommand::Reset { name: name(&n), url, params: params(), query: QueryParams::parse(&query) }
            }
            W::Select { name: n, url, query } => {
                NavCommand::Select { name: name(&n), url, params: params(), query: QueryParams::parse(&query) }
            }
            W::Pop => NavCommand::Pop,
        }
    }

    fn unit(_: ()) -> Vec<u8> {
        Vec::new()
    }

    /// What a call on a handle that is gone answers.
    fn default_reply(call: &HandleCall) -> Vec<u8> {
        use runtime_shared::primitives::portal::ViewportRect;
        match call {
            HandleCall::Rect => to_bytes(&ViewportRect::default()),
            HandleCall::Frame | HandleCall::AbsoluteFrame => to_bytes(&None::<ViewportRect>),
            HandleCall::InstallKeyframes { .. } => to_bytes(&false),
            HandleCall::SubscribeLayout { .. } => to_bytes(&0u32),
            HandleCall::ScrollOffset => to_bytes(&(0.0f32, 0.0f32)),
            _ => Vec::new(),
        }
    }
}
