//! What a compiled bundle requires of an app, read from the bundle itself.
//!
//! Each place the bundle reaches into the app left a record in its data
//! (`runtime_vocabulary::remote::site`), kept by the linker only if that
//! code is reachable. This finds the records by their magic prefix, then
//! runs each record's shape function in an interpreter (the shapes are
//! computed by the bundle's own code, the same code the app runs on its
//! side) and assembles a [`Requires`].
//!
//! The record layout (wasm32, `#[repr(C)]`, 40 bytes):
//!
//! | offset | field |
//! |---|---|
//! | 0 | magic (16 bytes, [`MAGIC`]) |
//! | 16 | kind (`u32`) |
//! | 20, 24 | key: pointer, length |
//! | 28, 32 | set: pointer, length |
//! | 36 | shape function (an index into the function table) |

use std::collections::{BTreeMap, HashMap};

use anyhow::{anyhow, bail, Context, Result};
use remote_bundle::manifest::Param;
use remote_bundle::Requires;

/// `runtime_vocabulary::remote::site::MAGIC`: the vocabulary isn't a
/// dependency of this tool, so the bytes are repeated, and a test pins them
/// against a real bundle (`tests/showcase_release.rs`).
pub const MAGIC: [u8; 16] = *b"\xffidealyst-site\x01\x00";
const COMPONENT: u32 = 1;
const HOST_FN: u32 = 2;
const REMOTE: u32 = 3;
const CONTEXT: u32 = 4;
const RECORD_LEN: usize = 40;

/// The table the shape functions are called through; exported because the
/// bundle is linked with `--export-table` (`BUNDLE_RUSTFLAGS`).
const TABLE: &str = "__indirect_function_table";

struct Record {
    kind: u32,
    key: String,
    set: String,
    shapes: u32,
}

/// The bundle's initial memory: each active data segment at its offset.
struct Image(Vec<(u32, Vec<u8>)>);

impl Image {
    fn read(&self, addr: u32, len: u32) -> Option<&[u8]> {
        self.0.iter().find_map(|(at, bytes)| {
            let start = addr.checked_sub(*at)? as usize;
            bytes.get(start..start.checked_add(len as usize)?)
        })
    }

    fn string(&self, ptr: u32, len: u32) -> Result<String> {
        // An empty `&str` may point anywhere (a dangling, aligned address).
        if len == 0 {
            return Ok(String::new());
        }
        let bytes = self.read(ptr, len).ok_or_else(|| anyhow!("a site record points outside the bundle's data"))?;
        String::from_utf8(bytes.to_vec()).context("a site record's text isn't UTF-8")
    }
}

fn image(wasm: &[u8]) -> Result<Image> {
    let mut segments = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let wasmparser::Payload::DataSection(reader) = payload.context("read the bundle")? else { continue };
        for data in reader {
            let data = data.context("read a data segment")?;
            let wasmparser::DataKind::Active { offset_expr, .. } = data.kind else { continue };
            let mut ops = offset_expr.get_operators_reader();
            let wasmparser::Operator::I32Const { value } = ops.read().context("read a data segment's offset")? else {
                bail!("a data segment's offset isn't a constant: was the bundle linked as position-independent code?");
            };
            segments.push((value as u32, data.data.to_vec()));
        }
    }
    Ok(Image(segments))
}

fn records(image: &Image) -> Result<Vec<Record>> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"));
    let mut out = Vec::new();
    for (base, bytes) in &image.0 {
        let mut from = 0;
        while let Some(found) = find(&bytes[from..], &MAGIC) {
            let at = from + found;
            from = at + 1;
            // Records are 4-byte aligned in memory; a match elsewhere is
            // some other data that happens to contain the bytes.
            if (*base as usize + at) % 4 != 0 || at + RECORD_LEN > bytes.len() {
                continue;
            }
            let r = &bytes[at..at + RECORD_LEN];
            out.push(Record {
                kind: word(r, 16),
                key: image.string(word(r, 20), word(r, 24))?,
                set: image.string(word(r, 28), word(r, 32))?,
                shapes: word(r, 36),
            });
        }
    }
    Ok(out)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Run each distinct shape function once, in a throwaway instance whose
/// imports all trap: a shape function only formats text, so reaching the
/// app is a bug in it, reported as such.
fn run_shapes(wasm: &[u8], tables: impl IntoIterator<Item = u32>) -> Result<HashMap<u32, String>> {
    use wasmi::{Engine, ExternType, Linker, Module, Store, Val};
    let engine = Engine::default();
    let module = Module::new(&engine, wasm).map_err(|e| anyhow!("the bundle doesn't load in the interpreter: {e}"))?;
    let mut store = Store::new(&engine, ());
    let mut linker = Linker::<()>::new(&engine);
    for import in module.imports() {
        if let ExternType::Func(ty) = import.ty() {
            let what = format!("{}::{}", import.module(), import.name());
            linker
                .func_new(import.module(), import.name(), ty.clone(), move |_, _, _| {
                    Err(wasmi::Error::new(format!("a shape function called the app ({what})")))
                })
                .map_err(|e| anyhow!("{e}"))?;
        }
    }
    let instance = linker.instantiate_and_start(&mut store, &module).map_err(|e| anyhow!("instantiate the bundle: {e}"))?;
    let table = instance
        .get_table(&store, TABLE)
        .ok_or_else(|| anyhow!("the bundle exports no `{TABLE}`: it wasn't linked with `--export-table`"))?;
    let memory = instance.get_memory(&store, "memory").ok_or_else(|| anyhow!("the bundle exports no memory"))?;
    let mut out = HashMap::new();
    for index in tables {
        if out.contains_key(&index) {
            continue;
        }
        let func = table
            .get(&store, u64::from(index))
            .and_then(|r| Option::<&wasmi::Func>::from(r.as_func()?).copied())
            .ok_or_else(|| anyhow!("site record names table entry {index}, which holds no function"))?;
        let mut result = [Val::I64(0)];
        func.call(&mut store, &[], &mut result).map_err(|e| anyhow!("a shape function failed: {e}"))?;
        let Val::I64(packed) = result[0] else { bail!("a shape function returned no i64") };
        let (ptr, len) = ((packed as u64 >> 32) as usize, (packed as u64 & 0xffff_ffff) as usize);
        let bytes = memory
            .data(&store)
            .get(ptr..ptr + len)
            .ok_or_else(|| anyhow!("a shape function's text is out of bounds"))?;
        out.insert(index, String::from_utf8(bytes.to_vec()).context("a shape function's text isn't UTF-8")?);
    }
    Ok(out)
}

/// `name\tshape\n` lines.
fn pairs(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| l.split_once('\t')).map(|(n, s)| (n.to_string(), s.to_string())).collect()
}

/// What `wasm` (a compiled bundle) requires of an app.
pub fn requires(wasm: &[u8]) -> Result<Requires> {
    let image = image(wasm)?;
    let records = records(&image)?;
    // No records, nothing to run (and nothing to instantiate: a bundle with
    // no reachable use of the app needs no table).
    let shapes = if records.is_empty() { HashMap::new() } else { run_shapes(wasm, records.iter().map(|r| r.shapes))? };
    let mut req = Requires::default();
    for r in &records {
        let text = &shapes[&r.shapes];
        match r.kind {
            COMPONENT => {
                let all: BTreeMap<String, String> = pairs(text).into_iter().collect();
                let props = req.components.entry(r.key.clone()).or_default();
                for name in r.set.lines() {
                    if name == "*" {
                        props.extend(all.clone());
                    } else {
                        // A set prop the bundle's copy doesn't have can't
                        // happen (it wouldn't compile); `?` if it does.
                        let shape = all.get(name).cloned().unwrap_or_else(|| "?".into());
                        props.insert(name.to_string(), shape);
                    }
                }
            }
            HOST_FN => {
                req.host_fns.insert(r.key.clone(), text.clone());
            }
            REMOTE => {
                let params = pairs(text).into_iter().map(|(name, shape)| Param { name, shape }).collect();
                req.remote.insert(r.key.clone(), params);
            }
            CONTEXT => {
                req.contexts.insert(r.key.clone(), text.clone());
            }
            other => bail!("a site record of unknown kind {other}: the bundle was built by a newer framework than this tool"),
        }
    }
    // Every host function the bundle imports is a requirement, record or
    // not: an import without one (a stub built by an older macro) is listed
    // with an unknown shape rather than left out.
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::ImportSection(reader) = payload.context("read the bundle's imports")? {
            for import in reader {
                let import = import.context("read an import")?;
                if import.module == "idealyst_host_fn" {
                    req.host_fns.entry(import.name.to_string()).or_insert_with(|| "?".into());
                }
            }
        }
    }
    Ok(req)
}

/// `wasm` without its shape functions: each record's table slot points at
/// one trapping function instead, and everything only the shape functions
/// reached (the shape impls, their formatting) is collected as dead code.
/// They exist for [`requires`] alone — a device never calls them — and
/// cost the showcase bundle ~35 KB raw, ~6 KB brotli. The records stay
/// (a few bytes each); their slots trap if anything ever called them.
pub fn strip_shapes(wasm: &[u8]) -> Result<Vec<u8>> {
    let records = records(&image(wasm)?)?;
    if records.is_empty() {
        return Ok(wasm.to_vec());
    }
    let slots: std::collections::HashSet<u32> = records.iter().map(|r| r.shapes).collect();
    let config = {
        let mut c = walrus::ModuleConfig::new();
        c.generate_producers_section(false);
        c
    };
    let mut module = walrus::Module::from_buffer_with_config(wasm, &config).map_err(|e| anyhow!("read the bundle: {e}"))?;
    let stub = {
        let mut b = walrus::FunctionBuilder::new(&mut module.types, &[], &[walrus::ValType::I64]);
        b.func_body().unreachable();
        b.finish(vec![], &mut module.funcs)
    };
    let mut replaced = 0;
    for elem in module.elements.iter_mut() {
        let walrus::ElementKind::Active { offset: walrus::ConstExpr::Value(walrus::ir::Value::I32(base)), .. } = elem.kind else {
            continue;
        };
        let walrus::ElementItems::Functions(funcs) = &mut elem.items else { continue };
        for (i, f) in funcs.iter_mut().enumerate() {
            if slots.contains(&(base as u32 + i as u32)) {
                *f = stub;
                replaced += 1;
            }
        }
    }
    if replaced != slots.len() {
        bail!("found {replaced} of the {} shape functions' table slots", slots.len());
    }
    walrus::passes::gc::run(&mut module);
    Ok(module.emit_wasm())
}
