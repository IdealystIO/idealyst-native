//! Load a streamed bundle (a wasm guest) and bind it to the app.
//!
//! # The model: one reactive graph, owned by the host
//!
//! A guest never owns reactive state. Every signal it creates is a real
//! `runtime_world` signal in the host's world, and the guest holds a `u32`
//! handle into this bundle's table. A guest effect is a real host effect
//! whose body calls back into the guest. So there is exactly one graph,
//! flushed once, glitch-free across the boundary by the kernel's own
//! argument — rather than a guest-side graph mirroring host signals through
//! proxies and lagging them by a flush.
//!
//! # Re-entrancy: why guest effects are created after the call returns
//!
//! The wasmi `Store` sits in a `RefCell`; every host→guest call borrows it.
//! A kernel effect runs its body immediately on creation, and a guest
//! effect's body is a guest call — so creating one *inside* the guest call
//! that asked for it would borrow the store twice. `effect_new` therefore
//! queues the callback, and [`Bundle`] creates the queued effects as soon as
//! the outer call returns, in the same reactive scope. The first run moves
//! from "inside the component body" to "right after it", which nothing can
//! observe: the body's own signal writes are staged until flush anyway.
//!
//! Everything else a guest can do mid-call is staging (`set`, `update`) or
//! allocation (`signal_new`), and none of it runs an effect synchronously.
//! The one path that does call back into the guest mid-call —
//! `signal_update`, which must apply the guest's closure to the STAGED
//! value — re-enters through the wasmi `Caller` it already holds instead of
//! the `RefCell`. A host flush during a guest call would break this; it
//! panics with a diagnostic rather than deadlocking (see [`Inner::store`]).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::Element;
use runtime_vocabulary::builders::{button, text, view};
use runtime_world::{ReadSignal, Signal, Value};
use rustc_hash::FxHashMap;
use stream_abi::host_fn::{AsyncCall, HostFnDef, HostFnKind, HOST_FN_MODULE};
use stream_abi::{exports, imports, Content, Manifest, Node, PropEntry, Wire, ABI_VERSION, IMPORT_MODULE};
use wasmi::{AsContextMut, Caller, CompilationMode, Config, ExternType, Linker, Memory, Module, Store, TypedFunc, Val, ValType};

pub use stream_abi;

/// The kernel bridge over wasm (host side): a bundle's reactive kernel on
/// this app's graph.
pub mod kernel;

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// A configured wasmi engine plus the linker carrying the host imports.
/// Build one per app and load every bundle through it.
pub struct StreamEngine {
    engine: wasmi::Engine,
    linker: Linker<GuestState>,
}

impl StreamEngine {
    /// wasmi's default: validate eagerly, translate each function to wasmi
    /// bytecode the first time it runs. Load cost scales with what the
    /// bundle executes, not with its size.
    pub fn new() -> Self {
        Self::with_mode(CompilationMode::LazyTranslation)
    }

    pub fn with_mode(mode: CompilationMode) -> Self {
        let mut config = Config::default();
        config.compilation_mode(mode);
        let engine = wasmi::Engine::new(&config);
        let mut linker = Linker::new(&engine);
        define_imports(&mut linker);
        StreamEngine { engine, linker }
    }
}

impl Default for StreamEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Host exports — components a bundle may mount by name
// ---------------------------------------------------------------------------

type HostComponent = Rc<dyn Fn(&mut GuestProps) -> Element>;

/// The host components this app binary offers to bundles, by stable name.
///
/// Names, not `TypeId`s: a `TypeId` is only meaningful inside one compiled
/// binary, and the whole point is that the bundle was compiled separately.
#[derive(Default, Clone)]
pub struct HostExports {
    map: FxHashMap<String, HostComponent>,
    host_fns: FxHashMap<&'static str, HostFnDef>,
}

impl HostExports {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn export(mut self, name: &str, f: impl Fn(&mut GuestProps) -> Element + 'static) -> Self {
        self.map.insert(name.to_string(), Rc::new(f));
        self
    }

    /// Let bundles call a `#[host_fn]`: `.host_fn(camera::take_photo::export())`.
    /// This list is the allowlist — a bundle that calls anything else is
    /// refused at load, so an OTA update cannot start using a capability
    /// the app never offered it.
    pub fn host_fn(mut self, def: HostFnDef) -> Self {
        self.host_fns.insert(def.path, def);
        self
    }
}

/// Positional props a guest passed to a host component.
pub struct GuestProps<'a> {
    bytes: &'a [u8],
    bundle: &'a Rc<Inner>,
    component: &'a str,
}

impl GuestProps<'_> {
    fn next<T: Wire>(&mut self, what: &str) -> T {
        T::decode(&mut self.bytes).unwrap_or_else(|| {
            panic!(
                "stream-host: bundle passed host component `{}` props that end or \
                 mismatch where {what} was expected",
                self.component
            )
        })
    }

    pub fn string(&mut self) -> String {
        self.next("a string")
    }

    /// Text that is fixed or recomputed by a guest closure.
    pub fn content(&mut self) -> Value<String> {
        let c: Content = self.next("text content");
        self.bundle.content(c)
    }

    /// A guest action (e.g. a press handler).
    pub fn action(&mut self) -> impl Fn() + 'static {
        let f = GuestFn::new(self.bundle, self.next("an action callback"));
        move || {
            f.call(&[]);
        }
    }
}

// ---------------------------------------------------------------------------
// Handle table — host signals the guest can address
// ---------------------------------------------------------------------------

/// Maps the encoded staged value to the encoded new value; an `Err` is a
/// guest trap, surfaced from the update instead of committing garbage.
type UpdateFn<'a> = &'a mut dyn FnMut(&[u8]) -> Result<Vec<u8>, wasmi::Error>;

/// A signal seen as bytes. The guest encodes and decodes; the host stores
/// guest-created signals as raw bytes and adapts typed host signals.
trait ByteSignal {
    /// Tracked read.
    fn read(&self, out: &mut Vec<u8>);
    fn write(&self, bytes: &[u8]) -> Result<(), String>;
    /// Staged read-modify-write.
    fn update(&self, f: UpdateFn<'_>) -> Result<(), wasmi::Error>;
}

/// A guest-created signal. Its value is the guest's encoding, so the host
/// never needs the type.
struct Raw(Signal<Vec<u8>>);

impl ByteSignal for Raw {
    fn read(&self, out: &mut Vec<u8>) {
        self.0.with(|v| out.extend_from_slice(v));
    }
    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        self.0.set(bytes.to_vec());
        Ok(())
    }
    fn update(&self, f: UpdateFn<'_>) -> Result<(), wasmi::Error> {
        let mut err = None;
        self.0.update(|old| match f(old) {
            Ok(new) => new,
            Err(e) => {
                err = Some(e);
                old.clone()
            }
        });
        err.map_or(Ok(()), Err)
    }
}

/// A typed host signal handed to the guest as a two-way prop.
struct Typed<T>(Signal<T>);

impl<T: Wire + PartialEq + 'static> ByteSignal for Typed<T> {
    fn read(&self, out: &mut Vec<u8>) {
        self.0.with(|v| v.encode(out));
    }
    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let v = T::from_bytes(bytes)
            .ok_or_else(|| format!("value does not decode as {}", std::any::type_name::<T>()))?;
        self.0.set(v);
        Ok(())
    }
    fn update(&self, f: UpdateFn<'_>) -> Result<(), wasmi::Error> {
        let mut err = None;
        self.0.update(|old| {
            let decoded = f(&old.to_bytes()).and_then(|new| {
                T::from_bytes(&new).ok_or_else(|| {
                    wasmi::Error::new(format!(
                        "update result does not decode as {}",
                        std::any::type_name::<T>()
                    ))
                })
            });
            match decoded {
                Ok(v) => v,
                Err(e) => {
                    err = Some(e);
                    // Re-encode → decode is the cheapest way back to an owned
                    // `T` without requiring `T: Clone`.
                    T::from_bytes(&old.to_bytes()).expect("Wire round-trip of the old value")
                }
            }
        });
        err.map_or(Ok(()), Err)
    }
}

/// A typed host signal handed to the guest as a read-only prop. The guest's
/// `ReadSignal` has no setter; this refuses a guest that forges a write.
struct ReadOnly<T>(ReadSignal<T>);

impl<T: Wire + PartialEq + 'static> ByteSignal for ReadOnly<T> {
    fn read(&self, out: &mut Vec<u8>) {
        self.0.with(|v| v.encode(out));
    }
    fn write(&self, _: &[u8]) -> Result<(), String> {
        Err("the handle is read-only".into())
    }
    fn update(&self, _: UpdateFn<'_>) -> Result<(), wasmi::Error> {
        Err(wasmi::Error::new("the handle is read-only"))
    }
}

#[derive(Default)]
struct Tables {
    /// Handles are never reused: a stale guest handle must not alias a later
    /// signal (the generational-slot bug class the kernel guards against
    /// with generations). A `u32` counter is plenty for a spike; production
    /// would add a generation and reuse slots.
    signals: RefCell<FxHashMap<u32, Rc<dyn ByteSignal>>>,
    next_handle: std::cell::Cell<u32>,
    /// Guest effects requested during the current call; see the module docs.
    pending_effects: RefCell<Vec<u32>>,
    /// Async host-function calls requested during the current guest call:
    /// `(body, encoded args, guest then-callback)`. Started when the call
    /// returns, for the same reason as `pending_effects` — a call that
    /// completes synchronously would re-enter the guest.
    pending_spawns: RefCell<Vec<(AsyncCall, Vec<u8>, u32)>>,
    /// Guest callbacks the host released while the store may be borrowed;
    /// forwarded to the guest at the start of the next call.
    pending_drops: RefCell<Vec<u32>>,
}

impl Tables {
    /// Register `sig` and release the handle when the enclosing reactive
    /// scope tears down (the component that is mounting).
    fn insert_scoped(self: &Rc<Self>, sig: Rc<dyn ByteSignal>) -> u32 {
        let h = self.next_handle.get();
        self.next_handle.set(h.checked_add(1).expect("stream-host: signal handle space exhausted"));
        self.signals.borrow_mut().insert(h, sig);
        let tables = self.clone();
        runtime_world::on_scope_drop(move || {
            tables.signals.borrow_mut().remove(&h);
        });
        h
    }

    fn get(&self, h: u32) -> Result<Rc<dyn ByteSignal>, wasmi::Error> {
        self.signals.borrow().get(&h).cloned().ok_or_else(|| {
            wasmi::Error::new(format!(
                "guest used signal handle {h}, which is unknown or whose scope has torn down"
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Guest state + imports
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Api {
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    mount: TypedFunc<(u32, u32, u32), u64>,
    call: TypedFunc<(u32, u32, u32), u64>,
    drop: TypedFunc<u32, ()>,
    live: TypedFunc<(), u32>,
}

struct GuestState {
    tables: Rc<Tables>,
    /// `None` only between instantiation and the export lookup in
    /// [`Bundle::load`]; no guest code that touches imports runs then.
    api: Option<Api>,
    /// Host-side buffer `signal_get` encodes into.
    scratch: Vec<u8>,
}

fn api(caller: &Caller<'_, GuestState>) -> Result<Api, wasmi::Error> {
    caller.data().api.ok_or_else(|| wasmi::Error::new("guest called an import before load finished"))
}

fn read_mem(ctx: impl wasmi::AsContext, memory: Memory, ptr: u32, len: u32) -> Result<Vec<u8>, wasmi::Error> {
    let mut buf = vec![0u8; len as usize];
    memory
        .read(ctx, ptr as usize, &mut buf)
        .map_err(|_| wasmi::Error::new(format!("guest pointer {ptr}+{len} is out of bounds")))?;
    Ok(buf)
}

/// Copy `args` into guest memory, invoke guest callback `cb`, copy the
/// result out. Generic over the context so `signal_update` can re-enter
/// through its `Caller` while the store is already borrowed.
fn guest_call(mut ctx: impl AsContextMut<Data = GuestState>, api: Api, cb: u32, args: &[u8]) -> Result<Vec<u8>, wasmi::Error> {
    let ptr = api.alloc.call(&mut ctx, args.len() as u32)?;
    api.memory
        .write(&mut ctx, ptr as usize, args)
        .map_err(|_| wasmi::Error::new("stream_alloc returned an out-of-bounds buffer"))?;
    let (rptr, rlen) = stream_abi::unpack(api.call.call(&mut ctx, (cb, ptr, args.len() as u32))?);
    read_mem(&ctx, api.memory, rptr, rlen)
}

fn define_imports(linker: &mut Linker<GuestState>) {
    let m = IMPORT_MODULE;
    linker
        .func_wrap(m, imports::SIGNAL_NEW, |caller: Caller<'_, GuestState>, ptr: u32, len: u32| -> Result<u32, wasmi::Error> {
            let bytes = read_mem(&caller, api(&caller)?.memory, ptr, len)?;
            let tables = caller.data().tables.clone();
            Ok(tables.insert_scoped(Rc::new(Raw(runtime_world::signal(bytes)))))
        })
        .expect("define signal_new");
    linker
        .func_wrap(
            m,
            imports::SIGNAL_GET,
            |mut caller: Caller<'_, GuestState>, h: u32, buf: u32, cap: u32| -> Result<u32, wasmi::Error> {
                let sig = caller.data().tables.get(h)?;
                // Reused across reads: a read is the hottest crossing, and a
                // fresh Vec per read was measured as a large share of it.
                let mut out = std::mem::take(&mut caller.data_mut().scratch);
                out.clear();
                sig.read(&mut out);
                let len = out.len() as u32;
                let result = if out.len() <= cap as usize {
                    let memory = api(&caller)?.memory;
                    memory
                        .write(&mut caller, buf as usize, &out)
                        .map_err(|_| wasmi::Error::new("signal_get buffer is out of bounds"))
                } else {
                    Ok(())
                };
                caller.data_mut().scratch = out;
                result.map(|()| len)
            },
        )
        .expect("define signal_get");
    linker
        .func_wrap(m, imports::SIGNAL_SET, |caller: Caller<'_, GuestState>, h: u32, ptr: u32, len: u32| -> Result<(), wasmi::Error> {
            let bytes = read_mem(&caller, api(&caller)?.memory, ptr, len)?;
            caller.data().tables.get(h)?.write(&bytes).map_err(|e| wasmi::Error::new(format!("signal_set on handle {h}: {e}")))
        })
        .expect("define signal_set");
    linker
        .func_wrap(m, imports::SIGNAL_UPDATE, |mut caller: Caller<'_, GuestState>, h: u32, cb: u32| -> Result<(), wasmi::Error> {
            let sig = caller.data().tables.get(h)?;
            let api = api(&caller)?;
            sig.update(&mut |old| guest_call(&mut caller, api, cb, old))
        })
        .expect("define signal_update");
    linker
        .func_wrap(m, imports::EFFECT_NEW, |caller: Caller<'_, GuestState>, cb: u32| {
            caller.data().tables.pending_effects.borrow_mut().push(cb);
        })
        .expect("define effect_new");
}

// ---------------------------------------------------------------------------
// Bundle
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum LoadError {
    /// Not valid wasm, or instantiation failed (e.g. it imports a host
    /// function this ABI does not define).
    Wasm(wasmi::Error),
    MissingExport(&'static str),
    BadManifest,
    AbiMismatch { bundle: u32, host: u32 },
    /// The bundle calls host functions this app does not export (or does
    /// not allow bundles to call).
    MissingHostFunctions(Vec<String>),
    /// Both sides have the host function, with different signatures.
    IncompatibleHostFunctions(Vec<HostFnMismatch>),
    /// The bundle mounts host components this app binary does not export.
    /// The "old binary, new bundle" case — refused before any component
    /// body runs.
    MissingHostComponents(Vec<String>),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Wasm(e) => write!(f, "bundle failed to load: {e}"),
            LoadError::MissingExport(name) => write!(f, "bundle does not export `{name}`"),
            LoadError::BadManifest => write!(f, "bundle manifest does not decode"),
            LoadError::AbiMismatch { bundle, host } => {
                write!(f, "bundle targets stream ABI {bundle}, this app speaks {host}")
            }
            LoadError::MissingHostFunctions(names) => {
                write!(f, "bundle calls host functions this app does not export: {}", names.join(", "))
            }
            LoadError::IncompatibleHostFunctions(list) => {
                write!(f, "bundle calls host functions whose signature changed: ")?;
                for (i, m) in list.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} (app {:016x}, bundle {:016x})", m.path, m.app_schema, m.bundle_schema)?;
                }
                Ok(())
            }
            LoadError::MissingHostComponents(names) => write!(
                f,
                "bundle needs host components this app does not export: {}",
                names.join(", ")
            ),
        }
    }
}

impl std::error::Error for LoadError {}

/// A host function both sides have, with different signature fingerprints.
#[derive(Debug, Clone, PartialEq)]
pub struct HostFnMismatch {
    pub path: String,
    pub app_schema: u64,
    pub bundle_schema: u64,
}

/// Check every host-function import against `exports`, and define the ones
/// that pass on `linker`. Runs before instantiation, so a refused bundle
/// never executes.
fn link_host_fns(module: &Module, exports: &HostExports, linker: &mut Linker<GuestState>) -> Result<(), LoadError> {
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    for import in module.imports() {
        if import.module() != HOST_FN_MODULE {
            continue;
        }
        let name = import.name();
        let Some((path, bundle_schema)) = stream_abi::host_fn::parse_import_name(name) else {
            missing.push(name.to_string());
            continue;
        };
        let Some(def) = exports.host_fns.get(path) else {
            missing.push(path.to_string());
            continue;
        };
        let ExternType::Func(ty) = import.ty() else {
            missing.push(path.to_string());
            continue;
        };
        // The schema covers asyncness, so equal schemas imply the import's
        // shape matches the kind; the shape check is belt and braces for a
        // hand-written import.
        let shape_ok = match def.kind {
            HostFnKind::Sync(_) => ty.params() == [ValType::I32, ValType::I32] && ty.results() == [ValType::I64],
            HostFnKind::Async(_) => ty.params() == [ValType::I32, ValType::I32, ValType::I32] && ty.results().is_empty(),
        };
        if def.schema != bundle_schema || !shape_ok {
            mismatched.push(HostFnMismatch { path: path.to_string(), app_schema: def.schema, bundle_schema });
            continue;
        }
        let path_owned: &'static str = def.path;
        let defined = match def.kind {
            HostFnKind::Sync(f) => linker.func_new(HOST_FN_MODULE, name, ty.clone(), move |mut caller, params, results| {
                let (ptr, len) = (params[0].i32().unwrap_or(0) as u32, params[1].i32().unwrap_or(0) as u32);
                let api = api(&caller)?;
                let args = read_mem(&caller, api.memory, ptr, len)?;
                let out = f(&args);
                let rptr = api.alloc.call(&mut caller, out.len() as u32)?;
                api.memory
                    .write(&mut caller, rptr as usize, &out)
                    .map_err(|_| wasmi::Error::new(format!("host_fn {path_owned}: stream_alloc returned an out-of-bounds buffer")))?;
                results[0] = Val::I64(stream_abi::pack(rptr, out.len() as u32) as i64);
                Ok(())
            }),
            HostFnKind::Async(f) => linker.func_new(HOST_FN_MODULE, name, ty.clone(), move |caller, params, _| {
                let (ptr, len) = (params[0].i32().unwrap_or(0) as u32, params[1].i32().unwrap_or(0) as u32);
                let then = params[2].i32().unwrap_or(0) as u32;
                let args = read_mem(&caller, api(&caller)?.memory, ptr, len)?;
                caller.data().tables.pending_spawns.borrow_mut().push((f, args, then));
                Ok(())
            }),
        };
        defined.expect("each host-function import name is unique within a module");
    }
    if !missing.is_empty() {
        return Err(LoadError::MissingHostFunctions(missing));
    }
    if !mismatched.is_empty() {
        return Err(LoadError::IncompatibleHostFunctions(mismatched));
    }
    Ok(())
}

impl From<wasmi::Error> for LoadError {
    fn from(e: wasmi::Error) -> Self {
        LoadError::Wasm(e)
    }
}

/// A loaded bundle: one wasm instance, shared by every mount of every
/// component it exports.
#[derive(Clone)]
pub struct Bundle {
    inner: Rc<Inner>,
}

pub struct Inner {
    /// Borrowed for the duration of each host→guest call. A failed borrow
    /// means something re-entered the guest through the host — see the
    /// module docs — and is reported as such instead of a bare BorrowMut.
    store: RefCell<Store<GuestState>>,
    instance: wasmi::Instance,
    api: Api,
    tables: Rc<Tables>,
    exports: HostExports,
    manifest: Manifest,
}

/// A guest closure the host holds. Dropping it releases the guest slot —
/// lazily, because a drop can happen while the store is borrowed.
struct GuestFn {
    id: u32,
    bundle: Rc<Inner>,
}

impl GuestFn {
    fn new(bundle: &Rc<Inner>, id: u32) -> Rc<Self> {
        Rc::new(GuestFn { id, bundle: bundle.clone() })
    }
    fn call(&self, args: &[u8]) -> Vec<u8> {
        self.bundle.call(self.id, args)
    }
}

impl Drop for GuestFn {
    fn drop(&mut self) {
        self.bundle.tables.pending_drops.borrow_mut().push(self.id);
    }
}

impl Bundle {
    /// Validate, instantiate, and check the manifest against `exports`.
    pub fn load(engine: &StreamEngine, wasm: &[u8], exports: HostExports) -> Result<Bundle, LoadError> {
        let module = Module::new(&engine.engine, wasm)?;
        let tables = Rc::new(Tables::default());
        let mut store = Store::new(&engine.engine, GuestState { tables: tables.clone(), api: None, scratch: Vec::new() });
        let mut linker = engine.linker.clone();
        link_host_fns(&module, &exports, &mut linker)?;
        let instance = linker.instantiate_and_start(&mut store, &module)?;

        macro_rules! func {
            ($name:expr) => {
                instance.get_typed_func(&store, $name).map_err(|_| LoadError::MissingExport($name))?
            };
        }
        let api = Api {
            memory: instance.get_memory(&store, "memory").ok_or(LoadError::MissingExport("memory"))?,
            alloc: func!(exports::ALLOC),
            mount: func!(exports::MOUNT),
            call: func!(exports::CALL),
            drop: func!(exports::DROP),
            live: func!(exports::LIVE_CALLBACKS),
        };
        let manifest_fn: TypedFunc<(), u64> = func!(exports::MANIFEST);
        let (ptr, len) = stream_abi::unpack(manifest_fn.call(&mut store, ())?);
        let manifest = Manifest::from_bytes(&read_mem(&store, api.memory, ptr, len)?).ok_or(LoadError::BadManifest)?;
        if manifest.abi != ABI_VERSION {
            return Err(LoadError::AbiMismatch { bundle: manifest.abi, host: ABI_VERSION });
        }
        let missing: Vec<String> =
            manifest.imports.iter().filter(|n| !exports.map.contains_key(*n)).cloned().collect();
        if !missing.is_empty() {
            return Err(LoadError::MissingHostComponents(missing));
        }
        store.data_mut().api = Some(api);

        Ok(Bundle { inner: Rc::new(Inner { store: RefCell::new(store), instance, api, tables, exports, manifest }) })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.inner.manifest
    }

    /// Props for a mount of one of this bundle's components.
    pub fn props(&self) -> HostProps {
        HostProps::new()
    }

    /// Can component `name` take `props`? The same check [`try_mount`]
    /// runs, without mounting — what an app runs against a freshly fetched
    /// bundle BEFORE swapping it in, so an incompatible update never
    /// replaces a working one.
    ///
    /// [`try_mount`]: Self::try_mount
    pub fn check(&self, name: &str, props: &HostProps) -> Result<(), MountError> {
        self.plan(name, props).map(|_| ())
    }

    fn plan(&self, name: &str, props: &HostProps) -> Result<(usize, Vec<Planned>), MountError> {
        let (index, schema) = self
            .inner
            .manifest
            .component(name)
            .ok_or_else(|| MountError::NoSuchComponent { component: name.to_string() })?;
        let mut planned = Vec::new();
        let mut problems = Vec::new();
        for want in &schema.props {
            match props.items.iter().position(|p| p.name == want.name) {
                None if want.required => {
                    problems.push(PropProblem::MissingRequired { prop: want.name.clone(), ty: want.ty.clone() })
                }
                // Optional and not sent: the guest's default applies.
                None => {}
                Some(i) => match fits(&want.ty, &props.items[i].ty) {
                    Some(narrow) => planned.push(Planned { item: i, narrow }),
                    None => problems.push(PropProblem::TypeChanged {
                        prop: want.name.clone(),
                        app: props.items[i].ty.clone(),
                        bundle: want.ty.clone(),
                    }),
                },
            }
        }
        // A prop the app sends that the bundle no longer declares is not a
        // problem: the component stopped needing it, and dropping it is what
        // a native call site would do after the same change.
        if problems.is_empty() {
            Ok((index, planned))
        } else {
            Err(MountError::IncompatibleProps { component: name.to_string(), problems })
        }
    }

    /// Mount component `name`, or say precisely why its props no longer
    /// fit — the streamed-component analogue of a server function's
    /// `IncompatibleVersion`: the app decides what to render instead.
    ///
    /// Must run inside the host world (a component body, or `world.enter`),
    /// like any element construction: the guest's signals and effects are
    /// owned by the scope this creates.
    pub fn try_mount(&self, name: &str, props: HostProps) -> Result<Element, MountError> {
        let (index, planned) = self.plan(name, &props)?;
        let inner = self.inner.clone();
        let name = name.to_string();
        Ok(runtime_scene::component_scope(move || {
            let mut entries = Vec::with_capacity(planned.len());
            for Planned { item, narrow } in planned {
                let prop = &props.items[item];
                let bytes = match &prop.value {
                    PropValue::Bytes(b) => b.clone(),
                    PropValue::Read(sig) => inner.tables.insert_scoped(sig.clone()).to_bytes(),
                    PropValue::Write { rw, ro } => {
                        // The bundle asked for a ReadSignal and the app offered
                        // a Signal: register the read-only view, so the
                        // narrowing is enforced, not just typed.
                        let sig = if narrow { ro.clone() } else { rw.clone() };
                        inner.tables.insert_scoped(sig).to_bytes()
                    }
                };
                entries.push(PropEntry { name: prop.name.clone(), bytes });
            }
            let bytes = entries.to_bytes();
            let out = inner.with_store(|store, api| {
                let ptr = api.alloc.call(&mut *store, bytes.len() as u32)?;
                api.memory
                    .write(&mut *store, ptr as usize, &bytes)
                    .map_err(|_| wasmi::Error::new("stream_alloc returned an out-of-bounds buffer"))?;
                let (rptr, rlen) = stream_abi::unpack(api.mount.call(&mut *store, (index as u32, ptr, bytes.len() as u32))?);
                read_mem(&*store, api.memory, rptr, rlen)
            });
            let out = out.unwrap_or_else(|e| panic!("stream-host: component `{name}` trapped while mounting: {e}"));
            inner.run_pending();
            let node = Node::from_bytes(&out)
                .unwrap_or_else(|| panic!("stream-host: component `{name}` returned an undecodable description"));
            inner.build(node)
        }))
    }

    /// [`try_mount`](Self::try_mount) for a mount the app has already
    /// [`check`](Self::check)ed; panics with the mismatch otherwise.
    pub fn mount(&self, name: &str, props: HostProps) -> Element {
        self.try_mount(name, props).unwrap_or_else(|e| panic!("stream-host: {e}"))
    }

    /// Guest callbacks still alive in the guest. Forwards pending releases
    /// first, so the count reflects every handle the host has dropped.
    pub fn live_callbacks(&self) -> u32 {
        self.inner
            .with_store(|store, api| api.live.call(&mut *store, ()))
            .unwrap_or_else(|e| panic!("stream-host: stream_live_callbacks trapped: {e}"))
    }

    /// Signal handles the guest can currently address. Drops back as each
    /// mounted component's scope tears down; a leak probe for tests.
    pub fn live_handles(&self) -> usize {
        self.inner.tables.signals.borrow().len()
    }

    #[doc(hidden)]
    /// Measurement hook: register a host signal under a handle without a
    /// reactive scope, for a guest benchmark export to read.
    pub fn __bench_handle<T: Wire + PartialEq + 'static>(&self, sig: ReadSignal<T>) -> u32 {
        let t = &self.inner.tables;
        let h = t.next_handle.get();
        t.next_handle.set(h + 1);
        t.signals.borrow_mut().insert(h, Rc::new(ReadOnly(sig)));
        h
    }

    #[doc(hidden)]
    /// Measurement hook: call a guest export of shape `(u32, u32) -> i64`.
    pub fn __call_export(&self, export: &str, a: u32, b: u32) -> i64 {
        self.inner
            .with_store(|store, _| {
                let f: TypedFunc<(u32, u32), i64> = self.inner.instance.get_typed_func(&*store, export)?;
                f.call(&mut *store, (a, b))
            })
            .unwrap_or_else(|e| panic!("stream-host: export `{export}` failed: {e}"))
    }
}

impl Inner {
    fn with_store<R>(
        &self,
        f: impl FnOnce(&mut Store<GuestState>, Api) -> Result<R, wasmi::Error>,
    ) -> Result<R, wasmi::Error> {
        let mut store = self.store.try_borrow_mut().unwrap_or_else(|_| {
            panic!(
                "stream-host: re-entered the guest while it was already running — something \
                 ran a guest effect or callback synchronously from inside a guest call \
                 (e.g. a host flush during a guest call)"
            )
        });
        let drops: Vec<u32> = std::mem::take(&mut *self.tables.pending_drops.borrow_mut());
        for id in drops {
            self.api.drop.call(&mut *store, id)?;
        }
        f(&mut store, self.api)
    }

    /// Invoke guest callback `cb` from outside any guest call.
    fn call(self: &Rc<Self>, cb: u32, args: &[u8]) -> Vec<u8> {
        let out = self
            .with_store(|store, api| guest_call(&mut *store, api, cb, args))
            .unwrap_or_else(|e| panic!("stream-host: guest callback {cb} trapped: {e}"));
        self.run_pending();
        out
    }

    /// Create the effects and start the async host calls the guest asked
    /// for during the call that just returned — in the current reactive
    /// context, which is what scopes them (see the module docs).
    fn run_pending(self: &Rc<Self>) {
        loop {
            let effects: Vec<u32> = std::mem::take(&mut *self.tables.pending_effects.borrow_mut());
            let spawns = std::mem::take(&mut *self.tables.pending_spawns.borrow_mut());
            if effects.is_empty() && spawns.is_empty() {
                break;
            }
            for cb in effects {
                let f = GuestFn::new(self, cb);
                runtime_world::effect(move || {
                    f.call(&[]);
                });
            }
            for (body, args, then) in spawns {
                // The framework's own `spawn_then`: it captures the scope
                // token HERE (the component being built, or the handler
                // being run), the IO always completes, and the guest's
                // `then` runs only if that scope is still alive. A dead
                // scope drops `then` unrun, which releases the guest slot.
                let then = GuestFn::new(self, then);
                runtime_vocabulary::scoped_spawn::spawn_then(body(args), move |result: Vec<u8>| {
                    then.call(&result);
                });
            }
        }
    }

    fn content(self: &Rc<Self>, c: Content) -> Value<String> {
        match c {
            Content::Static(s) => Value::Const(s),
            Content::Dyn(cb) => {
                let f = GuestFn::new(self, cb);
                Value::Dyn(Box::new(move || {
                    String::from_bytes(&f.call(&[]))
                        .unwrap_or_else(|| panic!("stream-host: text callback {cb} returned a non-string"))
                }))
            }
        }
    }

    fn build(self: &Rc<Self>, node: Node) -> Element {
        match node {
            Node::View(children) => view().children(children.into_iter().map(|c| self.build(c)).collect()).build(),
            Node::Text(c) => text().content(self.content(c)).build(),
            Node::Button { label, on_press } => {
                let f = GuestFn::new(self, on_press);
                button()
                    .label(self.content(label))
                    .on_press(move || {
                        f.call(&[]);
                    })
                    .build()
            }
            Node::Host { name, props } => {
                if !self.manifest.imports.contains(&name) {
                    panic!(
                        "stream-host: bundle mounted host component `{name}` without declaring it \
                         in its imports — the load-time check cannot have covered it"
                    );
                }
                let export = self.exports.map.get(&name).cloned().expect("checked at load");
                export(&mut GuestProps { bytes: &props, bundle: self, component: &name })
            }
        }
    }
}

/// Does a bundle prop typed `bundle` accept an app prop typed `app`?
/// `Some(narrow)` when it does; `narrow` means the app offered a two-way
/// `Signal` where the bundle wants a `ReadSignal` — accepted, because a
/// read-only view of a signal is always available, the same way a native
/// call site can pass `sig.read_only()`. The reverse widens a capability
/// the app never granted, so it does not fit.
fn fits(bundle: &str, app: &str) -> Option<bool> {
    if bundle == app {
        return Some(false);
    }
    let inner = app.strip_prefix("Signal<")?.strip_suffix('>')?;
    (stream_abi::read_signal_tag(inner) == bundle).then_some(true)
}

/// One prop the bundle accepts, matched to the app prop that fills it.
struct Planned {
    item: usize,
    narrow: bool,
}

/// What is wrong with one prop.
#[derive(Debug, Clone, PartialEq)]
pub enum PropProblem {
    /// The bundle's component requires a prop this app does not send.
    MissingRequired { prop: String, ty: String },
    /// Both sides have the prop, and the types do not fit.
    TypeChanged { prop: String, app: String, bundle: String },
}

impl std::fmt::Display for PropProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropProblem::MissingRequired { prop, ty } => {
                write!(f, "new required prop `{prop}: {ty}` that this app does not send")
            }
            PropProblem::TypeChanged { prop, app, bundle } => {
                write!(f, "prop `{prop}` is now `{bundle}`, this app sends `{app}`")
            }
        }
    }
}

/// Why a mount was refused. Nothing ran in the guest.
#[derive(Debug, Clone, PartialEq)]
pub enum MountError {
    NoSuchComponent { component: String },
    IncompatibleProps { component: String, problems: Vec<PropProblem> },
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::NoSuchComponent { component } => write!(f, "bundle has no component `{component}`"),
            MountError::IncompatibleProps { component, problems } => {
                write!(f, "`{component}` is incompatible with this app: ")?;
                for (i, p) in problems.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{p}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for MountError {}

#[derive(Clone)]
enum PropValue {
    Bytes(Vec<u8>),
    Read(Rc<dyn ByteSignal>),
    /// Both views of a two-way signal; [`fits`] decides which one the
    /// guest gets.
    Write { rw: Rc<dyn ByteSignal>, ro: Rc<dyn ByteSignal> },
}

#[derive(Clone)]
struct HostProp {
    name: String,
    ty: String,
    value: PropValue,
}

/// Named props for [`Bundle::try_mount`]. Cloneable so an app can
/// [`check`](Bundle::check) a new bundle against the exact props it will
/// mount with.
#[derive(Clone, Default)]
pub struct HostProps {
    items: Vec<HostProp>,
}

impl HostProps {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(mut self, name: &str, ty: String, value: PropValue) -> Self {
        assert!(
            !self.items.iter().any(|p| p.name == name),
            "stream-host: prop `{name}` passed twice"
        );
        self.items.push(HostProp { name: name.to_string(), ty, value });
        self
    }

    pub fn value<T: Wire>(self, name: &str, v: T) -> Self {
        self.push(name, T::type_tag(), PropValue::Bytes(v.to_bytes()))
    }

    /// Hand the guest a read-only view of a host signal.
    pub fn read_signal<T: Wire + PartialEq + 'static>(self, name: &str, sig: ReadSignal<T>) -> Self {
        self.push(name, stream_abi::read_signal_tag(&T::type_tag()), PropValue::Read(Rc::new(ReadOnly(sig))))
    }

    /// Hand the guest a two-way host signal. A bundle that declares the
    /// prop as `ReadSignal` gets only the read-only view.
    pub fn signal<T: Wire + PartialEq + 'static>(self, name: &str, sig: Signal<T>) -> Self {
        self.push(
            name,
            stream_abi::signal_tag(&T::type_tag()),
            PropValue::Write { rw: Rc::new(Typed(sig)), ro: Rc::new(ReadOnly(sig.read_only())) },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fits;

    #[test]
    fn identical_types_fit() {
        assert_eq!(fits("String", "String"), Some(false));
        assert_eq!(fits("Signal<i64>", "Signal<i64>"), Some(false));
    }

    #[test]
    fn a_two_way_signal_narrows_to_a_read_signal() {
        assert_eq!(fits("ReadSignal<i64>", "Signal<i64>"), Some(true));
        // ...but only for the same value type.
        assert_eq!(fits("ReadSignal<i64>", "Signal<f64>"), None);
    }

    #[test]
    fn a_read_signal_never_widens_and_values_never_convert() {
        assert_eq!(fits("Signal<i64>", "ReadSignal<i64>"), None);
        assert_eq!(fits("i64", "i32"), None);
        assert_eq!(fits("ReadSignal<i64>", "i64"), None);
    }
}
