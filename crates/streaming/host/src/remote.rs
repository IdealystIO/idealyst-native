//! The app side of `#[component(remote)]`: the loader the components mount
//! from.
//!
//! ```ignore
//! let remote = stream_host::remote::install(include_bytes!("bundle.wasm"))?;
//! // … later, a new build of the bundle:
//! remote.reload(&new_bytes)?;   // every mounted remote component remounts
//! ```
//!
//! A remote component's fn, in the app build, sends its props and asks the
//! installed loader to mount `__idealyst_remote_<Name>` from the current
//! bundle (`runtime_vocabulary::remote::host::__mount_remote`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_scene::Element;
use runtime_vocabulary::remote::host::{install_loader, Loader};
use runtime_world::Signal;

use crate::kernel::KernelBundle;

struct BundleLoader {
    engine: wasmi::Engine,
    current: RefCell<Rc<KernelBundle>>,
    /// Bumped on reload. Created on first read, inside the app's world.
    generation: Cell<Option<Signal<u64>>>,
}

impl BundleLoader {
    fn generation_signal(&self) -> Signal<u64> {
        if let Some(g) = self.generation.get() {
            return g;
        }
        // Owned by no component: it lives as long as the loader.
        let g = runtime_world::unscoped(|| runtime_world::signal(0u64));
        self.generation.set(Some(g));
        g
    }
}

impl Loader for BundleLoader {
    fn generation(&self) -> u64 {
        self.generation_signal().get()
    }

    fn mount(&self, component: &str, args: &[u8]) -> Result<Element, String> {
        let bundle = self.current.borrow().clone();
        let element = bundle
            .mount_remote(&format!("__idealyst_remote_{component}"), args)
            .map_err(|e| e.to_string())?;
        // The tree calls back into the bundle that built it; keep that
        // bundle alive exactly as long as the tree, so a reload can swap the
        // current one while old trees tear down against their own.
        runtime_world::on_scope_drop(move || drop(bundle));
        Ok(element)
    }
}

/// The installed loader; `reload` replaces its bundle.
pub struct RemoteApp {
    loader: Rc<BundleLoader>,
}

/// An interpreter configured for bundles.
pub fn engine() -> wasmi::Engine {
    let mut config = wasmi::Config::default();
    config.compilation_mode(wasmi::CompilationMode::LazyTranslation);
    wasmi::Engine::new(&config)
}

/// Load `wasm` and make it where this thread's `#[component(remote)]`
/// components mount from.
pub fn install(wasm: &[u8]) -> Result<RemoteApp, String> {
    let engine = engine();
    let bundle = KernelBundle::load(&engine, wasm).map_err(|e| e.to_string())?;
    let loader = Rc::new(BundleLoader {
        engine,
        current: RefCell::new(Rc::new(bundle)),
        generation: Cell::new(None),
    });
    watch(&loader);
    install_loader(loader.clone());
    Ok(RemoteApp { loader })
}

/// When the current bundle panics, remount every remote component: the
/// poisoned bundle refuses the mounts, so each shows the panic message in
/// its place (`__mount_remote`'s error text), and its old tree — whose
/// callbacks can no longer reach the bundle — is torn down.
fn watch(loader: &Rc<BundleLoader>) {
    let weak = Rc::downgrade(loader);
    let bundle = loader.current.borrow().clone();
    bundle.on_poison(move |_| {
        if let Some(loader) = weak.upgrade() {
            if let Some(g) = loader.generation.get() {
                g.update(|n| n + 1);
            }
        }
    });
}

impl RemoteApp {
    /// Replace the bundle; every mounted remote component remounts from it.
    /// On `Err` (it doesn't load) the current bundle stays.
    pub fn reload(&self, wasm: &[u8]) -> Result<(), String> {
        let bundle = KernelBundle::load(&self.loader.engine, wasm).map_err(|e| e.to_string())?;
        *self.loader.current.borrow_mut() = Rc::new(bundle);
        watch(&self.loader);
        if let Some(g) = self.loader.generation.get() {
            g.update(|n| n + 1);
        }
        Ok(())
    }
}
