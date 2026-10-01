//! The contract between a **streamed bundle** (a wasm guest) and the app
//! that loads it (the host). Both sides link this crate, so the encoding
//! cannot drift between them.
//!
//! # Shape of the boundary
//!
//! The host owns the ONE reactive graph. A guest never holds a signal — it
//! holds a `u32` handle into a per-bundle table on the host, and reads or
//! writes through the [`imports`]. Closures stay in the guest and cross as
//! callback ids; the host invokes them through [`exports::CALL`].
//!
//! A component mount returns a [`Node`] description, not live nodes: the
//! host turns it into a real `runtime_scene::Element`, wiring each
//! [`Content::Dyn`] and every callback id back into the guest.
//!
//! # Why a hand-rolled codec
//!
//! [`Wire`] is a few dozen lines of little-endian framing. A serde format
//! would be the production answer once props carry structs, but the spike
//! is measuring the *floor* cost of a bundle, and every dependency here is
//! linked into every guest.

#![forbid(unsafe_code)]

/// Bumped on any incompatible change to the imports, exports, or encoding.
/// A host refuses a bundle whose manifest names a different version.
pub const ABI_VERSION: u32 = 3;

/// The wasm import module every host function lives under.
pub const IMPORT_MODULE: &str = "idealyst_stream";

/// Host functions a guest imports from [`IMPORT_MODULE`].
pub mod imports {
    /// `(ptr, len) -> handle` — a new guest-owned signal, initial value
    /// encoded with [`Wire`](crate::Wire). Owned by the component scope
    /// that is mounting when it is created.
    pub const SIGNAL_NEW: &str = "signal_new";
    /// `(handle, buf_ptr, buf_cap) -> len` — a TRACKED read. Writes the
    /// value into the buffer when `len <= buf_cap`; otherwise the guest
    /// grows the buffer and asks again.
    pub const SIGNAL_GET: &str = "signal_get";
    /// `(handle, ptr, len)` — a staged write, committed at the host's
    /// next flush (same semantics as a native `Signal::set`).
    pub const SIGNAL_SET: &str = "signal_set";
    /// `(handle, callback)` — a staged read-modify-write: the host calls
    /// `callback` with the STAGED value and stages what it returns,
    /// matching native `Signal::update`.
    pub const SIGNAL_UPDATE: &str = "signal_update";
    /// `(callback)` — an effect whose body is guest callback `callback`.
    /// Created when the current guest call returns (see the host's
    /// `Bundle` docs for why it cannot run inline).
    pub const EFFECT_NEW: &str = "effect_new";
}

/// Functions a guest exports for the host to call.
pub mod exports {
    /// `() -> packed(ptr, len)` — the encoded [`Manifest`](crate::Manifest).
    pub const MANIFEST: &str = "stream_manifest";
    /// `(len) -> ptr` — guest-owned scratch the host writes inputs into.
    /// The next `MOUNT` / `CALL` takes ownership of it.
    pub const ALLOC: &str = "stream_alloc";
    /// `(component, props_ptr, props_len) -> packed(ptr, len)` — run a
    /// component body with encoded `Vec<PropEntry>` props; returns an
    /// encoded [`Node`](crate::Node).
    pub const MOUNT: &str = "stream_mount";
    /// `(callback, args_ptr, args_len) -> packed(ptr, len)` — invoke a
    /// guest closure; returns its encoded result (empty for actions).
    pub const CALL: &str = "stream_call";
    /// `(callback)` — the host no longer references this callback.
    pub const DROP: &str = "stream_drop";
    /// `() -> count` — live callbacks; a leak probe for tests.
    pub const LIVE_CALLBACKS: &str = "stream_live_callbacks";
}

/// Pack a guest `(ptr, len)` pair into one wasm `i64` return value.
pub fn pack(ptr: u32, len: u32) -> u64 {
    ((ptr as u64) << 32) | len as u64
}

/// Inverse of [`pack`].
pub fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// Little-endian length-prefixed encoding shared by both sides.
pub trait Wire: Sized {
    /// The type's name in prop schemas. Two sides agree on a prop's type
    /// exactly when their tags are equal, so it must be stable across
    /// compilations — a spelled-out name, never `type_name`.
    fn type_tag() -> String;
    fn encode(&self, out: &mut Vec<u8>);
    /// Consume one value from the front of `input`. `None` on truncation
    /// or a malformed value.
    fn decode(input: &mut &[u8]) -> Option<Self>;

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }

    /// Decode a value that must occupy all of `bytes`.
    fn from_bytes(mut bytes: &[u8]) -> Option<Self> {
        let v = Self::decode(&mut bytes)?;
        bytes.is_empty().then_some(v)
    }
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if input.len() < n {
        return None;
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Some(head)
}

macro_rules! wire_le {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn type_tag() -> String {
                stringify!($t).to_string()
            }
            fn encode(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn decode(input: &mut &[u8]) -> Option<Self> {
                let b = take(input, core::mem::size_of::<$t>())?;
                Some(<$t>::from_le_bytes(b.try_into().ok()?))
            }
        }
    )*};
}
wire_le!(u8, u32, i32, u64, i64, f64);

impl Wire for bool {
    fn type_tag() -> String {
        "bool".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(*self as u8);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

impl Wire for String {
    fn type_tag() -> String {
        "String".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        (self.len() as u32).encode(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        let n = u32::decode(input)? as usize;
        String::from_utf8(take(input, n)?.to_vec()).ok()
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn type_tag() -> String {
        format!("Vec<{}>", T::type_tag())
    }
    fn encode(&self, out: &mut Vec<u8>) {
        (self.len() as u32).encode(out);
        for v in self {
            v.encode(out);
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        let n = u32::decode(input)? as usize;
        // Cap the pre-allocation by what the input could possibly hold, so
        // a corrupt length cannot make the host allocate gigabytes.
        let mut v = Vec::with_capacity(n.min(input.len()));
        for _ in 0..n {
            v.push(T::decode(input)?);
        }
        Some(v)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn type_tag() -> String {
        format!("Option<{}>", T::type_tag())
    }
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                v.encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(None),
            1 => Some(Some(T::decode(input)?)),
            _ => None,
        }
    }
}

impl<T: Wire, E: Wire> Wire for Result<T, E> {
    fn type_tag() -> String {
        format!("Result<{}, {}>", T::type_tag(), E::type_tag())
    }
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Ok(v) => {
                out.push(0);
                v.encode(out);
            }
            Err(e) => {
                out.push(1);
                e.encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(Ok(T::decode(input)?)),
            1 => Some(Err(E::decode(input)?)),
            _ => None,
        }
    }
}

impl Wire for () {
    fn type_tag() -> String {
        "()".into()
    }
    fn encode(&self, _: &mut Vec<u8>) {}
    fn decode(_: &mut &[u8]) -> Option<Self> {
        Some(())
    }
}

/// Host functions: app code a bundle calls by name.
///
/// A `#[host_fn]` is defined once, in a crate both the app and the bundle
/// depend on. In the app it is the real function, plus a [`HostFnDef`] the
/// app exports to bundles. In a bundle build it is a stub that calls ONE
/// wasm import per function, under [`HOST_FN_MODULE`], named
/// [`import_name`]`(path, schema)`.
///
/// So the bundle's wasm import section IS its list of host functions —
/// written by the linker, never by hand, and only for the functions the
/// bundle actually calls (dead stubs are stripped). The host reads it at
/// load and refuses the bundle before any guest code runs if the app does
/// not export a function, or exports a different signature.
pub mod host_fn {
    use std::future::Future;
    use std::pin::Pin;

    /// The wasm import module every host function lives under.
    pub const HOST_FN_MODULE: &str = "idealyst_host_fn";

    /// A boxed async host-function body: encoded args in, encoded result out.
    pub type AsyncCall = fn(Vec<u8>) -> Pin<Box<dyn Future<Output = Vec<u8>>>>;

    #[derive(Clone, Copy)]
    pub enum HostFnKind {
        /// Import signature `(args_ptr, args_len) -> packed(ptr, len)`; the
        /// result buffer is allocated in the guest through `stream_alloc`.
        Sync(fn(&[u8]) -> Vec<u8>),
        /// Import signature `(args_ptr, args_len, then_callback)`. Returns at
        /// once; the host runs the future and calls `then_callback` with the
        /// encoded result, if the scope that started it is still alive.
        Async(AsyncCall),
    }

    /// What an app exports for one host function. Generated by
    /// `#[host_fn]` as `<fn_name>::export()`.
    #[derive(Clone, Copy)]
    pub struct HostFnDef {
        /// `module_path!()::fn_name` of the defining crate — identical on
        /// both sides because both compile the same definition.
        pub path: &'static str,
        /// Fingerprint of the signature (arg types, return type,
        /// asyncness), same scheme as `#[server]`'s schema hash.
        pub schema: u64,
        pub kind: HostFnKind,
    }

    /// The import name a bundle's stub links against.
    pub fn import_name(path: &str, schema: u64) -> String {
        format!("{path}#{schema:016x}")
    }

    /// Split an import name back into `(path, schema)`.
    pub fn parse_import_name(name: &str) -> Option<(&str, u64)> {
        let (path, hash) = name.rsplit_once('#')?;
        Some((path, u64::from_str_radix(hash, 16).ok()?))
    }
}

/// Text that is either fixed or computed by a guest closure.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Static(String),
    /// A guest callback returning an encoded `String`. The host runs it in
    /// a tracked context, so the signals it reads become dependencies.
    Dyn(u32),
}

impl Wire for Content {
    fn type_tag() -> String {
        "Content".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Content::Static(s) => {
                out.push(0);
                s.encode(out);
            }
            Content::Dyn(cb) => {
                out.push(1);
                cb.encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(Content::Static(String::decode(input)?)),
            1 => Some(Content::Dyn(u32::decode(input)?)),
            _ => None,
        }
    }
}

/// One node of a mounted component's description.
///
/// The spike carries a deliberately small vocabulary — enough to exercise
/// structure, reactive text, events, and host-component imports. Growing it
/// to the full primitive set is mechanical; the boundary design is not.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    View(Vec<Node>),
    Text(Content),
    Button { label: Content, on_press: u32 },
    /// A component the HOST exports, looked up by stable name. Its props are
    /// positional bytes that the host export decodes. (Not yet schema-checked
    /// like a bundle component's props — the same mechanism applies.)
    Host { name: String, props: Vec<u8> },
}

impl Wire for Node {
    fn type_tag() -> String {
        "Node".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Node::View(children) => {
                out.push(0);
                children.encode(out);
            }
            Node::Text(c) => {
                out.push(1);
                c.encode(out);
            }
            Node::Button { label, on_press } => {
                out.push(2);
                label.encode(out);
                on_press.encode(out);
            }
            Node::Host { name, props } => {
                out.push(3);
                name.encode(out);
                props.encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(Node::View(Vec::decode(input)?)),
            1 => Some(Node::Text(Content::decode(input)?)),
            2 => Some(Node::Button {
                label: Content::decode(input)?,
                on_press: u32::decode(input)?,
            }),
            3 => Some(Node::Host {
                name: String::decode(input)?,
                props: Vec::decode(input)?,
            }),
            _ => None,
        }
    }
}

/// The type tag of a read-only signal prop carrying `inner`.
pub fn read_signal_tag(inner: &str) -> String {
    format!("ReadSignal<{inner}>")
}

/// The type tag of a two-way signal prop carrying `inner`.
pub fn signal_tag(inner: &str) -> String {
    format!("Signal<{inner}>")
}

/// One prop a component declares.
#[derive(Debug, Clone, PartialEq)]
pub struct PropSchema {
    pub name: String,
    /// A [`Wire::type_tag`], or a [`read_signal_tag`] / [`signal_tag`].
    pub ty: String,
    /// `false` when the component has a default for it.
    pub required: bool,
}

impl Wire for PropSchema {
    fn type_tag() -> String {
        "PropSchema".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        self.name.encode(out);
        self.ty.encode(out);
        self.required.encode(out);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        Some(PropSchema { name: String::decode(input)?, ty: String::decode(input)?, required: bool::decode(input)? })
    }
}

/// A component a bundle exports, with the props it accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentSchema {
    pub name: String,
    pub props: Vec<PropSchema>,
}

impl Wire for ComponentSchema {
    fn type_tag() -> String {
        "ComponentSchema".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        self.name.encode(out);
        self.props.encode(out);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        Some(ComponentSchema { name: String::decode(input)?, props: Vec::decode(input)? })
    }
}

/// One named prop value on its way into a mount. Signals travel as their
/// `u32` handle.
#[derive(Debug, Clone, PartialEq)]
pub struct PropEntry {
    pub name: String,
    pub bytes: Vec<u8>,
}

impl Wire for PropEntry {
    fn type_tag() -> String {
        "PropEntry".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        self.name.encode(out);
        self.bytes.encode(out);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        Some(PropEntry { name: String::decode(input)?, bytes: Vec::decode(input)? })
    }
}

/// What a bundle provides and what it needs. The host checks this at load,
/// BEFORE any guest code runs, so an app binary that lacks an imported host
/// component fails loudly instead of rendering half a tree. Component prop
/// schemas are checked per mount, against the props the app actually sends.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub abi: u32,
    /// Components, in the order `stream_mount` indexes them.
    pub components: Vec<ComponentSchema>,
    /// Host components this bundle mounts by name.
    pub imports: Vec<String>,
}

impl Manifest {
    pub fn component(&self, name: &str) -> Option<(usize, &ComponentSchema)> {
        self.components.iter().enumerate().find(|(_, c)| c.name == name)
    }
}

impl Wire for Manifest {
    fn type_tag() -> String {
        "Manifest".into()
    }
    fn encode(&self, out: &mut Vec<u8>) {
        self.abi.encode(out);
        self.components.encode(out);
        self.imports.encode(out);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        Some(Manifest {
            abi: u32::decode(input)?,
            components: Vec::decode(input)?,
            imports: Vec::decode(input)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_round_trips() {
        let n = Node::View(vec![
            Node::Text(Content::Static("hi".into())),
            Node::Text(Content::Dyn(7)),
            Node::Button { label: Content::Static("go".into()), on_press: 3 },
            Node::Host { name: "Badge".into(), props: "x".to_string().to_bytes() },
        ]);
        assert_eq!(Node::from_bytes(&n.to_bytes()), Some(n));
    }

    #[test]
    fn truncated_input_is_rejected_not_misread() {
        let bytes = "hello".to_string().to_bytes();
        assert_eq!(String::from_bytes(&bytes[..bytes.len() - 1]), None);
        assert_eq!(String::from_bytes(&[bytes.as_slice(), &[0]].concat()), None);
    }

    #[test]
    fn corrupt_vec_length_does_not_preallocate_unbounded() {
        // u32::MAX elements claimed, nothing behind it: must fail, not OOM.
        let bytes = u32::MAX.to_bytes();
        assert_eq!(Vec::<u64>::from_bytes(&bytes), None);
    }

    #[test]
    fn manifest_round_trips_with_schemas() {
        let m = Manifest {
            abi: ABI_VERSION,
            components: vec![ComponentSchema {
                name: "Counter".into(),
                props: vec![
                    PropSchema { name: "title".into(), ty: String::type_tag(), required: true },
                    PropSchema { name: "step".into(), ty: i64::type_tag(), required: false },
                ],
            }],
            imports: vec!["Badge".into()],
        };
        assert_eq!(Manifest::from_bytes(&m.to_bytes()), Some(m));
    }

    #[test]
    fn type_tags_are_spelled_out() {
        assert_eq!(i64::type_tag(), "i64");
        assert_eq!(Vec::<String>::type_tag(), "Vec<String>");
        assert_eq!(read_signal_tag(&i64::type_tag()), "ReadSignal<i64>");
    }

    #[test]
    fn option_and_result_round_trip() {
        let v: Vec<Result<Option<String>, u32>> = vec![Ok(Some("a".into())), Ok(None), Err(7)];
        assert_eq!(Vec::from_bytes(&v.to_bytes()), Some(v));
        assert_eq!(Result::<i64, String>::type_tag(), "Result<i64, String>");
    }

    #[test]
    fn host_fn_import_names_round_trip() {
        let name = host_fn::import_name("spike_camera::take_photo", 0xabc);
        assert_eq!(name, "spike_camera::take_photo#0000000000000abc");
        assert_eq!(host_fn::parse_import_name(&name), Some(("spike_camera::take_photo", 0xabc)));
    }

    #[test]
    fn pack_round_trips() {
        assert_eq!(unpack(pack(0xdead_beef, 42)), (0xdead_beef, 42));
    }
}
