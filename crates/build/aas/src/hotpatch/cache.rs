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
//!  * `tls_init_data`: bytes of the bin's `__thread_data` section.
//!    Used when the stub needs to synthesize a TLV descriptor for
//!    a thread-local symbol referenced by the patch (TLS init
//!    bytes have to come from somewhere; copying the host's keeps
//!    the patch's initial value identical to what the host had).
//!  * `tls_init_sizes`: per-`{name}$tlv$init`-symbol
//!    `(offset_in_tdata, size)`. Mach-O nlist carries no size
//!    field, so we recover sizes by pair-adjacent-diffing sorted
//!    TLS symbol offsets — same trick dx uses (`patch.rs:319-345`
//!    in their tree).
//!
//! The cache mirrors `HotpatchModuleCache` in `dx`'s codebase;
//! the only structural divergence is that we drop the wasm /
//! Windows fields since we only target Mach-O for v1.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use object::{Object, ObjectSection, ObjectSymbol};

#[derive(Default)]
pub struct HostBinCache {
    /// `symbol_name → link-time address`. Exact-name lookups are the
    /// fast path used by [`Self::resolve_runtime`].
    pub symbols: HashMap<String, u64>,
    /// `hash-stripped_name → exact_name_in_symbols`. Rust's legacy
    /// mangler appends a `17h<16hex>E` content-hash suffix that
    /// varies per incremental compilation snapshot — the same logical
    /// item compiled in two artifacts (host bin vs patch dylib) ends
    /// up with different suffixes. The patch's tip object references
    /// the host item under the *patch's* hash; the host has it under
    /// its own. Both strip to the same key, so we keep this fallback
    /// map alongside `symbols`. See `framework_hot::diff` for the
    /// matching logic on the runtime side.
    stripped_to_full: HashMap<String, String>,
    /// Link-time address of `_main` / `main`.
    pub main_addr: u64,
    /// Raw bytes of `__thread_data`. Empty on platforms with no
    /// TLS section in the bin (rare; most Rust bins have at least
    /// one `thread_local!`).
    pub tls_init_data: Vec<u8>,
    /// For each `{name}$tlv$init` symbol, `(offset_in_tdata, size)`.
    pub tls_init_sizes: HashMap<String, (u64, u64)>,
}

impl HostBinCache {
    pub fn load(host_bin: &Path) -> Result<Self> {
        let data = std::fs::read(host_bin)
            .with_context(|| format!("read {}", host_bin.display()))?;
        let obj = object::File::parse(&*data)
            .with_context(|| format!("parse {}", host_bin.display()))?;

        let mut symbols: HashMap<String, u64> = HashMap::new();
        let mut stripped_to_full: HashMap<String, String> = HashMap::new();
        let mut main_addr: u64 = 0;
        for sym in obj.symbols() {
            let Ok(name) = sym.name() else { continue };
            if name.is_empty() || sym.address() == 0 {
                continue;
            }
            symbols.insert(name.to_string(), sym.address());
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
        if main_addr == 0 {
            anyhow::bail!(
                "host bin {} has no `_main`/`main` symbol — was it linked as a bin crate \
                 with `-Wl,-exported_symbol,_main` (or equivalent)?",
                host_bin.display()
            );
        }

        // Locate `__thread_data` (Mach-O TLS init section). On ELF
        // this would be `.tdata`. v1 is Mach-O only; we leave the
        // ELF path as TODO.
        let mut tls_init_data: Vec<u8> = Vec::new();
        let mut tls_section_addr: u64 = 0;
        let mut tls_section_size: u64 = 0;
        for section in obj.sections() {
            let Ok(name) = section.name() else { continue };
            if name == "__thread_data" || name == ".tdata" {
                tls_init_data = section.data().unwrap_or_default().to_vec();
                tls_section_addr = section.address();
                tls_section_size = section.size();
                break;
            }
        }

        // Build `{name}$tlv$init → (offset, size)`. Mach-O `nlist`
        // doesn't carry size, so we pair-diff sorted symbol offsets
        // within the TLS section. The final symbol's size is the
        // distance to the end of the section.
        let mut tls_init_sizes: HashMap<String, (u64, u64)> = HashMap::new();
        if tls_section_size > 0 {
            let mut tls_syms: Vec<(String, u64)> = Vec::new();
            for sym in obj.symbols() {
                let Ok(name) = sym.name() else { continue };
                if !name.ends_with("$tlv$init") {
                    continue;
                }
                let addr = sym.address();
                if addr < tls_section_addr
                    || addr >= tls_section_addr + tls_section_size
                {
                    continue;
                }
                tls_syms.push((name.to_string(), addr - tls_section_addr));
            }
            tls_syms.sort_by_key(|(_, o)| *o);
            for i in 0..tls_syms.len() {
                let (name, offset) = tls_syms[i].clone();
                let next_offset = if i + 1 < tls_syms.len() {
                    tls_syms[i + 1].1
                } else {
                    tls_section_size
                };
                let size = next_offset.saturating_sub(offset);
                tls_init_sizes.insert(name, (offset, size));
            }
        }

        Ok(Self {
            symbols,
            stripped_to_full,
            main_addr,
            tls_init_data,
            tls_init_sizes,
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
        if let Some(&link) = self.symbols.get(name) {
            return Some((link as i64 + slide) as u64);
        }
        let stripped = strip_mangle_hash(name);
        if stripped == name {
            return None; // not a hash-suffixed Rust symbol
        }
        let full = self.stripped_to_full.get(stripped)?;
        let &link = self.symbols.get(full)?;
        Some((link as i64 + slide) as u64)
    }
}

/// Strip Rust's legacy mangling content-hash suffix (`17h<16hex>E`,
/// 20 chars total) so the same item compiled in two artifacts at
/// different incremental moments keys to the same string. Returns
/// the input unchanged if it doesn't end in that shape.
///
/// Mirrors `framework_hot::diff::strip_mangle_hash` — kept private
/// here rather than depending on `framework-hot` because the host
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
