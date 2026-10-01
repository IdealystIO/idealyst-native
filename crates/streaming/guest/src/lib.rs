//! The guest side of a streamed bundle.
//!
//! A streamed component is ordinary Rust compiled to `wasm32`. What makes
//! it streamable is that its reactive state lives in the HOST: every
//! [`Signal`] here is a `u32` handle into the host's graph, and every read
//! or write is a call across the boundary. That keeps one reactive graph
//! per app — no proxy signals, no second graph lagging the first by a
//! flush — at the cost of a host call per signal access. The spike exists
//! to measure that cost.
//!
//! Closures never leave the guest. [`effect`], [`text_dyn`] and [`button`]
//! register their closure in a guest-side table and hand the host its id;
//! the host calls back through `stream_call` and releases it with
//! `stream_drop` when the owning scope tears down.
//!
//! On non-wasm targets the crate compiles (so the workspace builds) but
//! every host call panics: there is no host to call.

use std::cell::RefCell;
use std::marker::PhantomData;

pub use stream_abi::{ComponentSchema, Content, Manifest, Node, PropEntry, PropSchema, Wire, ABI_VERSION};

#[cfg(target_arch = "wasm32")]
mod sys {
    #[link(wasm_import_module = "idealyst_stream")]
    extern "C" {
        pub fn signal_new(ptr: *const u8, len: u32) -> u32;
        pub fn signal_get(handle: u32, buf: *mut u8, cap: u32) -> u32;
        pub fn signal_set(handle: u32, ptr: *const u8, len: u32);
        pub fn signal_update(handle: u32, callback: u32);
        pub fn effect_new(callback: u32);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::missing_safety_doc)]
mod sys {
    const NO_HOST: &str = "stream-guest: no host — streamed components run only inside a wasm host";
    pub unsafe fn signal_new(_: *const u8, _: u32) -> u32 {
        panic!("{NO_HOST}")
    }
    pub unsafe fn signal_get(_: u32, _: *mut u8, _: u32) -> u32 {
        panic!("{NO_HOST}")
    }
    pub unsafe fn signal_set(_: u32, _: *const u8, _: u32) {
        panic!("{NO_HOST}")
    }
    pub unsafe fn signal_update(_: u32, _: u32) {
        panic!("{NO_HOST}")
    }
    pub unsafe fn effect_new(_: u32) {
        panic!("{NO_HOST}")
    }
}

// ---------------------------------------------------------------------------
// Callback table
// ---------------------------------------------------------------------------

type Callback = Box<dyn FnMut(&[u8]) -> Vec<u8>>;

enum Slot {
    Empty,
    Ready(Callback),
    /// Taken out for the duration of its own call, so the callback can
    /// re-enter the table (register an effect, run a nested update) without
    /// a `RefCell` double borrow. `dropped` records a `stream_drop` that
    /// arrived mid-call; the slot is freed when the call returns.
    Running { dropped: bool },
}

#[derive(Default)]
struct Callbacks {
    slots: Vec<Slot>,
    free: Vec<u32>,
    live: u32,
}

thread_local! {
    static CALLBACKS: RefCell<Callbacks> = RefCell::default();
    /// Backing store for values returned to the host. Valid until the next
    /// export returns — the host copies out immediately.
    static RET: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn register(cb: Callback) -> u32 {
    CALLBACKS.with(|t| {
        let mut t = t.borrow_mut();
        t.live += 1;
        match t.free.pop() {
            Some(id) => {
                t.slots[id as usize] = Slot::Ready(cb);
                id
            }
            None => {
                t.slots.push(Slot::Ready(cb));
                (t.slots.len() - 1) as u32
            }
        }
    })
}

fn release(id: u32) {
    CALLBACKS.with(|t| {
        let mut t = t.borrow_mut();
        match t.slots.get_mut(id as usize) {
            Some(Slot::Ready(_)) => {
                t.slots[id as usize] = Slot::Empty;
                t.free.push(id);
                t.live -= 1;
            }
            Some(Slot::Running { dropped }) => *dropped = true,
            _ => panic!("stream-guest: host dropped callback {id}, which is not live"),
        }
    })
}

fn invoke(id: u32, args: &[u8]) -> Vec<u8> {
    let mut cb = CALLBACKS.with(|t| {
        let mut t = t.borrow_mut();
        let slot = t
            .slots
            .get_mut(id as usize)
            .unwrap_or_else(|| panic!("stream-guest: host called unknown callback {id}"));
        match std::mem::replace(slot, Slot::Running { dropped: false }) {
            Slot::Ready(cb) => cb,
            _ => panic!("stream-guest: host called callback {id}, which is not callable"),
        }
    });
    let out = cb(args);
    CALLBACKS.with(|t| {
        let mut t = t.borrow_mut();
        let dropped = matches!(t.slots[id as usize], Slot::Running { dropped: true });
        if dropped {
            t.slots[id as usize] = Slot::Empty;
            t.free.push(id);
            t.live -= 1;
        } else {
            t.slots[id as usize] = Slot::Ready(cb);
        }
    });
    out
}

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

/// Values up to this size are read into a stack buffer in ONE crossing.
/// Measured: the earlier thread-local `RefCell<Vec>` path cost ~300 ns per
/// read in the interpreter on top of a ~80 ns crossing — most signal values
/// (numbers, flags, short strings) fit here and skip all of it.
const INLINE_READ: usize = 64;

fn read_handle<T: Wire>(handle: u32) -> T {
    let mut stack = [0u8; INLINE_READ];
    // SAFETY: the host writes at most `INLINE_READ` bytes into `stack`.
    let len = unsafe { sys::signal_get(handle, stack.as_mut_ptr(), INLINE_READ as u32) } as usize;
    let decoded = if len <= INLINE_READ {
        T::from_bytes(&stack[..len])
    } else {
        // Too big for the stack: read again into an exactly-sized buffer.
        // The value cannot change between the two reads — writes are staged
        // until the host's flush, which never runs inside a guest call.
        let mut heap = vec![0u8; len];
        // SAFETY: the host writes at most `len` bytes into `heap`.
        unsafe { sys::signal_get(handle, heap.as_mut_ptr(), len as u32) };
        T::from_bytes(&heap)
    };
    match decoded {
        Some(v) => v,
        None => type_mismatch(handle),
    }
}

#[cold]
#[inline(never)]
fn type_mismatch(handle: u32) -> ! {
    panic!("stream-guest: signal {handle} holds a value of another type")
}

/// A two-way handle into the host's reactive graph.
pub struct Signal<T> {
    handle: u32,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Signal<T> {}

impl<T: Wire + 'static> Signal<T> {
    /// Tracked read: inside an effect or a [`text_dyn`] closure, this
    /// subscribes the caller on the host side.
    pub fn get(&self) -> T {
        read_handle(self.handle)
    }

    /// Staged write, committed at the host's next flush.
    pub fn set(&self, value: T) {
        let bytes = value.to_bytes();
        // SAFETY: `bytes` outlives the call; the host only reads it.
        unsafe { sys::signal_set(self.handle, bytes.as_ptr(), bytes.len() as u32) }
    }

    /// Read-modify-write against the STAGED value, so two updates in one
    /// batch compose (0 → 1 → 2) exactly like a native `Signal::update`.
    /// A `set(get() + 1)` would read the committed value both times.
    pub fn update(&self, f: impl FnOnce(&T) -> T + 'static) {
        let mut f = Some(f);
        let handle = self.handle;
        let cb = register(Box::new(move |old: &[u8]| {
            let old = T::from_bytes(old).unwrap_or_else(|| {
                panic!("stream-guest: signal {handle} holds a value of another type")
            });
            let f = f.take().expect("stream-guest: update callback ran twice");
            f(&old).to_bytes()
        }));
        // SAFETY: plain handle arguments.
        unsafe { sys::signal_update(self.handle, cb) };
        release(cb);
    }

    pub fn read_only(&self) -> ReadSignal<T> {
        ReadSignal { handle: self.handle, _t: PhantomData }
    }
}

/// A read-only handle. Has no write method, so a component that received
/// one provably cannot write the caller's state; the host also refuses
/// writes to read-only handles, for a guest that forges one.
pub struct ReadSignal<T> {
    handle: u32,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for ReadSignal<T> {}

impl<T: Wire + 'static> ReadSignal<T> {
    pub fn get(&self) -> T {
        read_handle(self.handle)
    }

    #[doc(hidden)]
    /// Measurement hook: wrap a raw handle the host registered directly.
    pub fn from_raw(handle: u32) -> Self {
        ReadSignal { handle, _t: PhantomData }
    }
}

#[doc(hidden)]
/// Measurement hook: one `signal_get` crossing into a caller buffer, with
/// no decoding — separates the boundary's cost from the guest-side codec.
pub fn __raw_signal_get(handle: u32, buf: &mut [u8]) -> u32 {
    // SAFETY: the host writes at most `buf.len()` bytes.
    unsafe { sys::signal_get(handle, buf.as_mut_ptr(), buf.len() as u32) }
}

/// A new signal, owned by the component scope that is mounting.
pub fn signal<T: Wire + 'static>(value: T) -> Signal<T> {
    let bytes = value.to_bytes();
    // SAFETY: `bytes` outlives the call; the host only reads it.
    let handle = unsafe { sys::signal_new(bytes.as_ptr(), bytes.len() as u32) };
    Signal { handle, _t: PhantomData }
}

/// An effect in the host's graph whose body is this closure. It is created
/// when the current host→guest call returns, then runs once to collect its
/// dependencies — the same first run a native effect gets, just after the
/// component body instead of inside it.
pub fn effect(mut f: impl FnMut() + 'static) {
    let cb = register(Box::new(move |_| {
        f();
        Vec::new()
    }));
    // SAFETY: plain handle argument.
    unsafe { sys::effect_new(cb) }
}

// ---------------------------------------------------------------------------
// Host functions
// ---------------------------------------------------------------------------

/// A pending call to an async `#[host_fn]`. Does nothing until handed to
/// [`spawn_then`], like an un-awaited future.
#[must_use = "a HostCall does nothing until passed to spawn_then"]
pub struct HostCall<T> {
    args: Vec<u8>,
    start: Box<dyn FnOnce(*const u8, u32, u32)>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Wire + 'static> HostCall<T> {
    #[doc(hidden)]
    /// Built by `#[host_fn]`'s bundle-side stub: `start` invokes the wasm
    /// import with the encoded args and the `then` callback id.
    pub fn new(args: Vec<u8>, start: impl FnOnce(*const u8, u32, u32) + 'static) -> Self {
        HostCall { args, start: Box::new(start), _t: PhantomData }
    }
}

/// Run an async host function and apply its result in a later turn — the
/// bundle-side twin of the framework's native `spawn_then(future, then)`,
/// with the same guarantee, because the host runs the call under exactly
/// that function: the IO always completes, and `then` runs only if the
/// scope that started it (the component being built, or the component
/// whose event handler is running) is still mounted. Otherwise `then` is
/// dropped unrun and its slot released.
///
/// There is no guest-side executor: the IO lives in the app, where it can
/// reach the platform. Composing several calls belongs inside one host
/// function.
pub fn spawn_then<T: Wire + 'static>(call: HostCall<T>, then: impl FnOnce(T) + 'static) {
    let mut then = Some(then);
    let cb = register(Box::new(move |bytes: &[u8]| {
        let value = T::from_bytes(bytes)
            .unwrap_or_else(|| panic!("stream-guest: host call result does not decode as {}", T::type_tag()));
        (then.take().expect("stream-guest: spawn_then callback ran twice"))(value);
        Vec::new()
    }));
    (call.start)(call.args.as_ptr(), call.args.len() as u32, cb);
}

// ---------------------------------------------------------------------------
// Node builders
// ---------------------------------------------------------------------------

pub fn view(children: Vec<Node>) -> Node {
    Node::View(children)
}

pub fn text(content: impl Into<String>) -> Node {
    Node::Text(Content::Static(content.into()))
}

/// Text recomputed by the host whenever a signal `f` reads changes.
pub fn text_dyn(f: impl Fn() -> String + 'static) -> Node {
    Node::Text(dyn_content(f))
}

pub fn button(label: impl Into<String>, on_press: impl Fn() + 'static) -> Node {
    Node::Button { label: Content::Static(label.into()), on_press: action(on_press) }
}

/// Mount a component the HOST exports. `name` must appear in the bundle's
/// `imports` list, which is how the host can refuse the bundle at load
/// instead of failing here.
pub fn host(name: &str, props: Props) -> Node {
    Node::Host { name: name.to_string(), props: props.bytes }
}

fn dyn_content(f: impl Fn() -> String + 'static) -> Content {
    Content::Dyn(register(Box::new(move |_| f().to_bytes())))
}

fn action(f: impl Fn() + 'static) -> u32 {
    register(Box::new(move |_| {
        f();
        Vec::new()
    }))
}

/// Positional props for a host component.
#[derive(Default)]
pub struct Props {
    bytes: Vec<u8>,
}

impl Props {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn string(mut self, s: impl Into<String>) -> Self {
        s.into().encode(&mut self.bytes);
        self
    }
    pub fn text_dyn(mut self, f: impl Fn() -> String + 'static) -> Self {
        dyn_content(f).encode(&mut self.bytes);
        self
    }
    pub fn action(mut self, f: impl Fn() + 'static) -> Self {
        action(f).encode(&mut self.bytes);
        self
    }
}

/// A type a streamed component can take as a prop: any [`Wire`] value, or
/// a signal handle. `tag` is what the manifest advertises and what the host
/// checks its own prop's type against before mounting.
pub trait PropType: Sized {
    fn tag() -> String;
    fn from_prop(bytes: &[u8]) -> Option<Self>;
}

impl<T: Wire> PropType for T {
    fn tag() -> String {
        T::type_tag()
    }
    fn from_prop(bytes: &[u8]) -> Option<Self> {
        T::from_bytes(bytes)
    }
}

impl<T: Wire + 'static> PropType for ReadSignal<T> {
    fn tag() -> String {
        stream_abi::read_signal_tag(&T::type_tag())
    }
    fn from_prop(bytes: &[u8]) -> Option<Self> {
        Some(ReadSignal { handle: u32::from_bytes(bytes)?, _t: PhantomData })
    }
}

impl<T: Wire + 'static> PropType for Signal<T> {
    fn tag() -> String {
        stream_abi::signal_tag(&T::type_tag())
    }
    fn from_prop(bytes: &[u8]) -> Option<Self> {
        Some(Signal { handle: u32::from_bytes(bytes)?, _t: PhantomData })
    }
}

/// The named props the host passed to one mount.
pub struct PropsIn {
    entries: Vec<PropEntry>,
}

impl PropsIn {
    /// Take prop `name`, or `None` when the host did not send it (the
    /// component's default applies). A prop that is present but does not
    /// decode as `T` panics: the host checked its type against this
    /// bundle's manifest before mounting, so that is a host bug.
    pub fn take<T: PropType>(&mut self, name: &str) -> Option<T> {
        let i = self.entries.iter().position(|e| e.name == name)?;
        let entry = self.entries.swap_remove(i);
        Some(T::from_prop(&entry.bytes).unwrap_or_else(|| {
            panic!("stream-guest: prop `{name}` does not decode as {}", T::tag())
        }))
    }
}

/// A component body behind its prop decoding. Built by [`bundle!`].
pub type Component = fn(&mut PropsIn) -> Node;

// ---------------------------------------------------------------------------
// Export plumbing — called from the `bundle!` expansion only.
// ---------------------------------------------------------------------------

#[doc(hidden)]
pub mod __rt {
    use super::*;

    fn ret(bytes: Vec<u8>) -> u64 {
        RET.with(|r| {
            let mut r = r.borrow_mut();
            *r = bytes;
            stream_abi::pack(r.as_ptr() as usize as u32, r.len() as u32)
        })
    }

    /// Reclaim a buffer handed out by [`alloc`].
    ///
    /// # Safety
    /// `(ptr, len)` must come from one [`alloc`] call and be used once.
    unsafe fn take_input(ptr: u32, len: u32) -> Box<[u8]> {
        let ptr = ptr as usize as *mut u8;
        // SAFETY: per the contract, this is the boxed slice `alloc` leaked.
        unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len as usize)) }
    }

    pub fn alloc(len: u32) -> u32 {
        let buf = vec![0u8; len as usize].into_boxed_slice();
        Box::into_raw(buf) as *mut u8 as usize as u32
    }

    pub fn manifest(components: Vec<ComponentSchema>, imports: &[&str]) -> u64 {
        ret(Manifest {
            abi: ABI_VERSION,
            components,
            imports: imports.iter().map(|s| s.to_string()).collect(),
        }
        .to_bytes())
    }

    /// # Safety
    /// `(ptr, len)` must come from [`alloc`].
    pub unsafe fn mount(table: &[Component], component: u32, ptr: u32, len: u32) -> u64 {
        // SAFETY: forwarded contract.
        let props = unsafe { take_input(ptr, len) };
        let entries = Vec::<PropEntry>::from_bytes(&props)
            .unwrap_or_else(|| panic!("stream-guest: props for component #{component} do not decode"));
        let body = table
            .get(component as usize)
            .unwrap_or_else(|| panic!("stream-guest: no component #{component}"));
        let node = body(&mut PropsIn { entries });
        ret(node.to_bytes())
    }

    /// # Safety
    /// `(ptr, len)` must come from [`alloc`].
    pub unsafe fn call(callback: u32, ptr: u32, len: u32) -> u64 {
        // SAFETY: forwarded contract.
        let args = unsafe { take_input(ptr, len) };
        ret(invoke(callback, &args))
    }

    /// Decode a sync host function's result buffer (allocated by the host
    /// through `stream_alloc`, so this side owns it).
    pub fn host_result<T: Wire>(packed: u64, name: &str) -> T {
        let (ptr, len) = stream_abi::unpack(packed);
        // SAFETY: the host allocated this buffer with `stream_alloc`.
        let bytes = unsafe { take_input(ptr, len) };
        T::from_bytes(&bytes)
            .unwrap_or_else(|| panic!("stream-guest: host_fn `{name}` returned a value that does not decode as {}", T::type_tag()))
    }

    pub fn drop_callback(callback: u32) {
        release(callback)
    }

    pub fn live_callbacks() -> u32 {
        CALLBACKS.with(|t| t.borrow().live)
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __has_default {
    () => {
        false
    };
    ($default:expr) => {
        true
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __prop_default {
    ($prop:ident) => {
        panic!(
            "stream-guest: required prop `{}` was not sent — the host's pre-mount check should have refused this mount",
            stringify!($prop)
        )
    };
    ($prop:ident, $default:expr) => {
        $default
    };
}

/// Declare a bundle: its components with their props (in mount-index
/// order) and the host components it imports. Expands to the wasm exports
/// the host calls, and a manifest carrying each component's prop schema.
///
/// A prop with `= default` is optional: an app that does not send it still
/// mounts, and the component sees the default.
///
/// ```ignore
/// fn counter(title: String, external: ReadSignal<i64>, step: i64) -> Node { … }
///
/// stream_guest::bundle! {
///     components: [
///         "Counter" => counter(title: String, external: ReadSignal<i64>, step: i64 = 1),
///     ],
///     imports: ["Badge"],
/// }
/// ```
///
/// The prop list repeats the fn signature. A `#[component(streamed)]`
/// attribute would read it off the signature instead; `macro_rules` cannot.
#[macro_export]
macro_rules! bundle {
    (
        components: [
            $(
                $name:literal => $body:ident ( $( $prop:ident : $ty:ty $(= $default:expr)? ),* $(,)? )
            ),* $(,)?
        ],
        imports: [$($import:literal),* $(,)?] $(,)?
    ) => {
        const _: () = {
            #[allow(dead_code)]
            fn schemas() -> ::std::vec::Vec<$crate::ComponentSchema> {
                ::std::vec![$(
                    $crate::ComponentSchema {
                        name: ::std::string::String::from($name),
                        props: ::std::vec![$(
                            $crate::PropSchema {
                                name: ::std::string::String::from(stringify!($prop)),
                                ty: <$ty as $crate::PropType>::tag(),
                                required: !$crate::__has_default!($($default)?),
                            }
                        ),*],
                    }
                ),*]
            }

            #[allow(dead_code)]
            static COMPONENTS: &[$crate::Component] = &[$(
                |props: &mut $crate::PropsIn| -> $crate::Node {
                    $(
                        let $prop: $ty = match props.take::<$ty>(stringify!($prop)) {
                            ::core::option::Option::Some(v) => v,
                            ::core::option::Option::None => $crate::__prop_default!($prop $(, $default)?),
                        };
                    )*
                    $body($($prop),*)
                }
            ),*];

            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            extern "C" fn stream_manifest() -> u64 {
                $crate::__rt::manifest(schemas(), &[$($import),*])
            }
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            extern "C" fn stream_alloc(len: u32) -> u32 {
                $crate::__rt::alloc(len)
            }
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            extern "C" fn stream_mount(component: u32, ptr: u32, len: u32) -> u64 {
                // SAFETY: the host passes a buffer from `stream_alloc`.
                unsafe { $crate::__rt::mount(COMPONENTS, component, ptr, len) }
            }
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            extern "C" fn stream_call(callback: u32, ptr: u32, len: u32) -> u64 {
                // SAFETY: the host passes a buffer from `stream_alloc`.
                unsafe { $crate::__rt::call(callback, ptr, len) }
            }
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            extern "C" fn stream_drop(callback: u32) {
                $crate::__rt::drop_callback(callback)
            }
            #[cfg(target_arch = "wasm32")]
            #[no_mangle]
            extern "C" fn stream_live_callbacks() -> u32 {
                $crate::__rt::live_callbacks()
            }
        };
    };
}
