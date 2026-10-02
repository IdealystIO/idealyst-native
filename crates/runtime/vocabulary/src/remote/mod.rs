//! The remote element codec: how a remote component's `Element` tree crosses
//! from its wasm bundle to the host app (crates/streaming).
//!
//! A remote bundle runs the real framework code — `#[component]`, `ui!`,
//! builders — against the bridged kernel, so its signals and effects already
//! live in the host's graph (runtime-world `bridge`). What it cannot hand
//! over directly is the `Element` it builds: payloads are `Box<dyn Any>` and
//! full of closures. So:
//!
//! - **[`bundle`]** (in the bundle) walks the tree into a [`Node`] — plain
//!   data. Every closure (a `Value::Dyn` getter, an `on_press`, a `Dyn`
//!   hole's builder, a keyed list's items and render, a stylesheet) is kept
//!   in a bundle-side table under a callback id, and the id crosses instead.
//!   A component's `Owned` crosses as the host scope id it already is.
//! - **[`host`]** (in the app) turns a [`Node`] back into an `Element` of
//!   real prims whose closures call the bundle by id, through a [`host::Link`].
//!   When the host drops such a closure its id is released, and the bundle
//!   frees the closure — the same lifetime rule the kernel's proxies follow.
//!
//! The decoded tree is then realized by the host's ordinary registry and
//! backend. Nothing on the host knows the subtree came from a bundle.
//!
//! # Plain data, and every primitive decided
//!
//! [`crossing`] names every builtin payload and says whether it crosses.
//! The `remote_elements` test fails when `register_builtins` gains a
//! payload this table has no entry for, so adding a primitive forces the
//! decision. A payload that does not cross panics at encode, naming itself
//! — never silently dropped. The same goes for a field of a crossing
//! payload that the codec does not carry yet (an `on_touch`, a `ref`).
//!
//! # App components are imported, not bundled
//!
//! A bundle uses the app's own components (idea-ui's `Card`, the app's
//! `Avatar`) by NAME: [`bundle::import`] emits a [`Node::Import`], and the
//! host looks the name up among the components it registered with
//! [`host::register_import`]. A bundle that needs a component the app does
//! not export fails to decode with [`host::DecodeError::MissingImport`] — the
//! "old binary, new bundle" case, reported instead of half-rendered.
//!
//! # Styles keep the app's theme
//!
//! Style rules cross with token NAMES intact (`runtime-shared`'s
//! `remote-serde`), so the app's theme resolves them. A stylesheet crosses
//! as its shape ([`runtime_shared::SheetShape`]); the host rebuilds the same
//! sheet with every closure proxied back to the bundle, and the host's style
//! engine (state, breakpoint and container overlays, the variant cache)
//! treats it as it treats any sheet.

use serde::{Deserialize, Serialize};

use runtime_shared::{SheetShape, StyleRules, VariantSet};

#[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
pub mod bundle;
pub mod handles;
#[cfg(not(idealyst_stream_guest))]
pub mod host;
/// The bundle half's wasm exports.
#[cfg(idealyst_stream_guest)]
pub mod wasm;

/// A bundle-side callback id. Local to one bundle: a host serving several
/// bundles holds one [`host::Link`] per bundle.
pub type Cb = u32;

/// One node of a remote tree.
#[derive(Serialize, Deserialize, Debug)]
pub enum Node {
    View {
        common: Common,
        safe_area: u8,
        preserves_focus: bool,
        is_container: bool,
        /// Event handlers, each a callback taking the encoded event and
        /// replying the encoded response (see [`Handler`]).
        on_touch: Option<Cb>,
        on_wheel: Option<Cb>,
        on_hover: Option<Cb>,
        /// Takes a [`WireFileDrop`].
        on_file_drop: Option<Cb>,
        children: Vec<Node>,
    },
    Pressable {
        common: Common,
        on_press: Cb,
        disabled: Option<Val<bool>>,
        preserves_focus: bool,
        children: Vec<Node>,
    },
    Text {
        common: Common,
        content: TextContent,
    },
    Button {
        common: Common,
        label: Val<String>,
        on_press: WireAction,
        leading_icon: Option<WireIcon>,
        trailing_icon: Option<WireIcon>,
        disabled: Option<Val<bool>>,
    },
    Image {
        common: Common,
        src: Val<String>,
        alt: Val<Option<String>>,
        /// Takes an `ImageLoadEvent`.
        on_load: Option<Cb>,
        on_error: Option<Cb>,
        asset: Option<WireAsset>,
    },
    Icon {
        common: Common,
        data: Val<WireIcon>,
        color: Option<Val<runtime_shared::Color>>,
        stroke: Option<Val<f32>>,
        draw_in: Option<runtime_shared::primitives::icon::StrokeAnimation>,
    },
    Link {
        common: Common,
        url: Val<String>,
        route: Option<String>,
        external: bool,
        on_activate: Option<Cb>,
        children: Vec<Node>,
    },
    Toggle {
        common: Common,
        value: Val<bool>,
        /// Takes a `bool`.
        on_change: Cb,
    },
    Slider {
        common: Common,
        value: Val<f32>,
        /// Takes an `f32`.
        on_change: Cb,
        min: f32,
        max: f32,
        step: Option<f32>,
    },
    ActivityIndicator {
        common: Common,
        size: Val<runtime_shared::primitives::activity_indicator::ActivityIndicatorSize>,
        color: Option<runtime_shared::Color>,
    },
    TextInput {
        common: Common,
        value: Val<String>,
        /// Takes a `String`.
        on_change: Cb,
        /// Takes a `KeyEvent`, replies a `KeyOutcome`.
        on_key_down: Option<Cb>,
        /// Replies a `BlurOutcome`.
        on_blur: Option<Cb>,
        /// Takes a `bool`.
        on_focus: Option<Cb>,
        placeholder: Val<Option<String>>,
        secure: Val<bool>,
    },
    TextArea {
        common: Common,
        value: Val<String>,
        on_change: Cb,
        on_key_down: Option<Cb>,
        placeholder: Option<String>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
    },
    ScrollView {
        common: Common,
        horizontal: bool,
        /// Takes `(f32, f32)`.
        on_scroll: Option<Cb>,
        on_end_reached: Option<Cb>,
        end_reached_threshold: f32,
        safe_area: Option<u8>,
        bounces: Option<bool>,
        always_bounce: Option<bool>,
        children: Vec<Node>,
    },
    /// A static `for` lowering: `row` takes a `usize`, replies a [`Node`].
    Repeat { count: usize, row: Cb },
    Presence {
        test_id: Option<String>,
        a11y: Option<Box<A11y>>,
        fill: Option<Cb>,
        /// Replies a [`Node`].
        child: Cb,
        /// Replies a `bool`.
        present: Cb,
        enter: Option<runtime_shared::primitives::presence::PresenceAnim>,
        exit: Option<runtime_shared::primitives::presence::PresenceAnim>,
    },
    Portal {
        target: WirePortalTarget,
        fill: Option<Cb>,
        on_dismiss: Option<Cb>,
        trap_focus: bool,
        style: Option<Box<Style>>,
        a11y: Option<Box<A11y>>,
        children: Vec<Node>,
    },
    Virtualizer {
        common: Common,
        /// Replies a `usize`.
        item_count: Cb,
        /// Takes a `usize`, replies a `u64`.
        item_key: Cb,
        /// Whether sizes are measured (`ItemSize::Measured`) or known.
        measured: bool,
        /// Takes a `usize`, replies an `f32`.
        item_size: Cb,
        /// Takes a `usize`, replies a [`Node`].
        render_item: Cb,
        /// `(capture, differs)`: `capture` takes a `usize` and replies an
        /// `Option<Cb>` (a snapshot held bundle-side, released by the host
        /// when it drops it); `differs` takes `(snapshot, usize)`, replies a
        /// `bool`.
        item_diff: Option<(Cb, Cb)>,
        overscan: f32,
        layout: runtime_shared::primitives::virtualizer::VirtualLayout,
        on_scroll: Option<Cb>,
        on_end_reached: Option<Cb>,
        end_reached_threshold: f32,
        safe_area: Option<u8>,
    },
    VirtualGrid {
        common: Common,
        /// Each replies a `usize`.
        col_count: Cb,
        row_count: Cb,
        /// Each takes a `usize`, replies an `f32`.
        col_width: Cb,
        row_height: Cb,
        /// Takes `(usize, usize)`, replies a `u64`.
        cell_key: Cb,
        /// Takes `(usize, usize)`, replies a [`Node`].
        render_cell: Cb,
        overscan: f32,
        on_scroll: Option<Cb>,
    },
    Fragment(Vec<Node>),
    /// `dyn_element`: rebuild on every fire. `build` replies a [`Node`].
    Dyn { build: Cb },
    /// `dyn_keyed` / `dyn_guarded`: `changed` replies a `bool`, `build` a
    /// [`Node`].
    Guarded { changed: Cb, build: Cb },
    /// `keyed`: `items` replies `Vec<(WireKey, Cb)>` (each row's item held
    /// bundle-side); `render` takes an item's id and replies a [`Node`].
    Keyed { items: Cb, render: Cb },
    /// A component boundary: the scope id is the host's (see
    /// `runtime_world::remote::claim_scope`).
    Owned { scope: u32, element: Box<Node> },
    /// An app component, by name; `props` are its serialized props.
    Import { name: String, props: Vec<u8>, children: Vec<Node> },
}

/// A `text`'s content.
#[derive(Serialize, Deserialize, Debug)]
pub enum TextContent {
    Value(Val<String>),
    /// Styled runs; a getter replies `Vec<TextRun>`.
    Runs(Vec<runtime_shared::styled_text::TextRun>),
}

/// An `IconData`. Its paths are `&'static` on the prim, so the host interns
/// each distinct icon (see `host::intern_icon`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WireIcon {
    pub view_box: (u16, u16),
    pub paths: Vec<String>,
    pub fill_rule: runtime_shared::primitives::icon::FillRule,
    pub filled: bool,
}

impl From<runtime_shared::primitives::icon::IconData> for WireIcon {
    fn from(i: runtime_shared::primitives::icon::IconData) -> Self {
        WireIcon {
            view_box: i.view_box,
            paths: i.paths.iter().map(|p| p.to_string()).collect(),
            fill_rule: i.fill_rule,
            filled: i.filled,
        }
    }
}

/// An image `Asset`: its id and where its bytes are. Embedded bytes cross
/// with it (a bundle's own asset); a bundled path names a file the APP
/// ships.
#[derive(Serialize, Deserialize, Debug)]
pub struct WireAsset {
    pub id: u64,
    pub source: WireAssetSource,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WireAssetSource {
    Embedded { bytes: Vec<u8>, extension: String },
    Bundled { path: String },
    BundledEmbedded { path: String, bytes: Vec<u8>, extension: String },
    Remote { url: String },
}

/// A `FileDropEvent` without the web-only opaque `source` (a bundle runs on
/// native targets, where files carry a path).
#[derive(Serialize, Deserialize, Debug)]
pub struct WireFileDrop {
    pub phase: WireDropPhase,
    pub position: runtime_shared::touch::TouchPoint,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WireDropPhase {
    Entered,
    Exited,
    Dropped(Vec<WireDroppedFile>),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WireDroppedFile {
    pub name: String,
    pub mime: String,
    pub size: Option<u64>,
    pub path: Option<std::path::PathBuf>,
}

/// A portal's target.
#[derive(Serialize, Deserialize, Debug)]
pub enum WirePortalTarget {
    Viewport(runtime_shared::primitives::portal::ViewportPlacement),
    Named(String),
    /// An anchor to a node the bundle holds a `ref` to. `rect` replies an
    /// `Option<ViewportRect>` — the bundle's `AnchorTarget::rect`, which
    /// asks the app's real handle (see [`handles`]).
    Anchor {
        rect: Cb,
        side: runtime_shared::primitives::portal::ElementSide,
        align: runtime_shared::primitives::portal::ElementAlign,
        offset: f32,
    },
}

/// What every crossing primitive carries.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Common {
    pub test_id: Option<String>,
    /// Boxed: a `Style` carries `StyleRules` inline (kilobytes), and the
    /// encoder recurses on a bundle's small stack — see
    /// `regression_a_node_stays_small_enough_for_a_bundles_stack`.
    pub style: Option<Box<Style>>,
    pub a11y: Option<Box<A11y>>,
    /// The prim's `ref_fill`: called once with the app's id for the real
    /// handle (see [`handles`]).
    pub fill: Option<Cb>,
}

impl Common {
    pub fn with_fill(mut self, fill: Option<Cb>) -> Self {
        self.fill = fill;
        self
    }
}

/// A `Value<T>`: a constant, or a getter the host's binding effect calls
/// (its reads subscribe that effect — the bundle's signals are the host's).
#[derive(Serialize, Deserialize, Debug)]
pub enum Val<T> {
    Const(T),
    Dyn(Cb),
}

/// A `StyleProp`.
#[derive(Serialize, Deserialize, Debug)]
pub enum Style {
    Rules(StyleRules),
    /// Replies `StyleRules`.
    Dynamic(Cb),
    Sheet(App),
    /// Replies an [`App`].
    SheetDynamic(Cb),
}

/// A `StyleApplication`.
#[derive(Serialize, Deserialize, Debug)]
pub struct App {
    pub sheet: SheetRef,
    pub variants: VariantSet,
    pub overrides: Option<StyleRules>,
    pub inline: Option<StyleRules>,
    /// `(cache key, compute)`; `compute` replies `StyleRules`.
    pub computed: Option<(String, Cb)>,
}

/// A stylesheet: its shape, and the callback that evaluates one of its
/// parts — takes `(SheetPart, VariantSet)`, replies `StyleRules`. The same
/// sheet always crosses under the same id while the host holds it, so the
/// host builds one proxy per bundle sheet and its variant cache is shared
/// by every node using it.
#[derive(Serialize, Deserialize, Debug)]
pub struct SheetRef {
    pub id: Cb,
    pub shape: SheetShape,
}

/// A button's `Action`. `fire` is the runtime evaluator; the rest is the
/// structured metadata generator backends read (`initial` as JSON text —
/// it is `serde_json::Value`, which a compact format cannot carry as is).
#[derive(Serialize, Deserialize, Debug)]
pub struct WireAction {
    pub fire: Cb,
    pub method: String,
    pub inputs: Vec<u64>,
    pub initial: String,
    pub output: Option<u64>,
}

/// `AccessibilityProps`, when not default.
#[derive(Serialize, Deserialize, Debug)]
pub struct A11y {
    pub label: Option<String>,
    pub hint: Option<String>,
    pub role: Option<runtime_shared::accessibility::Role>,
    pub traits: u16,
    pub hidden: bool,
    pub live_region: Option<runtime_shared::accessibility::LiveRegionPriority>,
    /// `(name, handler)`.
    pub actions: Vec<(String, Cb)>,
    pub identifier: Option<String>,
}

/// A keyed row's `Key`.
#[derive(Serialize, Deserialize, Debug)]
pub enum WireKey {
    Int(i64),
    UInt(u64),
    Str(String),
}

impl From<runtime_scene::Key> for WireKey {
    fn from(k: runtime_scene::Key) -> Self {
        match k {
            runtime_scene::Key::Int(v) => WireKey::Int(v),
            runtime_scene::Key::UInt(v) => WireKey::UInt(v),
            runtime_scene::Key::Str(v) => WireKey::Str(v),
        }
    }
}

impl From<WireKey> for runtime_scene::Key {
    fn from(k: WireKey) -> Self {
        match k {
            WireKey::Int(v) => runtime_scene::Key::Int(v),
            WireKey::UInt(v) => runtime_scene::Key::UInt(v),
            WireKey::Str(v) => runtime_scene::Key::Str(v),
        }
    }
}

/// Encode a wire value. The codec's only format.
pub fn to_bytes<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    postcard::to_allocvec(v).unwrap_or_else(|e| panic!("remote codec: encode failed: {e}"))
}

/// Decode a wire value.
pub fn from_bytes<'a, T: Deserialize<'a>>(b: &'a [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(b)
}

/// Whether a builtin payload crosses from a bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crossing {
    /// The codec carries it.
    Supported(&'static str),
    /// It panics at encode, with this reason.
    Unsupported(&'static str, &'static str),
}

impl Crossing {
    pub fn name(self) -> &'static str {
        match self {
            Crossing::Supported(n) | Crossing::Unsupported(n, _) => n,
        }
    }
}

/// The codec's decision for builtin payload `ty` (a `PrimCell<…Prim>`
/// type id); `None` for a payload it has never heard of — which, for a
/// builtin, is the completeness test failing.
pub fn crossing(ty: std::any::TypeId) -> Option<Crossing> {
    use crate::prims::*;
    use std::any::TypeId;
    const LATER: &str = "not carried by the remote codec yet";
    macro_rules! table {
        ($($prim:ty => $c:expr),* $(,)?) => {
            $(if ty == TypeId::of::<PrimCell<$prim>>() { return Some($c); })*
        };
    }
    table! {
        ViewPrim => Crossing::Supported("view"),
        PressablePrim => Crossing::Supported("pressable"),
        TextPrim => Crossing::Supported("text"),
        ButtonPrim => Crossing::Supported("button"),
        ImagePrim => Crossing::Supported("image"),
        IconPrim => Crossing::Supported("icon"),
        LinkPrim => Crossing::Supported("link"),
        TogglePrim => Crossing::Supported("toggle"),
        SliderPrim => Crossing::Supported("slider"),
        ActivityIndicatorPrim => Crossing::Supported("activity_indicator"),
        TextInputPrim => Crossing::Supported("text_input"),
        TextAreaPrim => Crossing::Supported("text_area"),
        ScrollViewPrim => Crossing::Supported("scroll_view"),
        RepeatPrim => Crossing::Supported("repeat (static `for` lowering)"),
        LazyPrim => Crossing::Unsupported(
            "lazy",
            "web code splitting has no meaning inside a bundle — `remote` and `lazy` are per-target alternatives",
        ),
        VirtualizerPrim => Crossing::Supported("virtualizer"),
        VirtualGridPrim => Crossing::Supported("virtual_grid"),
        GraphicsPrim => Crossing::Unsupported(
            "graphics",
            "it hands the author's code a native GPU surface, which a bundle — interpreted wasm — can't drive; \
             draw in an app component and use it from the remote component",
        ),
        PortalPrim => Crossing::Supported("portal"),
        PresencePrim => Crossing::Supported("presence"),
        StackNavigatorPrim => Crossing::Unsupported("stack navigator", LATER),
        SwapNavigatorPrim => Crossing::Unsupported("swap navigator", LATER),
        NavigatorOutletPrim => Crossing::Unsupported("navigator outlet", LATER),
    }
    None
}

// ---------------------------------------------------------------------------
// Props of a `#[component(remote)]`
// ---------------------------------------------------------------------------

/// How a `#[component(remote)]` prop crosses from the app to the bundle.
/// The macro calls `send` in the app's stub and `receive` in the bundle's
/// mount export, in parameter order.
///
/// - `ReadSignal<T>` / `Signal<T>` cross as HANDLES: the app exports its
///   signal (the export lives as long as the mounted component) and the
///   bundle imports it — reads subscribe in the app's graph, and a
///   `Signal`'s writes land in the app's signal.
/// - Plain values (`String`, numbers, `bool`, `Vec`/`Option` of them) cross
///   as a copy, fixed at mount.
///
/// Implement it for your own value types with [`remote_value!`].
pub trait RemoteProp: Sized + 'static {
    /// Encode `self` for the bundle; anything that must live as long as the
    /// mount (an export guard) goes in `keep`.
    #[cfg(not(idealyst_stream_guest))]
    fn send(&self, out: &mut Vec<u8>, keep: &mut host::Keep);
    /// Decode one prop from the front of `input`.
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn receive(input: &mut &[u8]) -> Self;
}

#[doc(hidden)]
pub fn __send_value<T: Serialize>(v: &T, out: &mut Vec<u8>) {
    out.extend_from_slice(&to_bytes(v));
}

#[doc(hidden)]
pub fn __receive_value<T: serde::de::DeserializeOwned>(input: &mut &[u8]) -> T {
    let (v, rest) = postcard::take_from_bytes(input)
        .unwrap_or_else(|e| panic!("remote component: a prop does not decode — the app and the bundle disagree about the props ({e})"));
    *input = rest;
    v
}

/// `impl RemoteProp` for value types that are `Serialize +
/// DeserializeOwned`: they cross as a copy, fixed at mount.
#[macro_export]
macro_rules! remote_value {
    ($($t:ty),* $(,)?) => {$(
        $crate::__import_value!($t);
        impl $crate::remote::RemoteProp for $t {
            #[cfg(not(idealyst_stream_guest))]
            fn send(&self, out: &mut ::std::vec::Vec<u8>, _keep: &mut $crate::remote::host::Keep) {
                $crate::remote::__send_value(self, out)
            }
            #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
            fn receive(input: &mut &[u8]) -> Self {
                $crate::remote::__receive_value(input)
            }
        }
    )*};
}

// RemoteProp only: `__import_value!` below covers ImportArg for these.
macro_rules! remote_prop_only {
    ($($t:ty),*) => {$(
        impl RemoteProp for $t {
            #[cfg(not(idealyst_stream_guest))]
            fn send(&self, out: &mut Vec<u8>, _keep: &mut host::Keep) {
                __send_value(self, out)
            }
            #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
            fn receive(input: &mut &[u8]) -> Self {
                __receive_value(input)
            }
        }
    )*};
}
remote_prop_only!(String, bool, char, i8, i16, i32, i64, u8, u16, u32, u64, usize, isize, f32, f64);

impl<T: RemoteProp> RemoteProp for Option<T> {
    #[cfg(not(idealyst_stream_guest))]
    fn send(&self, out: &mut Vec<u8>, keep: &mut host::Keep) {
        __send_value(&self.is_some(), out);
        if let Some(v) = self {
            v.send(out, keep);
        }
    }
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn receive(input: &mut &[u8]) -> Self {
        __receive_value::<bool>(input).then(|| T::receive(input))
    }
}

impl<T: RemoteProp> RemoteProp for Vec<T> {
    #[cfg(not(idealyst_stream_guest))]
    fn send(&self, out: &mut Vec<u8>, keep: &mut host::Keep) {
        __send_value(&(self.len() as u64), out);
        for v in self {
            v.send(out, keep);
        }
    }
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn receive(input: &mut &[u8]) -> Self {
        let n = __receive_value::<u64>(input);
        (0..n).map(|_| T::receive(input)).collect()
    }
}

fn codec_encode<T: Serialize>(v: &T, out: &mut Vec<u8>) {
    __send_value(v, out)
}

fn codec_decode<T: serde::de::DeserializeOwned>(b: &[u8]) -> Option<T> {
    from_bytes(b).ok()
}

/// The kernel codec a signal prop's value crosses with (postcard).
fn signal_codec<T: Serialize + serde::de::DeserializeOwned>() -> runtime_world::remote::Codec<T> {
    runtime_world::remote::Codec { encode: codec_encode::<T>, decode: codec_decode::<T> }
}

type SignalHandle = (u32, u32, u32);

impl<T: Serialize + serde::de::DeserializeOwned + PartialEq + 'static> RemoteProp for runtime_world::ReadSignal<T> {
    #[cfg(not(idealyst_stream_guest))]
    fn send(&self, out: &mut Vec<u8>, keep: &mut host::Keep) {
        let (h, guard) = runtime_world::remote::export_read_signal(*self, signal_codec::<T>());
        keep.push(Box::new(guard));
        __send_value::<SignalHandle>(&h, out);
    }
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn receive(input: &mut &[u8]) -> Self {
        runtime_world::remote_guest::import_read_signal(__receive_value::<SignalHandle>(input), signal_codec::<T>())
    }
}

impl<T: Serialize + serde::de::DeserializeOwned + PartialEq + 'static> RemoteProp for runtime_world::Signal<T> {
    #[cfg(not(idealyst_stream_guest))]
    fn send(&self, out: &mut Vec<u8>, keep: &mut host::Keep) {
        let (h, guard) = runtime_world::remote::export_signal(*self, signal_codec::<T>());
        keep.push(Box::new(guard));
        __send_value::<SignalHandle>(&h, out);
    }
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn receive(input: &mut &[u8]) -> Self {
        runtime_world::remote_guest::import_signal(__receive_value::<SignalHandle>(input), signal_codec::<T>())
    }
}

// ---------------------------------------------------------------------------
// App components used from a bundle (imports)
// ---------------------------------------------------------------------------
//
// Every component NOT marked `remote` lives in the app binary. In a bundle
// build, `#[component]` compiles such a component to a stub that sends its
// props and asks the app for its own copy, by name
// (`crate::__remote_import!`); in a native app build with `remote` on, it
// registers the component so it can be built when a bundle asks
// (`crate::__remote_app_component!`, into `host::APP_COMPONENTS`). Props
// cross with [`ImportArg`] — the bundle → app direction of [`RemoteProp`].

/// How a prop of an app component crosses from a bundle to the app.
///
/// Values are copied; callbacks, `Reactive` getters and children cross as
/// callbacks into the bundle (the element codec's ids); a `Signal` the
/// bundle created is PROMOTED into the app's arena
/// (`runtime_world::remote::receive_signal`) so the app's component reads
/// it natively. A props struct is an `ImportArg` when every field is — the
/// `#[props]` / `#[component]` emission implements it field by field
/// through [`Arg`], so a field type with no impl is a runtime error naming
/// the type, never a compile error in an app that never imports it.
pub trait ImportArg: Sized + 'static {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>);
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String>;
}

/// The probe the emission calls a prop's crossing through: [`ViaImport`]
/// when the type is an [`ImportArg`], else [`ViaUnsupported`] (autoref
/// specialization — the concrete field type picks the impl).
#[doc(hidden)]
pub struct Arg<T>(std::marker::PhantomData<T>);

impl<T> Arg<T> {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Arg(std::marker::PhantomData)
    }
}

#[doc(hidden)]
pub trait ViaImport<T> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(&self, v: T, out: &mut Vec<u8>);
    #[cfg(not(idealyst_stream_guest))]
    fn receive(&self, input: &mut &[u8], cx: &host::ImportCx) -> Result<T, String>;
}

impl<T: ImportArg> ViaImport<T> for Arg<T> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(&self, v: T, out: &mut Vec<u8>) {
        v.send(out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(&self, input: &mut &[u8], cx: &host::ImportCx) -> Result<T, String> {
        T::receive(input, cx)
    }
}

#[doc(hidden)]
pub trait ViaUnsupported<T> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(&self, v: T, out: &mut Vec<u8>);
    #[cfg(not(idealyst_stream_guest))]
    fn receive(&self, input: &mut &[u8], cx: &host::ImportCx) -> Result<T, String>;
}

impl<T> ViaUnsupported<T> for &Arg<T> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(&self, _v: T, _out: &mut Vec<u8>) {
        panic!(
            "a prop of type `{}` can't cross from a remote component to an app component yet",
            std::any::type_name::<T>()
        )
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(&self, _input: &mut &[u8], _cx: &host::ImportCx) -> Result<T, String> {
        Err(format!("a prop of type `{}` can't cross from a remote component yet", std::any::type_name::<T>()))
    }
}

#[doc(hidden)]
pub fn __try_receive_value<T: serde::de::DeserializeOwned>(input: &mut &[u8]) -> Result<T, String> {
    let (v, rest) = postcard::take_from_bytes(input).map_err(|e| e.to_string())?;
    *input = rest;
    Ok(v)
}

/// `ImportArg` for serializable value types (also emitted by
/// [`remote_value!`]).
#[macro_export]
#[doc(hidden)]
macro_rules! __import_value {
    ($($t:ty),* $(,)?) => {$(
        impl $crate::remote::ImportArg for $t {
            #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
            fn send(self, out: &mut ::std::vec::Vec<u8>) {
                $crate::remote::__send_value(&self, out)
            }
            #[cfg(not(idealyst_stream_guest))]
            fn receive(input: &mut &[u8], _cx: &$crate::remote::host::ImportCx) -> ::core::result::Result<Self, ::std::string::String> {
                $crate::remote::__try_receive_value(input)
            }
        }
    )*};
}

__import_value!(String, bool, char, i8, i16, i32, i64, u8, u16, u32, u64, usize, isize, f32, f64, StyleRules);

impl ImportArg for () {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, _out: &mut Vec<u8>) {}
    #[cfg(not(idealyst_stream_guest))]
    fn receive(_input: &mut &[u8], _cx: &host::ImportCx) -> Result<Self, String> {
        Ok(())
    }
}

impl<T: ImportArg> ImportArg for Option<T> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&self.is_some(), out);
        if let Some(v) = self {
            v.send(out);
        }
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String> {
        if __try_receive_value::<bool>(input)? {
            Ok(Some(T::receive(input, cx)?))
        } else {
            Ok(None)
        }
    }
}

impl<T: ImportArg> ImportArg for Vec<T> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&(self.len() as u64), out);
        for v in self {
            v.send(out);
        }
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String> {
        let n = __try_receive_value::<u64>(input)?;
        (0..n).map(|_| T::receive(input, cx)).collect()
    }
}

/// Children (and any `Element` prop): encoded by the element codec, so
/// whatever the bundle built under the app component crosses as a tree.
impl ImportArg for runtime_scene::Element {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&bundle::encode(self), out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String> {
        let node: Node = __try_receive_value(input)?;
        cx.build(node)
    }
}

/// A prop's `Reactive<T>`: a static value is copied; a live one becomes a
/// getter into the bundle, read by the app component's binding effects
/// (which subscribe to whatever the bundle's closure reads — the app's
/// graph either way).
impl<T> ImportArg for crate::glue::Reactive<T>
where
    T: Serialize + serde::de::DeserializeOwned + 'static,
{
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        match self {
            crate::glue::Reactive::Static(v) => {
                __send_value(&false, out);
                __send_value(&v, out);
            }
            crate::glue::Reactive::Dynamic(f) => {
                __send_value(&true, out);
                __send_value(&bundle::register_getter(move || to_bytes(&f())), out);
            }
        }
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String> {
        if __try_receive_value::<bool>(input)? {
            let get = cx.callback(__try_receive_value(input)?);
            // Read once now, while the bundle is known to be callable: the
            // getter then always has a last value to fall back on if the
            // bundle is poisoned later (`T` has no default to invent).
            get.get::<T>(&[]).ok_or_else(|| "the bundle stopped (it panicked)".to_string())?;
            Ok(crate::glue::Reactive::Dynamic(std::rc::Rc::new(move || {
                get.get::<T>(&[]).expect("primed at receive: a last value always exists")
            })))
        } else {
            Ok(crate::glue::Reactive::Static(__try_receive_value(input)?))
        }
    }
}

/// A callback prop: runs in the bundle.
impl ImportArg for std::rc::Rc<dyn Fn()> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&bundle::register_call(std::rc::Rc::new(move |_: &[u8]| {
            self();
            Vec::new()
        })), out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String> {
        let r = cx.callback(__try_receive_value(input)?);
        Ok(std::rc::Rc::new(move || {
            r.call(&[]);
        }))
    }
}

/// A one-argument callback prop (`on_change: Rc<dyn Fn(bool)>`): the app
/// calls it with a value, which crosses to the bundle.
impl<A> ImportArg for std::rc::Rc<dyn Fn(A)>
where
    A: Serialize + serde::de::DeserializeOwned + 'static,
{
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&bundle::register_call(std::rc::Rc::new(move |args: &[u8]| {
            let a: A = from_bytes(args).unwrap_or_else(|e| panic!("remote codec: a callback argument does not decode: {e}"));
            self(a);
            Vec::new()
        })), out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String> {
        let r = cx.callback(__try_receive_value(input)?);
        Ok(std::rc::Rc::new(move |a: A| {
            r.call(&to_bytes(&a));
        }))
    }
}

/// A signal prop. If the bundle created it, the app PROMOTES it: the value
/// moves into the app's arena and both sides share it from then on.
impl<T> ImportArg for runtime_world::Signal<T>
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + 'static,
{
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        let h = runtime_world::remote_guest::offer_signal(self, signal_codec::<T>());
        __send_value::<SignalHandle>(&h, out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], _cx: &host::ImportCx) -> Result<Self, String> {
        runtime_world::remote::receive_signal(__try_receive_value::<SignalHandle>(input)?, signal_codec::<T>())
    }
}

impl<T> ImportArg for runtime_world::ReadSignal<T>
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + 'static,
{
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        let h = runtime_world::remote_guest::offer_read_signal(self, signal_codec::<T>());
        __send_value::<SignalHandle>(&h, out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], _cx: &host::ImportCx) -> Result<Self, String> {
        runtime_world::remote::receive_signal(__try_receive_value::<SignalHandle>(input)?, signal_codec::<T>())
            .map(|s| s.read_only())
    }
}

/// A stylesheet prop: proxied like a node's sheet, so the app's style
/// engine (and theme) resolves it.
impl ImportArg for std::rc::Rc<runtime_shared::StyleSheet> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&bundle::sheet_ref(&self), out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], cx: &host::ImportCx) -> Result<Self, String> {
        Ok(cx.sheet(__try_receive_value(input)?))
    }
}

// ---- what `#[component]` / `#[props]` emit (no-ops unless this build hosts
// ---- or is a remote bundle) ----

/// `impl ImportArg` for a props struct, field by field.
#[cfg(all(feature = "remote", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_props {
    ($ty:ident { $($f:ident : $t:ty),* $(,)? }) => {
        impl $crate::remote::ImportArg for $ty {
            $crate::__remote_props_send! { $($f : $t),* }
            $crate::__remote_props_receive! { $($f : $t),* }
        }
    };
}

#[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_props_send {
    ($($f:ident : $t:ty),*) => {
        fn send(self, __out: &mut ::std::vec::Vec<u8>) {
            #[allow(unused_imports)]
            use $crate::remote::{ViaImport as _, ViaUnsupported as _};
            let Self { $($f),* } = self;
            $( (&$crate::remote::Arg::<$t>::new()).send($f, __out); )*
            let _ = __out;
        }
    };
}

#[cfg(not(any(idealyst_stream_guest, feature = "remote-loopback")))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_props_send {
    ($($t:tt)*) => {};
}

#[cfg(not(idealyst_stream_guest))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_props_receive {
    ($($f:ident : $t:ty),*) => {
        fn receive(
            __in: &mut &[u8],
            __cx: &$crate::remote::host::ImportCx,
        ) -> ::core::result::Result<Self, ::std::string::String> {
            #[allow(unused_imports)]
            use $crate::remote::{ViaImport as _, ViaUnsupported as _};
            let _ = (&__in, __cx);
            ::core::result::Result::Ok(Self { $($f: (&$crate::remote::Arg::<$t>::new()).receive(__in, __cx)?),* })
        }
    };
}

#[cfg(idealyst_stream_guest)]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_props_receive {
    ($($t:tt)*) => {};
}

/// Register an app component for bundles to import (native app builds).
#[cfg(all(feature = "remote", not(target_arch = "wasm32"), not(idealyst_stream_guest)))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_app_component {
    ($name:expr, $props:ty, |$p:ident| $call:expr) => {
        const _: () = {
            fn __build(
                __in: &mut &[u8],
                __cx: &$crate::remote::host::ImportCx,
            ) -> ::core::result::Result<$crate::remote::__Element, ::std::string::String> {
                #[allow(unused_imports)]
                use $crate::remote::{ViaImport as _, ViaUnsupported as _};
                let $p: $props = (&$crate::remote::Arg::<$props>::new()).receive(__in, __cx)?;
                ::core::result::Result::Ok($call)
            }
            #[$crate::remote::__linkme::distributed_slice($crate::remote::host::APP_COMPONENTS)]
            #[linkme(crate = $crate::remote::__linkme)]
            static __ENTRY: $crate::remote::host::AppComponent =
                $crate::remote::host::AppComponent { name: $name, build: __build };
        };
    };
}

/// The bundle-side body of an app component: send the props, import the
/// app's copy by name.
#[cfg(all(feature = "remote", any(idealyst_stream_guest, feature = "remote-loopback")))]
#[macro_export]
#[doc(hidden)]
macro_rules! __remote_import {
    ($name:expr, $props:ty, $value:expr) => {{
        #[allow(unused_imports)]
        use $crate::remote::{ViaImport as _, ViaUnsupported as _};
        let mut __out = ::std::vec::Vec::new();
        (&$crate::remote::Arg::<$props>::new()).send($value, &mut __out);
        $crate::remote::bundle::import_component($name, __out)
    }};
}

#[doc(hidden)]
pub use runtime_scene::Element as __Element;
#[doc(hidden)]
pub use linkme as __linkme;

// ---------------------------------------------------------------------------
// Navigator handles as props
// ---------------------------------------------------------------------------
//
// The app's navigator handle crosses as an id into the app's handle table
// (`handles`); the bundle gets a `NavHandle` whose commands cross back as
// `WireNav`, their typed params rebuilt from the url by the navigator. A
// `Ref<StackHandle>`-style prop crosses as the REF: the app reads it when
// the bundle navigates, as native code does — a screen is built before its
// navigator fills the ref, so a value taken at mount would be empty.

impl RemoteProp for crate::prims::NavHandle {
    #[cfg(not(idealyst_stream_guest))]
    fn send(&self, out: &mut Vec<u8>, keep: &mut host::Keep) {
        let (id, guard) = handles::hold_scoped(handles::Held::Nav(self.clone()));
        keep.push(Box::new(guard));
        __send_value(&id, out)
    }
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn receive(input: &mut &[u8]) -> Self {
        handles::nav_proxy(__receive_value(input))
    }
}

impl<H: crate::prims::NavHandleType> RemoteProp for runtime_shared::Ref<H> {
    #[cfg(not(idealyst_stream_guest))]
    fn send(&self, out: &mut Vec<u8>, keep: &mut host::Keep) {
        let r = *self;
        let (id, guard) = handles::hold_scoped(handles::Held::NavRef {
            get: std::rc::Rc::new(move || r.get().map(|h| h.nav_handle().clone())),
            original: std::rc::Rc::new(r),
        });
        keep.push(Box::new(guard));
        __send_value(&id, out)
    }
    /// Filled at once: the bundle's handle forwards to whatever the app's
    /// ref holds when it is used.
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn receive(input: &mut &[u8]) -> Self {
        let r = runtime_shared::Ref::new();
        r.fill(H::from_nav_handle(handles::nav_proxy(__receive_value(input))));
        r
    }
}

/// The app's id for a navigator handle a bundle hands back (to an app
/// component); only handles the bundle received from the app can cross.
#[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
fn nav_id_of(handle: &crate::prims::NavHandle) -> u32 {
    handles::nav_id(handle).unwrap_or_else(|| {
        panic!(
            "remote component: a navigator handle can cross to the app only if it came from the app — \\
             a navigator the remote component mounts itself can't be driven from app components yet"
        )
    })
}

impl ImportArg for crate::prims::NavHandle {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&nav_id_of(&self), out)
    }
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], _cx: &host::ImportCx) -> Result<Self, String> {
        let id: u32 = __try_receive_value(input)?;
        handles::held_nav(id).ok_or_else(|| format!("navigator handle {id} is no longer held"))
    }
}

impl<H: crate::prims::NavHandleType> ImportArg for runtime_shared::Ref<H> {
    #[cfg(any(idealyst_stream_guest, feature = "remote-loopback"))]
    fn send(self, out: &mut Vec<u8>) {
        __send_value(&self.get().map(|h| nav_id_of(h.nav_handle())), out)
    }
    /// The app's own `Ref` when the bundle's came from one; otherwise a ref
    /// holding the navigator handle.
    #[cfg(not(idealyst_stream_guest))]
    fn receive(input: &mut &[u8], _cx: &host::ImportCx) -> Result<Self, String> {
        let r = runtime_shared::Ref::new();
        if let Some(id) = __try_receive_value::<Option<u32>>(input)? {
            if let Some(original) = handles::held_nav_ref(id).and_then(|o| o.downcast_ref::<runtime_shared::Ref<H>>().copied()) {
                return Ok(original);
            }
            let nav = handles::held_nav(id).ok_or_else(|| format!("navigator handle {id} is no longer held"))?;
            r.fill(H::from_nav_handle(nav));
        }
        Ok(r)
    }
}
