//! Point a command module's exports past LLD's constructor wrappers.
//!
//! A wasm32 bin is linked as a *command*: LLD wraps every export except the
//! ones that already reach `__wasm_call_ctors` in a synthesized function
//!
//! ```text
//! call $__wasm_call_ctors
//! local.get 0 … local.get n-1
//! call $the_real_export
//! end
//! ```
//!
//! because a command is meant to be entered once. A web module is entered
//! on every event, callback and string transfer, so every JS → Rust call
//! re-runs every static constructor. Measured on `tests/own-glue/hybrid`
//! with wasm-bindgen 0.2.128: a ctor-bumped counter read 15 after boot and
//! a handful of clicks — and the wrapped exports included wasm-bindgen's
//! own `#[wasm_bindgen]` export, not only web-glue's. `inventory` survives
//! it only because its `submit` became idempotent for exactly this reason
//! (see inventory 0.3.24's crate docs, "WebAssembly and constructors").
//!
//! Own mode avoids the wrappers at link time (`--export=__wasm_call_ctors`
//! makes a reactor and the loader calls it once). Hybrid mode cannot:
//! wasm-bindgen's `__wbindgen_start` only calls `main(0, 0)`, so a reactor
//! would never run constructors at all. So this pass leaves `main`'s
//! wrapper alone — it is the one-time constructor call — and repoints
//! every other exported wrapper at the function it forwards to. Only the
//! export section changes; the wrappers stay behind as dead code, and no
//! function index moves.

use std::collections::HashMap;

use anyhow::{Context, Result};
use wasm_encoder::{ExportKind, ExportSection, Module, RawSection};
use wasmparser::{ExternalKind, Operator, Parser, Payload};

/// `Some((ctors, inner))` when `body` is exactly LLD's command-export
/// wrapper shape.
fn wrapper_shape(body: &wasmparser::FunctionBody<'_>) -> Result<Option<(u32, u32)>> {
    if body.get_locals_reader()?.get_count() != 0 {
        return Ok(None);
    }
    let mut ops = body.get_operators_reader()?;
    let Operator::Call { function_index: ctors } = ops.read()? else { return Ok(None) };
    let mut next_local = 0u32;
    loop {
        match ops.read()? {
            Operator::LocalGet { local_index } if local_index == next_local => next_local += 1,
            Operator::Call { function_index: inner } => {
                return Ok(match (ops.read()?, ops.eof()) {
                    (Operator::End, true) => Some((ctors, inner)),
                    _ => None,
                });
            }
            _ => return Ok(None),
        }
    }
}

/// Repoint every exported command wrapper except `main`'s at the function
/// it wraps. Returns the rewritten module and how many exports moved, or
/// `None` when the module has no `main` wrapper (a reactor or a cdylib —
/// nothing to do).
pub fn unwrap_command_exports(wasm: &[u8]) -> Result<Option<(Vec<u8>, usize)>> {
    // Pass 1: exports, and the bodies of exported functions.
    let mut func_imports = 0u32;
    let mut exports: Vec<(String, ExternalKind, u32)> = Vec::new();
    let mut exported: HashMap<u32, Option<(u32, u32)>> = HashMap::new();
    let mut next_defined = 0u32;
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.context("parse wasm")? {
            Payload::ImportSection(r) => {
                for i in r.into_imports() {
                    if matches!(i?.ty, wasmparser::TypeRef::Func(_) | wasmparser::TypeRef::FuncExact(_)) {
                        func_imports += 1;
                    }
                }
            }
            Payload::ExportSection(r) => {
                for e in r {
                    let e = e?;
                    if e.kind == ExternalKind::Func {
                        exported.insert(e.index, None);
                    }
                    exports.push((e.name.to_string(), e.kind, e.index));
                }
            }
            Payload::CodeSectionEntry(body) => {
                let index = func_imports + next_defined;
                next_defined += 1;
                if let Some(slot) = exported.get_mut(&index) {
                    *slot = wrapper_shape(&body)?;
                }
            }
            _ => {}
        }
    }
    let main_shape = exports
        .iter()
        .find(|(n, k, _)| n == "main" && *k == ExternalKind::Func)
        .and_then(|(_, _, i)| exported.get(i).copied().flatten());
    let Some((ctors, _)) = main_shape else { return Ok(None) };

    let mut moved = 0usize;
    let mut section = ExportSection::new();
    for (name, kind, index) in &exports {
        let mut index = *index;
        if *kind == ExternalKind::Func
            && name != "main"
            && let Some(Some((c, inner))) = exported.get(&index)
            && *c == ctors
        {
            index = *inner;
            moved += 1;
        }
        let kind = match kind {
            ExternalKind::Func | ExternalKind::FuncExact => ExportKind::Func,
            ExternalKind::Table => ExportKind::Table,
            ExternalKind::Memory => ExportKind::Memory,
            ExternalKind::Global => ExportKind::Global,
            ExternalKind::Tag => ExportKind::Tag,
        };
        section.export(name, kind, index);
    }
    if moved == 0 {
        return Ok(None);
    }

    // Pass 2: copy every section but the exports.
    let mut module = Module::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload?;
        if let Payload::ExportSection(_) = payload {
            module.section(&section);
        } else if let Some((id, range)) = payload.as_section() {
            module.section(&RawSection { id, data: &wasm[range] });
        }
    }
    Ok(Some((module.finish(), moved)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, Function, FunctionSection, GlobalSection, GlobalType, Instruction,
        TypeSection, ValType,
    };

    /// ctors (0) bumps a global; inner_main (1), inner_f (2); wrappers
    /// main_w (3) and f_w (4) in LLD's shape; g (5) is a plain export.
    fn command_module() -> Vec<u8> {
        let mut m = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], []); // 0
        types.ty().function([ValType::I32, ValType::I32], [ValType::I32]); // 1
        types.ty().function([ValType::I32], [ValType::I32]); // 2
        m.section(&types);
        let mut funcs = FunctionSection::new();
        for ty in [0, 1, 2, 1, 2, 2] {
            funcs.function(ty);
        }
        m.section(&funcs);
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType { val_type: ValType::I32, mutable: true, shared: false },
            &wasm_encoder::ConstExpr::i32_const(0),
        );
        m.section(&globals);
        let mut exports = ExportSection::new();
        exports.export("main", ExportKind::Func, 3);
        exports.export("f", ExportKind::Func, 4);
        exports.export("g", ExportKind::Func, 5);
        m.section(&exports);
        let mut code = CodeSection::new();
        let mut ctors = Function::new([]);
        ctors.instructions().global_get(0).i32_const(1).i32_add().global_set(0).end();
        code.function(&ctors);
        let mut inner_main = Function::new([]);
        inner_main.instructions().i32_const(0).end();
        code.function(&inner_main);
        let mut inner_f = Function::new([]);
        inner_f.instructions().global_get(0).end();
        code.function(&inner_f);
        let mut main_w = Function::new([]);
        main_w.instruction(&Instruction::Call(0));
        main_w.instructions().local_get(0).local_get(1).call(1).end();
        code.function(&main_w);
        let mut f_w = Function::new([]);
        f_w.instructions().call(0).local_get(0).call(2).end();
        code.function(&f_w);
        let mut g = Function::new([]);
        g.instructions().local_get(0).end();
        code.function(&g);
        m.section(&code);
        m.finish()
    }

    fn exports_of(wasm: &[u8]) -> Vec<(String, u32)> {
        let mut out = Vec::new();
        for p in Parser::new(0).parse_all(wasm) {
            if let Payload::ExportSection(r) = p.unwrap() {
                for e in r {
                    let e = e.unwrap();
                    out.push((e.name.to_string(), e.index));
                }
            }
        }
        out
    }

    #[test]
    fn regression_every_export_but_main_stops_rerunning_constructors() {
        let (out, moved) = unwrap_command_exports(&command_module()).unwrap().expect("rewritten");
        wasmparser::Validator::new().validate_all(&out).unwrap();
        assert_eq!(moved, 1);
        assert_eq!(
            exports_of(&out),
            [("main".into(), 3), ("f".into(), 2), ("g".into(), 5)],
            "main keeps its wrapper (the one-time ctor call); f points past its wrapper; g untouched"
        );
    }

    #[test]
    fn a_module_without_a_main_wrapper_is_left_alone() {
        let mut m = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], []);
        m.section(&types);
        assert!(unwrap_command_exports(&m.finish()).unwrap().is_none());
    }
}
