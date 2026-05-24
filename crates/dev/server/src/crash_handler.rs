//! SIGSEGV / SIGBUS handler for the sidecar.
//!
//! When the subsecond hot-patch dispatch jumps to a bad address — most
//! commonly because a patched function references a Rust symbol the
//! host bin didn't export (or a TLS slot the dylib didn't bind) — the
//! process dies from the kernel signal with no Rust panic to catch.
//! Without a handler the user just sees the sidecar disappear; the
//! runtime-server log shows nothing past "[runtime-server-app] patch applied; notifying ...".
//!
//! This module installs a sigaction-based handler that prints a single
//! line with the faulting address (`siginfo_t.si_addr`) and the
//! signal name, then restores the default handler and re-raises so
//! the process still dies with the correct exit status (any wait
//! loop on the host side sees the right thing).
//!
//! The handler is signal-safe: no allocation, no locks, no
//! reentrancy. Manual hex formatting into a stack buffer + `libc::write`
//! to fd 2. `format!` and `eprintln!` would deadlock if the crashing
//! thread held the allocator or the stderr lock at fault time —
//! both common during hot-patched code execution.

#[cfg(target_os = "macos")]
pub fn install() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handle_fatal as usize;
        sa.sa_flags = libc::SA_SIGINFO;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGBUS, &sa, std::ptr::null_mut());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install() {
    // Non-macOS sidecars aren't in scope for the runtime-server path right now;
    // a glibc/ELF variant can land alongside the linux backend work.
}

#[cfg(target_os = "macos")]
extern "C" fn handle_fatal(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ucontext: *mut libc::c_void,
) {
    // Extract faulting address. `si_addr` is filled for SIGSEGV/SIGBUS
    // — for *data* faults it's the memory the CPU tried to access;
    // for *instruction* faults it's the same as PC.
    let addr = unsafe { (*info).si_addr } as usize;

    // Extract PC from ucontext. On aarch64 macOS:
    //   ucontext_t.uc_mcontext: *mut __darwin_mcontext64
    //   __darwin_mcontext64.__ss: __darwin_arm_thread_state64
    //   __darwin_arm_thread_state64.__pc: u64
    //
    // Comparing PC vs si_addr disambiguates the crash class:
    //   pc == addr  → bad jump (executed at unmapped/non-X address)
    //   pc != addr  → bad load/store at addr from instruction at pc
    let regs = unsafe { read_regs_aarch64(ucontext) };
    let pc = regs.pc;

    // Write fault line.
    let mut buf = [0u8; 256];
    let n = write_hex_line(&mut buf, sig, addr, pc);
    unsafe { libc::write(libc::STDERR_FILENO, buf.as_ptr() as *const _, n); }

    // Write registers line (lr, fp, sp).
    let mut buf2 = [0u8; 256];
    let n2 = write_regs_line(&mut buf2, regs.lr, regs.fp, regs.sp);
    unsafe { libc::write(libc::STDERR_FILENO, buf2.as_ptr() as *const _, n2); }

    // Walk the frame chain — fp points at caller's saved fp/lr pair.
    // Print up to 12 frames before re-raising. Bounds-check each
    // pointer chase: if fp is unaligned or in an obviously-invalid
    // range, stop (don't fault while reporting a fault).
    let mut fp = regs.fp;
    for i in 0..12 {
        if fp == 0 || (fp & 0x7) != 0 || !looks_mapped(fp) {
            break;
        }
        // fp[0] = caller's fp ; fp[1] = caller's lr (return address)
        let caller_fp = unsafe { (fp as *const u64).read_unaligned() };
        let caller_lr = unsafe { ((fp + 8) as *const u64).read_unaligned() };
        let mut bufn = [0u8; 96];
        let nn = write_frame_line(&mut bufn, i, caller_lr);
        unsafe { libc::write(libc::STDERR_FILENO, bufn.as_ptr() as *const _, nn); }
        if caller_fp <= fp {
            break; // fp chain must walk upward; stop on bad chain
        }
        fp = caller_fp;
    }

    // Re-raise with default disposition so the kernel produces a core
    // (if enabled) and the process exits with the right status. Don't
    // call into Rust unwinding from here — signal context is too
    // hostile for it.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, std::ptr::null_mut());
        libc::raise(sig);
    }
}

/// Read the aarch64 program counter from `ucontext_t`. macOS-specific
/// layout: ucontext.uc_mcontext is a pointer to __darwin_mcontext64,
/// whose __ss field is __darwin_arm_thread_state64 (whose __pc is the
/// instruction pointer). All fields are stable since 10.x. Returns 0
/// if `ucontext` is null or any pointer chase is invalid (signal-safe
/// graceful fallback).
///
/// We don't link the libc crate's `mcontext_t` (it isn't always
/// stable across libc versions) — instead we hand-walk the
/// well-known offsets:
///   ucontext_t {
///     int uc_onstack;
///     sigset_t uc_sigmask;     // 4 bytes
///     stack_t  uc_stack;       // 24 bytes (ss_sp + ss_size + ss_flags + padding)
///     ucontext_t* uc_link;     // 8 bytes
///     size_t  uc_mcsize;       // 8 bytes
///     mcontext_t uc_mcontext;  // pointer
///   }
/// Total prefix before uc_mcontext on aarch64 macOS = 48 bytes.
/// mcontext_t points to {__es: arm_exception_state64 (24B), __ss: arm_thread_state64}
/// where arm_thread_state64.__pc is at offset 0x100 from start of struct.
///
/// Rather than rely on hand-computed offsets (fragile across SDK
/// versions), we use libc's `ucontext_t` if it has the right shape.
/// aarch64 architectural registers we care about for crash reporting.
struct Regs {
    pc: u64,
    lr: u64,
    fp: u64,
    sp: u64,
}

/// Read the aarch64 register snapshot from `ucontext_t`. macOS layout:
///   ucontext.uc_mcontext: *mut __darwin_mcontext64
///   __darwin_mcontext64.__ss: __darwin_arm_thread_state64
/// thread_state64 layout (offsets within __ss):
///   __x[29]  → 0    (232 bytes)
///   __fp     → 232
///   __lr     → 240
///   __sp     → 248
///   __pc     → 256
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn read_regs_aarch64(ucontext: *mut libc::c_void) -> Regs {
    if ucontext.is_null() {
        return Regs { pc: 0, lr: 0, fp: 0, sp: 0 };
    }
    let uc = ucontext as *const libc::ucontext_t;
    let mcontext_ptr = (*uc).uc_mcontext;
    if (mcontext_ptr as usize) == 0 {
        return Regs { pc: 0, lr: 0, fp: 0, sp: 0 };
    }
    let mc_bytes = mcontext_ptr as *const u8;
    let ss = mc_bytes.add(24); // skip __es (24 bytes) → start of __ss
    Regs {
        fp: (ss.add(232) as *const u64).read_unaligned(),
        lr: (ss.add(240) as *const u64).read_unaligned(),
        sp: (ss.add(248) as *const u64).read_unaligned(),
        pc: (ss.add(256) as *const u64).read_unaligned(),
    }
}

#[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
unsafe fn read_regs_aarch64(_ucontext: *mut libc::c_void) -> Regs {
    Regs { pc: 0, lr: 0, fp: 0, sp: 0 }
}

/// Crude check whether `addr` is in user-space mapped memory. We
/// can't actually query the kernel from a signal handler without
/// risking re-entrance, so we just rule out obvious garbage (0,
/// kernel space, etc.) and trust that fp chains walk upward into
/// valid stack territory. The frame-walk loop has a separate
/// "stop on bad chain" guard that catches cases where the heuristic
/// is wrong.
#[cfg(target_os = "macos")]
fn looks_mapped(addr: u64) -> bool {
    // macOS user-space addresses are in the lower half (< 0x8000_0000_0000)
    // and definitely non-zero. Practical Rust process layout puts stacks
    // in 0x16_xxxx_xxxx_xxxx ish on aarch64 macOS.
    addr > 0x1000 && addr < 0x0000_8000_0000_0000
}

/// Write `[runtime-server-app]   lr=0x<hex> fp=0x<hex> sp=0x<hex>\n`.
#[cfg(target_os = "macos")]
fn write_regs_line(buf: &mut [u8], lr: u64, fp: u64, sp: u64) -> usize {
    let mut n = 0;
    n += copy_bytes(buf, n, b"[runtime-server-app]   lr=0x");
    n += write_hex(buf, n, lr);
    n += copy_bytes(buf, n, b" fp=0x");
    n += write_hex(buf, n, fp);
    n += copy_bytes(buf, n, b" sp=0x");
    n += write_hex(buf, n, sp);
    n += copy_bytes(buf, n, b"\n");
    n
}

/// Write `[runtime-server-app]   frame N pc=0x<hex>\n`.
#[cfg(target_os = "macos")]
fn write_frame_line(buf: &mut [u8], i: usize, pc: u64) -> usize {
    let mut n = 0;
    n += copy_bytes(buf, n, b"[runtime-server-app]   frame ");
    // single-digit index, 0..=11 fits
    if i < 10 {
        buf[n] = b'0' + (i as u8);
        n += 1;
    } else {
        buf[n] = b'1';
        buf[n + 1] = b'0' + ((i - 10) as u8);
        n += 2;
    }
    n += copy_bytes(buf, n, b" pc=0x");
    n += write_hex(buf, n, pc);
    n += copy_bytes(buf, n, b"\n");
    n
}

/// Write `[runtime-server-app] FATAL SIG{NAME} addr=0x<hex> pc=0x<hex>\n` into
/// `buf`. Returns bytes written. Signal-safe (no allocation, no locks).
#[cfg(target_os = "macos")]
fn write_hex_line(buf: &mut [u8], sig: libc::c_int, addr: usize, pc: u64) -> usize {
    let prefix = b"\n[runtime-server-app] FATAL ";
    let mut n = 0;
    n += copy_bytes(buf, n, prefix);
    let name: &[u8] = match sig {
        libc::SIGSEGV => b"SIGSEGV",
        libc::SIGBUS => b"SIGBUS",
        _ => b"SIG?",
    };
    n += copy_bytes(buf, n, name);
    n += copy_bytes(buf, n, b" addr=0x");
    n += write_hex(buf, n, addr as u64);
    n += copy_bytes(buf, n, b" pc=0x");
    n += write_hex(buf, n, pc);
    n += copy_bytes(buf, n, b"\n");
    n
}

#[cfg(target_os = "macos")]
fn copy_bytes(dst: &mut [u8], at: usize, src: &[u8]) -> usize {
    let end = (at + src.len()).min(dst.len());
    let len = end - at;
    dst[at..end].copy_from_slice(&src[..len]);
    len
}

/// Big-endian hex of `value` into `dst[at..]`. Leading zeros stripped
/// (except for value=0 → "0"). Returns bytes written.
#[cfg(target_os = "macos")]
fn write_hex(dst: &mut [u8], at: usize, value: u64) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if value == 0 {
        if at < dst.len() {
            dst[at] = b'0';
            return 1;
        }
        return 0;
    }
    // Find the leading nibble.
    let mut start_shift: i32 = 60;
    while start_shift >= 0 && ((value >> start_shift) & 0xF) == 0 {
        start_shift -= 4;
    }
    let mut n = 0;
    while start_shift >= 0 {
        let nibble = ((value >> start_shift) & 0xF) as usize;
        if at + n < dst.len() {
            dst[at + n] = HEX[nibble];
            n += 1;
        }
        start_shift -= 4;
    }
    n
}
