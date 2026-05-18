//! Symbol-diff jump-table generator.
//!
//! ## Approach
//!
//! 1. **Cache the running binary's symbol map at startup.** Done
//!    lazily on first call to [`apply_from_dylib`] via
//!    [`once_cell::sync::OnceCell`] — equivalent — backed by
//!    [`std::sync::OnceLock`]. The bin file on disk is opened once
//!    (`std::env::current_exe`), parsed with `object`, and a
//!    `HashMap<String, u64>` (symbol name → link-time offset) is
//!    built. Subsequent rebuilds reuse this; the bin file is never
//!    overwritten because the watcher uses `cargo build --lib`.
//!
//! 2. **Parse the patch dylib's symbols** each time. Open with
//!    `object`, build the same map.
//!
//! 3. **Compute the diff.** For each symbol present in BOTH maps
//!    where the name is `main` or matches `__*_hot_impl`, emit a
//!    `(bin_offset, dylib_offset)` pair. Skip the rest — they're
//!    framework / dep code that doesn't participate in hot reload.
//!
//! 4. **Construct the `JumpTable`** and call [`apply_patch`].
//!    Subsecond's runtime takes over: it dlopens the dylib,
//!    computes the actual ASLR-corrected runtime addresses for
//!    every entry, and atomically swaps the global jump-table
//!    pointer so subsequent `framework_hot::call` sites dispatch
//!    to the patched code.
//!
//! ## Errors
//!
//! Returns [`DiffError`] enums for the named failure modes (file
//! missing, parse failed, no `main` symbol, etc.). Callers are
//! expected to log and continue running the previous code — a
//! patch failure must not crash the host.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use object::{Object, ObjectSymbol, SymbolKind};
use subsecond_types::{AddressMap, JumpTable};

/// Bin-side symbol map, cached at first use. Key: symbol name (as a
/// String). Value: link-time virtual address (the value `object`
/// returns for a symbol's `address()`).
static BIN_SYMBOLS: OnceLock<HashMap<String, u64>> = OnceLock::new();

#[derive(Debug)]
pub enum DiffError {
    Io(std::io::Error),
    Parse(String),
    /// No `main` symbol in the bin or dylib. Subsecond uses it as
    /// the ASLR reference for offset arithmetic.
    NoMain { in_: &'static str },
    Patch(subsecond::PatchError),
}

impl From<std::io::Error> for DiffError {
    fn from(e: std::io::Error) -> Self {
        DiffError::Io(e)
    }
}

/// Open `path`, parse symbols, return a map of name → link-time
/// offset for `main` and every `__*_hot_impl` entry.
fn read_symbol_map(path: &Path) -> Result<HashMap<String, u64>, DiffError> {
    let data =
        std::fs::read(path).map_err(|e| DiffError::Io(e))?;
    let obj = object::File::parse(&*data)
        .map_err(|e| DiffError::Parse(format!("{}: {}", path.display(), e)))?;
    let mut out = HashMap::new();
    for sym in obj.symbols() {
        if sym.kind() != SymbolKind::Text && sym.kind() != SymbolKind::Unknown {
            continue;
        }
        let Ok(name) = sym.name() else { continue };
        if !is_hot_symbol(name) {
            continue;
        }
        out.insert(name.to_string(), sym.address());
    }
    Ok(out)
}

/// Match the names that participate in hot reload:
/// - `main` (or `_main` on mach-o — symbol-table entries on macOS
///   have a leading underscore from the C ABI mangling).
/// - Any name containing the `__*_hot_impl` substring our
///   `#[component]` macro emits for inner function bodies.
fn is_hot_symbol(name: &str) -> bool {
    if name == "main" || name == "_main" {
        return true;
    }
    // Rust mangled names sometimes have a leading underscore on
    // mach-o. Strip it for the substring check.
    let stripped = name.strip_prefix('_').unwrap_or(name);
    stripped.contains("_hot_impl")
}

/// Build a [`JumpTable`] mapping the running binary's component
/// impls to the patch dylib's, then call [`crate::apply_patch`].
pub fn apply_from_dylib(dylib_path: &Path) -> Result<(), DiffError> {
    // Lazily snapshot bin symbols on first call.
    let bin_path = std::env::current_exe()?;
    let bin_map = BIN_SYMBOLS
        .get_or_init(|| read_symbol_map(&bin_path).unwrap_or_default());
    if bin_map.is_empty() {
        return Err(DiffError::Parse(format!(
            "bin {} has no hot symbols (was the host built with framework-core/hot-reload?)",
            bin_path.display(),
        )));
    }

    // Always re-read the patch dylib; its content changes per
    // rebuild.
    let dylib_map = read_symbol_map(dylib_path)?;

    // ASLR reference: any symbol present in BOTH the bin and the
    // patch dylib. `main` is the canonical pick when the patch is
    // built as a bin (which is what `dx serve` does); our AAS
    // dylib mode builds the patch as a Rust `dylib` with no `main`,
    // so we fall back to whichever `__*_hot_impl` symbol both
    // images share. The math subsecond does inside `apply_patch`
    // is symmetric: it only cares that the reference appears at a
    // known link-time offset in both images, so any common symbol
    // works.
    let (aslr_reference, new_base_address) = pick_reference(bin_map, &dylib_map)
        .ok_or(DiffError::NoMain { in_: "no common symbol" })?;

    // Pair every symbol present in both. Including the reference
    // symbol is harmless — subsecond's per-entry math reduces to
    // a no-op for the reference (the patched offset equals the
    // reference offset by construction).
    let mut map = AddressMap::default();
    for (name, &bin_addr) in bin_map.iter() {
        if name == "main" || name == "_main" {
            continue;
        }
        let Some(&dylib_addr) = dylib_map.get(name) else { continue };
        map.insert(bin_addr, dylib_addr);
    }

    let table = JumpTable {
        lib: dylib_path.to_path_buf(),
        map,
        aslr_reference,
        new_base_address,
        ifunc_count: 0,
    };

    // SAFETY: every entry's source/target is a function address
    // for a `__*_hot_impl` symbol, both compiled from the same
    // source by the same rustc; signatures match by construction.
    unsafe { crate::apply_patch(table) }.map_err(DiffError::Patch)
}

fn pick_main(map: &HashMap<String, u64>) -> Option<u64> {
    map.get("main").or_else(|| map.get("_main")).copied()
}

/// Return a `(bin_addr, dylib_addr)` pair for a symbol present in
/// both maps. Prefers `main` / `_main` when available (the bin
/// almost always has one) so the math matches stock subsecond's
/// expectations; otherwise falls back to any `__*_hot_impl` symbol
/// that exists in both images. Deterministic across runs given the
/// same source — picks the lexicographically smallest name so
/// `apply_patch` reapplied with the same dylib produces the same
/// reference each time.
fn pick_reference(
    bin: &HashMap<String, u64>,
    dylib: &HashMap<String, u64>,
) -> Option<(u64, u64)> {
    if let (Some(b), Some(d)) = (pick_main(bin), pick_main(dylib)) {
        return Some((b, d));
    }
    let mut common: Vec<&String> = bin.keys().filter(|k| dylib.contains_key(*k)).collect();
    common.sort();
    let first = common.into_iter().next()?;
    Some((*bin.get(first)?, *dylib.get(first)?))
}
