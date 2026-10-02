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
        content: Val<String>,
    },
    Button {
        common: Common,
        label: Val<String>,
        on_press: WireAction,
        disabled: Option<Val<bool>>,
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

/// What every crossing primitive carries.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Common {
    pub test_id: Option<String>,
    pub style: Option<Style>,
    pub a11y: Option<A11y>,
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
        ImagePrim => Crossing::Unsupported("image", LATER),
        IconPrim => Crossing::Unsupported("icon", LATER),
        LinkPrim => Crossing::Unsupported("link", LATER),
        TogglePrim => Crossing::Unsupported("toggle", LATER),
        SliderPrim => Crossing::Unsupported("slider", LATER),
        ActivityIndicatorPrim => Crossing::Unsupported("activity_indicator", LATER),
        TextInputPrim => Crossing::Unsupported("text_input", LATER),
        TextAreaPrim => Crossing::Unsupported("text_area", LATER),
        ScrollViewPrim => Crossing::Unsupported("scroll_view", LATER),
        RepeatPrim => Crossing::Unsupported("repeat (static `for` lowering)", LATER),
        LazyPrim => Crossing::Unsupported(
            "lazy",
            "web code splitting has no meaning inside a bundle — `remote` and `lazy` are per-target alternatives",
        ),
        VirtualizerPrim => Crossing::Unsupported("virtualizer", LATER),
        VirtualGridPrim => Crossing::Unsupported("virtual_grid", LATER),
        GraphicsPrim => Crossing::Unsupported("graphics", LATER),
        PortalPrim => Crossing::Unsupported("portal", LATER),
        PresencePrim => Crossing::Unsupported("presence", LATER),
        StackNavigatorPrim => Crossing::Unsupported("stack navigator", LATER),
        SwapNavigatorPrim => Crossing::Unsupported("swap navigator", LATER),
        NavigatorOutletPrim => Crossing::Unsupported("navigator outlet", LATER),
    }
    None
}
