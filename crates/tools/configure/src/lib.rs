//! Idealyst project configuration engine.
//!
//! This crate is the *logic* behind `idealyst configure` — the shared,
//! non-interactive core that both the CLI (which wraps it in a dialoguer
//! wizard) and the MCP server (the `configure_devcontainer` tool) drive.
//! Keeping it prompt-free is deliberate: the MCP server has no TTY, so
//! anything interactive would fork the two front-ends. Instead the core
//! exposes a plan → apply surface, and the CLI's wizard is just a way to
//! build the same [`devcontainer::ConfigureRequest`] the MCP tool builds
//! from its JSON arguments.
//!
//! Configuration domains: [`devcontainer`] (Dev Container sidecar services)
//! and [`vscode`] (workspace settings + extension recommendations). The module
//! layout leaves room for further peers without reshaping the crate.

pub mod devcontainer;
pub mod vscode;

pub(crate) mod jsonc;
