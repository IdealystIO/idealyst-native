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

/// Build the table.
///
/// `runtime_main` is the sidecar's live `main` address and
/// `app_runtime` its live app-root address (0 when unreported). The
/// two together are how the ROOT gets an entry: the root is plain user
/// code with no `__*_hot_impl` name to match on, so we convert its
/// runtime address to a link-time one through the ASLR slide, look up
/// which symbol lives there, and pair THAT name. See
/// [`root_symbol_at`].
pub fn build(
    patch_dylib: &Path,
    host_cache: &HostBinCache,
    runtime_main: u64,
    app_runtime: u64,
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
    // These are the ONLY functions reached via `dev_hot::call`
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

    // The app root, by ADDRESS rather than by name.
    //
    // `fn app() -> Element` is ordinary user code — the `#[component]`
    // split never touches it, so there is no `__*_hot_impl` symbol to
    // match. But the sidecar calls it through a fn POINTER, which is
    // exactly what subsecond's fast dispatch keys on, so one entry is
    // all it takes. Without this, an app whose whole tree lives in
    // `app()` — the scaffold's own shape — applies a patch that rebinds
    // nothing and re-renders the old code.
    match root_symbol_at(host_cache, runtime_main, app_runtime) {
        Some((name, link_addr)) => match patch_syms.get(&name) {
            Some(&patch_addr) => {
                map.insert(link_addr, patch_addr);
                dev_events::global().log("hotpatch", format!("app root paired: {name}"));
            }
            None => dev_events::global().log(
                "hotpatch",
                format!(
                    "app root `{name}` is not in the patch dylib — edits to the \
                     root fn itself will not apply (components below it still will)"
                ),
            ),
        },
        None => dev_events::global().log(
            "hotpatch",
            format!(
                "no app-root entry (runtime_main=0x{runtime_main:x} \
                 app_runtime=0x{app_runtime:x} cache_main=0x{:x}) — edits to the root fn \
                 itself will not apply",
                host_cache.main_addr,
            ),
        ),
    }
    dev_events::global().log("hotpatch", format!("jump table: {} entries", map.len()));

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


/// Which symbol the sidecar's live app-root address corresponds to,
/// plus its link-time address.
///
/// `app_runtime` and `runtime_main` are addresses in the RUNNING
/// process; `host_cache` holds link-time addresses. The difference
/// between the live `main` and the cached one is the ASLR slide, and
/// subtracting it converts the root's address into something the cache
/// can be searched by.
///
/// Returns `None` when the sidecar reported no root (`0`), when the
/// slide cannot be computed, or when no symbol sits at that address —
/// each of which only costs the root's entry, never correctness.
fn root_symbol_at(
    host_cache: &HostBinCache,
    runtime_main: u64,
    app_runtime: u64,
) -> Option<(String, u64)> {
    if app_runtime == 0 || runtime_main == 0 || host_cache.main_addr == 0 {
        return None;
    }
    let slide = runtime_main.checked_sub(host_cache.main_addr)?;
    let link_addr = app_runtime.checked_sub(slide)?;
    host_cache
        .symbols
        .iter()
        .find(|(_, sym)| sym.address == link_addr)
        .map(|(name, _)| (name.clone(), link_addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::SymbolKind;

    fn cache_with(main: u64, entries: &[(&str, u64)]) -> HostBinCache {
        let mut c = HostBinCache::default();
        c.main_addr = main;
        for (n, a) in entries {
            c.symbols.insert(
                (*n).to_string(),
                super::super::cache::CachedSymbol {
                    address: *a,
                    kind: SymbolKind::Text,
                    size: 0,
                    is_weak: false,
                },
            );
        }
        c
    }

    /// The root is found by undoing the ASLR slide, not by guessing at
    /// the mangled name — which varies with crate disambiguator,
    /// mangling version and where in the crate the fn is defined.
    #[test]
    fn root_symbol_is_located_through_the_aslr_slide() {
        let cache = cache_with(0x1000, &[("_main", 0x1000), ("_app", 0x2000)]);
        // Loaded at +0x40000.
        let found = root_symbol_at(&cache, 0x41000, 0x42000).expect("root found");
        assert_eq!(found, ("_app".to_string(), 0x2000));
    }

    /// A sidecar that reports no root (older build, or a boot path
    /// that does not know one) costs the root's entry and nothing
    /// else — never a wrong entry.
    #[test]
    fn an_unreported_root_is_simply_absent() {
        let cache = cache_with(0x1000, &[("_main", 0x1000)]);
        assert!(root_symbol_at(&cache, 0x41000, 0).is_none());
    }

    /// No symbol at the computed address means the cache and the
    /// running binary disagree — skip rather than pair something else.
    #[test]
    fn an_address_with_no_symbol_yields_nothing() {
        let cache = cache_with(0x1000, &[("_main", 0x1000)]);
        assert!(root_symbol_at(&cache, 0x41000, 0x49999).is_none());
    }
}
