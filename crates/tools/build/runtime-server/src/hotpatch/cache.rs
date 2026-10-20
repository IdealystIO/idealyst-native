//! Host bin symbol cache.
//!
//! Built once per dev session by parsing the freshly-linked
//! sidecar bin. Stores:
//!  * `symbols`: name → link-time address for every defined
//!    symbol. Used by [`super::stub::synthesize`] to resolve
//!    undefined refs in the patch's `.rcgu.o` files.
//!  * `main_addr`: the bin's link-time `_main`/`main` address.
//!    Combined with the running process's `dlsym("main")` it
//!    yields the ASLR slide.
//!  * `state_symbols`: every symbol in a writable-data, zero-fill or
//!    thread-local-descriptor section — the statics a patch's own
//!    copies are bound to ([`super::bind`]).
//!
//! The cache mirrors `HotpatchModuleCache` in `dx`'s codebase;
//! the only structural divergence is that we drop the wasm /
//! Windows fields since we only target Mach-O for v1.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};

/// Per-symbol metadata cached from the host bin. The `kind` field is
/// load-bearing: the stub generator gives a function (Text) a code
/// trampoline and everything else an absolute-address symbol, and
/// `bind` turns the patch's references to a thread-local (Tls) into GOT
/// loads of the binary's TLV descriptor.
///
/// A code trampoline for a TLS symbol SIGBUS-crashes the sidecar: the
/// dylib's `ldr x16, [tlv_desc]; blr x16` reads the trampoline's
/// `movz`/`movk` bytes as a function pointer.
#[derive(Clone)]
pub struct CachedSymbol {
    pub address: u64,
    pub kind: SymbolKind,
    pub size: u64,
    pub is_weak: bool,
}

#[derive(Default)]
pub struct HostBinCache {
    /// `symbol_name → cached metadata`. Exact-name lookups are the
    /// fast path used by [`Self::resolve_runtime`].
    pub symbols: HashMap<String, CachedSymbol>,
    /// `hash-stripped_name → exact_name_in_symbols`. Rust's legacy
    /// mangler appends a `17h<16hex>E` content-hash suffix that
    /// varies per incremental compilation snapshot — the same logical
    /// item compiled in two artifacts (host bin vs patch dylib) ends
    /// up with different suffixes. The patch's tip object references
    /// the host item under the *patch's* hash; the host has it under
    /// its own. Both strip to the same key, so we keep this fallback
    /// map alongside `symbols`. See `dev_hot::diff` for the
    /// matching logic on the runtime side.
    stripped_to_full: HashMap<String, String>,
    /// Link-time address of `_main` / `main`.
    pub main_addr: u64,
    /// Every symbol in a section that holds state — writable data,
    /// zero-fill, thread-local descriptors
    /// ([`super::bind::is_state_section`]), grouped for pairing: a
    /// patch's static of the same name is bound to this one
    /// ([`super::statics::pair`]).
    pub state_symbols: super::statics::ProgramStatics,
}

impl HostBinCache {
    pub fn load(host_bin: &Path) -> Result<Self> {
        let data = std::fs::read(host_bin)
            .with_context(|| format!("read {}", host_bin.display()))?;
        let obj = object::File::parse(&*data)
            .with_context(|| format!("parse {}", host_bin.display()))?;

        let mut symbols: HashMap<String, CachedSymbol> = HashMap::new();
        let mut stripped_to_full: HashMap<String, String> = HashMap::new();
        let mut main_addr: u64 = 0;
        let mut state_sections: HashMap<object::SectionIndex, bool> = HashMap::new();
        let mut state_symbols: Vec<String> = Vec::new();
        for sym in obj.symbols() {
            let Ok(name) = sym.name() else { continue };
            if name.is_empty() || sym.address() == 0 {
                continue;
            }
            if let Some(index) = sym.section_index() {
                let is_state = *state_sections.entry(index).or_insert_with(|| {
                    obj.section_by_index(index).is_ok_and(|s| {
                        let segment = s.segment_name().ok().flatten().unwrap_or("");
                        let flags = match s.flags() {
                            object::SectionFlags::MachO { flags } => flags,
                            _ => 0,
                        };
                        super::bind::is_state_section(segment, s.name().unwrap_or(""), flags)
                    })
                });
                if is_state {
                    state_symbols.push(name.to_string());
                }
            }
            symbols.insert(
                name.to_string(),
                CachedSymbol {
                    address: sym.address(),
                    kind: sym.kind(),
                    size: sym.size(),
                    is_weak: sym.is_weak(),
                },
            );
            // Mirror the hash-stripped key for the fallback lookup
            // in `resolve_runtime`. If two symbols hash-strip to the
            // same key (different generics with the same name?), the
            // last-write-wins. In practice the legacy mangler's
            // suffix already encodes generic params + cfg state, so
            // collisions are rare; if we ever hit one it'll be
            // visible in the dlopen failure mode and we can revisit.
            let stripped = strip_mangle_hash(name);
            if stripped != name {
                stripped_to_full.insert(stripped.to_string(), name.to_string());
            }
            if name == "_main" || name == "main" {
                main_addr = sym.address();
            }
        }
        // Re-check via the cached map (cheap clone; symbols map already
        // built). The address-based lookup above bailed on
        // `sym.address() == 0` so undefined symbols never entered the
        // map — this avoids leaking import-table placeholders into the
        // stub generator's `resolve_runtime` results.
        if main_addr == 0 {
            anyhow::bail!(
                "host bin {} has no `_main`/`main` symbol — was it linked as a bin crate \
                 with `-Wl,-exported_symbol,_main` (or equivalent)?",
                host_bin.display()
            );
        }

        Ok(Self {
            symbols,
            stripped_to_full,
            main_addr,
            state_symbols: super::statics::ProgramStatics::new(state_symbols.iter().map(String::as_str)),
        })
    }

    /// Resolve `name` to its runtime address given the ASLR slide
    /// (computed by the caller from `dlsym("main") - cache.main_addr`).
    ///
    /// Two-tier lookup:
    /// 1. Exact name — covers `_main`, plain C symbols, and Rust
    ///    items the patch happened to mangle identically to the host.
    /// 2. Hash-stripped fallback — Rust's legacy mangler appends a
    ///    `17h<16hex>E` content-hash suffix that varies between the
    ///    patch dylib's build and the host bin's build for *the same
    ///    logical item*. Without this fallback, every Rust framework
    ///    symbol that crosses the patch/host boundary would fail to
    ///    resolve, the stub generator would defer it to dyld, and
    ///    dlopen would then fail because it's not in any system
    ///    dylib either.
    pub fn resolve_runtime(&self, name: &str, slide: i64) -> Option<u64> {
        Some(self.resolve_runtime_with_meta(name, slide)?.0)
    }

    /// Variant of [`Self::resolve_runtime`] that also returns the
    /// cached symbol metadata (kind, size, weakness). The stub
    /// generator needs `kind` so it can route Text → trampoline,
    /// Tls → TLV stub, Data → absolute symbol; without that
    /// dispatch, TLS references blow up at runtime (see
    /// [`CachedSymbol`] doc).
    pub fn resolve_runtime_with_meta(
        &self,
        name: &str,
        slide: i64,
    ) -> Option<(u64, CachedSymbol)> {
        if let Some(sym) = self.symbols.get(name) {
            return Some(((sym.address as i64 + slide) as u64, sym.clone()));
        }
        let stripped = strip_mangle_hash(name);
        if stripped == name {
            return None;
        }
        let full = self.stripped_to_full.get(stripped)?;
        let sym = self.symbols.get(full)?;
        Some(((sym.address as i64 + slide) as u64, sym.clone()))
    }
}

/// Strip Rust's legacy mangling content-hash suffix (`17h<16hex>E`,
/// 20 chars total) so the same item compiled in two artifacts at
/// different incremental moments keys to the same string. Returns
/// the input unchanged if it doesn't end in that shape.
///
/// Mirrors `dev_hot::diff::strip_mangle_hash` — kept private
/// here rather than depending on `dev-hot` because the host
/// build-tools crate shouldn't pull in the runtime hot-reload
/// machinery just for one string utility.
fn strip_mangle_hash(name: &str) -> &str {
    const SUFFIX_LEN: usize = 20;
    let bytes = name.as_bytes();
    if bytes.len() < SUFFIX_LEN + 1 || bytes[bytes.len() - 1] != b'E' {
        return name;
    }
    let suffix_start = bytes.len() - SUFFIX_LEN;
    if &bytes[suffix_start..suffix_start + 3] != b"17h" {
        return name;
    }
    if !bytes[suffix_start + 3..bytes.len() - 1]
        .iter()
        .all(|b| b.is_ascii_hexdigit())
    {
        return name;
    }
    &name[..suffix_start]
}
