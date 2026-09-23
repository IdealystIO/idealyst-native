//! Drop wasm-bindgen's speculative `__wbindgen_jstag` import.
//!
//! # Why this exists
//!
//! `hotpatch_base` roots every function in the element table so
//! wasm-bindgen's dead-code pass keeps it. One consequence is that
//! wasm-bindgen then sees `#[wasm_bindgen(catch)]`-shaped imports it
//! would otherwise have collected, and its catch-wrapper transform adds
//! an imported tag:
//!
//! ```text
//! tag[0] <./app_bg.js.__wbindgen_jstag> <- ./app_bg.js.__wbindgen_jstag
//! ```
//!
//! A tag is the exceptions proposal, and `walrus` — which the
//! command-export neutralize pass and `wasm-split` both parse with — has
//! no `EXCEPTIONS` feature in its parser. So the very next pass died
//! with "failed to parse import section: exceptions proposal not
//! enabled", and a hot-patch dev build could not finish.
//!
//! # Why removing it is safe
//!
//! The import is a JS-side capability, not wasm code: the generated glue
//! supplies it as `__wbindgen_jstag: WebAssembly.JSTag`, and an import
//! object may carry entries the module does not ask for. What would NOT
//! be safe is removing it from a module that actually raises or catches,
//! so this refuses to touch a module that defines tags of its own — a
//! Tag section is the signal that exception machinery is real here rather
//! than speculative, and the caller then reports it instead of producing
//! something subtly broken.
//!
//! # Scope
//!
//! Hot-patch dev builds only. A normal build never roots the dead
//! wasm-bindgen machinery, so the import never appears and this never
//! runs.

use anyhow::{bail, Context, Result};
use wasmparser::{Parser, Payload};

/// The import wasm-bindgen adds for `WebAssembly.JSTag`.
const JS_TAG: &str = "__wbindgen_jstag";

/// Remove every imported tag from `wasm`, returning the new bytes.
/// `Ok(None)` when there was nothing to remove.
pub fn strip_imported_tags(wasm: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut import_range = None;
    let mut tag_names = Vec::new();
    let mut defines_tags = false;

    for payload in Parser::new(0).parse_all(wasm) {
        // A tag IMPORT makes wasmparser refuse the import section
        // outright, which is the whole problem — so the section's own
        // bytes are edited below rather than its parsed contents.
        let payload = match payload {
            Ok(p) => p,
            Err(_) => break,
        };
        match payload {
            Payload::ImportSection(reader) => import_range = Some(reader.range()),
            Payload::TagSection(_) => defines_tags = true,
            _ => {}
        }
    }

    let Some(range) = import_range else {
        return Ok(None);
    };
    let edited = edit_import_section(&wasm[range.clone()], &mut tag_names)
        .context("rewriting the import section to drop its tag imports")?;
    if tag_names.is_empty() {
        return Ok(None);
    }
    if defines_tags {
        bail!(
            "this module defines its own tags, so its exception machinery is real rather than \
             wasm-bindgen's speculative {JS_TAG} import — removing {tag_names:?} would break it"
        );
    }
    for name in &tag_names {
        if name != JS_TAG {
            bail!(
                "unexpected imported tag `{name}`: only wasm-bindgen's speculative `{JS_TAG}` \
                 is known to be safe to drop"
            );
        }
    }

    // Splice the rewritten section in, copying everything else verbatim.
    // The section's length prefix sits just before `range.start`, so the
    // header has to be rebuilt too.
    let header_start =
        section_header_start(&range).context("locating the import section's length prefix")?;
    let mut out = Vec::with_capacity(wasm.len());
    out.extend_from_slice(&wasm[..header_start]);
    out.push(0x02); // import section id
    write_leb(&mut out, edited.len() as u32);
    out.extend_from_slice(&edited);
    out.extend_from_slice(&wasm[range.end..]);
    Ok(Some(out))
}

/// The byte a section's id sits at, given the range of its body.
///
/// A section is `id` (one byte) then its body length as an unsigned
/// LEB128 then the body. The length's encoding is determined by the
/// body's own size, so re-encoding it gives the header's exact width.
fn section_header_start(range: &std::ops::Range<usize>) -> Option<usize> {
    let mut encoded = Vec::new();
    write_leb(&mut encoded, (range.end - range.start) as u32);
    range.start.checked_sub(1 + encoded.len())
}

/// Rewrite an import section's body, dropping every tag entry and
/// collecting their names.
///
/// Hand-decoded rather than re-encoded through a parser: `wasmparser`
/// rejects the section the moment it meets a tag, which is exactly the
/// input we have. Each entry is copied byte-for-byte when kept, so no
/// encoding detail has to round-trip correctly.
fn edit_import_section(body: &[u8], tag_names: &mut Vec<String>) -> Result<Vec<u8>> {
    let mut cursor = 0usize;
    let (count, _) = read_leb(body, &mut cursor).context("import count")?;

    let mut kept: Vec<&[u8]> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let start = cursor;
        let _module = read_name(body, &mut cursor).with_context(|| format!("import {i} module"))?;
        let name = read_name(body, &mut cursor).with_context(|| format!("import {i} name"))?;
        let kind = *body
            .get(cursor)
            .with_context(|| format!("import {i} has no descriptor"))?;
        cursor += 1;
        match kind {
            // func: type index
            0x00 => {
                read_leb(body, &mut cursor).context("func type index")?;
            }
            // table: reftype byte + limits
            0x01 => {
                cursor += 1;
                read_limits(body, &mut cursor).context("table limits")?;
            }
            // memory: limits
            0x02 => {
                read_limits(body, &mut cursor).context("memory limits")?;
            }
            // global: valtype byte + mutability byte
            0x03 => {
                cursor += 2;
            }
            // tag: attribute byte + type index
            0x04 => {
                cursor += 1;
                read_leb(body, &mut cursor).context("tag type index")?;
                tag_names.push(name.to_string());
                continue;
            }
            other => bail!("unknown import descriptor 0x{other:02x} on import {i}"),
        }
        if cursor > body.len() {
            bail!("import {i} runs past the end of the section");
        }
        kept.push(&body[start..cursor]);
    }

    let mut out = Vec::with_capacity(body.len());
    write_leb(&mut out, kept.len() as u32);
    for entry in kept {
        out.extend_from_slice(entry);
    }
    Ok(out)
}

fn read_name<'a>(body: &'a [u8], cursor: &mut usize) -> Result<&'a str> {
    let (len, _) = read_leb(body, cursor).context("name length")?;
    let end = cursor
        .checked_add(len as usize)
        .filter(|e| *e <= body.len())
        .context("name runs past the end of the section")?;
    let name = std::str::from_utf8(&body[*cursor..end]).context("name is not utf-8")?;
    *cursor = end;
    Ok(name)
}

/// Limits are a flags byte plus a minimum, and a maximum when bit 0 is
/// set. The other flag bits (shared, 64-bit) do not change the length.
fn read_limits(body: &[u8], cursor: &mut usize) -> Result<()> {
    let flags = *body.get(*cursor).context("limits flags")?;
    *cursor += 1;
    read_leb(body, cursor).context("limits minimum")?;
    if flags & 0x01 != 0 {
        read_leb(body, cursor).context("limits maximum")?;
    }
    Ok(())
}

fn read_leb(body: &[u8], cursor: &mut usize) -> Option<(u32, usize)> {
    let (mut value, mut shift) = (0u32, 0u32);
    loop {
        let byte = *body.get(*cursor)?;
        *cursor += 1;
        value |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Some((value, *cursor));
        }
        shift += 7;
        if shift > 31 {
            return None;
        }
    }
}

fn write_leb(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a module with an import section holding one function
    /// import, one memory import and one tag import — the shape
    /// wasm-bindgen produces once `hotpatch_base` has rooted the dead
    /// catch machinery.
    fn module_with_a_tag_import() -> Vec<u8> {
        let mut body = Vec::new();
        write_leb(&mut body, 3);
        // env.f : func type 0
        push_name(&mut body, "env");
        push_name(&mut body, "f");
        body.extend_from_slice(&[0x00, 0x00]);
        // env.memory : memory, limits {min 17, max 100}
        push_name(&mut body, "env");
        push_name(&mut body, "memory");
        body.push(0x02);
        body.push(0x01);
        write_leb(&mut body, 17);
        write_leb(&mut body, 100);
        // ./app_bg.js.__wbindgen_jstag : tag, attribute 0, type 1
        push_name(&mut body, "./app_bg.js");
        push_name(&mut body, JS_TAG);
        body.extend_from_slice(&[0x04, 0x00, 0x01]);

        let mut out = b"\0asm\x01\0\0\0".to_vec();
        out.push(0x02);
        write_leb(&mut out, body.len() as u32);
        out.extend_from_slice(&body);
        out
    }

    fn push_name(out: &mut Vec<u8>, s: &str) {
        write_leb(out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    }

    /// The tag goes and everything else stays, byte for byte. A
    /// re-encoding that "fixed up" a limits or name encoding on the way
    /// through would change bytes wasm-bindgen's own JS indexes by
    /// position.
    #[test]
    fn the_tag_import_is_dropped_and_the_others_are_untouched() {
        let original = module_with_a_tag_import();
        let stripped = strip_imported_tags(&original).unwrap().expect("a tag to drop");

        // The result must now be readable by a parser with no exceptions
        // feature — which is the entire point, since walrus's is one.
        let mut imports = Vec::new();
        for payload in Parser::new(0).parse_all(&stripped) {
            if let Payload::ImportSection(reader) = payload.unwrap() {
                for import in reader {
                    let import = import.unwrap();
                    imports.push(format!("{}.{}", import.module, import.name));
                }
            }
        }
        assert_eq!(imports, vec!["env.f", "env.memory"]);
    }

    /// A module with no tag import is handed back untouched rather than
    /// rewritten — a normal build must not pay for this at all.
    #[test]
    fn a_module_without_a_tag_import_is_left_alone() {
        let mut out = b"\0asm\x01\0\0\0".to_vec();
        let mut body = Vec::new();
        write_leb(&mut body, 1);
        push_name(&mut body, "env");
        push_name(&mut body, "f");
        body.extend_from_slice(&[0x00, 0x00]);
        out.push(0x02);
        write_leb(&mut out, body.len() as u32);
        out.extend_from_slice(&body);

        assert!(strip_imported_tags(&out).unwrap().is_none());
    }

    /// A module that DEFINES tags is really using exceptions, and
    /// dropping the import would break it. Say so rather than producing
    /// something subtly wrong.
    #[test]
    fn a_module_that_defines_its_own_tags_is_refused() {
        let mut out = module_with_a_tag_import();
        // Append an empty tag section (id 13).
        out.push(0x0d);
        out.push(0x01);
        out.push(0x00);
        let err = strip_imported_tags(&out).unwrap_err();
        assert!(format!("{err:#}").contains("defines its own tags"), "{err:#}");
    }
}
