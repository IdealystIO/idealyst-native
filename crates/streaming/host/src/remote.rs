//! The app side of `#[component(remote)]`: the loader the components mount
//! from.
//!
//! ```ignore
//! let remote = remote_host::remote::install(include_bytes!("bundle.wasm"))?;
//! // … later, a new build of the bundle:
//! remote.reload(&new_bytes)?;   // every mounted remote component remounts
//! ```
//!
//! A remote component's fn, in the app build, sends its props and asks the
//! installed loader to mount `__idealyst_remote_<module_path>::<Name>` from the current
//! bundle (`runtime_vocabulary::remote::host::__mount_remote`).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_scene::Element;
use runtime_vocabulary::remote::host::{install_loader, Loader};
use runtime_world::Signal;

use crate::kernel::KernelBundle;
use crate::LoadError;

pub use remote_bundle::manifest::Param;
pub use remote_bundle::{KeyId, Problem, Provides, PublicKey, Requires, Trust, TrustError};

/// How a loader loads bundles: what they may call, and which it accepts.
#[derive(Default)]
pub struct Options {
    /// The `#[host_fn]`s a bundle may call — the app's allowlist,
    /// `vec![my_sdk::take_photo::export(), …]`. A bundle calling anything
    /// else is refused at load, naming it.
    pub host_fns: Vec<runtime_vocabulary::remote::HostFnDef>,
    /// Which bundles load, by signature. The default accepts any bundle;
    /// an app that downloads its bundles should trust its release key and
    /// require a signature:
    ///
    /// ```ignore
    /// trust: Trust::default().key(PublicKey::from_hex(RELEASE_KEY)?).require_signature(),
    /// ```
    ///
    /// Checked before the module is even parsed, at install and at every
    /// reload.
    pub trust: Trust,
}

/// Check `wasm` against `trust` and, when its release build recorded what
/// it requires, against what this app [`provides`]; then load it.
fn load(wasm: &[u8], options: &Options) -> Result<KernelBundle, LoadError> {
    options.trust.check(wasm).map_err(LoadError::Untrusted)?;
    let bad = |e: remote_bundle::FormatError| LoadError::Wasm(wasmi::Error::new(e.to_string()));
    if let Some(requires) = remote_bundle::requires(wasm).map_err(bad)? {
        let provides = provides(&options.host_fns);
        let codec = remote_bundle::metadata(wasm).map_err(bad)?.map_or(provides.codec, |m| m.codec);
        let errors: Vec<_> = remote_bundle::check(&requires, codec, &provides).into_iter().filter(|p| p.is_error()).collect();
        if !errors.is_empty() {
            return Err(LoadError::Incompatible(errors));
        }
    }
    KernelBundle::load_with(&engine(), wasm, &options.host_fns)
}

/// What this app offers bundles, with `host_fns` as its allowlist: every
/// app component it registered and each prop's shape, the host functions,
/// the remote components it mounts and the parameters it sends, and the
/// context types. What a bundle's requirements are checked against
/// ([`remote_bundle::check`]) — at load, or before downloading one.
pub fn provides(host_fns: &[runtime_vocabulary::remote::HostFnDef]) -> Provides {
    use runtime_vocabulary::remote::{host, host_fn};
    let pairs = |list: Vec<(&'static str, String)>| list.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    Provides {
        codec: runtime_vocabulary::remote::CODEC_VERSION,
        components: host::APP_COMPONENTS.iter().map(|c| (c.name.to_string(), pairs((c.props)()))).collect(),
        host_fns: host_fns.iter().map(|f| (host_fn::import_name(f.path, f.schema), (f.shape)())).collect(),
        remote: host::REMOTE_MOUNTS
            .iter()
            .map(|m| {
                let params = (m.params)().into_iter().map(|(name, shape)| Param { name: name.to_string(), shape }).collect();
                (m.name.to_string(), params)
            })
            .collect(),
        contexts: runtime_vocabulary::remote::REMOTE_CONTEXTS.iter().map(|c| (c.name.to_string(), (c.shape)())).collect(),
    }
}

/// The bundle [`install`] and [`RemoteApp::reload`] manage. An app with
/// one bundle never names it; one with several uses [`RemoteApp::set`].
pub const DEFAULT_BUNDLE: &str = "";

/// One named bundle.
struct Slot {
    name: String,
    bundle: Rc<KernelBundle>,
    /// What its components' mounts follow; bumped when it is replaced,
    /// removed, or stops. Created on first read, inside the app's world.
    generation: Rc<Lazy>,
}

/// A remount counter created on first read (a loader may be installed
/// outside the app's world; reads happen inside it).
///
/// Its values come from the loader's one sequence (`BundleLoader::stamp`),
/// never from a counter of its own: a component's mount is keyed by the
/// value of whichever counter it follows, and it switches counters when it
/// goes from missing (`pending`) to served (its bundle's). Two counters
/// that both started at 0 gave that switch the same key, and the
/// placeholder never remounted.
#[derive(Default)]
struct Lazy(Cell<Option<Signal<u64>>>);

impl Lazy {
    fn signal(&self, stamp: &Cell<u64>) -> Signal<u64> {
        if let Some(g) = self.0.get() {
            return g;
        }
        // Owned by no component: it lives as long as the loader.
        let first = next(stamp);
        let g = runtime_world::unscoped(|| runtime_world::signal(first));
        self.0.set(Some(g));
        g
    }

    /// Move it on, if anything has read it (nothing to remount otherwise).
    fn bump(&self, stamp: &Cell<u64>) {
        if let Some(g) = self.0.get() {
            g.set(next(stamp));
        }
    }
}

fn next(stamp: &Cell<u64>) -> u64 {
    let n = stamp.get() + 1;
    stamp.set(n);
    n
}

struct BundleLoader {
    /// What every bundle this loader loads may call, and which it accepts.
    options: Options,
    /// In the order they were first set: a component two bundles provide
    /// mounts from the earlier one.
    bundles: RefCell<Vec<Slot>>,
    /// What components no bundle provides follow: bumped when a bundle is
    /// set, so they mount from it.
    pending: Lazy,
    /// Bumped on every change (`RemoteApp::__generation`).
    changes: Cell<u64>,
    /// The sequence every remount counter takes its values from ([`Lazy`]).
    stamp: Cell<u64>,
    /// Called with a component no bundle provides, when it mounts.
    on_missing: RefCell<Option<Rc<dyn Fn(&str)>>>,
}

impl BundleLoader {
    fn new(options: Options) -> Rc<BundleLoader> {
        Rc::new(BundleLoader {
            options,
            bundles: RefCell::new(Vec::new()),
            pending: Lazy::default(),
            changes: Cell::new(0),
            stamp: Cell::new(0),
            on_missing: RefCell::new(None),
        })
    }

    /// The bundle serving `component`, and its generation.
    fn serving(&self, component: &str) -> Option<(Rc<KernelBundle>, Rc<Lazy>)> {
        self.bundles
            .borrow()
            .iter()
            .find(|s| s.bundle.components().iter().any(|c| c == component))
            .map(|s| (s.bundle.clone(), s.generation.clone()))
    }

    fn set(self: &Rc<Self>, name: &str, wasm: &[u8]) -> Result<(), String> {
        let bundle = Rc::new(load(wasm, &self.options).map_err(|e| e.to_string())?);
        let generation = {
            let mut bundles = self.bundles.borrow_mut();
            match bundles.iter_mut().find(|s| s.name == name) {
                Some(slot) => {
                    slot.bundle = bundle.clone();
                    Some(slot.generation.clone())
                }
                None => {
                    bundles.push(Slot { name: name.to_string(), bundle: bundle.clone(), generation: Rc::default() });
                    None
                }
            }
        };
        self.watch(name, &bundle);
        self.changes.set(self.changes.get() + 1);
        // Outside the borrow: a bump runs effects, which mount.
        if let Some(g) = generation {
            g.bump(&self.stamp);
        }
        self.pending.bump(&self.stamp);
        Ok(())
    }

    fn remove(&self, name: &str) -> bool {
        let removed = {
            let mut bundles = self.bundles.borrow_mut();
            bundles.iter().position(|s| s.name == name).map(|i| bundles.remove(i))
        };
        let Some(slot) = removed else { return false };
        self.changes.set(self.changes.get() + 1);
        // Its components remount: from another bundle providing them, or
        // as missing.
        slot.generation.bump(&self.stamp);
        true
    }

    /// When `bundle` panics while it is still `name`'s bundle, remount its
    /// components: the stopped bundle refuses the mounts, so each shows the
    /// panic message in its place (`__mount_remote`'s error text), and its
    /// old tree, whose callbacks can no longer reach the bundle, is torn
    /// down. A bundle a reload replaced can still panic late (a tree of it
    /// tearing down, a handler something kept); remounting the current
    /// bundle's components for that would throw away their state for
    /// nothing. Weak, so the listener doesn't keep its own bundle alive.
    fn watch(self: &Rc<Self>, name: &str, bundle: &Rc<KernelBundle>) {
        let weak = Rc::downgrade(self);
        let me = Rc::downgrade(bundle);
        let name = name.to_string();
        bundle.on_poison(move |_| {
            let Some(loader) = weak.upgrade() else { return };
            let generation = loader
                .bundles
                .borrow()
                .iter()
                .find(|s| s.name == name && Weak::ptr_eq(&me, &Rc::downgrade(&s.bundle)))
                .map(|s| s.generation.clone());
            if let Some(g) = generation {
                loader.changes.set(loader.changes.get() + 1);
                g.bump(&loader.stamp);
            }
        });
    }
}

impl Loader for BundleLoader {
    fn generation(&self) -> u64 {
        self.pending.signal(&self.stamp).get()
    }

    fn generation_of(&self, component: &str) -> u64 {
        match self.serving(component) {
            Some((_, g)) => g.signal(&self.stamp).get(),
            None => self.pending.signal(&self.stamp).get(),
        }
    }

    fn mount(&self, component: &str, args: &[u8]) -> Result<Element, String> {
        let Some((bundle, _)) = self.serving(component) else {
            let hook = self.on_missing.borrow().clone();
            return match hook {
                // Someone is fetching it: hold its place until a bundle
                // that provides it is set (`pending`).
                Some(hook) => {
                    hook(component);
                    Ok(runtime_vocabulary::glue::empty_absolute_view())
                }
                None => Err(format!("no installed bundle provides `{component}`")),
            };
        };
        let element = bundle
            .mount_remote(&format!("{}{component}", crate::kernel::REMOTE_EXPORT_PREFIX), args)
            .map_err(|e| e.to_string())?;
        // The tree calls back into the bundle that built it; keep that
        // bundle alive exactly as long as the tree, so a reload can swap the
        // current one while old trees tear down against their own.
        runtime_world::on_scope_drop(move || drop(bundle));
        Ok(element)
    }
}

/// The installed loader: its bundles, by name.
pub struct RemoteApp {
    loader: Rc<BundleLoader>,
}

/// An interpreter configured for bundles.
///
/// One per loaded bundle, never shared across reloads: a wasmi `Engine`
/// never frees the code it compiled (its code map only grows) until the
/// engine itself drops. A bundle's store holds its engine, so the code goes
/// when the last tree built from that bundle does. A shared engine kept
/// every reloaded bundle's code: ~750 KB per reload of the showcase.
pub fn engine() -> wasmi::Engine {
    let mut config = wasmi::Config::default();
    config.compilation_mode(wasmi::CompilationMode::LazyTranslation);
    wasmi::Engine::new(&config)
}

/// Load `wasm` and make it where this thread's `#[component(remote)]`
/// components mount from. The bundle may call no `#[host_fn]`s; see
/// [`install_with`].
pub fn install(wasm: &[u8]) -> Result<RemoteApp, String> {
    install_with(wasm, Vec::new())
}

/// [`install`], letting this and every later (reloaded) bundle call
/// `host_fns` — the app's allowlist, `[my_sdk::take_photo::export(), …]`.
/// A bundle calling anything else is refused at load, naming it.
pub fn install_with(wasm: &[u8], host_fns: Vec<runtime_vocabulary::remote::HostFnDef>) -> Result<RemoteApp, String> {
    install_with_options(wasm, Options { host_fns, ..Options::default() })
}

/// [`install`] with [`Options`]: the host functions bundles may call, and
/// the [`Trust`] they must pass (signatures). Every later
/// [`reload`](RemoteApp::reload) is held to the same options.
pub fn install_with_options(wasm: &[u8], options: Options) -> Result<RemoteApp, String> {
    let loader = BundleLoader::new(options);
    loader.set(DEFAULT_BUNDLE, wasm)?;
    install_loader(loader.clone());
    Ok(RemoteApp { loader })
}

/// A loader with no bundle yet: add them by name with [`RemoteApp::set`].
/// Each remote component mounts from the bundle that exports it. Until
/// one does, it shows an error in its place — or, with
/// [`RemoteApp::on_missing`], an empty placeholder while the app fetches
/// it.
pub fn install_empty(options: Options) -> RemoteApp {
    let loader = BundleLoader::new(options);
    install_loader(loader.clone());
    RemoteApp { loader }
}

impl RemoteApp {
    /// The linear memory of every bundle, in bytes
    /// ([`KernelBundle::memory_bytes`]).
    pub fn memory_bytes(&self) -> usize {
        self.loader.bundles.borrow().iter().map(|s| s.bundle.memory_bytes()).sum()
    }

    /// The first bundle's interpreter, weakly (tests: a replaced bundle's
    /// engine must go once nothing uses it).
    #[doc(hidden)]
    pub fn __engine(&self) -> wasmi::EngineWeak {
        self.loader.bundles.borrow().first().expect("a bundle").bundle.__engine()
    }

    /// How many times the loader's bundles changed (set, removed, stopped).
    /// Tests.
    #[doc(hidden)]
    pub fn __generation(&self) -> u64 {
        self.loader.changes.get()
    }

    /// Replace the bundle; every remote component it serves remounts from
    /// the new one. On `Err` (it doesn't load) the current bundle stays.
    pub fn reload(&self, wasm: &[u8]) -> Result<(), String> {
        self.loader.set(DEFAULT_BUNDLE, wasm)
    }

    /// Add bundle `name`, or replace it. The remote components it served
    /// remount from it, and so do mounted components no bundle provided
    /// before. The others keep running, state and all. Held to the same
    /// [`Options`] as every bundle: on `Err` nothing changes.
    pub fn set(&self, name: &str, wasm: &[u8]) -> Result<(), String> {
        self.loader.set(name, wasm)
    }

    /// Remove bundle `name`; its components remount from another bundle
    /// that provides them, or as missing. `false` when there is none.
    pub fn remove(&self, name: &str) -> bool {
        self.loader.remove(name)
    }

    /// The bundles' names, in the order they were added.
    pub fn bundles(&self) -> Vec<String> {
        self.loader.bundles.borrow().iter().map(|s| s.name.clone()).collect()
    }

    /// Whether a bundle provides remote component `component`
    /// (`module_path::Name`).
    pub fn provides(&self, component: &str) -> bool {
        self.loader.serving(component).is_some()
    }

    /// Call `f` with each remote component that mounts while no bundle
    /// provides it, and show an empty placeholder in its place instead of
    /// an error: an app that downloads bundles on demand fetches it, then
    /// [`set`](Self::set)s it, and the component mounts from it.
    pub fn on_missing(&self, f: impl Fn(&str) + 'static) {
        *self.loader.on_missing.borrow_mut() = Some(Rc::new(f));
    }
}
