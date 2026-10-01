//! Model B, measured: the real framework inside a bundle.
//!
//! `RemoteCounter` is an ordinary `#[component]` written with `ui!`, in
//! `spike-components` — a plain crate the app also links natively. This
//! crate is only the bundle's wasm exports around it. The bundle realizes
//! the component with the actual kernel/scene/vocabulary against
//! `dev_server::WireRecordingBackend`, and hands the app the recorded wire
//! commands to replay into its own backend — runtime-server, run in a
//! sandbox for one subtree.
//!
//! The bundle has its OWN world. Host state crosses as mirrors:
//! - the `external` prop is a guest signal the app updates through
//!   `fg_set_external` whenever its own signal changes;
//! - `CurrentUser` is CONTEXT: the app's provided value is mirrored into a
//!   guest signal and `provide`d at the guest world's root, so the
//!   component reads it with a plain `inject`.
//!
//! Exports are hand-written for the measurement; the stream-guest
//! `bundle!`/manifest machinery would generate them.

/// The bundle's wasm exports. Only a bundle build has a host to call them.
#[cfg(idealyst_stream_guest)]
mod exports {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dev_server::newcore::SceneSession;
    use dev_server::WireRecordingBackend;
    use runtime_core::{provide, signal, ui, Signal};
    use stream_abi::Wire;
    use wire::{Command, DevToApp, EventArgs, HandlerId};

    use spike_components::{CurrentUser, RemoteCounter};

    struct Session {
        recorder: WireRecordingBackend,
        external: Signal<i64>,
        user: Signal<String>,
        // Last: dropping it unmounts against a still-live recorder.
        session: SceneSession,
    }

    thread_local! {
        static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
        static RET: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    fn ret(bytes: Vec<u8>) -> u64 {
        RET.with(|r| {
            let mut r = r.borrow_mut();
            *r = bytes;
            stream_abi::pack(r.as_ptr() as usize as u32, r.len() as u32)
        })
    }

    /// # Safety
    /// `(ptr, len)` must come from one `fg_alloc`.
    unsafe fn take_input(ptr: u32, len: u32) -> Box<[u8]> {
        // SAFETY: per the contract, this is the boxed slice `fg_alloc` leaked.
        unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr as usize as *mut u8, len as usize)) }
    }

    /// Commit, drain, and encode what changed — the sidecar's per-event step.
    fn sync(s: &Session) -> u64 {
        s.session.flush();
        let cmds: Vec<Command> = s.recorder.drain_commands();
        ret(wire::codec::encode(&DevToApp::Commands(cmds)).expect("wire encode"))
    }

    #[no_mangle]
    extern "C" fn fg_alloc(len: u32) -> u32 {
        Box::into_raw(vec![0u8; len as usize].into_boxed_slice()) as *mut u8 as usize as u32
    }

    /// Args: `title: String, external: i64, user: String`. Returns the initial
    /// scene as encoded `DevToApp::Commands`.
    #[no_mangle]
    extern "C" fn fg_mount(ptr: u32, len: u32) -> u64 {
        // SAFETY: the host passes a buffer from `fg_alloc`.
        let args = unsafe { take_input(ptr, len) };
        let mut input: &[u8] = &args;
        let title = String::decode(&mut input).expect("title");
        let external0 = i64::decode(&mut input).expect("external");
        let user0 = String::decode(&mut input).expect("user");

        dev_server::scheduler::install();
        let recorder = WireRecordingBackend::new();
        // The mirrors are created inside the session world (the mount closure
        // runs under its `enter`) and smuggled out to be written later.
        let mirrors: Rc<Cell<Option<(Signal<i64>, Signal<String>)>>> = Rc::new(Cell::new(None));
        let out = mirrors.clone();
        let session = SceneSession::mount(&recorder, |_| {}, move || {
            let external = signal(external0);
            let user = signal(user0);
            out.set(Some((external, user)));
            provide(CurrentUser(user.read_only()));
            let external = external.read_only();
            ui! { RemoteCounter(title = title, external = external) }
        });
        let (external, user) = mirrors.take().expect("mount ran");
        let s = Session { recorder, external, user, session };
        let packed = sync(&s);
        SESSION.with(|slot| *slot.borrow_mut() = Some(s));
        packed
    }

    fn with_session(f: impl FnOnce(&Session) -> u64) -> u64 {
        SESSION.with(|slot| f(slot.borrow().as_ref().expect("fg_mount first")))
    }

    /// The app's `external` signal changed: mirror it, return the delta.
    #[no_mangle]
    extern "C" fn fg_set_external(v: i64) -> u64 {
        with_session(|s| {
            s.external.set(v);
            sync(s)
        })
    }

    /// The app's `CurrentUser` context changed: mirror it, return the delta.
    #[no_mangle]
    extern "C" fn fg_set_user(ptr: u32, len: u32) -> u64 {
        // SAFETY: the host passes a buffer from `fg_alloc`.
        let bytes = unsafe { take_input(ptr, len) };
        let user = String::from_bytes(&bytes).expect("user");
        with_session(|s| {
            s.user.set(user);
            sync(s)
        })
    }

    /// A tap on the replayed tree: the client posts the handler id back.
    #[no_mangle]
    extern "C" fn fg_dispatch(handler: u64) -> u64 {
        with_session(|s| {
            s.recorder.dispatch_event(HandlerId(handler), EventArgs::Unit);
            sync(s)
        })
    }

    /// Unmount: drop the session (scope cleanups run) and return the
    /// teardown commands.
    #[no_mangle]
    extern "C" fn fg_unmount() -> u64 {
        SESSION.with(|slot| {
            let s = slot.borrow_mut().take().expect("fg_mount first");
            let Session { recorder, session, .. } = s;
            drop(session);
            ret(wire::codec::encode(&DevToApp::Commands(recorder.drain_commands())).expect("wire encode"))
        })
    }
}
