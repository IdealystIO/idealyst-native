//! RemoteCounter as a remote component: the bundle's only code is this
//! mount export. The component itself is `spike-components`' — the same
//! source the app links natively, which is what the parity test compares
//! against.
//!
//! Mount args (`stream_abi::Wire`): `title: String`, then the `external`
//! prop's handle (three `u32`s, from the host's `export_read_signal`).
//! `CurrentUser` arrives as host context declared under that name: the
//! handle of the host's user signal.

#[cfg(idealyst_stream_guest)]
mod exports {
    use runtime_vocabulary::remote::{bundle, to_bytes, wasm};
    use runtime_world::remote_guest::{import_read_signal, register_remote_context, Codec};
    use stream_abi::Wire;

    use spike_components::{remote_counter_view, CurrentUser};

    fn wire_encode<T: Wire>(v: &T, out: &mut Vec<u8>) {
        v.encode(out)
    }

    fn wire_decode<T: Wire>(b: &[u8]) -> Option<T> {
        T::from_bytes(b)
    }

    fn codec<T: Wire>() -> Codec<T> {
        Codec { encode: wire_encode::<T>, decode: wire_decode::<T> }
    }

    fn handle(b: &mut &[u8]) -> Option<(u32, u32, u32)> {
        Some((u32::decode(b)?, u32::decode(b)?, u32::decode(b)?))
    }

    #[no_mangle]
    extern "C" fn rc_mount(_ptr: u32, len: u32) -> i64 {
        let args = wasm::take_args(len);
        let mut b = &args[..];
        let title = String::decode(&mut b).expect("rc_mount: title");
        let external = handle(&mut b).expect("rc_mount: external handle");
        register_remote_context::<CurrentUser>("CurrentUser", |mut b| {
            Some(CurrentUser(import_read_signal(handle(&mut b)?, codec::<String>())))
        });
        // The mount's own scope: owns the prop import, and crosses to the
        // host as the root `Owned`, so unmounting there releases it.
        let tree = runtime_scene::component_scope(move || {
            let external = import_read_signal(external, codec::<i64>());
            // The view function, not the `RemoteCounter` component: a plain
            // `#[component]` would be imported from the app (see
            // `remote_counter_view`), and this fixture compares the bundle's
            // copy of the code with the app's.
            runtime_scene::component_scope(move || remote_counter_view(title.into(), external))
        });
        wasm::reply(to_bytes(&bundle::tree(tree)))
    }
}
