//! The framing of the `__idealyst_glue` custom section.
//!
//! LLD concatenates every same-named custom section of the objects it
//! links, in link order — which is NOT dependency order and not stable
//! across crate-graph changes. So each crate's contribution must be
//! self-delimiting and order-independent: a sequence of records, each
//!
//! ```text
//! b"IGLU" | version: u8 | kind: u8 | name_len: u16 LE | src_len: u32 LE | name | source
//! ```
//!
//! `kind` is [`KIND_RUNTIME`] (exactly one, web-glue's own `js/runtime.js`)
//! or [`KIND_MODULE`] (a crate's `js_module!`). The reader is
//! `wasm_carve::glue::parse_records`; the two sides share only this
//! layout and [`VERSION`], and a version mismatch is a build error there.
//!
//! Everything here is `const fn` so a record is a `[u8; N]` static built
//! at compile time — `#[link_section]` statics must be plain bytes with no
//! relocations.

/// Record magic.
pub const MAGIC: [u8; 4] = *b"IGLU";
/// Framing version. Bump on any layout change; the build pass refuses
/// records it does not understand.
pub const VERSION: u8 = 1;
/// The web-glue JS runtime (`G`).
pub const KIND_RUNTIME: u8 = 0;
/// A crate-shipped JS module, reachable from snippets as `G.m(name)`.
pub const KIND_MODULE: u8 = 1;
/// Bytes before the name.
pub const HEADER_LEN: usize = 4 + 1 + 1 + 2 + 4;

/// Encoded length of a record.
pub const fn len(name: &str, source: &str) -> usize {
    HEADER_LEN + name.len() + source.len()
}

/// Encode one record. `N` must equal [`len`]`(name, source)`; a mismatch
/// fails const evaluation, so it is a compile error, not a corrupt section.
pub const fn encode<const N: usize>(kind: u8, name: &str, source: &str) -> [u8; N] {
    assert!(N == len(name, source), "web-glue record length mismatch");
    assert!(name.len() <= u16::MAX as usize, "web-glue record name too long");
    assert!(source.len() <= u32::MAX as usize, "web-glue record source too long");
    let mut out = [0u8; N];
    out[0] = MAGIC[0];
    out[1] = MAGIC[1];
    out[2] = MAGIC[2];
    out[3] = MAGIC[3];
    out[4] = VERSION;
    out[5] = kind;
    let nl = (name.len() as u16).to_le_bytes();
    out[6] = nl[0];
    out[7] = nl[1];
    let sl = (source.len() as u32).to_le_bytes();
    out[8] = sl[0];
    out[9] = sl[1];
    out[10] = sl[2];
    out[11] = sl[3];
    let name = name.as_bytes();
    let mut i = 0;
    while i < name.len() {
        out[HEADER_LEN + i] = name[i];
        i += 1;
    }
    let source = source.as_bytes();
    let base = HEADER_LEN + name.len();
    let mut j = 0;
    while j < source.len() {
        out[base + j] = source[j];
        j += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_carries_its_name_and_source_byte_exact() {
        const NAME: &str = "demo/ünï";
        const SRC: &str = "return { x: \"✓\" };";
        const N: usize = len(NAME, SRC);
        const R: [u8; N] = encode::<N>(KIND_MODULE, NAME, SRC);
        assert_eq!(&R[..4], b"IGLU");
        assert_eq!(R[4], VERSION);
        assert_eq!(R[5], KIND_MODULE);
        assert_eq!(u16::from_le_bytes([R[6], R[7]]) as usize, NAME.len());
        assert_eq!(u32::from_le_bytes([R[8], R[9], R[10], R[11]]) as usize, SRC.len());
        assert_eq!(&R[HEADER_LEN..HEADER_LEN + NAME.len()], NAME.as_bytes());
        assert_eq!(&R[HEADER_LEN + NAME.len()..], SRC.as_bytes());
    }
}
