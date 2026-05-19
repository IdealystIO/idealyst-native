//! Vue → idealyst-native source porter. See `../README.md`.

pub mod composition;
pub mod parser;
pub mod script;
pub mod sfc;
pub mod template;

pub use port_core::ir;
pub use port_core::{ParseError, Parser};

pub fn lift(source: &str) -> Result<(ir::Module, ir::PortReport), ParseError> {
    let p = parser::VueParser::new();
    p.parse(source)
}

pub fn port(source: &str) -> Result<(String, ir::PortReport), ParseError> {
    let (module, report) = lift(source)?;
    Ok((port_core::emit::emit_module(&module), report))
}
