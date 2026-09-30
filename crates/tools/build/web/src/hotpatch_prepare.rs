//! Make a freshly linked wasm patch loadable — without rewriting it.
//!
//! # What changed, and why
//!
//! `wasm-ld --pie --experimental-pic` links a patch that imports what it
//! does not define (measured on a CrewForge screen crate, ~3,600 of
//! them):
//!
//! ```text
//! env.<mangled fn>                  3,462  base functions — mostly NOT exported
//! GOT.func.<sym>                      106  a base function's ADDRESS
//! GOT.mem.<sym>                         4  a base `static`'s address
//! env.memory / __stack_pointer / …      5  base exports
//! __wbindgen_placeholder__.* / other  few  wasm-bindgen's names, JS shims
//! ```
//!
//! subsecond's loader supplies exactly one namespace — `env`, the base's
//! exports plus `__memory_base`/`__table_base` — so this used to be a
//! walrus pass turning every other import into a local stub that
//! `call_indirect`s the base's table slot, and every `GOT` import into a
//! constant. Each of those rewrites renumbers the module, so walrus had
//! to parse and re-emit all of it: 0.7 s of every save on a Mac, 1.0 s in
//! the devcontainer, for a 27 MB patch.
//!
//! The page now loads the patch itself (`backend_web::hot_patch`) and can
//! supply ANY import. So nothing is rewritten: this module reads the
//! imports (a few milliseconds), decides where each one comes from, and
//! appends that decision to the module as a custom section — the
//! [`Plan`]. The page walks `WebAssembly.Module.imports(module)`, which
//! lists imports in the same order, and hands each one what the plan
//! says:
//!
//! * [`ImportSource::Runtime`] — a base export, or `__memory_base` /
//!   `__table_base`, exactly as subsecond supplied them;
//! * [`ImportSource::Slot`] — the base's function in that table slot,
//!   passed as the function itself (`table.get(slot)`). Its identity is
//!   the base's, so a `fn` pointer taken in the patch compares equal to
//!   one taken in the base, and a call is a direct call rather than the
//!   old stub's `call_indirect`;
//! * [`ImportSource::Global`] — a `GOT` entry: the base slot of a
//!   function, or the address of a base `static`;
//! * [`ImportSource::Glue`] — a web-glue import
//!   (`./__idealyst_glue.js`). Its JS is in its own name
//!   (`<key>\n<flags>\n<js>`, see `wasm_carve::glue`), so the page
//!   compiles it (`__idealystGlue.compileImport`) — a patch may call a
//!   binding the base never linked, and nothing had to be declared ahead
//!   of time. The patch's own glue records (a `js_module!` it links) are
//!   registered from its `__idealyst_glue` section; one that CHANGES a
//!   module the page already has is refused and the page reloads;
//! * [`ImportSource::Trap`] — a function that must never run (a
//!   wasm-bindgen descriptor), or one whose signature the base disagrees
//!   with. Calling it throws with the reason. The old stub trapped on
//!   the same calls, through `call_indirect`'s signature check or the
//!   null slot.
//!
//! # The two edits that remain
//!
//! Both are byte-level splices that renumber nothing:
//!
//! * **A wasm-bindgen cast the patch defines.** A crate that builds its
//!   own `Closure` instantiates `wbg_cast::breaks_if_inlined<F, T>`, and
//!   the patch carries the RAW copy — wasm-bindgen never ran over the
//!   patch, so its body is the descriptor call the base had replaced.
//!   That one body is swapped for "forward the arguments to the base's
//!   forwarder for the same instantiation" (`call_indirect` through its
//!   slot). Nothing can bind an import instead: the raw body passes its
//!   OWN address to `__wbindgen_describe_cast`. (`wasm-ld --wrap` would
//!   have redirected the callers, but it crashes lld in PIC mode.)
//! * **A start function that does more than relocate.** See
//!   [`is_relocation_only`]: the start section is dropped and
//!   `__wasm_apply_global_relocs` exported so the loader still runs it.
//!
//! Plus two strips: wasm-bindgen's descriptor sections (they describe the
//! base's bindings), and — in the served copy — the `name` section and
//! DWARF ([`crate::hotpatch_patch::strip_debug_sections`]).
//!
//! # When it cannot be done
//!
//! An import none of the above can supply is an error naming it, and the
//! caller's answer is a rebuild — always correct, where a patch
//! instantiated against a half-satisfied import set is not.

use std::collections::HashMap;
use std::ops::Range;

use anyhow::{bail, Context, Result};
use wasmparser::{BinaryReader, KnownCustom, Operator, TypeRef};

use crate::hotpatch_patch::BaseIndex;

/// The custom section the page reads the plan from.
pub const PLAN_SECTION: &str = "idealyst.hotpatch";
/// Bumped whenever [`Plan::encode`]'s layout changes; the page refuses a
/// version it does not know rather than misreading one.
pub const PLAN_VERSION: u32 = 2;

/// Where one import comes from. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportSource {
    Runtime,
    Slot(u32),
    Global { value: u32, mutable: bool },
    Trap(String),
    Glue,
}

/// What the page needs to load one patch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Bytes of linear memory the patch's data occupies from
    /// `__memory_base` (`dylink.0`'s mem-info). `None` when the module
    /// carries none; the page then reserves the module's byte length,
    /// which is what subsecond always did.
    pub memory_size: Option<u32>,
    /// Required alignment of `__memory_base`, as a power of two.
    pub memory_align: u32,
    /// One entry per import, in import-section order.
    pub imports: Vec<ImportSource>,
}

impl Plan {
    /// The section payload. Every integer is a little-endian `u32`:
    ///
    /// ```text
    /// version  has_memory_size  memory_size  memory_align  count
    /// count × ( tag:u8  [payload] )
    ///   0 runtime
    ///   1 slot     u32 slot
    ///   2 global   u32 value   (mutable)
    ///   3 global   u32 value   (immutable)
    ///   4 trap     u32 len, UTF-8 reason
    ///   5 glue     (the page compiles the import from its name)
    /// ```
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + self.imports.len() * 5);
        let u32le = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        u32le(&mut out, PLAN_VERSION);
        u32le(&mut out, self.memory_size.is_some() as u32);
        u32le(&mut out, self.memory_size.unwrap_or(0));
        u32le(&mut out, self.memory_align);
        u32le(&mut out, self.imports.len() as u32);
        for import in &self.imports {
            match import {
                ImportSource::Runtime => out.push(0),
                ImportSource::Slot(slot) => {
                    out.push(1);
                    u32le(&mut out, *slot);
                }
                ImportSource::Global { value, mutable } => {
                    out.push(if *mutable { 2 } else { 3 });
                    u32le(&mut out, *value);
                }
                ImportSource::Glue => out.push(5),
                ImportSource::Trap(why) => {
                    out.push(4);
                    u32le(&mut out, why.len() as u32);
                    out.extend_from_slice(why.as_bytes());
                }
            }
        }
        out
    }

    /// The inverse of [`Self::encode`]. The page has its own reader (it
    /// does not link this crate); this one is what the tests pin the
    /// format with.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut at = 0usize;
        let u32le = |at: &mut usize| -> Result<u32> {
            let b = bytes.get(*at..*at + 4).context("plan truncated")?;
            *at += 4;
            Ok(u32::from_le_bytes(b.try_into().expect("four bytes")))
        };
        let version = u32le(&mut at)?;
        if version != PLAN_VERSION {
            bail!("plan version {version}, expected {PLAN_VERSION}");
        }
        let has_size = u32le(&mut at)? != 0;
        let size = u32le(&mut at)?;
        let memory_align = u32le(&mut at)?;
        let count = u32le(&mut at)?;
        let mut imports = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let tag = *bytes.get(at).context("plan truncated")?;
            at += 1;
            imports.push(match tag {
                0 => ImportSource::Runtime,
                1 => ImportSource::Slot(u32le(&mut at)?),
                2 | 3 => ImportSource::Global { value: u32le(&mut at)?, mutable: tag == 2 },
                4 => {
                    let len = u32le(&mut at)? as usize;
                    let s = bytes.get(at..at + len).context("plan truncated")?;
                    at += len;
                    ImportSource::Trap(String::from_utf8_lossy(s).into_owned())
                }
                5 => ImportSource::Glue,
                other => bail!("unknown plan tag {other}"),
            });
        }
        Ok(Plan { memory_size: has_size.then_some(size), memory_align, imports })
    }
}

/// A prepared patch: the module to serve (still NAMED — strip it for
/// the wire with [`crate::hotpatch_patch::strip_debug_sections`]) and the
/// plan appended to it.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub wasm: Vec<u8>,
    pub plan: Plan,
}

/// Decide where each of `patch`'s imports comes from, splice the two
/// edits the module docs describe, and append the plan.
///
/// `patch` is `wasm-ld`'s output, as linked. Fails — naming every import
/// nothing can supply — when the patch cannot be loaded against `base`.
pub fn prepare(patch: &[u8], base: &BaseIndex) -> Result<Prepared> {
    let module = Module::read(patch)?;
    let mut unresolved: Vec<String> = Vec::new();

    let mut imports = Vec::with_capacity(module.imports.len());
    for import in &module.imports {
        imports.push(resolve_import(import, &module, base, &mut unresolved));
    }

    // Casts the patch defines: bodies to replace with a forward to the
    // base's forwarder. See the module docs.
    let mut replaced_bodies: HashMap<u32, Vec<u8>> = HashMap::new();
    for (local, &type_index) in module.func_types.iter().enumerate() {
        let func = module.func_imports + local as u32;
        let Some(name) = module.names.get(&func) else { continue };
        if !crate::hotpatch_base::is_bindgen_cast(name) {
            continue;
        }
        match base.ifunc.get(name) {
            Some(&slot) => {
                let table = module.table.context(
                    "the patch imports no `__indirect_function_table`, so there is no table to \
                     call the base through — it was linked without `--experimental-pic`",
                )?;
                let params = module.types.get(type_index as usize).context("bad type index")?.0.len();
                replaced_bodies.insert(local as u32, forward_body(params as u32, slot, type_index, table));
            }
            None => unresolved.push(format!(
                "{name} — a wasm-bindgen cast (a `Closure` or value type crossing into JS) \
                 the base never had; only wasm-bindgen can generate its import"
            )),
        }
    }

    if !unresolved.is_empty() {
        unresolved.sort();
        bail!(
            "the patch imports {} thing(s) the running module cannot supply, so it would \
             instantiate against a half-satisfied import set:\n  {}",
            unresolved.len(),
            unresolved.join("\n  "),
        );
    }

    // A start that does more than relocate would fire before the jump
    // table is committed; see `is_relocation_only`.
    let drop_start = match module.start {
        Some(start) => !is_relocation_only(patch, &module, start)?,
        None => false,
    };
    let add_export = if drop_start {
        module
            .function_named("__wasm_apply_global_relocs")
            .filter(|f| !module.exports.iter().any(|e| e.kind == 0 && e.index == *f))
    } else {
        None
    };

    let plan = Plan {
        memory_size: module.mem_info.map(|(size, _)| size),
        memory_align: module.mem_info.map(|(_, align)| align).unwrap_or(0),
        imports,
    };
    let wasm = splice(patch, &module, &replaced_bodies, drop_start, add_export, &plan)?;
    Ok(Prepared { wasm, plan })
}

fn resolve_import(
    import: &Import,
    module: &Module,
    base: &BaseIndex,
    unresolved: &mut Vec<String>,
) -> ImportSource {
    let name = import.name.as_str();
    let func_sig = match import.ty {
        TypeRef::Func(t) => Some(module.types.get(t as usize).map(sig_string).unwrap_or_default()),
        _ => None,
    };
    // A base function, passed as itself — after checking the base agrees
    // about its signature: an import of the wrong type fails the WHOLE
    // instantiation, where the old `call_indirect` stub trapped only on
    // the call.
    let slot = |slot: u32| -> ImportSource {
        match (func_sig.as_deref(), base.slot_sigs.get(&slot)) {
            (Some(want), Some(have)) if want != have => ImportSource::Trap(format!(
                "{}.{name}: the patch calls it as ({want}), the base's slot {slot} holds ({have})",
                import.module
            )),
            _ => ImportSource::Slot(slot),
        }
    };
    let global = |value: u32| -> ImportSource {
        let mutable = matches!(import.ty, TypeRef::Global(g) if g.mutable);
        ImportSource::Global { value, mutable }
    };

    match import.module.as_str() {
        // wasm-ld's PIC indirection for a function's address. The value
        // it wants is a table index.
        "GOT.func" => match (base.ifunc.get(name), import.ty) {
            (Some(&index), TypeRef::Global(_)) => global(index),
            (None, _) => {
                unresolved.push(format!("GOT.func.{name} — {}", base.diagnose(name)));
                ImportSource::Runtime
            }
            (_, _) => {
                unresolved.push(format!("GOT.func.{name} (not a global)"));
                ImportSource::Runtime
            }
        },

        // Same, for a `static` in a crate the patch did not recompile.
        "GOT.mem" => match (base.data.get(name), import.ty) {
            (Some(&address), TypeRef::Global(_)) => global(address),
            (None, _) => {
                unresolved.push(format!(
                    "GOT.mem.{name} (no data symbol of that name in the base — was it linked \
                     without `--emit-relocs`?)"
                ));
                ImportSource::Runtime
            }
            (_, _) => {
                unresolved.push(format!("GOT.mem.{name} (not a global)"));
                ImportSource::Runtime
            }
        },

        // web-glue: the snippet is in the name, the page compiles it.
        wasm_carve::glue::IMPORT_MODULE => match (
            wasm_carve::glue::parse_import_name(name),
            &func_sig,
        ) {
            (Ok(_), Some(_)) => ImportSource::Glue,
            (Err(e), _) => {
                unresolved.push(format!("{}.{name:?}: {e:#}", import.module));
                ImportSource::Runtime
            }
            (Ok(_), None) => {
                unresolved.push(format!("{}.{name:?} (a glue import that is not a function)", import.module));
                ImportSource::Runtime
            }
        },

        // wasm-bindgen's namespace, which the runtime does not provide.
        "__wbindgen_placeholder__" | "__wbindgen_externref_xform__" => {
            if crate::hotpatch_base::is_bindgen_internal(name) {
                // A descriptor symbol. wasm-bindgen interprets these at
                // bindgen time and deletes them, so the base has none;
                // the patch only imports one because `--no-gc-sections`
                // kept the machinery that references it. Nothing calls it
                // at run time — and if something does, it throws saying
                // so rather than running whatever sits at some plausible
                // slot.
                if func_sig.is_some() {
                    ImportSource::Trap(format!(
                        "{}.{name}: a wasm-bindgen descriptor, which only runs at bindgen time",
                        import.module
                    ))
                } else {
                    unresolved.push(format!(
                        "{}.{name} (a descriptor symbol that is not a function)",
                        import.module
                    ));
                    ImportSource::Runtime
                }
            } else if let (Some(&index), Some(_)) = (
                base.ifunc
                    .get(name)
                    .or_else(|| base.ifunc.get(&crate::hotpatch_base::shim_trampoline_name(name))),
                &func_sig,
            ) {
                // Either the function itself, or — for a JS shim, which
                // is an import in the base and has no body of its own —
                // the forwarding body `hotpatch_base` rooted for it.
                slot(index)
            } else {
                unresolved.push(format!(
                    "{}.{name} (the base has no function of that name in its table — if it is a \
                     raw JS shim it exists only as an import there, and a patch cannot reach it)",
                    import.module
                ));
                ImportSource::Runtime
            }
        }

        // The runtime's own namespace: a base export needs nothing, and a
        // private function is reached through its table slot.
        "env" => {
            if base.exports.contains(name) || name == "__memory_base" || name == "__table_base" {
                return ImportSource::Runtime;
            }
            match (base.ifunc.get(name), &func_sig) {
                (Some(&index), Some(_)) => slot(index),
                _ => {
                    unresolved.push(format!("env.{name} — {}", base.diagnose(name)));
                    ImportSource::Runtime
                }
            }
        }

        // Any other namespace is an import the base has too — a
        // `#[component(lazy)]` loader from `./__wasm_split.js`, say — and
        // is reached through the forwarding body `hotpatch_base` rooted.
        other => match (base.ifunc.get(&crate::hotpatch_base::shim_trampoline_name(name)), &func_sig) {
            (Some(&index), Some(_)) => slot(index),
            _ => {
                unresolved.push(format!(
                    "{other}.{name} (the base has no forwarding body for this import in its table)"
                ));
                ImportSource::Runtime
            }
        },
    }
}

/// Whether `start` only relocates the patch itself: it IS
/// `__wasm_apply_global_relocs`, or a body made solely of calls to that
/// and `__wasm_init_memory`.
///
/// Why that one start is kept: under `--pie`, wasm-ld makes
/// `__wasm_apply_global_relocs` the start function. It adds `__table_base`
/// to every `GOT.func.internal.*` global — the address of a function the
/// PATCH defines, which the linker can only write relative to the
/// patch's own element segment. Dropped, those globals stay relative: on
/// the lab, `<Option<u64> as Debug>::fmt` reached `format!` as fn pointer
/// 0 and the first render trapped with "null function". With any
/// zero-initialized static the start is `__wasm_start`, calling that and
/// `__wasm_init_memory` (which zero-fills the patch's `.bss`). Both touch
/// only the patch's own globals and freshly grown memory.
///
/// Anything else in `start` would fire at instantiation, before the jump
/// table is committed, initializing state the base already initialized
/// against a half-applied patch — so that start is dropped, and the
/// relocations exported for the loader to run instead.
fn is_relocation_only(wasm: &[u8], module: &Module, start: u32) -> Result<bool> {
    const RELOCATION: [&str; 2] = ["__wasm_apply_global_relocs", "__wasm_init_memory"];
    let name_of = |f: u32| module.names.get(&f).map(String::as_str);
    if name_of(start) == Some("__wasm_apply_global_relocs") {
        return Ok(true);
    }
    let Some(local) = start.checked_sub(module.func_imports) else {
        return Ok(false);
    };
    let Some(range) = module.bodies.get(local as usize) else {
        return Ok(false);
    };
    let body = wasmparser::FunctionBody::new(BinaryReader::new(&wasm[range.clone()], range.start));
    let mut ops = body.get_operators_reader()?;
    let mut calls = 0;
    while !ops.eof() {
        match ops.read()? {
            Operator::Call { function_index } => {
                if !name_of(function_index).is_some_and(|n| RELOCATION.contains(&n)) {
                    return Ok(false);
                }
                calls += 1;
            }
            Operator::End => {}
            _ => return Ok(false),
        }
    }
    Ok(calls > 0)
}

/// A function body forwarding its `params` arguments to table slot
/// `slot`, called with type `type_index` through table `table`.
fn forward_body(params: u32, slot: u32, type_index: u32, table: u32) -> Vec<u8> {
    let mut body = Vec::new();
    leb_u32(&mut body, 0); // no locals beyond the parameters
    for i in 0..params {
        body.push(0x20); // local.get
        leb_u32(&mut body, i);
    }
    // The callee index goes on the stack last — `call_indirect` pops it
    // first, then the arguments beneath it.
    body.push(0x41); // i32.const
    leb_i32(&mut body, slot as i32);
    body.push(0x11); // call_indirect
    leb_u32(&mut body, type_index);
    leb_u32(&mut body, table);
    body.push(0x0b); // end
    body
}

/// Copy `wasm` section by section, applying the edits, and append the
/// plan. Nothing is renumbered: every edit replaces one section's
/// contents or drops a section whole.
fn splice(
    wasm: &[u8],
    module: &Module,
    replaced_bodies: &HashMap<u32, Vec<u8>>,
    drop_start: bool,
    add_export: Option<u32>,
    plan: &Plan,
) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(wasm.len() + plan.imports.len() * 5 + 64);
    out.extend_from_slice(&wasm[..8]);
    let mut exports_written = add_export.is_none();

    let write_exports = |out: &mut Vec<u8>, extra: u32| {
        let mut contents = Vec::new();
        leb_u32(&mut contents, module.exports.len() as u32 + 1);
        for e in &module.exports {
            leb_u32(&mut contents, e.name.len() as u32);
            contents.extend_from_slice(e.name.as_bytes());
            contents.push(e.kind);
            leb_u32(&mut contents, e.index);
        }
        let name = "__wasm_apply_global_relocs";
        leb_u32(&mut contents, name.len() as u32);
        contents.extend_from_slice(name.as_bytes());
        contents.push(0); // func
        leb_u32(&mut contents, extra);
        section(out, 7, &contents);
    };

    for s in &module.sections {
        // An export section to append to that the module does not have:
        // it goes where one would be, before the first section that
        // must follow it.
        if !exports_written && s.id != 0 && s.id != 7 && order(s.id) > order(7) {
            write_exports(&mut out, add_export.expect("checked"));
            exports_written = true;
        }
        match s.id {
            0 if s.custom_name.as_deref().is_some_and(|n| n.contains("__wasm_bindgen")) => {
                // wasm-bindgen's descriptor section describes the BASE's
                // bindings; nothing reads it here, and it is bytes the
                // browser would download on every save.
            }
            7 if !exports_written => {
                write_exports(&mut out, add_export.expect("checked"));
                exports_written = true;
            }
            8 if drop_start => {}
            10 if !replaced_bodies.is_empty() => {
                let mut contents = Vec::new();
                leb_u32(&mut contents, module.bodies.len() as u32);
                for (i, range) in module.bodies.iter().enumerate() {
                    let body: &[u8] = match replaced_bodies.get(&(i as u32)) {
                        Some(new) => new,
                        None => &wasm[range.clone()],
                    };
                    leb_u32(&mut contents, body.len() as u32);
                    contents.extend_from_slice(body);
                }
                section(&mut out, 10, &contents);
            }
            _ => out.extend_from_slice(&wasm[s.whole.clone()]),
        }
    }
    if !exports_written {
        write_exports(&mut out, add_export.expect("checked"));
    }

    let mut custom = Vec::new();
    leb_u32(&mut custom, PLAN_SECTION.len() as u32);
    custom.extend_from_slice(PLAN_SECTION.as_bytes());
    custom.extend_from_slice(&plan.encode());
    section(&mut out, 0, &custom);
    Ok(out)
}

/// A section's position in the order the spec requires (custom sections
/// excepted — they may go anywhere).
fn order(id: u8) -> u8 {
    match id {
        1 => 1,   // type
        2 => 2,   // import
        3 => 3,   // function
        4 => 4,   // table
        5 => 5,   // memory
        13 => 6,  // tag
        6 => 7,   // global
        7 => 8,   // export
        8 => 9,   // start
        9 => 10,  // element
        12 => 11, // data count
        10 => 12, // code
        11 => 13, // data
        _ => 0,
    }
}

// ── reading ─────────────────────────────────────────────────────────────

struct Section {
    id: u8,
    /// From the id byte to the end of the contents.
    whole: Range<usize>,
    custom_name: Option<String>,
}

struct Import {
    module: String,
    name: String,
    ty: TypeRef,
}

#[derive(Clone)]
struct Export {
    name: String,
    kind: u8,
    index: u32,
}

/// What [`prepare`] needs from the linked patch, read once.
struct Module {
    sections: Vec<Section>,
    /// Each type as (params, results) signature strings' sources.
    types: Vec<(Vec<char>, Vec<char>)>,
    imports: Vec<Import>,
    func_imports: u32,
    /// Type index of each LOCAL function.
    func_types: Vec<u32>,
    /// Index of the imported `__indirect_function_table`.
    table: Option<u32>,
    exports: Vec<Export>,
    start: Option<u32>,
    names: HashMap<u32, String>,
    /// Each local function's body (after the size prefix).
    bodies: Vec<Range<usize>>,
    /// `dylink.0` mem-info: (memory size, alignment as a power of two).
    mem_info: Option<(u32, u32)>,
}

impl Module {
    fn read(wasm: &[u8]) -> Result<Self> {
        let mut m = Module {
            sections: Vec::new(),
            types: Vec::new(),
            imports: Vec::new(),
            func_imports: 0,
            func_types: Vec::new(),
            table: None,
            exports: Vec::new(),
            start: None,
            names: HashMap::new(),
            bodies: Vec::new(),
            mem_info: None,
        };
        let mut table_imports = 0u32;
        for payload in wasmparser::Parser::new(0).parse_all(wasm) {
            use wasmparser::Payload::*;
            match payload.context("parsing the linked patch")? {
                TypeSection(reader) => {
                    for ty in reader.into_iter_err_on_gc_types() {
                        let ty = ty.context("parsing a type")?;
                        m.types.push((
                            ty.params().iter().map(val_char).collect(),
                            ty.results().iter().map(val_char).collect(),
                        ));
                    }
                }
                ImportSection(reader) => {
                    for import in reader {
                        let import = import.context("parsing an import")?;
                        match import.ty {
                            TypeRef::Func(_) => m.func_imports += 1,
                            TypeRef::Table(_) => {
                                if import.module == "env" && import.name == "__indirect_function_table" {
                                    m.table = Some(table_imports);
                                }
                                table_imports += 1;
                            }
                            _ => {}
                        }
                        m.imports.push(Import {
                            module: import.module.to_string(),
                            name: import.name.to_string(),
                            ty: import.ty,
                        });
                    }
                }
                FunctionSection(reader) => {
                    for ty in reader {
                        m.func_types.push(ty.context("parsing a function type index")?);
                    }
                }
                ExportSection(reader) => {
                    for e in reader {
                        let e = e.context("parsing an export")?;
                        m.exports.push(Export {
                            name: e.name.to_string(),
                            kind: e.kind as u8,
                            index: e.index,
                        });
                    }
                }
                StartSection { func, .. } => m.start = Some(func),
                CodeSectionEntry(body) => m.bodies.push(body.range()),
                CustomSection(c) => match c.as_known() {
                    KnownCustom::Name(reader) => {
                        for sub in reader {
                            let Ok(wasmparser::Name::Function(map)) = sub else { continue };
                            for naming in map {
                                let naming = naming.context("parsing a function name")?;
                                m.names.insert(naming.index, naming.name.to_string());
                            }
                        }
                    }
                    KnownCustom::Dylink0(reader) => {
                        for sub in reader {
                            if let Ok(wasmparser::Dylink0Subsection::MemInfo(info)) = sub {
                                m.mem_info = Some((info.memory_size, info.memory_alignment));
                            }
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        m.sections = sections(wasm)?;
        Ok(m)
    }

    fn function_named(&self, name: &str) -> Option<u32> {
        self.names.iter().find(|(_, n)| n.as_str() == name).map(|(i, _)| *i)
    }
}

/// The module's top-level sections, walked directly: `wasmparser` gives
/// each section's contents but not where its header starts, and a
/// verbatim copy needs both.
fn sections(wasm: &[u8]) -> Result<Vec<Section>> {
    let mut out = Vec::new();
    let mut at = 8usize;
    while at < wasm.len() {
        let id = wasm[at];
        let (len, len_bytes) = read_leb(&wasm[at + 1..]).context("a section size")?;
        let contents = at + 1 + len_bytes;
        let end = contents + len as usize;
        if end > wasm.len() {
            bail!("section {id} at byte {at} runs past the end of the module");
        }
        let custom_name = (id == 0)
            .then(|| {
                let (n, nb) = read_leb(&wasm[contents..]).ok()?;
                let start = contents + nb;
                std::str::from_utf8(wasm.get(start..start + n as usize)?).ok().map(str::to_string)
            })
            .flatten();
        out.push(Section { id, whole: at..end, custom_name });
        at = end;
    }
    Ok(out)
}

/// One character per value type, so two signatures compare as strings
/// ([`BaseIndex::slot_sigs`] uses the same alphabet).
pub(crate) fn val_char(v: &wasmparser::ValType) -> char {
    match v {
        wasmparser::ValType::I32 => 'i',
        wasmparser::ValType::I64 => 'I',
        wasmparser::ValType::F32 => 'f',
        wasmparser::ValType::F64 => 'F',
        wasmparser::ValType::V128 => 'v',
        wasmparser::ValType::Ref(r) if r.is_extern_ref() => 'x',
        wasmparser::ValType::Ref(r) if r.is_func_ref() => 'r',
        wasmparser::ValType::Ref(_) => '?',
    }
}

/// `params>results`, in [`val_char`]'s alphabet.
pub(crate) fn sig_string((params, results): &(Vec<char>, Vec<char>)) -> String {
    let mut s: String = params.iter().collect();
    s.push('>');
    s.extend(results.iter());
    s
}

fn read_leb(bytes: &[u8]) -> Result<(u32, usize)> {
    let (mut value, mut shift) = (0u64, 0u32);
    for (i, b) in bytes.iter().enumerate().take(5) {
        value |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok((value as u32, i + 1));
        }
        shift += 7;
    }
    bail!("malformed LEB128")
}

fn section(out: &mut Vec<u8>, id: u8, contents: &[u8]) {
    out.push(id);
    leb_u32(out, contents.len() as u32);
    out.extend_from_slice(contents);
}

fn leb_u32(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn leb_i32(out: &mut Vec<u8>, mut v: i32) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use walrus::{
        ir, ConstExpr, ElementItems, ElementKind, FunctionBuilder, ImportKind, Module as WModule,
        RefType, ValType,
    };

    /// A patch-shaped module: a PIC element segment offset by an imported
    /// `__table_base`, the base's table imported as
    /// `env.__indirect_function_table`, plus whatever imports the test
    /// wants. Built with walrus — a test fixture, not the code under test.
    fn patch_module(imports: &[(&str, &str)]) -> WModule {
        let mut module = WModule::default();
        let (table, _) = module.add_import_table(
            "env",
            "__indirect_function_table",
            false,
            0,
            None,
            RefType::FUNCREF,
        );
        let table_base = module.add_import_global("env", "__table_base", ValType::I32, false, false);
        let ty = module.types.add(&[], &[]);
        let mut called = Vec::new();
        for (namespace, name) in imports {
            if namespace.starts_with("GOT.") {
                module.add_import_global(namespace, name, ValType::I32, true, false);
            } else {
                let (func, _) = module.add_import_func(namespace, name, ty);
                called.push(func);
            }
        }
        // Calls each imported function: walrus drops an import nothing
        // calls on emit, and a real patch imports only what it calls.
        let mut builder = FunctionBuilder::new(&mut module.types, &[ValType::I32], &[ValType::I32]);
        let arg = module.locals.add(ValType::I32);
        {
            let mut body = builder.name("__Probe_hot_impl".to_string()).func_body();
            for func in called {
                body.call(func);
            }
            body.local_get(arg);
        }
        let own = module.funcs.add_local(builder.local_func(vec![arg]));
        module.elements.add(
            ElementKind::Active { table, offset: ConstExpr::Global(table_base.0) },
            ElementItems::Functions(vec![own]),
        );
        module
    }

    fn patch(imports: &[(&str, &str)]) -> Vec<u8> {
        patch_module(imports).emit_wasm()
    }

    fn base_with(ifunc: &[(&str, u32)], exports: &[&str], data: &[(&str, u32)]) -> BaseIndex {
        // Every real base exports these three — `--export-memory`,
        // `--export-table` and `--export=__stack_pointer`.
        let standard = ["memory", "__indirect_function_table", "__stack_pointer"];
        BaseIndex {
            ifunc: ifunc.iter().map(|(n, i)| (n.to_string(), *i)).collect(),
            exports: standard.iter().chain(exports.iter()).map(|s| s.to_string()).collect(),
            data: data.iter().map(|(n, a)| (n.to_string(), *a)).collect(),
            slot_sigs: Default::default(),
            all_funcs: ifunc.iter().map(|(n, _)| n.to_string()).collect(),
        }
    }

    /// Each import, as `module.name`, paired with the plan's source for it.
    fn sources(prepared: &Prepared) -> Vec<(String, ImportSource)> {
        let module = Module::read(&prepared.wasm).unwrap();
        assert_eq!(module.imports.len(), prepared.plan.imports.len(), "one plan entry per import");
        module
            .imports
            .iter()
            .map(|i| format!("{}.{}", i.module, i.name))
            .zip(prepared.plan.imports.iter().cloned())
            .collect()
    }

    fn source_of(prepared: &Prepared, import: &str) -> ImportSource {
        sources(prepared)
            .into_iter()
            .find(|(n, _)| n == import)
            .unwrap_or_else(|| panic!("no import {import}"))
            .1
    }

    /// Every prepared module must still be a valid module whose imports
    /// are the linked patch's, in the same order: the page pairs the plan
    /// with `WebAssembly.Module.imports`, so a reordered or dropped import
    /// would hand every later one the wrong value.
    fn check_intact(raw: &[u8], prepared: &Prepared) {
        wasmparser::Validator::new().validate_all(&prepared.wasm).expect("a valid module");
        let names = |w: &[u8]| {
            Module::read(w).unwrap().imports.iter().map(|i| format!("{}.{}", i.module, i.name)).collect::<Vec<_>>()
        };
        assert_eq!(names(raw), names(&prepared.wasm), "the imports moved");
        let section = Module::read(&prepared.wasm)
            .unwrap()
            .sections
            .iter()
            .filter(|s| s.custom_name.as_deref() == Some(PLAN_SECTION))
            .count();
        assert_eq!(section, 1, "exactly one plan section");
        // And the section decodes to the plan that was returned.
        let payload = wasmparser::Parser::new(0)
            .parse_all(&prepared.wasm)
            .find_map(|p| match p.unwrap() {
                wasmparser::Payload::CustomSection(c) if c.name() == PLAN_SECTION => Some(c.data().to_vec()),
                _ => None,
            })
            .unwrap();
        assert_eq!(Plan::decode(&payload).unwrap(), prepared.plan);
    }

    fn prepare_ok(raw: &[u8], base: &BaseIndex) -> Prepared {
        let prepared = prepare(raw, base).unwrap_or_else(|e| panic!("should prepare: {e:#}"));
        check_intact(raw, &prepared);
        prepared
    }

    /// The core case. Every private Rust `fn` has internal linkage, so no
    /// linker flag exports it — but `hotpatch_base` put it in the table,
    /// and the page hands the patch the function in that slot.
    #[test]
    fn a_function_the_base_does_not_export_is_its_table_slot() {
        let raw = patch(&[("env", "_RNvCs_4core9panicking5panic")]);
        let base = base_with(&[("_RNvCs_4core9panicking5panic", 412)], &[], &[]);
        let prepared = prepare_ok(&raw, &base);
        assert_eq!(source_of(&prepared, "env._RNvCs_4core9panicking5panic"), ImportSource::Slot(412));
    }

    /// A base export (and the two bases) is the runtime's to supply.
    #[test]
    fn an_import_the_base_exports_is_the_runtimes() {
        let raw = patch(&[("env", "memory_intrinsic")]);
        let prepared = prepare_ok(&raw, &base_with(&[], &["memory_intrinsic"], &[]));
        assert_eq!(source_of(&prepared, "env.memory_intrinsic"), ImportSource::Runtime);
        assert_eq!(source_of(&prepared, "env.__table_base"), ImportSource::Runtime);
    }

    /// wasm-bindgen's namespace: the base has the function itself in its
    /// table, rooted there by `hotpatch_base`.
    #[test]
    fn a_wasm_bindgen_placeholder_is_its_table_slot() {
        let raw = patch(&[("__wbindgen_placeholder__", "__wbindgen_exn_store")]);
        let prepared = prepare_ok(&raw, &base_with(&[("__wbindgen_exn_store", 91)], &[], &[]));
        assert_eq!(
            source_of(&prepared, "__wbindgen_placeholder__.__wbindgen_exn_store"),
            ImportSource::Slot(91)
        );
    }

    /// A web-glue import carries its JS in its name, so the page compiles
    /// it — including a binding the base never linked (decision 1 of the
    /// phase-2 brief: no pre-declared bindings). A glue import whose name
    /// is not `<key>\n<flags>\n<js>` is an error naming it.
    #[test]
    fn a_web_glue_import_is_compiled_by_the_page() {
        let name = "app::js_poke(u32)\n\n(h) => G.get(h).poke()";
        let raw = patch(&[(wasm_carve::glue::IMPORT_MODULE, name)]);
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        assert_eq!(
            source_of(&prepared, &format!("{}.{name}", wasm_carve::glue::IMPORT_MODULE)),
            ImportSource::Glue
        );
        let plan = Plan::decode(&Plan { memory_size: None, memory_align: 0, imports: vec![ImportSource::Glue] }.encode())
            .unwrap();
        assert_eq!(plan.imports, [ImportSource::Glue], "tag 5 round-trips");

        let bad = patch(&[(wasm_carve::glue::IMPORT_MODULE, "no-key-line")]);
        let message = format!("{:#}", prepare(&bad, &base_with(&[], &[], &[])).unwrap_err());
        assert!(message.contains("no-key-line"), "{message}");
    }

    /// A descriptor symbol runs only at bindgen time; the base has none.
    /// It traps if called, saying what it is — where the old stub jumped
    /// to the null slot.
    #[test]
    fn a_wasm_bindgen_descriptor_traps() {
        let raw = patch(&[("__wbindgen_placeholder__", "__wbindgen_describe")]);
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        match source_of(&prepared, "__wbindgen_placeholder__.__wbindgen_describe") {
            ImportSource::Trap(why) => assert!(why.contains("descriptor"), "{why}"),
            other => panic!("expected a trap, got {other:?}"),
        }
    }

    /// A raw JS shim exists only as an IMPORT in the base: there is no
    /// slot, and no honest way to fake one. Named, so the rebuild says why.
    #[test]
    fn a_js_shim_the_base_only_imports_is_an_error_naming_it() {
        let raw = patch(&[("__wbindgen_placeholder__", "__wbg_log_51db52")]);
        let message = format!("{:#}", prepare(&raw, &base_with(&[], &[], &[])).unwrap_err());
        assert!(message.contains("__wbg_log_51db52"), "{message}");
        assert!(message.contains("raw JS shim"), "{message}");
    }

    /// wasm-ld takes a function's address through a GOT global under PIC.
    /// The value it wants is the base's table index — and a patch-taken
    /// pointer then compares equal to a base-taken one.
    #[test]
    fn a_got_func_global_is_the_base_table_index() {
        let raw = patch(&[("GOT.func", "_RNvCs_3app7handler")]);
        let prepared = prepare_ok(&raw, &base_with(&[("_RNvCs_3app7handler", 77)], &[], &[]));
        assert_eq!(
            source_of(&prepared, "GOT.func._RNvCs_3app7handler"),
            ImportSource::Global { value: 77, mutable: true }
        );
    }

    /// A `static` in a crate the patch did not recompile: its absolute
    /// address in the base.
    #[test]
    fn a_got_mem_global_is_the_base_data_address() {
        let raw = patch(&[("GOT.mem", "_RNvCs_3app8GREETING")]);
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[("_RNvCs_3app8GREETING", 1_048_612)]));
        assert_eq!(
            source_of(&prepared, "GOT.mem._RNvCs_3app8GREETING"),
            ImportSource::Global { value: 1_048_612, mutable: true }
        );
    }

    /// The failure that must never be silent: an import nothing can
    /// supply. The caller rebuilds, and needs to know which symbol forced
    /// it.
    #[test]
    fn an_import_nothing_can_supply_is_an_error_naming_it() {
        let raw = patch(&[("env", "_RNvCs_3app12never_existed")]);
        let message = format!("{:#}", prepare(&raw, &base_with(&[], &[], &[])).unwrap_err());
        assert!(message.contains("never_existed"), "{message}");
        assert!(message.contains("does not contain this function at all"), "{message}");
    }

    /// A lazy loader from `./__wasm_split.js`: the runtime provides only
    /// `env`, so it is the base's forwarding body for it.
    #[test]
    fn an_import_from_another_namespace_is_its_trampolines_slot() {
        let name = "__wasm_split_00_lazy_body";
        let tramp = crate::hotpatch_base::shim_trampoline_name(name);
        let raw = patch(&[("./__wasm_split.js", name)]);
        let prepared = prepare_ok(&raw, &base_with(&[(tramp.as_str(), 5)], &[], &[]));
        assert_eq!(source_of(&prepared, &format!("./__wasm_split.js.{name}")), ImportSource::Slot(5));
    }

    /// The page passes a base function as ITSELF, and an import of the
    /// wrong type fails the whole instantiation. A signature the base
    /// disagrees with becomes a trap on call — what the old stub's
    /// `call_indirect` signature check did — and a matching one a slot.
    #[test]
    fn a_signature_the_base_disagrees_with_traps_instead_of_failing_the_load() {
        let raw = patch(&[("env", "_RNvCs_3app5other")]);
        let mut base = base_with(&[("_RNvCs_3app5other", 12)], &[], &[]);
        base.slot_sigs.insert(12, "i>".to_string());
        match source_of(&prepare_ok(&raw, &base), "env._RNvCs_3app5other") {
            ImportSource::Trap(why) => assert!(why.contains("slot 12") && why.contains("(i>)"), "{why}"),
            other => panic!("expected a trap, got {other:?}"),
        }
        base.slot_sigs.insert(12, ">".to_string());
        assert_eq!(source_of(&prepare_ok(&raw, &base), "env._RNvCs_3app5other"), ImportSource::Slot(12));
    }

    const CAST: &str = "_RINvNvNtCs8e_12wasm_bindgen4___rt8wbg_cast17breaks_if_inlinedReNtB6_7JsValueEB6_";

    /// A module whose own crate instantiated a cast intrinsic, taking an
    /// argument so the forward has something to pass.
    fn patch_with_cast() -> Vec<u8> {
        let mut module = patch_module(&[]);
        let mut b = FunctionBuilder::new(&mut module.types, &[ValType::I32], &[ValType::I32]);
        let arg = module.locals.add(ValType::I32);
        b.name(CAST.to_string()).func_body().unreachable();
        let cast = module.funcs.add_local(b.local_func(vec![arg]));
        let seg = module.elements.iter().next().unwrap().id();
        if let ElementItems::Functions(ids) = &mut module.elements.get_mut(seg).items {
            ids.push(cast);
        }
        module.emit_wasm()
    }

    /// Regression: a patch whose crate instantiated a cast intrinsic
    /// carries a RAW copy, unprocessed by wasm-bindgen; kept as is, the
    /// first patched code to create that closure would trap. Its body is
    /// spliced to forward to the base's forwarder — its argument, then
    /// the slot, then `call_indirect` — and a cast the base never had is
    /// refused.
    #[test]
    fn regression_a_cast_the_patch_defines_forwards_to_the_bases_forwarder() {
        let raw = patch_with_cast();
        let mut base = base_with(&[], &[], &[]);
        base.ifunc.insert(CAST.to_string(), 41);
        let prepared = prepare_ok(&raw, &base);

        let module = Module::read(&prepared.wasm).unwrap();
        let cast = *module.names.iter().find(|(_, n)| n.as_str() == CAST).unwrap().0;
        let range = module.bodies[(cast - module.func_imports) as usize].clone();
        let body = wasmparser::FunctionBody::new(BinaryReader::new(&prepared.wasm[range.clone()], range.start));
        let ops: Vec<String> = {
            let mut r = body.get_operators_reader().unwrap();
            let mut v = Vec::new();
            while !r.eof() {
                v.push(format!("{:?}", r.read().unwrap()));
            }
            v
        };
        assert_eq!(ops.len(), 4, "{ops:?}");
        assert!(ops[0].starts_with("LocalGet { local_index: 0 }"), "{ops:?}");
        assert!(ops[1].starts_with("I32Const { value: 41 }"), "{ops:?}");
        assert!(ops[2].starts_with("CallIndirect"), "{ops:?}");

        let message = format!("{:#}", prepare(&raw, &base_with(&[], &[], &[])).unwrap_err());
        assert!(message.contains("wasm-bindgen cast"), "{message}");
    }

    /// A module with `start` set to a new function calling `names`.
    fn start_calling(names: &[&str]) -> Vec<u8> {
        let mut module = patch_module(&[]);
        let mut callees = Vec::new();
        for name in names {
            let mut b = FunctionBuilder::new(&mut module.types, &[], &[]);
            b.name((*name).to_string()).func_body();
            callees.push(module.funcs.add_local(b.local_func(vec![])));
        }
        let mut b = FunctionBuilder::new(&mut module.types, &[], &[]);
        let mut body = b.name("__wasm_start".to_string()).func_body();
        for c in &callees {
            body.call(*c);
        }
        module.start = Some(module.funcs.add_local(b.local_func(vec![])));
        module.emit_wasm()
    }

    fn exports_of(wasm: &[u8]) -> Vec<String> {
        Module::read(wasm).unwrap().exports.iter().map(|e| e.name.clone()).collect()
    }

    /// A start that does anything but relocate would fire during
    /// instantiation — before the jump table is committed — re-running
    /// initialization the base already did. Dropped.
    #[test]
    fn the_patch_never_keeps_an_arbitrary_start_function() {
        let mut module = patch_module(&[]);
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        builder.name("ctor".to_string()).func_body();
        module.start = Some(module.funcs.add_local(builder.local_func(vec![])));
        let raw = module.emit_wasm();
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        assert!(Module::read(&prepared.wasm).unwrap().start.is_none());
    }

    /// Regression (the E2E fixture): a patch with a zero-initialized
    /// static starts with `__wasm_start` = global relocs +
    /// `__wasm_init_memory`. Dropping it left the internal GOT relative,
    /// and the first render trapped with "null function".
    #[test]
    fn regression_a_relocating_wasm_start_wrapper_is_kept() {
        let raw = start_calling(&["__wasm_apply_global_relocs", "__wasm_init_memory"]);
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        let module = Module::read(&prepared.wasm).unwrap();
        let start = module.start.expect("the relocating wrapper was dropped");
        assert_eq!(module.names.get(&start).map(String::as_str), Some("__wasm_start"));
        assert!(!exports_of(&prepared.wasm).contains(&"__wasm_apply_global_relocs".to_string()),
            "kept AND exported would relocate twice");
    }

    /// A start that also runs ctors is dropped, and the relocations it
    /// would have run are exported for the loader.
    #[test]
    fn a_start_that_runs_ctors_is_dropped_but_relocations_are_exported() {
        let raw = start_calling(&["__wasm_apply_global_relocs", "__wasm_call_ctors"]);
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        assert!(Module::read(&prepared.wasm).unwrap().start.is_none());
        assert!(exports_of(&prepared.wasm).contains(&"__wasm_apply_global_relocs".to_string()),
            "{:?}", exports_of(&prepared.wasm));
    }

    /// Regression: the first patch on the lab trapped with "null function"
    /// in `core::fmt::write` — `__wasm_apply_global_relocs`, the start,
    /// rebases every `GOT.func.internal.*` global by `__table_base`, and
    /// clearing it left `<Option<u64> as Debug>::fmt` as fn pointer 0.
    #[test]
    fn regression_the_global_reloc_start_function_survives() {
        let mut module = patch_module(&[]);
        let got = module.globals.add_local(ValType::I32, true, false, ConstExpr::Value(ir::Value::I32(0)));
        let table_base = module
            .imports
            .iter()
            .find_map(|i| match (i.name.as_str(), &i.kind) {
                ("__table_base", ImportKind::Global(g)) => Some(*g),
                _ => None,
            })
            .unwrap();
        let mut builder = FunctionBuilder::new(&mut module.types, &[], &[]);
        builder
            .name("__wasm_apply_global_relocs".to_string())
            .func_body()
            .global_get(table_base)
            .global_get(got)
            .binop(ir::BinaryOp::I32Add)
            .global_set(got);
        module.start = Some(module.funcs.add_local(builder.local_func(vec![])));
        let raw = module.emit_wasm();
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        let module = Module::read(&prepared.wasm).unwrap();
        let start = module.start.expect("the global-reloc start function was dropped");
        assert_eq!(module.names.get(&start).map(String::as_str), Some("__wasm_apply_global_relocs"));
    }

    /// wasm-bindgen's descriptor section describes the base's bindings and
    /// is dropped; every other custom section is kept.
    #[test]
    fn wasm_bindgen_sections_are_dropped() {
        let mut module = patch_module(&[]);
        module.customs.add(walrus::RawCustomSection { name: "__wasm_bindgen_unstable".into(), data: vec![1, 2, 3] });
        module.customs.add(walrus::RawCustomSection { name: "producers2".into(), data: vec![4] });
        let raw = module.emit_wasm();
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        let names: Vec<_> = Module::read(&prepared.wasm).unwrap().sections.iter().filter_map(|s| s.custom_name.clone()).collect();
        assert!(!names.iter().any(|n| n.contains("__wasm_bindgen")), "{names:?}");
        assert!(names.contains(&"producers2".to_string()), "{names:?}");
    }

    /// The page reserves exactly the patch's data, from `dylink.0`, rather
    /// than the module's byte length — which is what grew the page's
    /// memory by the whole patch on every save.
    #[test]
    fn the_plan_carries_dylink_memory_info() {
        let mut raw = patch(&[]);
        // dylink.0 { mem-info (1): size 4096, align 2^3, table 1, table-align 0 }
        let mut body = Vec::new();
        leb_u32(&mut body, b"dylink.0".len() as u32);
        body.extend_from_slice(b"dylink.0");
        let mut meminfo = Vec::new();
        for v in [4096u32, 3, 1, 0] {
            leb_u32(&mut meminfo, v);
        }
        body.push(1);
        leb_u32(&mut body, meminfo.len() as u32);
        body.extend_from_slice(&meminfo);
        section(&mut raw, 0, &body);
        let prepared = prepare_ok(&raw, &base_with(&[], &[], &[]));
        assert_eq!(prepared.plan.memory_size, Some(4096));
        assert_eq!(prepared.plan.memory_align, 3);
    }

    /// The plan's bytes, pinned. `backend_web::hot_patch::decode_plan`
    /// reads this exact layout; change one without the other and the page
    /// misreads every import. The version number exists for that change.
    #[test]
    fn the_plan_layout_is_pinned() {
        let plan = Plan {
            memory_size: Some(0x0102),
            memory_align: 3,
            imports: vec![
                ImportSource::Runtime,
                ImportSource::Slot(7),
                ImportSource::Global { value: 9, mutable: true },
                ImportSource::Global { value: 10, mutable: false },
                ImportSource::Trap("ab".into()),
                ImportSource::Glue,
            ],
        };
        let bytes = plan.encode();
        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            2,0,0,0,  1,0,0,0,  2,1,0,0,  3,0,0,0,  6,0,0,0,
            0,
            1, 7,0,0,0,
            2, 9,0,0,0,
            3, 10,0,0,0,
            4, 2,0,0,0, b'a', b'b',
            5,
        ];
        assert_eq!(bytes, expected);
        assert_eq!(Plan::decode(&bytes).unwrap(), plan);
    }
}
