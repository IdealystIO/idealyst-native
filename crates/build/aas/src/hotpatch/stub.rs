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
//! ## TLS path (deferred)
//!
//! TLS symbols still get a code trampoline, which is fine when
//! the framework owns the thread-locals (the framework's TLV
//! opcodes resolve through the host's text, accessing the host's
//! TLV slots). For tip-side `thread_local!` declarations the
//! proper fix is `add_symbol_data(SymbolKind::Tls, …)` which the
//! `object` crate writer auto-synthesizes a `__thread_vars`
//! descriptor for. Wired in a follow-up.

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

use super::cache::HostBinCache;

pub fn synthesize(
    tip_objs: &[PathBuf],
    cache: &HostBinCache,
    runtime_main: u64,
    // Kept in the signature for compatibility with the prior
    // assembly-file path; ignored now that the object writer
    // emits Mach-O bytes directly. We still want a stable
    // disk location for `stub.o` though so the link step finds
    // a deterministic input.
    _stub_s_path: &Path,
    stub_o_path: &Path,
) -> Result<()> {
    // 1. Walk every tip object for undefined externals.
    let mut undefs: HashSet<String> = HashSet::new();
    for obj_path in tip_objs {
        let data = std::fs::read(obj_path)
            .with_context(|| format!("read {}", obj_path.display()))?;
        let file = object::File::parse(&*data)
            .with_context(|| format!("parse {}", obj_path.display()))?;
        for sym in file.symbols() {
            if !sym.is_undefined() {
                continue;
            }
            let Ok(name) = sym.name() else { continue };
            if name.is_empty() {
                continue;
            }
            if is_system_symbol(name) {
                continue;
            }
            undefs.insert(name.to_string());
        }
    }

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
    let mut resolved: Vec<(String, u64)> = Vec::with_capacity(undefs.len());
    let mut deferred: Vec<String> = Vec::new();
    for u in &undefs {
        if let Some(runtime_addr) = cache.resolve_runtime(u, slide) {
            resolved.push((u.clone(), runtime_addr));
        } else {
            deferred.push(u.clone());
        }
    }
    if !deferred.is_empty() {
        eprintln!(
            "[hotpatch] {} undefined refs deferred to dyld (system / dynamic): {}{}",
            deferred.len(),
            deferred.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
            if deferred.len() > 8 {
                format!(", … +{} more", deferred.len() - 8)
            } else {
                String::new()
            },
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
    for (name, addr) in &resolved {
        let bytes = encode_trampoline(*addr);
        emit_symbol(&mut obj, text_id, name, &bytes);
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
