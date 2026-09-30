//! Append forwarding functions and table entries to a module — without an
//! IR, and without moving anything already there.
//!
//! A hot-reload base roots every function in `__indirect_function_table`
//! and adds forwarding bodies for the functions a patch cannot otherwise
//! reach (see `build_web::hotpatch_base`). Both edits only ADD: new
//! functions go after the last one, new slots after the last active
//! segment's last entry. No existing index changes, so every original
//! body is copied as bytes — the code section is one `memcpy` — and only
//! the function, table, element and `name` sections are re-encoded.
//!
//! The walrus pass this replaces parsed the whole module into IR and
//! re-emitted it: 6.9–9 s of every warm CrewForge rebuild on a Mac.

use anyhow::{Context, Result, bail, ensure};
use wasm_encoder::{
    ElementSection, FunctionSection, Module, NameMap, NameSection, RawSection, TableSection,
    reencode::{self, Reencode},
};
use wasmparser::{KnownCustom, Name, Parser, Payload};

use crate::{
    emit::{push_active_funcs, table_type},
    module::{ElemItem, ModuleIndex, read_leb},
};

/// A function appended to the module: forwards every parameter to
/// `target` and returns what it returns. Typed as `target` is.
#[derive(Clone, Debug)]
pub struct Forwarder {
    pub name: String,
    pub target: u32,
}

/// What to add, and which custom sections survive.
pub struct Additions<'a> {
    /// Appended in order: the `i`th gets index `total_funcs + i`.
    pub forwarders: &'a [Forwarder],
    /// Function indices (original or appended) to append to the LAST
    /// active element segment, in order.
    pub root: &'a [u32],
    /// Whether a custom section is kept, by name.
    pub keep_custom: &'a dyn Fn(&str) -> bool,
}

pub fn append(source: &ModuleIndex<'_>, add: &Additions<'_>) -> Result<Vec<u8>> {
    let total = source.total_funcs();
    let forwarder_types: Vec<u32> = add
        .forwarders
        .iter()
        .map(|f| {
            source
                .func_types
                .get(f.target as usize)
                .copied()
                .with_context(|| format!("forwarder {} targets function {} of {total}", f.name, f.target))
        })
        .collect::<Result<_>>()?;
    for f in add.root {
        ensure!(*f < total + add.forwarders.len() as u32, "rooting function {f}, which does not exist");
    }

    // The segment that grows, and the table it writes into.
    let last_active = source
        .elems
        .iter()
        .rposition(|e| e.active.is_some())
        .context(
            "the base module has no active element segment to grow — \
             was it linked without `--export-table`?",
        )?;
    let (table, offset) = source.elems[last_active].active.expect("filtered to active");
    ensure!(
        table >= source.table_imports,
        "the element segment writes an imported table, which cannot be grown"
    );
    let added = add.root.len() as u64;
    let needed = offset as u64 + (source.elems[last_active].items.len() as u64 + added);
    let mut grown = source.table_types[table as usize];
    grown.initial = (grown.initial + added).max(needed);
    if let Some(max) = grown.maximum {
        grown.maximum = Some(max.max(grown.initial));
    }

    let function_section = || {
        let mut funcs = FunctionSection::new();
        for ty in &source.func_types[source.func_imports as usize..] {
            funcs.function(*ty);
        }
        for ty in &forwarder_types {
            funcs.function(*ty);
        }
        funcs
    };
    let code_section = |existing: Option<&[u8]>| -> Result<Vec<u8>> {
        let mut code = Vec::with_capacity(existing.map_or(0, <[u8]>::len) + 16 * add.forwarders.len());
        wasm_encoder::Encode::encode(&(source.defined_funcs() + add.forwarders.len() as u32), &mut code);
        if let Some(payload) = existing {
            let (_, bodies_start) = read_leb(payload, 0)?;
            code.extend_from_slice(&payload[bodies_start..]);
        }
        for f in add.forwarders {
            let params = source.type_params[source.func_types[f.target as usize] as usize];
            let mut func = wasm_encoder::Function::new([]);
            for p in 0..params {
                func.instructions().local_get(p);
            }
            func.instructions().call(f.target);
            func.instructions().end();
            wasm_encoder::Encode::encode(&func, &mut code);
        }
        Ok(code)
    };

    // A module with no defined functions has neither a function nor a
    // code section; forwarders need both, each at its place in section
    // order, which is by RANK, not by id.
    let rank = |id: u8| SECTION_ORDER.iter().position(|s| *s == id);
    let mut missing: Vec<u8> = Vec::new();
    if !add.forwarders.is_empty() {
        for id in [3u8, 10] {
            if source.section(id).is_none() {
                missing.push(id);
            }
        }
    }
    let mut module = Module::new();
    let mut insert_missing = |module: &mut Module, before: Option<u8>| -> Result<()> {
        while let Some(&id) = missing.first() {
            if before.is_some_and(|b| rank(b) < rank(id)) {
                break;
            }
            match id {
                3 => {
                    module.section(&function_section());
                }
                _ => {
                    module.section(&RawSection { id: 10, data: &code_section(None)? });
                }
            }
            missing.remove(0);
        }
        Ok(())
    };
    for section in &source.sections {
        let (_, payload_start) = read_leb(source.bytes, section.range.start + 1)?;
        let payload = &source.bytes[payload_start..section.range.end];
        if section.id != 0 {
            insert_missing(&mut module, Some(section.id))?;
        }
        match section.id {
            3 => {
                module.section(&function_section());
            }
            4 => {
                let mut tables = TableSection::new();
                for idx in source.table_imports..source.table_types.len() as u32 {
                    let ty = if idx == table { grown } else { source.table_types[idx as usize] };
                    tables.table(table_type(&ty)?);
                }
                module.section(&tables);
            }
            9 => {
                let mut elems = ElementSection::new();
                for (i, elem) in source.elems.iter().enumerate() {
                    let Some((table, offset)) = elem.active else {
                        bail!("a passive, declared or non-constant element segment");
                    };
                    let mut items: Vec<u32> = elem
                        .items
                        .iter()
                        .map(|item| match item {
                            ElemItem::Func(f) => Ok(*f),
                            ElemItem::Null => bail!("ref.null in an element segment"),
                        })
                        .collect::<Result<_>>()?;
                    if i == last_active {
                        items.extend_from_slice(add.root);
                    }
                    push_active_funcs(&mut elems, table, offset, elem.expr_ref_type, &items);
                }
                module.section(&elems);
            }
            10 => {
                module.section(&RawSection { id: 10, data: &code_section(Some(payload))? });
            }
            0 => {
                let name = section.custom_name.as_deref().unwrap_or("");
                if !(add.keep_custom)(name) {
                    continue;
                }
                if name == "name" {
                    module.section(&names(source, total, add.forwarders)?);
                } else {
                    module.section(&RawSection { id: 0, data: payload });
                }
            }
            id => {
                module.section(&RawSection { id, data: payload });
            }
        }
    }
    insert_missing(&mut module, None)?;
    // Forwarders are found by name afterwards; a module that had no
    // `name` section gets one holding theirs.
    if source.custom("name").is_none() && !add.forwarders.is_empty() && (add.keep_custom)("name") {
        let mut names = NameMap::new();
        for (i, f) in add.forwarders.iter().enumerate() {
            names.append(total + i as u32, &f.name);
        }
        let mut section = NameSection::new();
        section.functions(&names);
        module.section(&section);
    }
    Ok(module.finish())
}

/// Non-custom sections in the order a module must list them: data count
/// (12) sits before code, and tags (13) between memory and globals.
const SECTION_ORDER: [u8; 13] = [1, 2, 3, 4, 5, 13, 6, 7, 8, 9, 12, 10, 11];

/// The `name` section with the forwarders' names appended to the
/// function map. Their indices are past every existing one, so the map
/// stays in increasing order; every other subsection is unchanged.
fn names(source: &ModuleIndex<'_>, total: u32, forwarders: &[Forwarder]) -> Result<NameSection> {
    let reader = Parser::new(0)
        .parse_all(source.bytes)
        .find_map(|p| match p {
            Ok(Payload::CustomSection(c)) if c.name() == "name" => Some(c),
            _ => None,
        })
        .context("re-reading the name section")?;
    let KnownCustom::Name(subsections) = reader.as_known() else {
        bail!("the `name` section does not parse as one");
    };
    let mut out = NameSection::new();
    let mut identity = Identity;
    let mut wrote_functions = false;
    for sub in subsections {
        match sub? {
            Name::Function(map) => {
                let mut names = NameMap::new();
                for naming in map {
                    let naming = naming?;
                    names.append(naming.index, naming.name);
                }
                for (i, f) in forwarders.iter().enumerate() {
                    names.append(total + i as u32, &f.name);
                }
                out.functions(&names);
                wrote_functions = true;
            }
            sub => identity
                .parse_custom_name_subsection(&mut out, sub)
                .map_err(|e: reencode::Error<std::convert::Infallible>| anyhow::anyhow!("{e:?}"))?,
        }
    }
    ensure!(wrote_functions || forwarders.is_empty(), "the name section has no function names");
    Ok(out)
}

/// Re-encodes a name subsection unchanged: no index moves.
struct Identity;

impl Reencode for Identity {
    type Error = std::convert::Infallible;
}
