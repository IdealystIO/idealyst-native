//! The spike's built bundles, release-built for wasm32 by this crate's build
//! script and embedded for the tests.

/// Fetching a bundle over HTTP (moved to `remote-host`; re-exported for the
/// spike's callers).
pub use remote_host::fetch;
pub mod guest_build;

/// `spike-kernelguest` (the kernel bridge test bundle): plain runtime-world
/// code whose graph is the host's. Release-built for wasm32 by this crate's
/// build script.
pub const KERNEL_GUEST_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spike_kernelguest.wasm"));

/// Phase 4: `RemoteCounter` as a remote component — bridged kernel, element
/// codec (`spike-remoteguest`).
pub const REMOTE_GUEST_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spike_remoteguest.wasm"));

/// `spike-remoteattr`: a `#[component(remote)]` component's bundle build.
pub const REMOTE_ATTR_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spike_remoteattr.wasm"));
/// `spike-ideaui`: remote components using every idea-ui component.
pub const IDEA_UI_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spike_ideaui.wasm"));
