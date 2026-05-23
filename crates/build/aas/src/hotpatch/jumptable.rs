//! Build `subsecond_types::JumpTable` from a freshly-linked patch
//! dylib against the cached host symbol table.
//!
//! For each `__*_hot_impl` symbol (the `#[component]` macro's
//! split inner-fn) present in BOTH the host bin and the patch
//! dylib, emit a `(host_link_addr, patch_link_addr)` pair into
//! the address map. Subsecond's `apply_patch` adds the two slides
//! (host ASLR + patch dlopen) at apply time, producing real
//! runtime addresses.
//!
//! The JumpTable also carries:
//!  * `lib`: path to the patch dylib (subsecond dlopens it again
//!    inside `apply_patch` — that's fine, dyld dedupes by path).
//!  * `aslr_reference`: host bin's link-time `main` address.
//!  * `new_base_address`: patch dylib's link-time `main` address.
//!  * `ifunc_count`: wasm-only; 0 on Mach-O.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use object::{Object, ObjectSymbol};
use subsecond_types::{AddressMap, JumpTable};

use super::cache::HostBinCache;

pub fn build(
    patch_dylib: &Path,
    host_cache: &HostBinCache,
    _runtime_main: u64,
) -> Result<JumpTable> {
    let data = std::fs::read(patch_dylib)
        .with_context(|| format!("read {}", patch_dylib.display()))?;
    let obj = object::File::parse(&*data)
        .with_context(|| format!("parse {}", patch_dylib.display()))?;

    // Build the patch dylib's symbol map (name → link-time addr).
    let mut patch_syms: HashMap<String, u64> = HashMap::new();
    let mut patch_main: u64 = 0;
    for sym in obj.symbols() {
        let Ok(name) = sym.name() else { continue };
        if name.is_empty() || sym.address() == 0 {
            continue;
        }
        patch_syms.insert(name.to_string(), sym.address());
        if name == "_main" || name == "main" {
            patch_main = sym.address();
        }
    }
    if patch_main == 0 {
        anyhow::bail!(
            "patch dylib {} has no `_main`/`main` symbol — subsecond uses it as the \
             ASLR anchor. Ensure the user crate's `fn main` is in the tip objects.",
            patch_dylib.display()
        );
    }

    // Only redirect `__*_hot_impl` symbols — the inner functions
    // the `#[component]` macro emits in its hot-reload split form.
    // These are the ONLY functions reached via `framework_hot::call`
    // at runtime; redirecting anything else (e.g. generic
    // monomorphizations of stdlib helpers like `unwrap_failed`,
    // `Option::map<...>`, etc.) is at best wasted entries and at
    // worst routes a same-name-different-ABI helper through a
    // wrong-target trampoline, which crashes the moment the host
    // calls one. dx restricts its table the same way.
    let mut map = AddressMap::default();
    for (name, &patch_addr) in &patch_syms {
        if !is_hot_impl_symbol(name) {
            continue;
        }
        if let Some(host_sym) = host_cache.symbols.get(name) {
            map.insert(host_sym.address, patch_addr);
        }
    }

    Ok(JumpTable {
        lib: patch_dylib.to_path_buf(),
        map,
        aslr_reference: host_cache.main_addr,
        new_base_address: patch_main,
        ifunc_count: 0,
    })
}

/// True if the symbol name contains `_hot_impl` — what the
/// `#[component]` macro emits for its inner-function split. Mach-O
/// symbols carry the C-ABI leading underscore; mangled names embed
/// the ident anywhere. Both legacy mangling (`__ZN…hot_impl…E`) and
/// v0 mangling (`__R…hot_impl…`) contain the literal substring.
fn is_hot_impl_symbol(name: &str) -> bool {
    name.contains("_hot_impl")
}

