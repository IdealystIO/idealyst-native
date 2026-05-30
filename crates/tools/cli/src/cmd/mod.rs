pub mod brs;
pub mod build;
pub mod check;
pub mod clean;
pub mod dev;
pub mod doctor;
pub mod icon;
pub mod init;
pub mod mcp;
pub mod new;
pub mod run;
pub mod rustc_capture;
pub mod scaffold;
pub mod scaffold_template;
pub mod serve;
pub mod sync;

/// Shorthand for the "not implemented yet" stub each command returns
/// while the CLI is being fleshed out. Centralizing it keeps the
/// message format consistent and makes it easy to grep for what's
/// left to build.
fn todo(cmd: &str) -> anyhow::Result<()> {
    eprintln!("[idealyst {cmd}] not yet implemented");
    Ok(())
}
