//! A native hot patch runs against the RUNNING binary's statics.
//!
//! The bug this guards: a patch re-emits the edited crate, so every
//! `static` / `thread_local!` that crate defines was defined a second
//! time inside the patch dylib. Code the patch redirected wrote one copy
//! and the base code still running read the other. CrewForge's kiosk
//! stored the report sign-in from a patched screen and its request
//! headers, built by unpatched code, read "Not signed in".
//!
//! This compiles a real Rust binary, runs it, lets it put state into its
//! statics, then drives `HotPatchBuilder::build` exactly as the dev host
//! does — replaying the captured rustc invocation over an edited source,
//! synthesizing the stub, linking the dylib — and has the running
//! process `dlopen` the patch and call the patched function. The patched
//! function must see the state the base wrote:
//!
//! * a module-level writable `static` (the sign-in case);
//! * a `static` declared inside a function body;
//! * a `const`-initialized `thread_local!` and a lazily initialized one;
//! * a writable `static` the base binary does not have gets storage of
//!   its own (it is new), and an immutable `static` keeps a fresh
//!   address per patch, which `stylesheet!`'s cache key relies on.
//!
//! macOS on Apple silicon only: that is the one target the native patch
//! builder links for (`hotpatch::link`).

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use build_runtime_server::hotpatch::{CapturedInvocation, HotPatchBuilder};

/// The base program. It writes state into its statics, prints its
/// `main` address (the ASLR anchor the sidecar reports in `Hello`), then
/// loads whatever patch the test hands it on stdin and calls the
/// patch's `probe_read` / `probe_key`.
const BASE: &str = r#"
use std::cell::Cell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::BufRead;
use std::sync::atomic::{AtomicU32, Ordering};

pub static SESSION: AtomicU32 = AtomicU32::new(0);
static KEY: u8 = 0;

thread_local! {
    static TOKEN: Cell<u32> = const { Cell::new(0) };
    static LAZY: Cell<u32> = Cell::new(0);
}

// A constructor, as `inventory::submit!` emits one: dyld ran it when the
// binary started. The patch shares the state it initialized, so loading
// the patch must not run it again.
static CTOR_RUNS: AtomicU32 = AtomicU32::new(0);
extern "C" fn register() {
    CTOR_RUNS.fetch_add(1, Ordering::SeqCst);
}
#[used]
#[link_section = "__DATA,__mod_init_func,mod_init_funcs"]
static REGISTER: extern "C" fn() = register;

#[inline(never)]
fn visits() -> u32 {
    static VISITS: AtomicU32 = AtomicU32::new(0);
    VISITS.fetch_add(1, Ordering::SeqCst) + 1
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn probe_read() -> u64 {
    // BODY
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn probe_key() -> usize {
    &KEY as *const u8 as usize
}

extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

fn main() {
    SESSION.store(42, Ordering::SeqCst);
    TOKEN.with(|t| t.set(7));
    LAZY.with(|t| t.set(5));
    visits();
    visits();
    // RTLD_DEFAULT is (void*)-2 on Darwin.
    let main_addr = unsafe { dlsym(-2isize as *mut c_void, c"main".as_ptr()) } as usize;
    println!("main={main_addr:x}");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    let path = CString::new(line.trim()).unwrap();
    unsafe {
        // RTLD_NOW | RTLD_LOCAL
        let handle = dlopen(path.as_ptr(), 0x2 | 0x4);
        if handle.is_null() {
            println!("dlopen failed: {}", CStr::from_ptr(dlerror()).to_string_lossy());
            return;
        }
        let read = dlsym(handle, c"probe_read".as_ptr());
        let key = dlsym(handle, c"probe_key".as_ptr());
        let read: extern "C" fn() -> u64 = std::mem::transmute(read);
        let key: extern "C" fn() -> usize = std::mem::transmute(key);
        println!("read={}", read());
        println!("read-again={}", read());
        println!("key-fresh={}", key() != probe_key());
        println!("ctor-runs={}", CTOR_RUNS.load(Ordering::SeqCst));
    }
}
"#;

/// The edit: a body-only change to `probe_read`, which now reads every
/// kind of static the base wrote. `FRESH` is a writable static the base
/// never had.
const PATCHED_BODY: &str = r#"
    static FRESH: AtomicU32 = AtomicU32::new(900);
    let fresh = FRESH.fetch_add(1, Ordering::SeqCst) as u64;
    SESSION.load(Ordering::SeqCst) as u64 * 1_000_000_000
        + TOKEN.with(|t| t.get()) as u64 * 10_000_000
        + LAZY.with(|t| t.get()) as u64 * 1_000_000
        + visits() as u64 * 1_000
        + fresh"#;

/// The active toolchain's `rustc`, by absolute path: a replay runs with
/// the captured environment only, which here is empty, so a bare
/// `rustc` would not be found on its `PATH`.
fn rustc() -> String {
    let out = Command::new("rustc").args(["--print", "sysroot"]).output().expect("run rustc");
    let sysroot = String::from_utf8(out.stdout).unwrap();
    Path::new(sysroot.trim()).join("bin/rustc").display().to_string()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("idealyst-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The argv a cargo build of this crate would capture, minus what the
/// fixture does not need. `-Csave-temps` / `-Clink-dead-code` are the
/// fat-build flags (`fat_build_env`), so the base binary keeps every
/// symbol the patch may resolve against.
fn base_args(dir: &Path) -> Vec<String> {
    [
        "--crate-name",
        "probe",
        "--edition=2021",
        "src/main.rs",
        "--crate-type",
        "bin",
        "--emit=link",
        "-C",
        "opt-level=0",
        "-C",
        "debuginfo=0",
        "-C",
        "metadata=hotpatchstatics",
        "-Csave-temps=true",
        "-Clink-dead-code",
        "--out-dir",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(std::iter::once(dir.join("deps").display().to_string()))
    .collect()
}

#[test]
fn regression_a_patched_function_reads_the_running_binarys_statics() {
    let dir = scratch("hotpatch-statics");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("deps")).unwrap();
    std::fs::write(dir.join("src/main.rs"), BASE).unwrap();

    // ── 1. The base build ────────────────────────────────────────────
    let args = base_args(&dir);
    let rustc = rustc();
    let status = Command::new(&rustc).args(&args).current_dir(&dir).status().expect("run rustc");
    assert!(status.success(), "the base build failed");
    let host_bin = dir.join("deps/probe");
    assert!(host_bin.exists(), "no base binary at {}", host_bin.display());

    // The capture the fat build's rustc wrapper would have written.
    let captures = dir.join("captures");
    std::fs::create_dir_all(&captures).unwrap();
    let capture = CapturedInvocation {
        rustc,
        args,
        cwd: dir.display().to_string(),
        env: Vec::new(),
    };
    std::fs::write(captures.join("probe.bin.json"), serde_json::to_vec(&capture).unwrap()).unwrap();

    // ── 2. Run it ────────────────────────────────────────────────────
    let mut child = Command::new(&host_bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the base binary");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let main_addr = u64::from_str_radix(
        line.trim().strip_prefix("main=").unwrap_or_else(|| panic!("no main address: {line:?}")),
        16,
    )
    .unwrap();

    // ── 3. Edit a body and build the patch as the dev host does ─────
    std::fs::write(dir.join("src/main.rs"), BASE.replace("    // BODY\n    0", PATCHED_BODY)).unwrap();
    let builder = HotPatchBuilder::new(captures, &host_bin, dir.join("patches"), PathBuf::from("idealyst"))
        .expect("parse the base binary");
    let patch = builder.build("probe", main_addr, 0);
    let patch = match patch {
        Ok(p) => p,
        Err(e) => {
            let _ = child.kill();
            panic!("building the patch failed: {e:#}");
        }
    };

    // ── 4. Load it into the running process ──────────────────────────
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", patch.dylib.display()).unwrap();
    drop(stdin);
    let mut rest = String::new();
    for l in out.lines() {
        rest.push_str(&l.unwrap());
        rest.push('\n');
    }
    let _ = child.wait();

    // SESSION 42, TOKEN 7, LAZY 5, VISITS 2 → 3 on this call, FRESH 900.
    assert!(
        rest.contains("read=42075003900\n"),
        "the patched function did not read the running binary's statics \
         (expected read=42075003900: SESSION 42, TOKEN 7, LAZY 5, visits 3, fresh 900):\n{rest}"
    );
    // The second call proves the writes land in the same storage: VISITS
    // and the new FRESH both advance.
    assert!(rest.contains("read-again=42075004901\n"), "a second call did not advance the statics:\n{rest}");
    // An immutable static is content, not state: the patch keeps its own
    // copy, at its own address. `stylesheet!` keys its cache by exactly
    // such an address, and a patched sheet must miss that cache.
    assert!(rest.contains("key-fresh=true\n"), "an immutable static was bound to the base's copy:\n{rest}");
    // dyld ran the binary's constructor at launch. Loading the patch runs
    // the patch's initializers, and its copy of that constructor now acts
    // on the binary's statics — run again, it would register twice (an
    // `inventory` node pushed onto a list it already heads makes a cycle).
    assert!(rest.contains("ctor-runs=1\n"), "the binary's constructor ran again when the patch loaded:\n{rest}");

    let _ = std::fs::remove_dir_all(&dir);
}
