//! Host side of the model-B prototype: load `spike-fullguest` into wasmi and
//! exchange wire commands with it. The app replays what comes back through
//! its replay client (`dev_client::WireBackend`) — the runtime-server path,
//! with the "server" in-process and sandboxed.
//!
//! Hand-written against `spike-fullguest`'s exports for the measurement;
//! the real loader would fold this into `stream_host::Bundle`.

use stream_abi::Wire;
use wasmi::{CompilationMode, Config, Engine, Linker, Memory, Module, Store, TypedFunc};
use wire::{Command, DevToApp};

pub fn engine(mode: CompilationMode) -> Engine {
    let mut config = Config::default();
    config.compilation_mode(mode);
    Engine::new(&config)
}

pub struct FullGuest {
    store: Store<()>,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    mount: TypedFunc<(u32, u32), u64>,
    set_external: TypedFunc<i64, u64>,
    set_user: TypedFunc<(u32, u32), u64>,
    dispatch: TypedFunc<u64, u64>,
    unmount: TypedFunc<(), u64>,
}

impl FullGuest {
    /// The bundle imports nothing: the whole framework is inside it.
    pub fn load(engine: &Engine, wasm: &[u8]) -> Result<Self, wasmi::Error> {
        let module = Module::new(engine, wasm)?;
        let mut store = Store::new(engine, ());
        let instance = Linker::<()>::new(engine).instantiate_and_start(&mut store, &module)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| wasmi::Error::new("bundle exports no memory"))?;
        Ok(FullGuest {
            alloc: instance.get_typed_func(&store, "fg_alloc")?,
            mount: instance.get_typed_func(&store, "fg_mount")?,
            set_external: instance.get_typed_func(&store, "fg_set_external")?,
            set_user: instance.get_typed_func(&store, "fg_set_user")?,
            dispatch: instance.get_typed_func(&store, "fg_dispatch")?,
            unmount: instance.get_typed_func(&store, "fg_unmount")?,
            memory,
            store,
        })
    }

    fn input(&mut self, bytes: &[u8]) -> (u32, u32) {
        let ptr = self.alloc.call(&mut self.store, bytes.len() as u32).expect("fg_alloc");
        self.memory.write(&mut self.store, ptr as usize, bytes).expect("write args");
        (ptr, bytes.len() as u32)
    }

    fn commands(&mut self, packed: u64) -> Vec<Command> {
        let (ptr, len) = stream_abi::unpack(packed);
        let mut bytes = vec![0u8; len as usize];
        self.memory.read(&self.store, ptr as usize, &mut bytes).expect("read commands");
        match wire::codec::decode::<DevToApp>(&bytes).expect("wire decode") {
            DevToApp::Commands(c) => c,
            other => panic!("expected DevToApp::Commands, got {other:?}"),
        }
    }

    /// Mount `RemoteCounter` with props `title` / `external` and the
    /// `CurrentUser` context value `user`. Returns the initial scene.
    pub fn mount(&mut self, title: &str, external: i64, user: &str) -> Vec<Command> {
        let mut args = Vec::new();
        title.to_string().encode(&mut args);
        external.encode(&mut args);
        user.to_string().encode(&mut args);
        let (ptr, len) = self.input(&args);
        let packed = self.mount.call(&mut self.store, (ptr, len)).expect("fg_mount");
        self.commands(packed)
    }

    /// The app's `external` signal changed.
    pub fn set_external(&mut self, v: i64) -> Vec<Command> {
        let packed = self.set_external.call(&mut self.store, v).expect("fg_set_external");
        self.commands(packed)
    }

    /// The app's `CurrentUser` context changed.
    pub fn set_user(&mut self, user: &str) -> Vec<Command> {
        let (ptr, len) = self.input(&user.to_string().to_bytes());
        let packed = self.set_user.call(&mut self.store, (ptr, len)).expect("fg_set_user");
        self.commands(packed)
    }

    /// A tap the replay client reported for `handler`.
    pub fn dispatch(&mut self, handler: u64) -> Vec<Command> {
        let packed = self.dispatch.call(&mut self.store, handler).expect("fg_dispatch");
        self.commands(packed)
    }

    pub fn unmount(&mut self) -> Vec<Command> {
        let packed = self.unmount.call(&mut self.store, ()).expect("fg_unmount");
        self.commands(packed)
    }
}

/// The handler id of the button labelled `label` in a command stream — what
/// a real client posts back as `AppToDev::Event` when that button is tapped.
pub fn button_handler(cmds: &[Command], label: &str) -> Option<u64> {
    cmds.iter().find_map(|c| match c {
        Command::CreateButton { label: l, on_click, .. } if l == label => Some(on_click.0),
        _ => None,
    })
}
