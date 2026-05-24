//! Shared TSX parser + IR lifter for the TSX-input porters.
//!
//! Pipeline:
//!
//! ```text
//!   source: &str
//!       │
//!       ▼  swc_ecma_parser
//!   swc_ecma_ast::Module
//!       │
//!       ▼  walk + Lifter::classify_call
//!   port_core::ir::Module + PortReport
//! ```
//!
//! Each frontend (React, Solid) provides a [`Lifter`]
//! implementation that recognizes its own reactive primitives.
//! Everything else — finding components, lifting props,
//! traversing JSX, rewriting setter identifiers — is shared.

pub mod lift;

pub use lift::{
    lift_module, BindingPattern, CallContext, LiftedCall, Lifter, ReactiveKind, ReadStyle,
};

use port_core::ir::{Module, PortReport};
use port_core::ParseError;
use swc_common::{sync::Lrc, FileName, SourceMap};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

/// Parse a TSX source string with swc, lift via the supplied
/// [`Lifter`], and return the porter IR + a port report. Sets the
/// `source_tool` on the produced module so the emitted header
/// names the right tool.
pub fn parse_and_lift(
    source: &str,
    tool_name: &str,
    lifter: &dyn Lifter,
) -> Result<(Module, PortReport), ParseError> {
    let (ast_module, cm) = parse(source, true)?;
    let (mut module, report) = lift_module(&ast_module, lifter, &cm)?;
    module.source_tool = tool_name.to_string();
    Ok((module, report))
}

/// Raw swc parse helper — used by per-framework script lifters
/// (`port-vue`, `port-svelte`) that have their own walker.
///
/// Returns the parsed AST and the `SourceMap` paired with it so
/// callers can convert spans to line numbers via
/// `cm.lookup_char_pos(span.lo).line`.
pub fn parse(
    source: &str,
    tsx: bool,
) -> Result<(swc_ecma_ast::Module, Lrc<SourceMap>), ParseError> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), source.to_string());
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax { tsx, ..Default::default() }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let module_ast = parser
        .parse_typescript_module()
        .map_err(|e| ParseError::new(format!("TS parse: {:?}", e.kind())))?;
    Ok((module_ast, cm))
}

// Re-export the swc AST types Vue/Svelte walkers need so they
// don't have to depend on swc directly.
pub use swc_common as common;
pub use swc_ecma_ast as ast;

/// Extract just the TypeScript type definitions from a source
/// file — `interface X { … }` and `type X = { … }` shapes —
/// without doing any component lifting. Used by the project-
/// level driver to harvest types from non-component files
/// (`*.ts` declaration modules, type-only re-exports, etc.) so
/// `createContext<ImportedType>(...)` aliases referencing those
/// types can be resolved cross-file.
pub fn extract_types(
    source: &str,
    tsx: bool,
) -> Result<std::collections::HashMap<String, port_core::ir::PropsType>, ParseError> {
    let (module_ast, _cm) = parse(source, tsx)?;
    let mut out = std::collections::HashMap::new();
    for item in &module_ast.body {
        if let Some((name, props)) = lift::extract_type_decl(item) {
            out.insert(name, props);
        }
    }
    Ok(out)
}
