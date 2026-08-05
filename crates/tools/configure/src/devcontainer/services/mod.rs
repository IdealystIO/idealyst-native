//! Built-in devcontainer services. Each module defines one [`DevService`];
//! [`super::service::registry`] lists them.
//!
//! [`DevService`]: super::service::DevService

pub mod claude;
pub mod codex;
pub mod database;
pub mod idealyst_cli;
pub mod minio;
pub mod redis;

/// `postCreateCommand` snippet handing a root-owned mount point to the
/// container user. Lifecycle string commands already run under `sh -c`, so
/// no wrapper is needed; the sudo fallback covers non-root remote users
/// (devcontainer base images grant passwordless sudo — `-n` fails fast
/// instead of prompting when they don't).
pub(crate) fn chown_cmd(path: &str) -> String {
    format!(
        "chown -R \"$(id -u):$(id -g)\" {path} 2>/dev/null || sudo -n chown -R \"$(id -u):$(id -g)\" {path}"
    )
}
