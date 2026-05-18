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
    /// `symbol_name → link-time address`.
    pub symbols: HashMap<String, u64>,
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
        let mut main_addr: u64 = 0;
        for sym in obj.symbols() {
            let Ok(name) = sym.name() else { continue };
            if name.is_empty() || sym.address() == 0 {
                continue;
            }
            symbols.insert(name.to_string(), sym.address());
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
            main_addr,
            tls_init_data,
            tls_init_sizes,
        })
    }

    /// Resolve `name` to its runtime address given the ASLR slide
    /// (computed by the caller from `dlsym("main") - cache.main_addr`).
    pub fn resolve_runtime(&self, name: &str, slide: i64) -> Option<u64> {
        let &link = self.symbols.get(name)?;
        Some((link as i64 + slide) as u64)
    }
}
