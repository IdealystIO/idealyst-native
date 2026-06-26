pub mod brs;
pub mod build;
pub mod catalog_wrapper;
pub mod check;
pub mod clean;
pub mod dev;
pub mod docs;
pub mod doctor;
pub mod export;
pub mod export_codegen;
pub mod icon;
pub mod init;
pub mod lint;
pub mod mcp;
pub mod new;
pub mod publish;
pub mod run;
pub mod rustc_capture;
pub mod scaffold;
pub mod scaffold_template;
pub mod serve;
pub mod sync;
pub mod test;

/// Shorthand for the "not implemented yet" stub each command returns
/// while the CLI is being fleshed out. Centralizing it keeps the
/// message format consistent and makes it easy to grep for what's
/// left to build.
fn todo(cmd: &str) -> anyhow::Result<()> {
    eprintln!("[idealyst {cmd}] not yet implemented");
    Ok(())
}
