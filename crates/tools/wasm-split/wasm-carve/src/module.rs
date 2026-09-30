//! A read-only index over a wasm module's bytes.
//!
//! Nothing here builds an instruction IR. Every section is recorded as a
//! byte range, and only the small tables the splitter reasons about are
//! parsed out (imports, exports, element segments, data segments, names,
//! body ranges). Emitting a split module is then a matter of copying ranges
//! and re-encoding the handful of sections that change — which is what keeps
//! memory proportional to the module's SIZE rather than to its code volume
//! times walrus's ~60 bytes of IR per byte of code.

use std::{collections::HashMap, ops::Range};

use anyhow::{Context, Result, ensure};
use wasmparser::{
    ConstExpr, DataKind, ElementItems, ElementKind, ExternalKind, KnownCustom, Name, Operator,
    Parser, Payload, TypeRef,
};

/// One top-level section.
#[derive(Clone, Debug)]
pub struct Section {
    /// Section id (0 = custom).
    pub id: u8,
    /// The whole section, id and size prefix included.
    pub range: Range<usize>,
    /// The custom section's name, for `id == 0`.
    pub custom_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Import<'a> {
    pub module: &'a str,
    pub name: &'a str,
    pub ty: TypeRef,
}

#[derive(Clone, Debug)]
pub struct Export<'a> {
    pub name: &'a str,
    pub kind: ExternalKind,
    pub index: u32,
}

/// A table slot's function, as an element segment spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElemItem {
    Func(u32),
    /// `ref.null` (or any non-`ref.func` expression) in an expression list.
    Null,
}

#[derive(Clone, Debug)]
pub struct Elem {
    /// `(table, offset)` when active with a constant `i32.const` offset.
    pub active: Option<(u32, i32)>,
    /// Active with a non-constant offset, passive, or declared: the splitter
    /// leaves these alone.
    pub is_active: bool,
    /// Whether the segment spells its items as expressions (and with which
    /// reference type), or as a plain function-index list.
    pub expr_ref_type: Option<wasmparser::RefType>,
    pub items: Vec<ElemItem>,
}

#[derive(Clone, Debug)]
pub struct DataSegment {
    /// `(memory, offset)` when active with a constant `i32.const` offset.
    pub active_const: Option<(u32, i32)>,
    pub passive: bool,
    /// The segment's bytes.
    pub data: Range<usize>,
    /// The whole encoded segment, for copying it unchanged.
    pub entry: Range<usize>,
}

pub struct ModuleIndex<'a> {
    pub bytes: &'a [u8],
    pub sections: Vec<Section>,
    pub imports: Vec<Import<'a>>,
    /// Parameter count of each type (0 for a non-function type).
    pub type_params: Vec<u32>,
    /// Type index of every function, imports first — the function index
    /// space.
    pub func_types: Vec<u32>,
    pub func_imports: u32,
    pub table_types: Vec<wasmparser::TableType>,
    pub table_imports: u32,
    pub memory_types: Vec<wasmparser::MemoryType>,
    pub memory_imports: u32,
    pub global_types: Vec<wasmparser::GlobalType>,
    pub global_imports: u32,
    /// Exception tags, imports first. wasm-bindgen ≥ 0.2.128 imports
    /// `WebAssembly.JSTag` as one (`__wbindgen_jstag`) and catches JS
    /// exceptions against it.
    pub tag_types: Vec<wasmparser::TagType>,
    pub tag_imports: u32,
    pub exports: Vec<Export<'a>>,
    pub start: Option<u32>,
    pub elems: Vec<Elem>,
    pub data: Vec<DataSegment>,
    pub data_count: Option<u32>,
    /// Payload offset of the code section: relocation and walrus
    /// `original_range` offsets count from here.
    pub code_payload_start: usize,
    /// Per defined function: the entry INCLUDING its size prefix, and the
    /// body alone.
    pub bodies: Vec<(Range<usize>, Range<usize>)>,
    /// Function index → name, from the `name` section.
    pub func_names: HashMap<u32, &'a str>,
    /// Every `i32.const` a global is initialized to.
    pub global_init_consts: Vec<i32>,
}

impl<'a> ModuleIndex<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut m = ModuleIndex {
            bytes,
            sections: Vec::new(),
            imports: Vec::new(),
            type_params: Vec::new(),
            func_types: Vec::new(),
            func_imports: 0,
            table_types: Vec::new(),
            table_imports: 0,
            memory_types: Vec::new(),
            memory_imports: 0,
            global_types: Vec::new(),
            global_imports: 0,
            tag_types: Vec::new(),
            tag_imports: 0,
            exports: Vec::new(),
            start: None,
            elems: Vec::new(),
            data: Vec::new(),
            data_count: None,
            code_payload_start: 0,
            bodies: Vec::new(),
            func_names: HashMap::new(),
            global_init_consts: Vec::new(),
        };
        m.sections = top_level_sections(bytes)?;

        for payload in Parser::new(0).parse_all(bytes) {
            match payload? {
                Payload::TypeSection(reader) => {
                    for rec in reader {
                        for sub in rec?.into_types() {
                            m.type_params.push(match &sub.composite_type.inner {
                                wasmparser::CompositeInnerType::Func(ft) => ft.params().len() as u32,
                                _ => 0,
                            });
                        }
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import?;
                        match import.ty {
                            TypeRef::Func(ty) | TypeRef::FuncExact(ty) => {
                                m.func_types.push(ty);
                                m.func_imports += 1;
                            }
                            TypeRef::Table(t) => {
                                m.table_types.push(t);
                                m.table_imports += 1;
                            }
                            TypeRef::Memory(t) => {
                                m.memory_types.push(t);
                                m.memory_imports += 1;
                            }
                            TypeRef::Global(t) => {
                                m.global_types.push(t);
                                m.global_imports += 1;
                            }
                            TypeRef::Tag(t) => {
                                m.tag_types.push(t);
                                m.tag_imports += 1;
                            }
                        }
                        m.imports.push(Import { module: import.module, name: import.name, ty: import.ty });
                    }
                }
                Payload::FunctionSection(reader) => {
                    for ty in reader {
                        m.func_types.push(ty?);
                    }
                }
                Payload::TableSection(reader) => {
                    for table in reader {
                        m.table_types.push(table?.ty);
                    }
                }
                Payload::MemorySection(reader) => {
                    for mem in reader {
                        m.memory_types.push(mem?);
                    }
                }
                Payload::GlobalSection(reader) => {
                    for global in reader {
                        let global = global?;
                        if let Some(c) = const_i32(&global.init_expr) {
                            m.global_init_consts.push(c);
                        }
                        m.global_types.push(global.ty);
                    }
                }
                Payload::TagSection(reader) => {
                    for tag in reader {
                        m.tag_types.push(tag?);
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export?;
                        m.exports.push(Export { name: export.name, kind: export.kind, index: export.index });
                    }
                }
                Payload::StartSection { func, .. } => m.start = Some(func),
                Payload::ElementSection(reader) => {
                    for elem in reader {
                        m.elems.push(parse_elem(elem?)?);
                    }
                }
                Payload::DataCountSection { count, .. } => m.data_count = Some(count),
                Payload::DataSection(reader) => {
                    for data in reader {
                        let data = data?;
                        let (active_const, passive) = match &data.kind {
                            DataKind::Passive => (None, true),
                            DataKind::Active { memory_index, offset_expr } => {
                                (const_i32(offset_expr).map(|o| (*memory_index, o)), false)
                            }
                        };
                        let start = data.range.end - data.data.len();
                        m.data.push(DataSegment {
                            active_const,
                            passive,
                            data: start..data.range.end,
                            entry: data.range.clone(),
                        });
                    }
                }
                Payload::CodeSectionStart { range, .. } => m.code_payload_start = range.start,
                Payload::CodeSectionEntry(body) => {
                    // `body.range()` is the body alone; its size prefix is
                    // the bytes between the previous entry's end (or the
                    // section's count LEB) and here. Recorded by
                    // re-reading the prefix length from the bytes.
                    let body_range = body.range();
                    let prefix_start = m
                        .bodies
                        .last()
                        .map(|(entry, _)| entry.end)
                        .unwrap_or_else(|| {
                            let (_, after_count) = read_leb(bytes, m.code_payload_start)
                                .expect("code section count");
                            after_count
                        });
                    m.bodies.push((prefix_start..body_range.end, body_range));
                }
                Payload::CustomSection(reader) => {
                    if let KnownCustom::Name(names) = reader.as_known() {
                        for name in names {
                            if let Ok(Name::Function(map)) = name {
                                for naming in map {
                                    let naming = naming?;
                                    m.func_names.insert(naming.index, naming.name);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        ensure!(
            m.bodies.len() + m.func_imports as usize == m.func_types.len(),
            "{} bodies for {} defined functions",
            m.bodies.len(),
            m.func_types.len() - m.func_imports as usize,
        );
        Ok(m)
    }

    pub fn defined_funcs(&self) -> u32 {
        self.bodies.len() as u32
    }

    pub fn total_funcs(&self) -> u32 {
        self.func_types.len() as u32
    }

    /// The body entry (size prefix included) of function `index`, which
    /// must be defined.
    pub fn body_entry(&self, index: u32) -> &'a [u8] {
        let (entry, _) = &self.bodies[(index - self.func_imports) as usize];
        &self.bytes[entry.clone()]
    }

    pub fn section(&self, id: u8) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }

    pub fn custom(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.id == 0 && s.custom_name.as_deref() == Some(name))
    }

    /// The payload of a custom section.
    pub fn custom_payload(&self, name: &str) -> Option<&'a [u8]> {
        let section = self.custom(name)?;
        let (_, payload_start) = read_leb(self.bytes, section.range.start + 1).ok()?;
        let (name_len, name_start) = read_leb(self.bytes, payload_start).ok()?;
        Some(&self.bytes[name_start + name_len as usize..section.range.end])
    }

    /// Every function index a defined function's body references directly:
    /// `call`, `return_call` and `ref.func` operands. Streams the operators;
    /// nothing is kept but the indices.
    pub fn direct_refs(&self, index: u32, out: &mut Vec<u32>) -> Result<()> {
        let (_, body) = &self.bodies[(index - self.func_imports) as usize];
        let fb = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(
            &self.bytes[body.clone()],
            body.start,
        ));
        let mut ops = fb.get_operators_reader()?;
        while !ops.eof() {
            match ops.read()? {
                Operator::Call { function_index }
                | Operator::ReturnCall { function_index }
                | Operator::RefFunc { function_index } => out.push(function_index),
                _ => {}
            }
        }
        Ok(())
    }
}

impl<'a> ModuleIndex<'a> {
    /// Every address a defined function's body names as a constant: each
    /// `i32.const` and each load/store `offset` (LLVM folds a static's
    /// address into the offset immediate). Used where no relocation
    /// records the references — functions wasm-bindgen wrote.
    pub fn const_addresses(&self, index: u32, out: &mut Vec<u32>) -> Result<()> {
        let (_, body) = &self.bodies[(index - self.func_imports) as usize];
        let fb = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(
            &self.bytes[body.clone()],
            body.start,
        ));
        let mut ops = fb.get_operators_reader()?;
        while !ops.eof() {
            use Operator::*;
            match ops.read()? {
                I32Const { value } => out.push(value as u32),
                I32Load { memarg } | I64Load { memarg } | F32Load { memarg } | F64Load { memarg }
                | I32Load8S { memarg } | I32Load8U { memarg } | I32Load16S { memarg }
                | I32Load16U { memarg } | I64Load8S { memarg } | I64Load8U { memarg }
                | I64Load16S { memarg } | I64Load16U { memarg } | I64Load32S { memarg }
                | I64Load32U { memarg } | I32Store { memarg } | I64Store { memarg }
                | F32Store { memarg } | F64Store { memarg } | I32Store8 { memarg }
                | I32Store16 { memarg } | I64Store8 { memarg } | I64Store16 { memarg }
                | I64Store32 { memarg } | V128Load { memarg } | V128Store { memarg } => {
                    out.push(memarg.offset as u32)
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn parse_elem(elem: wasmparser::Element<'_>) -> Result<Elem> {
    let (active, is_active) = match &elem.kind {
        ElementKind::Active { table_index, offset_expr } => {
            (const_i32(offset_expr).map(|o| (table_index.unwrap_or(0), o)), true)
        }
        ElementKind::Passive | ElementKind::Declared => (None, false),
    };
    let (expr_ref_type, items) = match elem.items {
        ElementItems::Functions(funcs) => {
            let items = funcs.into_iter().map(|f| f.map(ElemItem::Func)).collect::<Result<_, _>>()?;
            (None, items)
        }
        ElementItems::Expressions(ty, exprs) => {
            let mut items = Vec::new();
            for expr in exprs {
                let expr = expr?;
                let mut ops = expr.get_operators_reader();
                let item = match ops.read()? {
                    Operator::RefFunc { function_index } => ElemItem::Func(function_index),
                    _ => ElemItem::Null,
                };
                items.push(item);
            }
            (Some(ty), items)
        }
    };
    Ok(Elem { active, is_active, expr_ref_type, items })
}

/// The value of a constant `i32.const N` expression.
pub fn const_i32(expr: &ConstExpr<'_>) -> Option<i32> {
    let mut ops = expr.get_operators_reader();
    match (ops.read().ok()?, ops.read().ok()?) {
        (Operator::I32Const { value }, Operator::End) => Some(value),
        _ => None,
    }
}

pub fn read_leb(bytes: &[u8], mut pos: usize) -> Result<(u32, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(pos).context("truncated LEB128")?;
        pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((u32::try_from(value).context("LEB128 overflows u32")?, pos));
        }
        shift += 7;
        ensure!(shift < 35, "LEB128 longer than 5 bytes");
    }
}

fn top_level_sections(bytes: &[u8]) -> Result<Vec<Section>> {
    ensure!(bytes.len() >= 8 && &bytes[..4] == b"\0asm", "not a wasm module");
    let mut sections = Vec::new();
    let mut pos = 8;
    while pos < bytes.len() {
        let id = bytes[pos];
        let (size, payload) = read_leb(bytes, pos + 1)?;
        let end = payload + size as usize;
        ensure!(end <= bytes.len(), "section {id} overruns the module");
        let custom_name = if id == 0 {
            let (len, name_start) = read_leb(bytes, payload)?;
            Some(String::from_utf8_lossy(&bytes[name_start..name_start + len as usize]).into_owned())
        } else {
            None
        };
        sections.push(Section { id, range: pos..end, custom_name });
        pos = end;
    }
    Ok(sections)
}
