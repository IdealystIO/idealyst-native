//! `idealyst link-patch <mode-dir>` — dx-style thin-link hot-patch
//! builder.
//!
//! ## Architecture
//!
//! Replaces the rustc-replay path with a custom link step that
//! mirrors what `dx serve` does for Dioxus. Each patch becomes a
//! dylib whose link inputs are ONLY:
//!
//! - The user crate's freshly-emitted `.rcgu.o` files
//! - A **synthesized stub object** containing one tiny function per
//!   undefined symbol. Each stub is `movz/movk` building an
//!   absolute address into a scratch register followed by `br` to
//!   that address — the symbol's *runtime address inside the
//!   already-running host bin*.
//!
//! That means the patch dylib carries zero LC_LOAD_DYLIB
//! dependencies on framework crates and zero lazy-bind entries to
//! resolve. `dlopen` reduces to mach-o parse + mmap; on macOS
//! aarch64 we observe <30 ms apply times that way (vs. ~440 ms with
//! the older rlib-bloated patch). Combined with rustc-replay
//! incremental compilation this gets the full edit→re-render cycle
//! to ~250-400 ms.
//!
//! ## Inputs
//!
//! - `<mode-dir>/.rustc-args/docs.cdylib.json` — captured rustc
//!   invocation for the user crate. We replay it with
//!   `--emit=obj` instead of `--emit=link` so rustc emits raw
//!   object files and skips its own link step.
//! - `<mode-dir>/host-symbols.json` — every defined symbol in the
//!   host bin with its link-time address. Captured by the initial
//!   `idealyst build --aas` after the host bin is produced.
//! - `<mode-dir>/host-base.txt` — the running host bin's runtime
//!   `_main` address. Written by the host at startup so the linker
//!   process (a separate cargo-invoked child) can compute the
//!   ASLR slide.
//!
//! ## Outputs
//!
//! - `<workspace>/target/debug/libpatch.dylib` (overwriting the
//!   previous patch). Hardlinked / copied to a unique
//!   `libpatch-apply-N.dylib` by the host on apply to defeat
//!   dyld's path-keyed dlopen cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use object::{Object, ObjectSymbol};
use serde::{Deserialize, Serialize};

use crate::cmd::rustc_capture::CapturedInvocation;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// AAS dylib sub-workspace dir.
    pub mode_dir: PathBuf,
}

/// On-disk record of every defined symbol in the host bin. Written
/// once during the initial fat build; read on every patch link.
#[derive(Serialize, Deserialize, Default)]
pub struct HostSymbolTable {
    /// `symbol_name → link-time virtual address` (the value
    /// `object` reports for the symbol).
    pub symbols: HashMap<String, u64>,
    /// Link-time address of `_main` (or `main`) — used to compute
    /// the host's ASLR slide at patch-link time, by subtracting it
    /// from the runtime `_main` address the host writes to disk.
    pub main_addr: u64,
}

pub fn run(args: Args) -> Result<()> {
    let mode_dir = args.mode_dir;
    let capture_dir = mode_dir.join(".rustc-args");
    let host_syms_path = mode_dir.join("host-symbols.json");
    let host_base_path = mode_dir.join("host-base.txt");

    let host_syms: HostSymbolTable = {
        let data = std::fs::read(&host_syms_path).with_context(|| {
            format!(
                "read host symbol table at {}; was `idealyst build --aas` run with capture enabled?",
                host_syms_path.display()
            )
        })?;
        serde_json::from_slice(&data).with_context(|| {
            format!("parse host symbol table at {}", host_syms_path.display())
        })?
    };

    let runtime_main: u64 = {
        let s = std::fs::read_to_string(&host_base_path).with_context(|| {
            format!(
                "read host runtime base at {}; is the host running?",
                host_base_path.display()
            )
        })?;
        s.trim()
            .strip_prefix("0x")
            .map(|x| u64::from_str_radix(x, 16))
            .unwrap_or_else(|| s.trim().parse::<u64>())
            .with_context(|| format!("parse host base address from {:?}", s))?
    };
    let aslr_slide: i64 = runtime_main as i64 - host_syms.main_addr as i64;

    // Step 1: re-emit the user crate's object files. Replay docs's
    // captured rustc invocation with `--emit=obj` so rustc skips
    // linking. The .rcgu.o files land in the user crate's
    // incremental-compile dir.
    let docs_capture: CapturedInvocation = load_capture(&capture_dir, "docs")
        .with_context(|| "load docs rustc capture")?;
    let docs_objs = run_rustc_emit_obj(&docs_capture)?;
    if docs_objs.is_empty() {
        anyhow::bail!("rustc --emit=obj produced no object files");
    }

    // Step 2: walk every emitted object for undefined external
    // symbols. Each one needs a stub.
    let mut wanted: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for obj in &docs_objs {
        let data = std::fs::read(obj)
            .with_context(|| format!("read {}", obj.display()))?;
        let file = object::File::parse(&*data)
            .with_context(|| format!("parse {}", obj.display()))?;
        for sym in file.symbols() {
            if !sym.is_undefined() {
                continue;
            }
            let Ok(name) = sym.name() else { continue };
            if name.is_empty() {
                continue;
            }
            // Skip libc / system symbols — dyld can resolve those
            // against libSystem when the patch dylib loads.
            if is_system_symbol(name) {
                continue;
            }
            if seen.insert(name.to_string()) {
                wanted.push(name.to_string());
            }
        }
    }

    // Step 3: resolve each wanted symbol against the host's table.
    let mut resolved: Vec<(String, u64)> = Vec::with_capacity(wanted.len());
    let mut unresolved: Vec<String> = Vec::new();
    for name in &wanted {
        // Mach-O symbol names from object's `.symbols()` are
        // already prefixed with `_` for C-ABI conventions.
        // host_syms.symbols stores them as `object` reads them
        // from the bin, which keeps the underscore — so a direct
        // lookup works.
        if let Some(&link_time) = host_syms.symbols.get(name) {
            let runtime = (link_time as i64 + aslr_slide) as u64;
            resolved.push((name.clone(), runtime));
        } else {
            unresolved.push(name.clone());
        }
    }
    if !unresolved.is_empty() {
        eprintln!(
            "[link-patch] {} undefined refs cannot be resolved from host bin:",
            unresolved.len()
        );
        for (i, u) in unresolved.iter().enumerate().take(20) {
            eprintln!("  {}: {}", i, u);
        }
        if unresolved.len() > 20 {
            eprintln!("  ... and {} more", unresolved.len() - 20);
        }
        anyhow::bail!(
            "patch link aborted: missing {} symbols. Was the host built with `-Wl,-export_dynamic`?",
            unresolved.len()
        );
    }

    // Step 4: synthesize the stub object. We write aarch64 assembly
    // and run the system assembler — simpler than emitting Mach-O
    // bytes by hand and equally fast.
    let stub_dir = mode_dir.join(".stub");
    std::fs::create_dir_all(&stub_dir)?;
    let stub_s = stub_dir.join("stub.s");
    let stub_o = stub_dir.join("stub.o");
    write_stub_assembly(&stub_s, &resolved)?;
    assemble_stub(&stub_s, &stub_o)?;

    // Step 5: link the patch dylib. Bare `ld -dylib` with the user
    // crate's .o files + the stub.o, no rlibs.
    let patch_out = PathBuf::from(&docs_capture.cwd)
        .ancestors()
        .find(|p| p.file_name().map(|n| n == "debug").unwrap_or(false))
        // Fallback: use the workspace target dir under the
        // capture's cwd. The `cwd` from cargo is typically the
        // crate root; we want target/debug for output.
        .map(|p| p.join("libpatch.dylib"))
        .unwrap_or_else(|| {
            // Last resort — write into the mode_dir.
            mode_dir.join("libpatch.dylib")
        });
    // Actually the host expects `libpatch.dylib` in either
    // <workspace>/target/debug/ or its deps/ subdir. The captured
    // rustc command's --out-dir tells us where deps/ lives.
    let deps_dir = parse_out_dir(&docs_capture).unwrap_or_else(|| stub_dir.clone());
    let patch_out_deps = deps_dir.join("libpatch.dylib");
    let _ = patch_out;
    link_dylib(&docs_objs, &stub_o, &patch_out_deps)?;
    // Also hardlink (or copy) up to target/debug/libpatch.dylib for
    // the host's `patch_path()` discovery to find it via its
    // primary path.
    if let Some(deps_parent) = deps_dir.parent() {
        let canonical = deps_parent.join("libpatch.dylib");
        let _ = std::fs::remove_file(&canonical);
        let _ = std::fs::hard_link(&patch_out_deps, &canonical)
            .or_else(|_| std::fs::copy(&patch_out_deps, &canonical).map(|_| ()));
    }
    Ok(())
}

/// Locate a captured rustc invocation by crate name.
fn load_capture(capture_dir: &Path, crate_name: &str) -> Result<CapturedInvocation> {
    let prefix = format!("{crate_name}.");
    for entry in std::fs::read_dir(capture_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".json") {
            let data = std::fs::read(entry.path())?;
            return Ok(serde_json::from_slice(&data)?);
        }
    }
    anyhow::bail!("no capture for crate `{crate_name}` in {}", capture_dir.display())
}

/// Run rustc with the captured args modified to `--emit=obj`.
/// Returns the list of `.rcgu.o` files rustc produced. We discover
/// them by reading the artifacts json messages rustc emits when
/// `--json=artifacts` is in the captured args (cargo always passes
/// it).
fn run_rustc_emit_obj(captured: &CapturedInvocation) -> Result<Vec<PathBuf>> {
    // Build a modified arg vector: replace `--emit=link,…` with
    // `--emit=obj` so rustc skips linking and writes raw object
    // files. We deliberately preserve `--crate-type` — rustc
    // needs at least one (otherwise it defaults to `bin` and
    // complains the crate has no `main`). Keep only the FIRST
    // crate-type (typically `rlib`) so rustc emits a single
    // codegen layout; cdylib's extra outputs aren't useful for
    // our stub-linker path.
    let mut args: Vec<String> = Vec::with_capacity(captured.args.len() + 2);
    let mut emit_set = false;
    let mut crate_type_set = false;
    let mut i = 0;
    while i < captured.args.len() {
        let a = &captured.args[i];
        if a == "--emit" {
            args.push("--emit=obj".to_string());
            emit_set = true;
            i += 2;
            continue;
        }
        if a.starts_with("--emit=") {
            args.push("--emit=obj".to_string());
            emit_set = true;
            i += 1;
            continue;
        }
        if a == "--crate-type" {
            if !crate_type_set {
                args.push("--crate-type".to_string());
                if let Some(v) = captured.args.get(i + 1) {
                    // Force `rlib` regardless of cargo's preference
                    // — rlib produces a clean .o emission shape.
                    let _ = v;
                    args.push("rlib".to_string());
                }
                crate_type_set = true;
            }
            i += 2;
            continue;
        }
        if a.starts_with("--crate-type=") {
            if !crate_type_set {
                args.push("--crate-type=rlib".to_string());
                crate_type_set = true;
            }
            i += 1;
            continue;
        }
        args.push(a.clone());
        i += 1;
    }
    if !emit_set {
        args.push("--emit=obj".to_string());
    }
    if !crate_type_set {
        args.push("--crate-type=rlib".to_string());
    }

    let mut cmd = Command::new(&captured.rustc);
    cmd.args(&args).current_dir(&captured.cwd);
    cmd.env_clear();
    for (k, v) in &captured.env {
        cmd.env(k, v);
    }
    // rustc with `--error-format=json` writes ALL its JSON messages
    // (diagnostics AND artifact notifications) to stderr; stdout
    // stays empty. So we capture stderr, pipe a copy of any
    // *rendered* diagnostic lines back to the user's terminal, and
    // parse the JSON to find emitted `.o` paths.
    let output = cmd
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("spawn rustc for patch object emission")?;
    if !output.status.success() {
        // Echo stderr to the user before failing — they'll want to
        // see the compile error.
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &output.stderr);
        anyhow::bail!("rustc --emit=obj exited with {}", output.status);
    }

    let mut out = Vec::new();
    for line in output.stderr.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        match val.get("$message_type").and_then(|v| v.as_str()) {
            Some("artifact") => {
                let Some(path) = val.get("artifact").and_then(|v| v.as_str()) else {
                    continue;
                };
                if path.ends_with(".o") {
                    out.push(PathBuf::from(path));
                }
            }
            Some("diagnostic") => {
                if let Some(rendered) = val.get("rendered").and_then(|v| v.as_str()) {
                    eprint!("{}", rendered);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Pull `--out-dir <DIR>` out of the captured argv.
fn parse_out_dir(captured: &CapturedInvocation) -> Option<PathBuf> {
    let mut iter = captured.args.iter();
    while let Some(a) = iter.next() {
        if a == "--out-dir" {
            return iter.next().map(PathBuf::from);
        }
        if let Some(rest) = a.strip_prefix("--out-dir=") {
            return Some(PathBuf::from(rest));
        }
    }
    None
}

/// Filter out symbols that the system loader will resolve against
/// libSystem etc. Keeping them as undefined refs in the patch is
/// fine; they'll lazy-bind correctly.
fn is_system_symbol(name: &str) -> bool {
    // Anything in `_dyld_*`, `_pthread_*`, `_dispatch_*`, plus a
    // handful of obvious libc primitives. The patch can keep these
    // as lazy binds — dyld resolves them against libSystem in <ms
    // because `-lSystem` is on the final `ld` command line.
    //
    // `__Unwind_*` are the Itanium C++ ABI unwinder primitives —
    // libunwind / libSystem provides them. `__tlv_*` are macOS
    // thread-local-variable trampolines, also in libSystem. Both
    // appear as undefined refs in nearly every Rust object file
    // and never need to be host-resolved.
    name.starts_with("_dyld_")
        || name.starts_with("_pthread_")
        || name.starts_with("_dispatch_")
        || name.starts_with("__os_log")
        || name.starts_with("_objc_")
        || name.starts_with("__Unwind_")
        || name.starts_with("__tlv_")
        || matches!(
            name,
            "_malloc"
                | "_free"
                | "_calloc"
                | "_realloc"
                | "_posix_memalign"
                | "_memcpy"
                | "_memmove"
                | "_memset"
                | "_memcmp"
                | "_bcmp"
                | "_strlen"
                | "_strcmp"
                | "_strncmp"
                | "_strcpy"
                | "_strncpy"
                | "_strcat"
                | "_write"
                | "_read"
                | "_close"
                | "_open"
                | "_abort"
                | "_exit"
                | "___error"
                | "_dlsym"
                | "_dlopen"
                | "_dlclose"
                | "_dlerror"
        )
}

/// Synthesize an aarch64 assembly file with one trampoline per
/// resolved symbol. Each trampoline materializes the 64-bit
/// runtime address with movz/movk and branches to it.
///
/// We also emit a `_main` stub (no-op, just `ret`) so subsecond's
/// `apply_patch` — which does `lib.get(b"main")` to anchor the
/// dylib's ASLR — finds something to anchor against.
fn write_stub_assembly(path: &Path, stubs: &[(String, u64)]) -> Result<()> {
    let mut out = String::with_capacity(64 + stubs.len() * 256);
    out.push_str(".section __TEXT,__text,regular,pure_instructions\n");
    out.push_str(".p2align 2\n\n");
    // Synthetic main — subsecond uses `_main`'s address as the
    // patch dylib's ASLR base. It's never called.
    out.push_str(".globl _main\n");
    out.push_str("_main:\n");
    out.push_str("    ret\n\n");
    for (name, addr) in stubs {
        let lo0 = (*addr) & 0xFFFF;
        let lo1 = (*addr >> 16) & 0xFFFF;
        let lo2 = (*addr >> 32) & 0xFFFF;
        let lo3 = (*addr >> 48) & 0xFFFF;
        out.push_str(&format!(".globl {name}\n"));
        out.push_str(&format!("{name}:\n"));
        out.push_str(&format!("    movz x16, #0x{:x}\n", lo0));
        if lo1 != 0 {
            out.push_str(&format!("    movk x16, #0x{:x}, lsl #16\n", lo1));
        }
        if lo2 != 0 {
            out.push_str(&format!("    movk x16, #0x{:x}, lsl #32\n", lo2));
        }
        if lo3 != 0 {
            out.push_str(&format!("    movk x16, #0x{:x}, lsl #48\n", lo3));
        }
        out.push_str("    br x16\n\n");
    }
    std::fs::write(path, out)
        .with_context(|| format!("write stub assembly to {}", path.display()))?;
    Ok(())
}

fn assemble_stub(src: &Path, out: &Path) -> Result<()> {
    let status = Command::new("clang")
        .args(["-c", "-arch", "arm64"])
        .arg(src)
        .arg("-o")
        .arg(out)
        .status()
        .context("spawn clang to assemble stub.s")?;
    if !status.success() {
        anyhow::bail!("clang -c exited with {status}");
    }
    Ok(())
}

/// Final link: produce `libpatch.dylib` by combining the user
/// crate's `.rcgu.o` files with our stub object. No rlibs, no
/// framework dylibs — the stub object already encodes every
/// external reference as an absolute jump.
fn link_dylib(user_objs: &[PathBuf], stub_obj: &Path, out: &Path) -> Result<()> {
    let mut cmd = Command::new("ld");
    cmd.args([
        "-dylib",
        "-arch",
        "arm64",
        "-platform_version",
        "macos",
        "12.0",
        "12.0",
        // No external dylib dependencies, but we still need libSystem
        // for any system-symbol lazy binds that survived our filter.
        "-lSystem",
        "-syslibroot",
    ])
    .arg(
        Command::new("xcrun")
            .args(["--show-sdk-path"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk".to_string()),
    );
    for o in user_objs {
        cmd.arg(o);
    }
    cmd.arg(stub_obj);
    cmd.arg("-o").arg(out);
    let status = cmd.status().context("spawn ld for patch link")?;
    if !status.success() {
        anyhow::bail!("ld exited with {status}");
    }
    Ok(())
}
