//! The server hot-patch agent: a library loaded into the dev session's
//! server process that applies function patches to it while it runs.
//!
//! `idealyst dev` compiles this file on its own with `rustc --crate-type
//! cdylib --cfg idealyst_hot_agent` (no cargo, no dependencies) and has
//! the server load it at exec: `idealyst run-linked`, the runner the
//! session gives `cargo run`, sets `DYLD_INSERT_LIBRARIES`. The server's
//! own code knows nothing about it. `build_runtime_server`'s
//! `hotpatch::detour` module builds the patches and documents the design;
//! this file is the half that runs inside the server.
//!
//! # What it does
//!
//! On load it connects to the session's Unix socket (path in
//! `IDEALYST_HOT_AGENT_SOCKET`), reports `hello`, and waits for commands.
//! An `apply` names a patch dylib and a file of address pairs:
//!
//! 1. `dlopen` the patch. Its initializers run, and every symbol the
//!    patch's code needs that the patch does not define resolves into
//!    the running binary (the session linked it that way).
//! 2. For each pair, overwrite the START of the binary's function with a
//!    jump to the patch's copy. Every caller — a direct call, a function
//!    pointer, a vtable — then runs the new body from its next call on.
//!
//! # The jump, and why other threads are stopped while it is written
//!
//! When the patch's copy is within ±128 MB, the jump is one `b`. It
//! usually is not (dyld maps the patch wherever it likes), so the jump is
//! `ldr x16, #8; br x16; .quad target` — 16 bytes over the function's
//! first four instructions. x16 is the intra-procedure-call scratch
//! register the arm64 ABI lets any call clobber, which the linker's own
//! veneers rely on.
//!
//! Sixteen bytes can be torn: a thread that already ran the old first
//! instruction and then fetches the new second one branches through a
//! register nobody loaded; a thread inside a callee called from those
//! bytes returns into the middle of the new sequence. Islands placed
//! within `b` reach would avoid both, but a large debug server leaves no
//! room for them: CrewForge's 181 MB `__TEXT` is followed by 322 MB of
//! `__LINKEDIT`, so the middle of its text has no free address in reach.
//! So every other thread is SUSPENDED, and the patch is written only when
//! none of them has its program counter, its link register, or a return
//! address on its frame-pointer chain inside a rewritten window. If one
//! does, the threads resume and the agent tries again a millisecond
//! later. While they are suspended the agent allocates nothing and takes
//! no lock — a suspended thread may hold the allocator's.
//!
//! A function shorter than 16 bytes cannot hold the long jump. The
//! session leaves such a function out when its patch copy is
//! byte-identical (it did not change); one that changed needs its patch
//! within `b` reach, or the patch is refused.
//!
//! # Why through a new page
//!
//! The binary's text is mapped read-execute, and macOS refuses a
//! writable-and-executable mapping to an ordinary process (measured:
//! `mach_vm_protect(…, RWX | COPY)` fails with `KERN_PROTECTION_FAILURE`),
//! while making the page writable WITHOUT execute faults every thread
//! that runs code on it meanwhile. So each touched page is rebuilt
//! elsewhere (a copy with the new instructions), made read-execute, and
//! then mapped OVER the original in one call (`mach_vm_remap` with
//! `VM_FLAGS_OVERWRITE`). Measured on macOS 26: the copy executes, and a
//! page swapped under eight threads calling into it never faulted.
//!
//! # Safety the session guarantees, not this file
//!
//! A pair is only ever (base fn, patch fn) of the same mangled name, so
//! the signature is the same. The session refuses edits that a detour
//! cannot carry (a changed signature or type, a generic, inline or async
//! body, a closure) and rebuilds instead; see the builder's docs.

#![allow(clippy::missing_safety_doc)]

use std::io::{BufRead, BufReader, Write};

/// The protocol's version, in `hello`. The session refuses an agent of
/// another version: the two are built from the same CLI, so a mismatch
/// means a stale agent dylib.
pub const PROTOCOL: u32 = 2;

/// The environment variable naming the session's socket.
pub const SOCKET_ENV: &str = "IDEALYST_HOT_AGENT_SOCKET";

/// Set to `1`, every detour takes the long jump, even with the patch in
/// `b` reach: a test's small binary and its patch are usually that close,
/// and the long jump is the path a real server takes.
pub const LONG_JUMPS_ENV: &str = "IDEALYST_HOT_AGENT_LONG_JUMPS";

// ── arm64 encoding ──────────────────────────────────────────────────────

pub mod arm64 {
    /// How far a `b` reaches, either way: a signed 26-bit word offset.
    pub const B_REACH: i64 = 128 * 1024 * 1024;

    /// Bytes the long jump overwrites.
    pub const LONG_JUMP: u64 = 16;

    /// `b <to>` placed at `from`, or `None` when `to` is out of reach or
    /// either address is misaligned.
    pub fn b(from: u64, to: u64) -> Option<u32> {
        if from % 4 != 0 || to % 4 != 0 {
            return None;
        }
        let delta = to as i64 - from as i64;
        if !(-B_REACH..B_REACH).contains(&delta) {
            return None;
        }
        let imm26 = ((delta >> 2) as u32) & 0x03FF_FFFF;
        Some(0x1400_0000 | imm26)
    }

    /// The target of a `b` instruction at `from`, if `word` is one.
    pub fn b_target(from: u64, word: u32) -> Option<u64> {
        if word & 0xFC00_0000 != 0x1400_0000 {
            return None;
        }
        // Sign-extend the 26-bit immediate.
        let imm = ((word & 0x03FF_FFFF) << 6) as i32 >> 6;
        Some((from as i64 + (imm as i64) * 4) as u64)
    }

    /// A jump to `target` from anywhere: `ldr x16, #8; br x16; .quad
    /// target`, as four little-endian words.
    pub fn long_jump(target: u64) -> [u32; 4] {
        [
            0x5800_0050, // ldr x16, #8
            0xD61F_0200, // br x16
            target as u32,
            (target >> 32) as u32,
        ]
    }
}

// ── the protocol ────────────────────────────────────────────────────────

pub mod proto {
    use std::path::PathBuf;

    /// What the session asks.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Command {
        /// Load `dylib` and detour every pair in `pairs`.
        Apply { seq: u64, dylib: PathBuf, pairs: PathBuf },
        /// Answer `pong`: a liveness probe.
        Ping,
    }

    pub fn parse_command(line: &str) -> Result<Command, String> {
        let line = line.trim_end_matches(['\n', '\r']);
        if line == "ping" {
            return Ok(Command::Ping);
        }
        let rest = line.strip_prefix("apply ").ok_or_else(|| format!("unknown command {line:?}"))?;
        let (seq, paths) = rest.split_once(' ').ok_or("apply: no paths")?;
        let seq = seq.parse().map_err(|_| "apply: no seq")?;
        // Tab-separated, so a path may contain spaces.
        let (dylib, pairs) = paths.split_once('\t').ok_or("apply: paths must be tab-separated")?;
        Ok(Command::Apply { seq, dylib: PathBuf::from(dylib), pairs: PathBuf::from(pairs) })
    }

    pub fn format_apply(seq: u64, dylib: &std::path::Path, pairs: &std::path::Path) -> String {
        format!("apply {seq} {}\t{}\n", dylib.display(), pairs.display())
    }

    /// `hello <protocol> <pid> <slide hex> <exe>`.
    pub fn format_hello(protocol: u32, pid: u32, slide: u64, exe: &str) -> String {
        format!("hello {protocol} {pid} {slide:x} {exe}\n")
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Hello {
        pub protocol: u32,
        pub pid: u32,
        pub slide: u64,
        pub exe: String,
    }

    pub fn parse_hello(line: &str) -> Option<Hello> {
        let line = line.trim_end_matches(['\n', '\r']);
        let mut w = line.splitn(5, ' ');
        if w.next()? != "hello" {
            return None;
        }
        Some(Hello {
            protocol: w.next()?.parse().ok()?,
            pid: w.next()?.parse().ok()?,
            slide: u64::from_str_radix(w.next()?, 16).ok()?,
            exe: w.next().unwrap_or("").to_string(),
        })
    }

    /// The agent's answer to an `apply`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Reply {
        Ok { seq: u64, detoured: usize, micros: u64 },
        Err { seq: u64, message: String },
        Pong,
    }

    pub fn format_reply(r: &Reply) -> String {
        match r {
            Reply::Ok { seq, detoured, micros } => format!("ok {seq} {detoured} {micros}\n"),
            // One line: the message must not carry the delimiter.
            Reply::Err { seq, message } => format!("err {seq} {}\n", message.replace(['\n', '\r'], " ")),
            Reply::Pong => "pong\n".into(),
        }
    }

    pub fn parse_reply(line: &str) -> Option<Reply> {
        let line = line.trim_end_matches(['\n', '\r']);
        let mut w = line.splitn(3, ' ');
        match w.next()? {
            "ok" => {
                let seq = w.next()?.parse().ok()?;
                let mut rest = w.next()?.split(' ');
                Some(Reply::Ok { seq, detoured: rest.next()?.parse().ok()?, micros: rest.next()?.parse().ok()? })
            }
            "err" => Some(Reply::Err { seq: w.next()?.parse().ok()?, message: w.next().unwrap_or("").to_string() }),
            "pong" => Some(Reply::Pong),
            _ => None,
        }
    }

    /// One function to redirect, at link-time addresses: where it is in
    /// the binary, how many bytes it has there, and where its copy is in
    /// the patch.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Pair {
        pub base: u64,
        pub size: u64,
        pub patch: u64,
    }

    /// The pairs file: `anchor <hex>` (the patch's link-time `main`), then
    /// one `<base hex> <size hex> <patch hex>` per line.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct Pairs {
        pub anchor: u64,
        pub pairs: Vec<Pair>,
    }

    pub fn format_pairs(p: &Pairs) -> String {
        let mut s = format!("anchor {:x}\n", p.anchor);
        for pair in &p.pairs {
            s.push_str(&format!("{:x} {:x} {:x}\n", pair.base, pair.size, pair.patch));
        }
        s
    }

    pub fn parse_pairs(text: &str) -> Result<Pairs, String> {
        let mut lines = text.lines();
        let anchor = lines
            .next()
            .and_then(|l| l.strip_prefix("anchor "))
            .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())
            .ok_or("pairs: no anchor line")?;
        let mut pairs = Vec::new();
        for (i, l) in lines.enumerate() {
            if l.is_empty() {
                continue;
            }
            let malformed = || format!("pairs: line {} malformed", i + 2);
            let mut w = l.split(' ').map(|h| u64::from_str_radix(h, 16));
            let (Some(Ok(base)), Some(Ok(size)), Some(Ok(patch)), None) = (w.next(), w.next(), w.next(), w.next())
            else {
                return Err(malformed());
            };
            pairs.push(Pair { base, size, patch });
        }
        Ok(Pairs { anchor, pairs })
    }
}

// ── planning (pure) ─────────────────────────────────────────────────────

pub mod plan {
    use super::arm64;

    /// One function to redirect, at run-time addresses.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Detour {
        pub from: u64,
        pub size: u64,
        pub to: u64,
    }

    /// The words to write for every detour, and the windows no thread
    /// may be inside while they are written: `(start, end)`, exclusive at
    /// both ends — a thread about to run the first instruction runs the
    /// whole new sequence, and one returning to the first byte past it
    /// runs old code that did not change.
    #[derive(Debug, Default, PartialEq, Eq)]
    pub struct Writes {
        pub words: Vec<(u64, u32)>,
        pub windows: Vec<(u64, u64)>,
    }

    /// Route every detour: one `b` when the patch is in reach, else the
    /// long jump, which needs 16 bytes of function. `Err` names the first
    /// function neither fits.
    pub fn route(detours: &[Detour], long_only: bool) -> Result<Writes, u64> {
        let mut out = Writes::default();
        for d in detours {
            if let Some(word) = arm64::b(d.from, d.to).filter(|_| !long_only) {
                out.words.push((d.from, word));
                continue;
            }
            if d.size < arm64::LONG_JUMP || d.from % 4 != 0 {
                return Err(d.from);
            }
            for (i, word) in arm64::long_jump(d.to).into_iter().enumerate() {
                out.words.push((d.from + 4 * i as u64, word));
            }
            out.windows.push((d.from, d.from + arm64::LONG_JUMP));
        }
        out.windows.sort_unstable();
        Ok(out)
    }

    /// Whether `addr` falls strictly inside one of the sorted `windows`.
    /// Allocation-free: it runs while other threads are suspended.
    pub fn in_window(addr: u64, windows: &[(u64, u64)]) -> bool {
        let i = windows.partition_point(|w| w.0 < addr);
        i > 0 && addr < windows[i - 1].1
    }

    /// The new words grouped by the page they are on: what each page swap
    /// writes.
    pub fn pages(words: &[(u64, u32)], page: u64) -> Vec<(u64, Vec<(u64, u32)>)> {
        let mut sorted = words.to_vec();
        sorted.sort_by_key(|w| w.0);
        let mut out: Vec<(u64, Vec<(u64, u32)>)> = Vec::new();
        for (addr, word) in sorted {
            let start = addr / page * page;
            match out.last_mut() {
                Some((s, ws)) if *s == start => ws.push((addr, word)),
                _ => out.push((start, vec![(addr, word)])),
            }
        }
        out
    }
}

// ── the running process (macOS) ─────────────────────────────────────────

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod sys {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};

    extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
        fn getpagesize() -> c_int;
        fn mach_task_self() -> u32;
        fn mach_thread_self() -> u32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
        fn mach_vm_allocate(task: u32, addr: *mut u64, size: u64, flags: i32) -> i32;
        fn mach_vm_deallocate(task: u32, addr: u64, size: u64) -> i32;
        fn mach_vm_protect(task: u32, addr: u64, size: u64, set_max: i32, prot: i32) -> i32;
        fn mach_vm_read_overwrite(task: u32, addr: u64, size: u64, data: u64, out: *mut u64) -> i32;
        #[allow(clippy::too_many_arguments)]
        fn mach_vm_remap(
            target: u32,
            addr: *mut u64,
            size: u64,
            mask: u64,
            flags: i32,
            src_task: u32,
            src_addr: u64,
            copy: i32,
            cur: *mut i32,
            max: *mut i32,
            inherit: u32,
        ) -> i32;
        fn task_threads(task: u32, list: *mut *mut u32, count: *mut u32) -> i32;
        fn thread_suspend(thread: u32) -> i32;
        fn thread_resume(thread: u32) -> i32;
        fn thread_get_state(thread: u32, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
        fn sys_icache_invalidate(start: *mut c_void, len: usize);
    }

    const RTLD_NOW: c_int = 0x2;
    const RTLD_LOCAL: c_int = 0x4;
    const VM_FLAGS_FIXED: i32 = 0;
    const VM_FLAGS_ANYWHERE: i32 = 1;
    const VM_FLAGS_OVERWRITE: i32 = 0x4000;
    const PROT_R: i32 = 1;
    const PROT_X: i32 = 4;
    const VM_INHERIT_COPY: u32 = 1;
    /// `ARM_THREAD_STATE64` and its size in 32-bit words: x0–x28, fp, lr,
    /// sp, pc, cpsr, pad.
    const ARM_THREAD_STATE64: i32 = 6;
    const ARM_THREAD_STATE64_COUNT: u32 = 68;
    /// Frames walked per thread before giving up on finding the bottom.
    const MAX_FRAMES: usize = 4096;

    pub fn page_size() -> u64 {
        unsafe { getpagesize() as u64 }
    }

    /// `dlopen` the patch with every symbol bound now: a symbol the
    /// running binary cannot supply fails HERE, with dyld's message,
    /// instead of at the first call into it.
    pub fn load(path: &std::path::Path) -> Result<*mut c_void, String> {
        let c = CString::new(path.as_os_str().to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let h = unsafe { dlopen(c.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if h.is_null() {
            let msg = unsafe {
                let e = dlerror();
                if e.is_null() { "dlopen failed".to_string() } else { CStr::from_ptr(e).to_string_lossy().into_owned() }
            };
            return Err(msg);
        }
        Ok(h)
    }

    pub fn symbol(handle: *mut c_void, name: &str) -> Option<u64> {
        let c = CString::new(name).ok()?;
        let p = unsafe { dlsym(handle, c.as_ptr()) };
        (!p.is_null()).then_some(p as u64)
    }

    /// The main executable's ASLR slide and its `__TEXT` range.
    pub struct Image {
        pub slide: u64,
        pub text_start: u64,
        pub text_end: u64,
    }

    pub fn main_image() -> Result<Image, String> {
        #[repr(C)]
        struct MachHeader64 {
            magic: u32,
            cputype: i32,
            cpusubtype: i32,
            filetype: u32,
            ncmds: u32,
            sizeofcmds: u32,
            flags: u32,
            reserved: u32,
        }
        #[repr(C)]
        struct SegmentCommand64 {
            cmd: u32,
            cmdsize: u32,
            segname: [u8; 16],
            vmaddr: u64,
            vmsize: u64,
            fileoff: u64,
            filesize: u64,
            maxprot: i32,
            initprot: i32,
            nsects: u32,
            flags: u32,
        }
        extern "C" {
            fn _dyld_image_count() -> u32;
            fn _dyld_get_image_header(i: u32) -> *const MachHeader64;
            fn _dyld_get_image_vmaddr_slide(i: u32) -> isize;
        }
        const MH_EXECUTE: u32 = 0x2;
        const LC_SEGMENT_64: u32 = 0x19;
        unsafe {
            // Searched, not assumed to be image 0: with a library
            // inserted, dyld's image list does not start with the
            // executable (measured on macOS 26).
            let index = (0.._dyld_image_count())
                .find(|&i| {
                    let h = _dyld_get_image_header(i);
                    !h.is_null() && (*h).filetype == MH_EXECUTE
                })
                .ok_or("dyld lists no main executable")?;
            let h = _dyld_get_image_header(index);
            let slide = _dyld_get_image_vmaddr_slide(index) as u64;
            let mut p = (h as *const u8).add(std::mem::size_of::<MachHeader64>());
            for _ in 0..(*h).ncmds {
                let lc = p as *const SegmentCommand64;
                if (*lc).cmd == LC_SEGMENT_64 {
                    let name = CStr::from_bytes_until_nul(&(*lc).segname).map(|c| c.to_bytes()).unwrap_or(b"");
                    if name == b"__TEXT" {
                        let text_start = (*lc).vmaddr.wrapping_add(slide);
                        return Ok(Image { slide, text_start, text_end: text_start + (*lc).vmsize });
                    }
                }
                p = p.add((*lc).cmdsize as usize);
            }
            Err("the executable has no __TEXT segment".into())
        }
    }

    /// A replacement for one page of text: a read-execute copy carrying
    /// the new words, ready to be mapped over the original.
    pub struct Prepared {
        pub start: u64,
        scratch: u64,
        len: u64,
    }

    impl Drop for Prepared {
        fn drop(&mut self) {
            unsafe { mach_vm_deallocate(mach_task_self(), self.scratch, self.len) };
        }
    }

    pub fn prepare(start: u64, page: u64, words: &[(u64, u32)]) -> Result<Prepared, String> {
        unsafe {
            let task = mach_task_self();
            let mut scratch = 0u64;
            let kr = mach_vm_allocate(task, &mut scratch, page, VM_FLAGS_ANYWHERE);
            if kr != 0 {
                return Err(format!("mach_vm_allocate failed ({kr})"));
            }
            let prepared = Prepared { start, scratch, len: page };
            std::ptr::copy_nonoverlapping(start as *const u8, scratch as *mut u8, page as usize);
            for (addr, word) in words {
                ((scratch + (addr - start)) as *mut u32).write(*word);
            }
            let kr = mach_vm_protect(task, scratch, page, 0, PROT_R | PROT_X);
            if kr != 0 {
                return Err(format!("mach_vm_protect(RX) of a new page failed ({kr})"));
            }
            Ok(prepared)
        }
    }

    /// Map a prepared page over its original. No allocation: it runs
    /// with the other threads suspended.
    unsafe fn swap(p: &Prepared) -> i32 {
        let task = mach_task_self();
        let (mut dst, mut cur, mut max) = (p.start, 0, 0);
        let kr = mach_vm_remap(
            task, &mut dst, p.len, 0, VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE, task, p.scratch, 0, &mut cur, &mut max,
            VM_INHERIT_COPY,
        );
        if kr != 0 {
            return kr;
        }
        if dst != p.start {
            return -1;
        }
        sys_icache_invalidate(p.start as *mut c_void, p.len as usize);
        0
    }

    /// Whether a suspended thread may be inside one of `windows`: its pc,
    /// its lr, or a return address on its frame chain. A word that cannot
    /// be read ends the walk. No allocation.
    unsafe fn busy(thread: u32, windows: &[(u64, u64)]) -> bool {
        let mut state = [0u32; ARM_THREAD_STATE64_COUNT as usize];
        let mut count = ARM_THREAD_STATE64_COUNT;
        if thread_get_state(thread, ARM_THREAD_STATE64, state.as_mut_ptr(), &mut count) != 0 {
            // Cannot see it: assume the worst.
            return true;
        }
        let reg = |i: usize| (state[2 * i] as u64) | ((state[2 * i + 1] as u64) << 32);
        let (fp, lr, pc) = (reg(29), reg(30), reg(32));
        if super::plan::in_window(pc, windows) || super::plan::in_window(lr, windows) {
            return true;
        }
        let task = mach_task_self();
        let mut fp = fp;
        for _ in 0..MAX_FRAMES {
            if fp == 0 || fp % 8 != 0 {
                break;
            }
            let mut record = [0u64; 2];
            let mut got = 0u64;
            if mach_vm_read_overwrite(task, fp, 16, record.as_mut_ptr() as u64, &mut got) != 0 || got != 16 {
                break;
            }
            if super::plan::in_window(record[1], windows) {
                return true;
            }
            // Frames grow down, so the caller's record is higher; anything
            // else is not a frame chain.
            if record[0] <= fp {
                break;
            }
            fp = record[0];
        }
        false
    }

    /// Suspend every other thread, and swap `pages` in if none of them is
    /// inside a window; resume them either way. `Ok(false)`: a thread was
    /// busy, try again. No allocation between suspend and resume.
    pub fn swap_stopped(pages: &[Prepared], windows: &[(u64, u64)]) -> Result<bool, String> {
        unsafe {
            let task = mach_task_self();
            let me = mach_thread_self();
            let mut list: *mut u32 = std::ptr::null_mut();
            let mut count = 0u32;
            let kr = task_threads(task, &mut list, &mut count);
            if kr != 0 {
                mach_port_deallocate(task, me);
                return Err(format!("task_threads failed ({kr})"));
            }
            let threads = std::slice::from_raw_parts(list, count as usize);
            let mut suspended = 0usize;
            let mut failed = 0i32;
            for &t in threads {
                if t == me {
                    continue;
                }
                if thread_suspend(t) == 0 {
                    suspended += 1;
                } else {
                    // A thread that exited after `task_threads` listed it.
                    continue;
                }
            }
            let _ = suspended;
            let clear = windows.is_empty() || threads.iter().all(|&t| t == me || !busy(t, windows));
            if clear {
                for p in pages {
                    let kr = swap(p);
                    if kr != 0 {
                        failed = kr;
                        break;
                    }
                }
            }
            for &t in threads {
                if t != me {
                    thread_resume(t);
                }
                mach_port_deallocate(task, t);
            }
            mach_vm_deallocate(task, list as u64, count as u64 * 4);
            mach_port_deallocate(task, me);
            if failed != 0 {
                // A failure mid-way left earlier pages swapped: the patch
                // is partly applied, and the session rebuilds and restarts
                // this process.
                return Err(format!("mach_vm_remap of a text page failed ({failed})"));
            }
            Ok(clear)
        }
    }
}

/// Apply one patch: load it, then detour every pair. Returns how many
/// functions were redirected.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn apply(dylib: &std::path::Path, pairs_file: &std::path::Path) -> Result<usize, String> {
    /// How often a busy thread may push the swap back, a millisecond
    /// apart, before the patch is refused.
    const ATTEMPTS: usize = 2000;
    let text = std::fs::read_to_string(pairs_file).map_err(|e| format!("read {}: {e}", pairs_file.display()))?;
    let pairs = proto::parse_pairs(&text)?;
    let image = sys::main_image()?;
    let handle = sys::load(dylib)?;
    let anchor = sys::symbol(handle, "main").ok_or("the patch has no `main` anchor")?;
    let patch_slide = anchor.wrapping_sub(pairs.anchor);
    let mut detours = Vec::with_capacity(pairs.pairs.len());
    for p in &pairs.pairs {
        let from = p.base.wrapping_add(image.slide);
        if from < image.text_start || from + p.size > image.text_end {
            return Err(format!("0x{from:x} is not in the executable's text: the session's view of the binary is stale"));
        }
        detours.push(plan::Detour { from, size: p.size, to: p.patch.wrapping_add(patch_slide) });
    }
    let long_only = std::env::var_os(LONG_JUMPS_ENV).is_some_and(|v| v == "1");
    let writes = plan::route(&detours, long_only)
        .map_err(|at| format!("the function at 0x{at:x} is too short for a jump and the patch is out of its reach"))?;
    let page = sys::page_size();
    let prepared = plan::pages(&writes.words, page)
        .into_iter()
        .map(|(start, words)| sys::prepare(start, page, &words))
        .collect::<Result<Vec<_>, _>>()?;
    for _ in 0..ATTEMPTS {
        if sys::swap_stopped(&prepared, &writes.windows)? {
            return Ok(detours.len());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    Err(format!("a thread stayed inside a function being patched for {ATTEMPTS} attempts"))
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub fn apply(_dylib: &std::path::Path, _pairs_file: &std::path::Path) -> Result<usize, String> {
    Err("server hot patching supports macOS on Apple silicon only".into())
}

/// The agent's session: hello, then one command per line until the
/// session closes the socket.
#[cfg(unix)]
pub fn serve(socket: &str) -> std::io::Result<()> {
    let stream = std::os::unix::net::UnixStream::connect(socket)?;
    let mut out = stream.try_clone()?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let slide = sys::main_image().map(|i| i.slide).unwrap_or(0);
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let slide = 0u64;
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    out.write_all(proto::format_hello(PROTOCOL, std::process::id(), slide, &exe).as_bytes())?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        let reply = match proto::parse_command(&line) {
            Ok(proto::Command::Ping) => proto::Reply::Pong,
            Ok(proto::Command::Apply { seq, dylib, pairs }) => {
                let started = std::time::Instant::now();
                match std::panic::catch_unwind(|| apply(&dylib, &pairs)) {
                    Ok(Ok(detoured)) => {
                        proto::Reply::Ok { seq, detoured, micros: started.elapsed().as_micros() as u64 }
                    }
                    Ok(Err(message)) => proto::Reply::Err { seq, message },
                    Err(_) => proto::Reply::Err { seq, message: "the agent panicked applying the patch".into() },
                }
            }
            Err(message) => proto::Reply::Err { seq: 0, message },
        };
        out.write_all(proto::format_reply(&reply).as_bytes())?;
    }
    Ok(())
}

/// Remove this agent from the environment, so a process the server
/// starts (a worker, a shell-out) does not load it too.
#[cfg(all(unix, idealyst_hot_agent))]
fn scrub_environment() {
    for var in ["DYLD_INSERT_LIBRARIES", "LD_PRELOAD"] {
        if let Ok(v) = std::env::var(var) {
            let kept: Vec<&str> = v.split(':').filter(|p| !p.contains("idealyst_hot_agent")).collect();
            // SAFETY: runs from the library's initializer, before `main`
            // and before this agent starts its thread.
            unsafe {
                if kept.is_empty() {
                    std::env::remove_var(var);
                } else {
                    std::env::set_var(var, kept.join(":"));
                }
            }
        }
    }
    unsafe { std::env::remove_var(SOCKET_ENV) };
}

#[cfg(all(unix, idealyst_hot_agent))]
extern "C" fn agent_init() {
    let Ok(socket) = std::env::var(SOCKET_ENV) else { return };
    scrub_environment();
    let _ = std::thread::Builder::new().name("idealyst-hot-agent".into()).spawn(move || {
        if let Err(e) = serve(&socket) {
            eprintln!("[idealyst hot agent] session channel closed: {e}");
        }
    });
}

#[cfg(all(unix, idealyst_hot_agent))]
#[used]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func,mod_init_funcs")]
#[cfg_attr(target_os = "linux", link_section = ".init_array")]
static AGENT_INIT: extern "C" fn() = agent_init;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_round_trips_both_ways() {
        let from = 0x1_0000_4000;
        for to in [from + 4, from - 4, from + 0x07FF_FFFC, from - 0x0800_0000] {
            let w = arm64::b(from, to).expect("in reach");
            assert_eq!(arm64::b_target(from, w), Some(to), "to {to:x}");
        }
    }

    #[test]
    fn a_branch_out_of_reach_or_misaligned_is_refused() {
        let from = 0x1_0000_0000;
        assert_eq!(arm64::b(from, from + 0x0800_0000), None);
        assert_eq!(arm64::b(from, from - 0x0800_0004), None);
        assert_eq!(arm64::b(from + 2, from + 8), None);
    }

    #[test]
    fn the_long_jump_loads_its_target_from_the_literal_after_the_branch() {
        let w = arm64::long_jump(0x1122_3344_5566_7788);
        assert_eq!(w[0], 0x5800_0050);
        assert_eq!(w[1], 0xD61F_0200);
        assert_eq!((w[3] as u64) << 32 | w[2] as u64, 0x1122_3344_5566_7788);
    }

    #[test]
    fn commands_and_replies_round_trip() {
        let line = proto::format_apply(7, "/tmp/a b/libpatch-1.dylib".as_ref(), "/tmp/a b/patch-1.pairs".as_ref());
        assert_eq!(
            proto::parse_command(&line),
            Ok(proto::Command::Apply {
                seq: 7,
                dylib: "/tmp/a b/libpatch-1.dylib".into(),
                pairs: "/tmp/a b/patch-1.pairs".into()
            })
        );
        assert_eq!(proto::parse_command("ping\n"), Ok(proto::Command::Ping));
        for r in [
            proto::Reply::Ok { seq: 3, detoured: 120, micros: 900 },
            proto::Reply::Err { seq: 4, message: "dlopen failed".into() },
            proto::Reply::Pong,
        ] {
            assert_eq!(proto::parse_reply(&proto::format_reply(&r)), Some(r));
        }
        let hello = proto::format_hello(PROTOCOL, 42, 0x4000, "/x/server bin");
        assert_eq!(
            proto::parse_hello(&hello),
            Some(proto::Hello { protocol: PROTOCOL, pid: 42, slide: 0x4000, exe: "/x/server bin".into() })
        );
    }

    #[test]
    fn an_error_message_stays_on_one_line() {
        let r = proto::Reply::Err { seq: 1, message: "a\nb".into() };
        assert_eq!(proto::format_reply(&r), "err 1 a b\n");
    }

    #[test]
    fn pairs_round_trip() {
        let p = proto::Pairs {
            anchor: 0x3f00,
            pairs: vec![
                proto::Pair { base: 0x1000, size: 0x40, patch: 0x2000 },
                proto::Pair { base: 0x1040, size: 0x8, patch: 0x2400 },
            ],
        };
        assert_eq!(proto::parse_pairs(&proto::format_pairs(&p)), Ok(p));
        assert!(proto::parse_pairs("10 20 30\n").is_err(), "a file without an anchor is refused");
        assert!(proto::parse_pairs("anchor 1\n10 20\n").is_err(), "a line without a size is refused");
    }

    /// In reach: one `b` and no window, since a single aligned word
    /// cannot be torn. Out of reach: four words and a window.
    #[test]
    fn a_detour_is_a_branch_in_reach_and_a_long_jump_otherwise() {
        let far = 0x7000_0000_0000u64;
        let w = plan::route(
            &[
                plan::Detour { from: 0x1_0000_1000, size: 0x40, to: 0x1_0000_9000 },
                plan::Detour { from: 0x1_0000_2000, size: 0x10, to: far },
            ],
            false,
        )
        .unwrap();
        assert_eq!(w.words.len(), 1 + 4);
        assert_eq!(w.words[0].0, 0x1_0000_1000);
        assert_eq!(arm64::b_target(0x1_0000_1000, w.words[0].1), Some(0x1_0000_9000));
        assert_eq!(w.windows, vec![(0x1_0000_2000, 0x1_0000_2010)]);
    }

    /// A function too short to hold the long jump would have its
    /// neighbour overwritten.
    #[test]
    fn a_short_function_out_of_reach_is_refused() {
        let far = 0x7000_0000_0000u64;
        assert_eq!(plan::route(&[plan::Detour { from: 0x1_0000_2000, size: 12, to: far }], false), Err(0x1_0000_2000));
        // Forced long, a short function in reach is refused too.
        let near = plan::Detour { from: 0x1_0000_2000, size: 12, to: 0x1_0000_3000 };
        assert!(plan::route(&[near], false).is_ok());
        assert_eq!(plan::route(&[near], true), Err(0x1_0000_2000));
    }

    /// A thread about to run the first instruction runs the whole new
    /// sequence, and one returning just past the window runs unchanged
    /// code: only the inside is busy.
    #[test]
    fn only_the_inside_of_a_window_is_busy() {
        let windows = [(0x1000, 0x1010), (0x2000, 0x2010)];
        for (addr, busy) in [(0x1000, false), (0x1004, true), (0x100c, true), (0x1010, false), (0x1fff, false), (0x2008, true), (0x3000, false), (0, false)] {
            assert_eq!(plan::in_window(addr, &windows), busy, "{addr:x}");
        }
        assert!(!plan::in_window(0x1004, &[]));
    }

    #[test]
    fn writes_group_by_page() {
        let pages = plan::pages(&[(0x8010, 1), (0x4004, 2), (0x4ffc, 3)], 0x4000);
        assert_eq!(pages, vec![(0x4000, vec![(0x4004, 2), (0x4ffc, 3)]), (0x8000, vec![(0x8010, 1)])]);
    }
}
