//! Synthesize the patch's stub object.
//!
//! For each undefined external symbol in the tip's `.rcgu.o`
//! files, emit a trampoline that jumps to the host bin's runtime
//! address for that symbol. The trampoline lives in the patch's
//! `__TEXT` and resolves the dylib's link-time reference, so when
//! the patch executes a call instruction it ends up running the
//! host's pre-linked code.
//!
//! v1 supports macOS aarch64. The trampoline shape is:
//!
//! ```asm
//! <name>:
//!     movz x16, #<lo16>
//!     movk x16, #<lo32_high16>, lsl #16
//!     movk x16, #<hi32_low16>,  lsl #32
//!     movk x16, #<hi32_high16>, lsl #48
//!     br x16
//! ```
//!
//! Up to 5 instructions (20 bytes) per stub; 3-4 typical (12-16 B).
//! We assemble the trampolines in-process via the `object` crate's
//! Mach-O writer — no `clang -c` subprocess, no temp `.s` file. On
//! macOS that saves ~25 ms per patch cycle (one fewer process
//! spawn).
//!
//! ## Statics and thread-locals
//!
//! A data symbol is defined as an ABSOLUTE symbol at the running
//! binary's address, and a thread-local as the absolute address of the
//! binary's TLV descriptor. Neither can be reached by a page-relative or
//! a thread-local fixup from a dylib; [`super::bind`] has already turned
//! every such reference into a GOT load, which an absolute symbol
//! satisfies. The statics the patch itself defines and the binary
//! already has are bound the same way, through the names in
//! [`Bindings::absolute`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use object::write::{
    Mangling, Object as WriteObject, StandardSection, Symbol as WriteSymbol, SymbolSection,
};
use object::{
    Architecture, BinaryFormat, Endianness, Object, ObjectSymbol, SymbolFlags, SymbolKind,
    SymbolScope,
};

use super::bind::Bindings;
use super::cache::HostBinCache;

pub fn synthesize(
    tip_objs: &[PathBuf],
    cache: &HostBinCache,
    bindings: &Bindings,
    runtime_main: u64,
    // Kept in the signature for compatibility with the prior
    // assembly-file path; ignored now that the object writer
    // emits Mach-O bytes directly. We still want a stable
    // disk location for `stub.o` though so the link step finds
    // a deterministic input.
    _stub_s_path: &Path,
    stub_o_path: &Path,
) -> Result<()> {
    // 1. Walk every tip object for undefined externals — undefined in
    // the PATCH, not in one object of it. A replay compiles the crate in
    // up to 256 codegen units, and a function one unit calls is very
    // often defined by another unit of the same replay. Stubbing that
    // name as well defines it twice, and the link fails with `duplicate
    // symbol` on every patch of a crate bigger than one unit.
    let mut undefs: HashSet<String> = HashSet::new();
    let mut defined: HashSet<String> = HashSet::new();
    for obj_path in tip_objs {
        let data = std::fs::read(obj_path)
            .with_context(|| format!("read {}", obj_path.display()))?;
        let file = object::File::parse(&*data)
            .with_context(|| format!("parse {}", obj_path.display()))?;
        for sym in file.symbols() {
            let Ok(name) = sym.name() else { continue };
            if name.is_empty() {
                continue;
            }
            if !sym.is_undefined() {
                if sym.is_global() {
                    defined.insert(name.to_string());
                }
                continue;
            }
            if is_system_symbol(name) {
                continue;
            }
            undefs.insert(name.to_string());
        }
    }
    undefs.retain(|name| !defined.contains(name));
    // The names `bind` pointed bound statics at are not in the binary;
    // they are defined below from `bindings`, at the binary's address.
    undefs.retain(|name| !bindings.absolute.contains_key(name) && name != super::bind::CTOR_RAN);

    // 2. Resolve each against the host cache. Symbols we can pin to
    // the host get a trampoline so the patch dylib jumps to the
    // already-loaded host code. Anything we can't pin we leave as
    // an undefined external — `link::link_dylib` passes
    // `-Wl,-undefined,dynamic_lookup` so dyld resolves them at
    // dlopen time. This covers anything in a system dylib
    // (libsystem_m for `_sin`/`_cos`, libsystem_c for the dozens of
    // libc fns not enumerated in `is_system_symbol`, CoreFoundation,
    // etc.) without us needing a complete allow-list.
    //
    // Genuinely-missing symbols (not in host, not in any system
    // dylib) still fail — but they fail later, at
    // `apply_patch`-time dlopen, with a precise "Symbol not found"
    // diagnostic. Better than aborting the patch up front and
    // forcing the user to manually classify every new libc/libm
    // reference their app picks up.
    let slide: i64 = runtime_main as i64 - cache.main_addr as i64;
    let mut resolved: Vec<(String, u64, super::cache::CachedSymbol)> =
        Vec::with_capacity(undefs.len());
    let mut deferred: Vec<String> = Vec::new();
    for u in &undefs {
        if let Some((runtime_addr, meta)) = cache.resolve_runtime_with_meta(u, slide) {
            resolved.push((u.clone(), runtime_addr, meta));
        } else {
            deferred.push(u.clone());
        }
    }
    if !deferred.is_empty() {
        // Partition: system symbols can safely defer to dyld (libSystem
        // etc.), but Rust-mangled symbols starting with `__ZN` are
        // monomorphizations internal to the host bin. The host bin
        // doesn't *export* them — they're private to its link unit —
        // so dyld can't find them at patch-dlopen time. The dylib
        // links cleanly (dyld_dynamic_lookup is permissive at link
        // time) but the first call to that symbol in the patched code
        // jumps to a null/garbage address → SIGSEGV → sidecar dies →
        // the user sees "hot-update didn't apply changes" because the
        // process died mid-rerender.
        //
        // This is the failure mode behind the welcome-example hot-
        // patch crash: a constant change perturbed codegen enough to
        // introduce a new `Rc<T>::deref` monomorphization that didn't
        // exist in the host bin's monomorphization set. Hash-stripped
        // fallback in `HostBinCache::resolve_runtime` covers
        // suffix-mismatches for the *same* logical instance — but if
        // the host bin never instantiated this specific type, there's
        // nothing to strip to.
        //
        // Bail loudly with the offending names so the host's
        // `try_hotpatch` → `respawn_sidecar` fallback ladder kicks in.
        // Respawn is slow (1-2s vs. ~1s for a hot-patch) but correct;
        // a silent SIGSEGV is the worst possible outcome.
        let rust_internal: Vec<&String> = deferred
            .iter()
            .filter(|n| n.starts_with("__ZN") || n.starts_with("_$LT"))
            .collect();
        if !rust_internal.is_empty() {
            anyhow::bail!(
                "hot-patch would defer {} Rust-internal monomorphization(s) to dyld \
                 that the host bin doesn't export — patched code would SIGSEGV on \
                 first call. Falling back to full sidecar respawn. Examples: {}{}",
                rust_internal.len(),
                rust_internal
                    .iter()
                    .take(3)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                if rust_internal.len() > 3 {
                    format!(", … +{} more", rust_internal.len() - 3)
                } else {
                    String::new()
                },
            );
        }
        dev_events::global().log(
            "hotpatch",
            format!(
                "{} undefined refs deferred to dyld (system / dynamic): {}{}",
                deferred.len(),
                deferred.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
                if deferred.len() > 8 {
                    format!(", … +{} more", deferred.len() - 8)
                } else {
                    String::new()
                },
            ),
        );
    }

    // Does the tip already define `_main`? Bin tips do; rlib tips
    // don't. We emit a stub `_main: ret` if absent so subsecond's
    // `apply_patch` can dlsym it as the ASLR anchor.
    let has_main_in_tip = scan_for_main(tip_objs)?;

    // 3. Build the Mach-O object in memory. One section
    // (`__TEXT,__text`), one symbol per trampoline.
    let mut obj = WriteObject::new(
        BinaryFormat::MachO,
        Architecture::Aarch64,
        Endianness::Little,
    );
    // Disable the writer's automatic Mach-O mangling (which prepends
    // a leading underscore to every symbol). The names we read from
    // the tip's nlist already include the C-ABI underscore prefix —
    // re-prepending would store e.g. `__main` where dlsym(b"main")
    // wants to find `_main`, and likewise mangle every framework
    // symbol the tip referenced. With mangling off, names round-trip
    // verbatim.
    obj.set_mangling(Mangling::None);
    let text_id = obj.section_id(StandardSection::Text);

    // Each trampoline's instruction sequence aligned to 4 bytes.
    if !has_main_in_tip {
        // _main: ret  →  0xD65F03C0  (RET, lr in x30)
        emit_symbol(&mut obj, text_id, "_main", &encode_ret());
    }

    // Dispatch on symbol kind:
    //
    // * Text: a jump trampoline. The dylib's `bl` lands on it, it jumps
    //   to the binary's code, which returns straight to the caller
    //   (`br x16`, lr untouched).
    //
    // * Everything else — statics, globals, and thread-locals — an
    //   absolute symbol at the binary's runtime address. For a static
    //   that IS the static. For a thread-local it is the binary's TLV
    //   descriptor: `bind` rewrote the patch's thread-local loads into
    //   GOT loads of that address, so the patch calls the binary's own
    //   thunk and gets the binary's slot for the thread. A code
    //   trampoline here would be wrong for both — data written through it
    //   overwrites instructions, and a TLV access loads its first 8 bytes
    //   as the thunk pointer and branches into them.
    for (name, abs_addr, sym) in &resolved {
        use object::write::SymbolSection as WS;
        use object::{SymbolKind as K, SymbolScope};

        match sym.kind {
            K::Text => {
                let bytes = encode_trampoline(*abs_addr);
                emit_symbol(&mut obj, text_id, name, &bytes);
            }
            other => {
                // Darwin reports many statics as Unknown.
                let kind = if matches!(other, K::Unknown | K::Tls) { K::Data } else { other };
                obj.add_symbol(WriteSymbol {
                    name: name.as_bytes().to_vec(),
                    value: *abs_addr,
                    size: 0,
                    scope: SymbolScope::Linkage,
                    kind,
                    weak: sym.is_weak,
                    section: WS::Absolute,
                    flags: SymbolFlags::None,
                });
            }
        }
    }
    if bindings.ctor_ran {
        emit_symbol(&mut obj, text_id, super::bind::CTOR_RAN, &encode_ret());
    }
    for (name, link_addr) in &bindings.absolute {
        obj.add_symbol(WriteSymbol {
            name: name.as_bytes().to_vec(),
            value: (*link_addr as i64 + slide) as u64,
            size: 0,
            scope: SymbolScope::Linkage,
            kind: SymbolKind::Data,
            weak: false,
            section: SymbolSection::Absolute,
            flags: SymbolFlags::None,
        });
    }

    let bytes = obj.write().context("serialize stub Mach-O")?;
    std::fs::write(stub_o_path, &bytes)
        .with_context(|| format!("write {}", stub_o_path.display()))?;
    Ok(())
}

/// Insert a defined symbol into `__TEXT,__text` pointing at the
/// given instruction bytes. Symbol is global so the patch dylib's
/// linker can resolve undefined refs in the tip objects against it.
fn emit_symbol(obj: &mut WriteObject<'_>, text_id: object::write::SectionId, name: &str, code: &[u8]) {
    let sym_id = obj.add_symbol(WriteSymbol {
        name: name.as_bytes().to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(text_id),
        flags: SymbolFlags::None,
    });
    obj.add_symbol_data(sym_id, text_id, code, 4);
}

/// Emit `movz x16, #lo0 / movk lsl #16 / movk lsl #32 / movk lsl
/// #48 / br x16` for the given absolute address. Skip the movks
/// whose immediate is zero — purely a size optimization.
///
/// aarch64 instruction encoding (sf=1 → 64-bit):
///   MOVZ x16, #imm16, lsl #(hw*16):  base 0xD2800010 + (hw<<21) + (imm16<<5)
///   MOVK x16, #imm16, lsl #(hw*16):  base 0xF2800010 + (hw<<21) + (imm16<<5)
///   BR x16:                          0xD61F0200
fn encode_trampoline(addr: u64) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(20);
    let lo0 = (addr & 0xFFFF) as u32;
    let lo1 = ((addr >> 16) & 0xFFFF) as u32;
    let lo2 = ((addr >> 32) & 0xFFFF) as u32;
    let lo3 = ((addr >> 48) & 0xFFFF) as u32;

    // MOVZ always emitted (sets x16 and zeros the rest).
    push_u32_le(&mut out, 0xD2800010 | (lo0 << 5));
    // MOVK for any non-zero higher half-word.
    if lo1 != 0 {
        push_u32_le(&mut out, 0xF2A00010 | (lo1 << 5));
    }
    if lo2 != 0 {
        push_u32_le(&mut out, 0xF2C00010 | (lo2 << 5));
    }
    if lo3 != 0 {
        push_u32_le(&mut out, 0xF2E00010 | (lo3 << 5));
    }
    // BR x16
    push_u32_le(&mut out, 0xD61F0200);
    out
}

/// 0xD65F03C0 = `ret` (RET to x30).
fn encode_ret() -> Vec<u8> {
    0xD65F03C0_u32.to_le_bytes().to_vec()
}

fn push_u32_le(buf: &mut Vec<u8>, word: u32) {
    buf.extend_from_slice(&word.to_le_bytes());
}

fn scan_for_main(tip_objs: &[PathBuf]) -> Result<bool> {
    for obj_path in tip_objs {
        let data = std::fs::read(obj_path)?;
        let file = object::File::parse(&*data)?;
        for sym in file.symbols() {
            let Ok(name) = sym.name() else { continue };
            if (name == "_main" || name == "main") && !sym.is_undefined() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Symbols dyld resolves at dlopen time against libSystem etc.
/// Keeping them as undefined refs in the patch is correct; the
/// `-Wl,-undefined,dynamic_lookup` linker flag picks them up.
fn is_system_symbol(name: &str) -> bool {
    name.starts_with("_dyld_")
        || name.starts_with("_pthread_")
        || name.starts_with("_dispatch_")
        || name.starts_with("__os_log")
        || name.starts_with("_objc_")
        || name.starts_with("__Unwind_")
        || name.starts_with("__tlv_")
        || matches!(
            name,
            // libc allocator + string + io
            "_malloc" | "_free" | "_calloc" | "_realloc" | "_posix_memalign"
                | "_memcpy" | "_memmove" | "_memset" | "_memcmp" | "_bcmp"
                | "_strlen" | "_strcmp" | "_strncmp" | "_strcpy" | "_strncpy"
                | "_strcat" | "_write" | "_read" | "_close" | "_open"
                | "_abort" | "_exit" | "___error"
                | "_dlsym" | "_dlopen" | "_dlclose" | "_dlerror"
                | "_getenv" | "_setenv" | "_unsetenv" | "_environ"
                | "_clock_gettime" | "_mach_absolute_time"
                // libc fs + process
                | "_fcntl" | "_lseek" | "_pread" | "_pwrite" | "_stat"
                | "_fstat" | "_lstat" | "_ftruncate" | "_truncate"
                | "_unlink" | "_rmdir" | "_mkdir" | "_chmod" | "_fchmod"
                | "_chdir" | "_getcwd" | "_getpid" | "_getppid"
                | "_kill" | "_waitpid" | "_pipe" | "_dup" | "_dup2"
                | "_isatty" | "_ttyname" | "_readlink" | "_symlink"
                | "_access" | "_realpath" | "_mkstemp" | "_mkdtemp"
                | "_sysctl" | "_sysctlbyname" | "_mmap" | "_munmap"
                | "_mprotect" | "_msync" | "_madvise"
                // libc threads + signals
                | "_sigaction" | "_sigaltstack" | "_raise"
                | "_setjmp" | "_longjmp" | "__setjmp" | "__longjmp"
                | "_getrlimit" | "_setrlimit" | "_getrusage"
        )
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;
    use crate::hotpatch::cache::CachedSymbol;

    /// A replay compiles the crate in many codegen units, and one unit
    /// often calls a function another unit defines. Stubbing that name
    /// too defined it twice, and every patch of a crate bigger than one
    /// unit failed to link with `duplicate symbol` — the native tier then
    /// always fell back to a respawn.
    #[test]
    fn regression_a_function_another_unit_defines_is_not_stubbed() {
        let dir = std::env::temp_dir().join(format!("idealyst-stub-units-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let assemble = |name: &str, source: &str| -> PathBuf {
            let s = dir.join(format!("{name}.s"));
            let o = dir.join(format!("{name}.o"));
            std::fs::write(&s, source).unwrap();
            let ok = std::process::Command::new("cc").args(["-c", "-arch", "arm64"]).arg(&s).arg("-o").arg(&o).status().unwrap();
            assert!(ok.success());
            o
        };
        let defines = assemble("unit0", ".text\n.globl _helper\n.p2align 2\n_helper:\n  ret\n");
        let calls = assemble("unit1", ".text\n.globl _caller\n.p2align 2\n_caller:\n  b _helper\n");
        let mut cache = HostBinCache::default();
        cache.main_addr = 0x1_0000_0000;
        for (name, address) in [("_main", 0x1_0000_0000u64), ("_helper", 0x1_0000_1000)] {
            cache.symbols.insert(name.into(), CachedSymbol { address, kind: SymbolKind::Text, size: 0, is_weak: false });
        }
        let objs = vec![defines, calls];
        let stub = dir.join("stub.o");
        synthesize(&objs, &cache, &Bindings::default(), 0x1_0000_0000, &dir.join("stub.s"), &stub).unwrap();
        let data = std::fs::read(&stub).unwrap();
        let file = object::File::parse(&*data).unwrap();
        assert!(
            !file.symbols().any(|s| s.name() == Ok("_helper")),
            "the stub defines `_helper`, which the patch's own unit0 defines"
        );
        crate::hotpatch::link::link_dylib(&objs, &stub, &dir.join("patch.dylib")).expect("the patch links");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
