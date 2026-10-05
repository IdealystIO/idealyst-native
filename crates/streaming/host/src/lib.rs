//! Load remote components' bundles into the app.
//!
//! A remote bundle is the app's own Rust — `#[component(remote)]`
//! components, built for wasm32 with `--cfg idealyst_stream_guest` — run
//! under wasmi. Its reactive kernel is runtime-world's BRIDGED engine, so
//! every signal, effect, scope and context value it creates lives in the
//! app's ONE graph, and its trees cross through runtime-vocabulary's
//! element codec to be realized by the app's own registry and backend.
//!
//! - [`kernel`]: the host side of the kernel bridge over wasm
//!   ([`kernel::KernelBundle`]) — instantiation, the `idealyst_kernel`
//!   imports, `#[host_fn]` linking, mounting a remote component.
//! - [`remote`]: the loader `#[component(remote)]` mounts from
//!   ([`remote::install`], [`remote::install_with`], [`remote::RemoteApp`]).
//! - [`fetch`]: a minimal HTTP GET for pulling a bundle from `stream-serve`.
//!
//! See `crates/streaming/README.md` for the design and its measurements.

pub mod fetch;
/// The kernel bridge over wasm (host side): a bundle's reactive kernel on
/// this app's graph.
pub mod kernel;
pub mod remote;

/// Why a bundle was refused at load. Every check runs before any bundle
/// code does.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// Not valid wasm, or instantiation failed (e.g. it imports a kernel
    /// function this app does not define, or lacks a required export).
    Wasm(wasmi::Error),
    /// The bundle calls host functions this app does not export (or does
    /// not allow bundles to call).
    MissingHostFunctions(Vec<String>),
    /// Both sides have the host function, with different signatures.
    IncompatibleHostFunctions(Vec<HostFnMismatch>),
    /// The bundle encodes values differently from this app: it was built
    /// against another version of the framework's codec (`None`: one from
    /// before bundles reported it).
    IncompatibleCodec { app: u32, bundle: Option<u32> },
    /// The app's [`Trust`](remote_bundle::Trust) refused the bundle: it
    /// isn't signed, is signed by a key the app doesn't trust, or was
    /// changed after signing.
    Untrusted(remote_bundle::TrustError),
    /// The bundle needs what this app doesn't have: a component, a prop, a
    /// host function, or one that crosses differently (its requires
    /// section, checked against [`remote::provides`]). Each problem named.
    Incompatible(Vec<remote_bundle::Problem>),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Wasm(e) => write!(f, "bundle failed to load: {e}"),
            LoadError::MissingHostFunctions(names) => {
                write!(f, "bundle calls host functions this app does not export: {}", names.join(", "))
            }
            LoadError::Untrusted(e) => write!(f, "bundle refused: {e}"),
            LoadError::Incompatible(problems) => {
                let list: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
                write!(f, "bundle needs what this app doesn't have: {}", list.join("; "))
            }
            LoadError::IncompatibleCodec { app, bundle } => {
                let bundle = bundle.map_or_else(|| "1 (unreported)".to_string(), |v| v.to_string());
                write!(f, "bundle was built with codec version {bundle}, this app reads version {app}: rebuild the bundle against this app's framework version")
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
        }
    }
}

impl std::error::Error for LoadError {}

impl From<wasmi::Error> for LoadError {
    fn from(e: wasmi::Error) -> Self {
        LoadError::Wasm(e)
    }
}

/// A host function both sides have, with different signature fingerprints.
#[derive(Debug, Clone, PartialEq)]
pub struct HostFnMismatch {
    pub path: String,
    pub app_schema: u64,
    pub bundle_schema: u64,
}
