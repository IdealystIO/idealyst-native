//! The server hot-patch agent: a library loaded into the dev session's
//! server process that applies function patches to it while it runs.
//!
//! `idealyst dev` compiles this file on its own with `rustc --crate-type
//! cdylib --cfg idealyst_hot_agent` (no cargo, no dependencies) and has
//! the server load it at exec: `idealyst run-linked`, the runner the
//! session gives `cargo run`, sets `DYLD_INSERT_LIBRARIES` (macOS) or
//! `LD_PRELOAD` (Linux). The server's
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
//! usually is not (the loader maps the patch wherever it likes), so the jump is
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
//! On Linux, which has no call to suspend another thread, "suspended"
//! means stopped by a signal whose handler waits until the agent is done
//! (the Linux `sys` module explains how).
//!
//! # Writing the text: in place on Linux, through a new page on macOS
//!
//! Linux lets the process make its own text writable and executable for
//! the moment of the write (`mprotect`), so with the other threads
//! stopped the agent writes the words in place and cleans the
//! instruction cache.
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

// ── the running process (Linux) ─────────────────────────────────────────

/// Linux has no call that suspends another thread of the process, so the
/// agent stops them with a signal: each thread's handler records where
/// it is (pc, lr, frame pointer) and waits on a futex until the agent
/// releases it. Every thread then is either in the handler or about to
/// enter it, so none runs the text being written. Text pages are made
/// writable (`mprotect` RWX — Linux allows it, unlike macOS), the words
/// written, the instruction cache cleaned, and the pages made read-execute
/// again before the threads go. Leaving the handler is an exception
/// return, which synchronizes each thread's instruction fetch with the
/// new words.
///
/// The structures are declared here, not taken from the `libc` crate: the
/// session compiles this file on its own, with no dependencies. The tests
/// check them against `libc`'s.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
mod sys {
    use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

    #[repr(C)]
    pub struct SigAction {
        pub sa_sigaction: usize,
        pub sa_mask: [u64; 16],
        pub sa_flags: c_int,
        pub sa_restorer: usize,
    }

    #[repr(C)]
    pub struct DlPhdrInfo {
        pub dlpi_addr: u64,
        pub dlpi_name: *const c_char,
        pub dlpi_phdr: *const Phdr,
        pub dlpi_phnum: u16,
    }

    #[repr(C)]
    pub struct Phdr {
        pub p_type: u32,
        pub p_flags: u32,
        pub p_offset: u64,
        pub p_vaddr: u64,
        pub p_paddr: u64,
        pub p_filesz: u64,
        pub p_memsz: u64,
        pub p_align: u64,
    }

    extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
        fn getpagesize() -> c_int;
        fn getpid() -> c_int;
        fn syscall(number: c_long, ...) -> c_long;
        fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
        fn sigaction(sig: c_int, act: *const SigAction, old: *mut SigAction) -> c_int;
        fn __libc_current_sigrtmin() -> c_int;
        fn __clear_cache(start: *mut c_char, end: *mut c_char);
        fn dl_iterate_phdr(
            cb: unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int,
            data: *mut c_void,
        ) -> c_int;
    }

    const RTLD_NOW: c_int = 0x2;
    const RTLD_LOCAL: c_int = 0;
    const PROT_READ: c_int = 1;
    const PROT_WRITE: c_int = 2;
    const PROT_EXEC: c_int = 4;
    const SA_SIGINFO: c_int = 4;
    const SA_RESTART: c_int = 0x1000_0000;
    const SYS_GETTID: c_long = 178;
    const SYS_OPENAT: c_long = 56;
    const SYS_CLOSE: c_long = 57;
    const SYS_READ: c_long = 63;
    const SYS_PROCESS_VM_READV: c_long = 270;
    const AT_FDCWD: c_int = -100;
    const SYS_TGKILL: c_long = 131;
    const SYS_FUTEX: c_long = 98;
    const FUTEX_WAIT_PRIVATE: c_int = 128;
    const FUTEX_WAKE_PRIVATE: c_int = 129;
    const PT_LOAD: u32 = 1;
    const PF_X: u32 = 1;
    /// Offsets into the arm64 `ucontext_t` the handler is given: its
    /// `mcontext_t` (16-aligned after flags, link, stack and the 128-byte
    /// signal mask) holds `fault_address`, then `regs[31]`, `sp`, `pc`.
    pub const UC_REGS: usize = 184;
    pub const UC_PC: usize = UC_REGS + 32 * 8;
    /// Frames walked per thread before giving up on finding the bottom.
    const MAX_FRAMES: usize = 4096;
    /// Threads the agent can stop at once; a process with more refuses the
    /// patch (the session rebuilds).
    const MAX_THREADS: usize = 4096;
    /// How long every listed thread has to reach the handler.
    const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

    pub fn page_size() -> u64 {
        unsafe { getpagesize() as u64 }
    }

    /// `dlopen` the patch with every symbol bound now: a symbol nothing
    /// supplies fails HERE, with the loader's message.
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

    /// The executable's load address (its "slide": a PIE is linked at 0)
    /// and the range of its executable segment.
    pub struct Image {
        pub slide: u64,
        pub text_start: u64,
        pub text_end: u64,
    }

    pub fn main_image() -> Result<Image, String> {
        unsafe extern "C" fn first(info: *mut DlPhdrInfo, _: usize, data: *mut c_void) -> c_int {
            let info = &*info;
            // The executable is the object with no name; the loader lists
            // it first, ahead of preloaded libraries.
            if !info.dlpi_name.is_null() && *info.dlpi_name != 0 {
                return 0;
            }
            let out = &mut *(data as *mut Option<Image>);
            for i in 0..info.dlpi_phnum as usize {
                let p = &*info.dlpi_phdr.add(i);
                if p.p_type == PT_LOAD && p.p_flags & PF_X != 0 {
                    let start = info.dlpi_addr + p.p_vaddr;
                    *out = Some(Image { slide: info.dlpi_addr, text_start: start, text_end: start + p.p_memsz });
                    return 1;
                }
            }
            0
        }
        let mut out: Option<Image> = None;
        unsafe { dl_iterate_phdr(first, &mut out as *mut _ as *mut c_void) };
        out.ok_or_else(|| "the loader lists no executable segment for the program".into())
    }

    /// The new words of one text page.
    pub struct Prepared {
        pub start: u64,
        len: u64,
        words: Vec<(u64, u32)>,
    }

    pub fn prepare(start: u64, page: u64, words: &[(u64, u32)]) -> Result<Prepared, String> {
        Ok(Prepared { start, len: page, words: words.to_vec() })
    }

    // The stop protocol's shared state, one stop at a time ([`STOP_LOCK`]).
    /// Held for a whole stop. Two concurrent stops would each signal the
    /// other's caller into the handler, and both would wait for a release
    /// neither can give. The agent's one thread is the only caller in a
    /// server; tests stop from several threads at once. A caller blocked
    /// here is stopped like any other thread meanwhile.
    static STOP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static GENERATION: AtomicU32 = AtomicU32::new(0);
    /// Set while a stop is in progress. A signal that reaches its thread
    /// after the stop gave up on it (the thread had signals blocked past
    /// the deadline) finds it clear and returns at once, instead of
    /// waiting for a release that already happened.
    static STOPPING: AtomicBool = AtomicBool::new(false);
    static ARRIVED: AtomicUsize = AtomicUsize::new(0);
    static LEFT: AtomicUsize = AtomicUsize::new(0);
    static SLOT: AtomicUsize = AtomicUsize::new(0);
    /// Per stopped thread: pc, lr, fp, thread id.
    static STATE: [[AtomicU64; 4]; MAX_THREADS] = [const { [const { AtomicU64::new(0) }; 4] }; MAX_THREADS];
    static INSTALLED: std::sync::Once = std::sync::Once::new();

    /// Runs on each stopped thread. Async-signal-safe: atomics and futex
    /// calls only.
    unsafe extern "C" fn on_stop(_: c_int, _: *mut c_void, ctx: *mut c_void) {
        // The generation first: a release clears STOPPING before it moves
        // the generation, so a handler that sees STOPPING set waits on a
        // generation the release has not moved yet.
        let generation = GENERATION.load(Ordering::Acquire);
        if !STOPPING.load(Ordering::Acquire) {
            return;
        }
        let i = SLOT.fetch_add(1, Ordering::AcqRel);
        if i < MAX_THREADS {
            let uc = ctx as *const u8;
            let reg = |at: usize| (uc.add(at) as *const u64).read();
            STATE[i][0].store(reg(UC_PC), Ordering::Relaxed);
            STATE[i][1].store(reg(UC_REGS + 30 * 8), Ordering::Relaxed);
            STATE[i][2].store(reg(UC_REGS + 29 * 8), Ordering::Relaxed);
            STATE[i][3].store(syscall(SYS_GETTID) as u64, Ordering::Relaxed);
        }
        ARRIVED.fetch_add(1, Ordering::Release);
        while GENERATION.load(Ordering::Acquire) == generation {
            syscall(SYS_FUTEX, GENERATION.as_ptr(), FUTEX_WAIT_PRIVATE, generation, std::ptr::null::<c_void>());
        }
        LEFT.fetch_add(1, Ordering::Release);
    }

    fn stop_signal() -> c_int {
        // Real-time signals above the C library's own; this one is not
        // used by glibc, Rust's runtime or tokio.
        unsafe { __libc_current_sigrtmin() + 7 }
    }

    fn install() -> Result<(), String> {
        let mut err = Ok(());
        INSTALLED.call_once(|| unsafe {
            let act = SigAction {
                sa_sigaction: on_stop as *const () as usize,
                // Every signal blocked while the handler runs.
                sa_mask: [u64::MAX; 16],
                sa_flags: SA_SIGINFO | SA_RESTART,
                sa_restorer: 0,
            };
            if sigaction(stop_signal(), &act, std::ptr::null_mut()) != 0 {
                err = Err(format!("sigaction failed: {}", std::io::Error::last_os_error()));
            }
        });
        err
    }

    fn me() -> i32 {
        unsafe { syscall(SYS_GETTID) as i32 }
    }

    /// The process's other threads, by id.
    fn others() -> Result<Vec<i32>, String> {
        let me = me();
        let dir = std::fs::read_dir("/proc/self/task").map_err(|e| format!("list /proc/self/task: {e}"))?;
        Ok(dir
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .filter(|&t| t != me)
            .collect())
    }

    /// How many of the `signalled` threads have neither reached the
    /// handler (by the first `arrived` slots) nor exited. No allocation.
    unsafe fn pending(signalled: &[i32], arrived: usize, pid: c_int) -> usize {
        let arrived = arrived.min(MAX_THREADS);
        signalled
            .iter()
            .filter(|&&t| !(0..arrived).any(|i| STATE[i][3].load(Ordering::Relaxed) == t as u64))
            // Signal 0 checks the thread exists; ESRCH once it has exited.
            .filter(|&&t| syscall(SYS_TGKILL, pid, t, 0) == 0)
            .count()
    }

    /// Release every stopped thread and wait until all have left the
    /// handler: the next stop reuses the slots.
    fn release(stopped: usize) {
        LEFT.store(0, Ordering::Relaxed);
        STOPPING.store(false, Ordering::Release);
        GENERATION.fetch_add(1, Ordering::Release);
        unsafe { syscall(SYS_FUTEX, GENERATION.as_ptr(), FUTEX_WAKE_PRIVATE, i32::MAX) };
        let deadline = std::time::Instant::now() + STOP_DEADLINE;
        while LEFT.load(Ordering::Acquire) < stopped && std::time::Instant::now() < deadline {
            std::hint::spin_loop();
        }
    }

    /// Whether a stopped thread may be inside one of `windows`: its pc,
    /// its lr, or a return address on its frame chain. No allocation.
    unsafe fn busy(slot: usize, windows: &[(u64, u64)]) -> bool {
        let (pc, lr, mut fp) = (
            STATE[slot][0].load(Ordering::Relaxed),
            STATE[slot][1].load(Ordering::Relaxed),
            STATE[slot][2].load(Ordering::Relaxed),
        );
        if super::plan::in_window(pc, windows) || super::plan::in_window(lr, windows) {
            return true;
        }
        for _ in 0..MAX_FRAMES {
            if fp == 0 || fp % 8 != 0 {
                break;
            }
            // Read through the kernel, which fails instead of faulting on
            // a frame pointer that does not point into a stack.
            let Some(record) = read_frame(fp) else { break };
            if super::plan::in_window(record[1], windows) {
                return true;
            }
            // Frames grow down: the caller's record is higher.
            if record[0] <= fp {
                break;
            }
            fp = record[0];
        }
        false
    }

    #[repr(C)]
    struct IoVec {
        base: *mut c_void,
        len: usize,
    }

    /// A frame record (saved fp, saved lr) at `fp`, or `None` when that
    /// memory is not readable. No allocation.
    unsafe fn read_frame(fp: u64) -> Option<[u64; 2]> {
        let mut record = [0u64; 2];
        let local = IoVec { base: record.as_mut_ptr() as *mut c_void, len: 16 };
        let remote = IoVec { base: fp as *mut c_void, len: 16 };
        let n = syscall(SYS_PROCESS_VM_READV, getpid(), &local as *const IoVec, 1usize, &remote as *const IoVec, 1usize, 0usize);
        (n == 16).then_some(record)
    }

    /// How many threads the process has now, from `/proc/self/status`,
    /// read with raw system calls into a stack buffer: it runs while other
    /// threads are stopped, and one of them may hold the allocator's lock.
    unsafe fn thread_count() -> Option<usize> {
        let fd = syscall(SYS_OPENAT, AT_FDCWD, c"/proc/self/status".as_ptr(), 0 as c_int);
        if fd < 0 {
            return None;
        }
        let mut buf = [0u8; 4096];
        let mut len = 0usize;
        loop {
            let n = syscall(SYS_READ, fd, buf.as_mut_ptr().add(len), buf.len() - len);
            if n <= 0 {
                break;
            }
            len += n as usize;
            if len == buf.len() {
                break;
            }
        }
        syscall(SYS_CLOSE, fd);
        let text = &buf[..len];
        let key = b"\nThreads:";
        let at = text.windows(key.len()).position(|w| w == key)? + key.len();
        let mut count = 0usize;
        let mut digits = 0;
        for &b in &text[at..] {
            match b {
                b' ' | b'\t' if digits == 0 => continue,
                b'0'..=b'9' => {
                    count = count * 10 + (b - b'0') as usize;
                    digits += 1;
                }
                _ => break,
            }
        }
        (digits > 0).then_some(count)
    }

    /// Why a write failed, formatted only once the threads are released.
    enum WriteError {
        Writable(i32),
        Executable(i32),
    }

    /// Write the words of `pages`, in place. Runs with every other thread
    /// stopped; allocates nothing.
    unsafe fn write(pages: &[Prepared]) -> Result<(), WriteError> {
        let errno = || std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        for p in pages {
            let addr = p.start as *mut c_void;
            if mprotect(addr, p.len as usize, PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
                return Err(WriteError::Writable(errno()));
            }
            for &(at, word) in &p.words {
                std::ptr::write_volatile(at as *mut u32, word);
            }
            __clear_cache(p.start as *mut c_char, (p.start + p.len) as *mut c_char);
            if mprotect(addr, p.len as usize, PROT_READ | PROT_EXEC) != 0 {
                return Err(WriteError::Executable(errno()));
            }
        }
        Ok(())
    }

    /// Stop every other thread, and write `pages` if none of them is
    /// inside a window; release them either way. `Ok(false)`: a thread was
    /// busy, try again.
    pub fn swap_stopped(pages: &[Prepared], windows: &[(u64, u64)]) -> Result<bool, String> {
        let _one_at_a_time = STOP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        install()?;
        let threads = others()?;
        if threads.len() > MAX_THREADS {
            return Err(format!("{} threads; the agent stops at most {MAX_THREADS}", threads.len()));
        }
        SLOT.store(0, Ordering::Relaxed);
        ARRIVED.store(0, Ordering::Relaxed);
        STOPPING.store(true, Ordering::Release);
        let pid = unsafe { getpid() };
        let sig = stop_signal();
        // Threads that exited since the listing are dropped from it here,
        // or by `pending` below when they exit after the signal: a thread
        // that ends before its handler runs never arrives (tokio's
        // blocking pool ends idle threads all the time).
        let mut signalled: Vec<i32> = Vec::with_capacity(threads.len());
        for &t in &threads {
            if unsafe { syscall(SYS_TGKILL, pid, t, sig) } == 0 {
                signalled.push(t);
            }
        }
        let deadline = std::time::Instant::now() + STOP_DEADLINE;
        loop {
            let arrived = ARRIVED.load(Ordering::Acquire);
            if arrived >= signalled.len() || unsafe { pending(&signalled, arrived, pid) } == 0 {
                break;
            }
            if std::time::Instant::now() > deadline {
                let waiting = unsafe { pending(&signalled, arrived, pid) };
                release(arrived);
                return Err(format!("{waiting} of {} threads did not stop for the patch", signalled.len()));
            }
            std::hint::spin_loop();
        }
        let stopped = ARRIVED.load(Ordering::Acquire);
        // Every thread the process has now must be one that stopped: a
        // thread started after the listing is running. Try again.
        let unchanged = unsafe { thread_count() } == Some(stopped + 1);
        let clear = unchanged && (windows.is_empty() || (0..stopped).all(|i| unsafe { !busy(i, windows) }));
        let written = if clear { unsafe { write(pages) } } else { Ok(()) };
        release(stopped);
        // A failure mid-way left earlier pages written: the patch is partly
        // applied, and the session rebuilds and restarts this process.
        match written {
            Ok(()) => Ok(clear),
            Err(WriteError::Writable(e)) => Err(format!("mprotect(RWX) of a text page failed (errno {e})")),
            Err(WriteError::Executable(e)) => Err(format!("mprotect(RX) of a text page failed (errno {e})")),
        }
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
#[cfg(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64"))]
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

#[cfg(not(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64")))]
pub fn apply(_dylib: &std::path::Path, _pairs_file: &std::path::Path) -> Result<usize, String> {
    Err("server hot patching supports arm64 macOS and Linux only".into())
}

/// The agent's session: hello, then one command per line until the
/// session closes the socket.
#[cfg(unix)]
pub fn serve(socket: &str) -> std::io::Result<()> {
    let stream = std::os::unix::net::UnixStream::connect(socket)?;
    let mut out = stream.try_clone()?;
    #[cfg(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64"))]
    let slide = sys::main_image().map(|i| i.slide).unwrap_or(0);
    #[cfg(not(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64")))]
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

/// The Linux `sys` checked inside a real process (run in a Linux arm64
/// container: `docker run … cargo test -p build-runtime-server`).
#[cfg(all(test, target_os = "linux", target_arch = "aarch64"))]
mod linux_tests {
    use super::sys;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    /// The agent declares the C structures itself (it compiles with no
    /// dependencies); a wrong offset reads the wrong register, and a
    /// thread inside a window would be patched under.
    #[test]
    fn the_hand_declared_structures_match_libc() {
        assert_eq!(std::mem::size_of::<sys::SigAction>(), std::mem::size_of::<libc::sigaction>());
        assert_eq!(std::mem::offset_of!(sys::SigAction, sa_flags), std::mem::offset_of!(libc::sigaction, sa_flags));
        let mcontext = std::mem::offset_of!(libc::ucontext_t, uc_mcontext);
        assert_eq!(sys::UC_REGS, mcontext + std::mem::offset_of!(libc::mcontext_t, regs));
        assert_eq!(sys::UC_PC, mcontext + std::mem::offset_of!(libc::mcontext_t, pc));
    }

    /// A thread that ends between its stop signal and its handler never
    /// arrives; the stop must count it out instead of waiting for it until
    /// the deadline and refusing the patch. Threads come and go here the
    /// whole time, as a tokio blocking pool's do.
    #[test]
    fn regression_a_thread_that_exits_during_a_stop_does_not_fail_it() {
        let stop = Arc::new(AtomicBool::new(false));
        let churn = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let short: Vec<_> = (0..4).map(|_| std::thread::spawn(|| std::hint::black_box(0))).collect();
                    for t in short {
                        t.join().unwrap();
                    }
                }
            })
        };
        let mut completed = 0;
        for round in 0..300 {
            if sys::swap_stopped(&[], &[]).unwrap_or_else(|e| panic!("round {round}: {e}")) {
                completed += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        churn.join().unwrap();
        assert!(completed > 0, "no round completed");
    }

    #[test]
    fn the_executable_image_contains_this_code() {
        let image = sys::main_image().unwrap();
        let here = the_executable_image_contains_this_code as usize as u64;
        assert!((image.text_start..image.text_end).contains(&here));
    }

    /// Stopping the world under busy threads and threads blocked in system
    /// calls, many times over: every round completes, and every thread
    /// keeps running afterwards.
    #[test]
    fn every_thread_stops_and_resumes_round_after_round() {
        let stop = Arc::new(AtomicBool::new(false));
        let work = Arc::new(AtomicU64::new(0));
        let mut threads = Vec::new();
        for _ in 0..6 {
            let (stop, work) = (stop.clone(), work.clone());
            threads.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    work.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for _ in 0..2 {
            let stop = stop.clone();
            threads.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(3));
                }
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        // A round may report "try again" when the test harness started or
        // ended a thread meanwhile; it must never fail.
        let mut completed = 0;
        for round in 0..300 {
            if sys::swap_stopped(&[], &[]).unwrap_or_else(|e| panic!("round {round}: {e}")) {
                completed += 1;
            }
        }
        assert!(completed > 0, "no round completed");
        let before = work.load(Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(work.load(Ordering::Relaxed) > before, "the workers did not resume");
        stop.store(true, Ordering::Relaxed);
        for t in threads {
            t.join().unwrap();
        }
    }
}

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
