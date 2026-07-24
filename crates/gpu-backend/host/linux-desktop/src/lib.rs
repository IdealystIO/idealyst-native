//! GTK4/Linux shell for the wgpu render backend. See the crate-level
//! notes in `Cargo.toml` for why this host adopts a GL context instead
//! of creating a swapchain.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{mount, LinuxHostHandle, MountError};
