//! A server hot patch redirects a RUNNING process's functions in place.
//!
//! This builds a real two-crate program the way cargo would — a library
//! (`shared`) and a binary (`srv`) that calls it — and runs it with the
//! hot-patch agent loaded (`DYLD_INSERT_LIBRARIES` / `LD_PRELOAD`, as
//! `idealyst run-linked` does). Eight threads call `shared::greeting` the whole time
//! through every way a server reaches a function: a direct call from the
//! binary's crate, a function pointer stored at startup, and a trait
//! object built at startup. The test then edits the library's body,
//! drives `ServerPatchBuilder` exactly as the dev session does (replay of
//! the library alone, never the binary), hands the patch to the agent,
//! and checks:
//!
//! * every path returns the new value, in the SAME process;
//! * the hammering threads never saw anything but the old or the new
//!   value (no torn detour, no crash) — with every detour forced onto the
//!   16-byte long jump, which is written with the other threads
//!   suspended;
//! * a writable static the library owns kept its state across the patch;
//! * a second patch of the same crate re-redirects the same functions;
//! * a patch whose live code needs a function the binary does not contain
//!   is refused by the builder, which reads what the link left undefined,
//!   and the process carries on with what it ran before;
//! * a generic instantiation rustc shares from an upstream crate does not
//!   refuse a patch: not when only code the link strips names it, and not
//!   when live code does, since it comes from the upstream rlib.
//!
//! arm64 macOS and Linux, the hosts the native patch builder links for
//! (Mach-O / ELF). On a Mac, run the Linux leg in an arm64 container:
//!
//! ```text
//! docker run --rm -v $PWD:/work -w /work mcr.microsoft.com/devcontainers/rust:1 \
//!   cargo test -p build-runtime-server --test server_detour
//! ```

#![cfg(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64"))]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use build_runtime_server::hotpatch::detour::{agent, PatchCrate, ServerPatchBuilder, AGENT_CFG, AGENT_CRATE, AGENT_SOURCE};
use build_runtime_server::hotpatch::CapturedInvocation;

const SHARED: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};

pub static CALLS: AtomicU64 = AtomicU64::new(0);

#[inline(never)]
pub fn greeting() -> u64 {
    CALLS.fetch_add(1, Ordering::Relaxed);
    // BODY
    1
}

pub trait Greeter: Send + Sync {
    fn greet(&self) -> u64;
}

pub struct Plain;

impl Greeter for Plain {
    #[inline(never)]
    fn greet(&self) -> u64 {
        greeting() * 10
    }
}

#[inline(never)]
pub fn calls() -> u64 {
    CALLS.load(Ordering::Relaxed)
}
"#;

const SRV: &str = r#"
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

static STOP: AtomicBool = AtomicBool::new(false);

fn main() {
    // Built once, at startup — like a router or a middleware chain.
    let stored: fn() -> u64 = shared::greeting;
    let dynamic: Arc<dyn shared::Greeter> = Arc::new(shared::Plain);
    let bad = Arc::new(AtomicU64::new(0));
    let mut threads = Vec::new();
    for _ in 0..8 {
        let bad = bad.clone();
        let dynamic = dynamic.clone();
        threads.push(std::thread::spawn(move || {
            while !STOP.load(Ordering::Relaxed) {
                let v = shared::greeting();
                let d = dynamic.greet();
                if !matches!(v, 1 | 2 | 3) || d != v * 10 && !matches!(d, 10 | 20 | 30) {
                    bad.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    println!("ready pid={}", std::process::id());
    for line in std::io::stdin().lock().lines() {
        match line.unwrap().as_str() {
            "check" => println!(
                "direct={} stored={} dynamic={} calls_grew={} bad={}",
                shared::greeting(),
                stored(),
                dynamic.greet(),
                shared::calls() > 1000,
                bad.load(Ordering::Relaxed),
            ),
            _ => break,
        }
    }
    STOP.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
}
"#;

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

fn to_strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// What cargo passes rustc for the library at the dev profile.
fn shared_args(dir: &Path) -> Vec<String> {
    let mut a = to_strings(&[
        "--crate-name", "shared", "--edition=2021", "shared/src/lib.rs", "--crate-type", "lib",
        "--emit=dep-info,metadata,link", "-C", "opt-level=0", "-C", "debuginfo=0", "-C",
        "metadata=serverdetour", "-C", "extra-filename=-serverdetour",
    ]);
    a.push("--out-dir".into());
    a.push(dir.join("deps").display().to_string());
    a.push("-C".into());
    a.push(format!("incremental={}", dir.join("incremental").display()));
    a
}

fn srv_args(dir: &Path) -> Vec<String> {
    let mut a = to_strings(&[
        "--crate-name", "srv", "--edition=2021", "srv/src/main.rs", "--crate-type", "bin", "--emit=link", "-C",
        "opt-level=0", "-C", "debuginfo=0", "-C", "metadata=serverdetoursrv",
    ]);
    a.push("--out-dir".into());
    a.push(dir.join("deps").display().to_string());
    a.push("--extern".into());
    a.push(format!("shared={}", dir.join("deps/libshared-serverdetour.rlib").display()));
    a.push("-L".into());
    a.push(dir.join("deps").display().to_string());
    a
}

fn run(rustc: &str, args: &[String], dir: &Path) {
    let out = Command::new(rustc).args(args).current_dir(dir).output().expect("run rustc");
    assert!(out.status.success(), "rustc failed:\n{}", String::from_utf8_lossy(&out.stderr));
}

struct Server {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn check(&mut self) -> String {
        writeln!(self.stdin, "check").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        line.trim().to_string()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct AgentSession {
    reader: BufReader<std::os::unix::net::UnixStream>,
    writer: std::os::unix::net::UnixStream,
    hello: agent::proto::Hello,
}

impl AgentSession {
    fn apply(&mut self, seq: u64, dylib: &Path, pairs: &Path) -> agent::proto::Reply {
        self.writer.write_all(agent::proto::format_apply(seq, dylib, pairs).as_bytes()).unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        agent::proto::parse_reply(&line).unwrap_or_else(|| panic!("unreadable reply {line:?}"))
    }
}

fn build_agent(dir: &Path) -> PathBuf {
    let src = dir.join("hot_agent.rs");
    std::fs::write(&src, AGENT_SOURCE).unwrap();
    let out = dir.join(format!("lib{AGENT_CRATE}.{}", if cfg!(target_os = "macos") { "dylib" } else { "so" }));
    let status = Command::new(rustc())
        .args(["--edition=2021", "--crate-type", "cdylib", "--crate-name", AGENT_CRATE, "--cfg", AGENT_CFG])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "the agent did not compile");
    out
}

/// Start `exe` with the agent loaded, and take its hello: the session's
/// socket, then the server.
fn start_server(exe: &Path, agent_lib: &Path, dir: &Path) -> (Server, AgentSession) {
    let socket = dir.join("agent.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut child = Command::new(exe)
        .env(if cfg!(target_os = "macos") { "DYLD_INSERT_LIBRARIES" } else { "LD_PRELOAD" }, agent_lib)
        .env(agent::SOCKET_ENV, &socket)
        // The patch of a binary this small usually lands within `b`
        // reach; a real server's does not. Exercise the long jump, which
        // is written with the other threads suspended.
        .env(agent::LONG_JUMPS_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    let pid = child.id();
    assert_eq!(ready.trim(), format!("ready pid={pid}"));
    let server = Server { child, stdin, stdout };

    listener.set_nonblocking(false).unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut hello = String::new();
    reader.read_line(&mut hello).unwrap();
    let hello = agent::proto::parse_hello(&hello).unwrap_or_else(|| panic!("no hello: {hello:?}"));
    assert_eq!(hello.pid, pid);
    assert_eq!(hello.protocol, agent::PROTOCOL);
    (server, AgentSession { reader, writer: stream, hello })
}

#[test]
fn a_running_server_runs_the_patched_body_through_every_kind_of_call() {
    let dir = scratch("server-detour");
    for d in ["shared/src", "srv/src", "deps", "patches"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::write(dir.join("shared/src/lib.rs"), SHARED).unwrap();
    std::fs::write(dir.join("srv/src/main.rs"), SRV).unwrap();
    let rustc = rustc();
    run(&rustc, &shared_args(&dir), &dir);
    run(&rustc, &srv_args(&dir), &dir);
    let exe = dir.join("deps/srv");
    let agent_lib = build_agent(&dir);

    let (mut server, mut session) = start_server(&exe, &agent_lib, &dir);
    let pid = server.child.id();

    assert_eq!(server.check(), "direct=1 stored=1 dynamic=10 calls_grew=true bad=0");
    let capture = CapturedInvocation { rustc: rustc.clone(), args: shared_args(&dir), cwd: dir.display().to_string(), env: vec![] };
    let builder = ServerPatchBuilder::new(&exe, dir.join("patches")).unwrap();
    let slide = session.hello.slide;
    let patch_shared = |body: &str| {
        std::fs::write(dir.join("shared/src/lib.rs"), SHARED.replace("    // BODY\n    1", body)).unwrap();
        let crates = [PatchCrate { crate_name: "shared".into(), capture: CapturedInvocation { ..clone(&capture) } }];
        builder.build(&crates, slide, &|| false, &|_, _| {})
    };

    // ── 1. A body edit, applied while eight threads call it ──────────
    let p = patch_shared("    2").expect("build the patch");
    assert!(p.detours >= 3, "greeting, Plain::greet and calls should all be redirected: {p:?}");
    match session.apply(1, &p.dylib, &p.pairs) {
        agent::proto::Reply::Ok { detoured, .. } => assert_eq!(detoured, p.detours),
        other => panic!("the agent refused the patch: {other:?}"),
    }
    assert_eq!(server.check(), "direct=2 stored=2 dynamic=20 calls_grew=true bad=0");

    // ── 2. A second edit of the same crate ────────────────────────────
    let p = patch_shared("    3").expect("build the patch");
    assert!(matches!(session.apply(2, &p.dylib, &p.pairs), agent::proto::Reply::Ok { .. }));
    assert_eq!(server.check(), "direct=3 stored=3 dynamic=30 calls_grew=true bad=0");

    // ── 3. Live code needing a function the binary never linked ──────
    // `std::fs::canonicalize` is nowhere in the base: dead-stripped from
    // it, so the patch imports it and `dlopen(RTLD_NOW)` cannot bind it.
    match patch_shared("    std::fs::canonicalize(\"/\").map(|p| p.as_os_str().len() as u64 + 40).unwrap_or(41)") {
        Err(e) => {
            let message = format!("{e:#}");
            assert!(message.contains("canonicalize"), "{message}");
        }
        Ok(p) => match session.apply(3, &p.dylib, &p.pairs) {
            agent::proto::Reply::Err { message, .. } => {
                assert!(message.contains("canonicalize") || message.contains("symbol"), "{message}")
            }
            other => panic!("a patch with an unbindable import was applied: {other:?}"),
        },
    }
    assert_eq!(server.check(), "direct=3 stored=3 dynamic=30 calls_grew=true bad=0", "a refused patch changed nothing");
    assert_eq!(server.child.id(), pid);
    let _ = std::fs::remove_dir_all(&dir);
}

// An upstream crate that compiles an instantiation of a std generic for its
// own use. rustc at `opt-level = 0` shares it: a crate downstream refers to
// this crate's copy rather than compiling one. The binary below never calls
// `up` at all, so its link strips the copy.
const UP: &str = r#"
pub fn pick(v: &[f32]) -> Option<&f32> {
    v.get(1usize)
}

pub fn plain_nobody_calls() -> u64 {
    std::hint::black_box(7)
}
"#;

const SHARED_OVER_UP: &str = r#"
pub use up::pick as _pick;

#[inline(never)]
pub fn live() -> u64 {
    let _ = std::hint::black_box(0u64);
    // BODY
    1
}

/// Nothing in the binary reaches this, so the link strips it from the base
/// and from every patch; its use of the shared instantiation is all there is.
pub fn nobody_calls_this(v: &[f32]) -> Option<&f32> {
    v.get(1usize)
}
"#;

const SRV_OVER_SHARED: &str = r#"
use std::io::BufRead;

fn main() {
    println!("ready pid={}", std::process::id());
    for line in std::io::stdin().lock().lines() {
        match line.unwrap().as_str() {
            "check" => println!("live={}", shared::live()),
            _ => break,
        }
    }
}
"#;

/// CrewForge A16: `crewforge-core` carried 15 such instantiations, on every
/// save, whatever the edit. The builder refused the patch at its stub, before
/// the link had stripped the only code that named them, and said `dyld` on
/// Linux while it was at it.
#[test]
fn regression_a_generic_shared_from_upstream_does_not_refuse_a_patch() {
    let dir = scratch("server-upstream");
    for d in ["up/src", "shared/src", "srv/src", "deps", "patches"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::write(dir.join("up/src/lib.rs"), UP).unwrap();
    std::fs::write(dir.join("shared/src/lib.rs"), SHARED_OVER_UP).unwrap();
    std::fs::write(dir.join("srv/src/main.rs"), SRV_OVER_SHARED).unwrap();
    let rustc = rustc();
    let deps = dir.join("deps").display().to_string();
    let flags = |name: &str, src: &str, metadata: &str| {
        let mut a = to_strings(&["--crate-name", name, "--edition=2021", src, "-C", "opt-level=0", "-C", "debuginfo=0"]);
        a.extend(["-C".into(), format!("metadata={metadata}"), "-C".into(), format!("extra-filename=-{metadata}")]);
        a.extend(["--out-dir".into(), deps.clone(), "-L".into(), format!("dependency={deps}")]);
        a
    };
    let lib = |name: &str, metadata: &str| {
        let mut a = flags(name, &format!("{name}/src/lib.rs"), metadata);
        a.extend(to_strings(&["--crate-type", "lib", "--emit=dep-info,metadata,link"]));
        a.extend(["-C".into(), format!("incremental={}", dir.join("incremental").display())]);
        a
    };
    let up_rlib = dir.join("deps/libup-upstreamup.rlib");
    let up_extern = format!("up={}", up_rlib.display());
    let shared_args = {
        let mut a = lib("shared", "upstreamshared");
        a.extend(["--extern".into(), up_extern.clone()]);
        a
    };
    run(&rustc, &lib("up", "upstreamup"), &dir);
    run(&rustc, &shared_args, &dir);
    let mut srv = flags("srv", "srv/src/main.rs", "upstreamsrv");
    srv.extend(to_strings(&["--crate-type", "bin", "--emit=link"]));
    srv.extend(["--extern".into(), format!("shared={}", dir.join("deps/libshared-upstreamshared.rlib").display())]);
    srv.extend(["--extern".into(), up_extern]);
    run(&rustc, &srv, &dir);
    let exe = dir.join("deps/srv-upstreamsrv");
    let agent_lib = build_agent(&dir);
    let (mut server, mut session) = start_server(&exe, &agent_lib, &dir);
    let pid = server.child.id();
    let check = |server: &mut Server| {
        writeln!(server.stdin, "check").unwrap();
        let mut line = String::new();
        server.stdout.read_line(&mut line).unwrap();
        line.trim().to_string()
    };
    assert_eq!(check(&mut server), "live=1");

    let capture = CapturedInvocation { rustc: rustc.clone(), args: shared_args, cwd: dir.display().to_string(), env: vec![] };
    let builder = ServerPatchBuilder::new(&exe, dir.join("patches")).unwrap();
    let slide = session.hello.slide;
    let patch = |body: &str| {
        std::fs::write(dir.join("shared/src/lib.rs"), SHARED_OVER_UP.replace("    // BODY\n    1", body)).unwrap();
        let crates = [PatchCrate { crate_name: "shared".into(), capture: CapturedInvocation { ..clone(&capture) } }];
        let steps = std::sync::Mutex::new(Vec::new());
        let built = builder.build(&crates, slide, &|| false, &|name, _| steps.lock().unwrap().push(name));
        (built, steps.into_inner().unwrap())
    };
    let nm = |symbol: &str| {
        let out = Command::new("nm").arg(&exe).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(symbol)
    };

    // ── 1. Only stripped code names the instantiation ────────────────
    assert!(!nm("Sf3getj"), "the fixture's binary must not hold `<[f32]>::get::<usize>`");
    let (built, steps) = patch("    2");
    let p = built.expect("dead code's reference must not refuse the patch");
    assert!(!steps.contains(&"upstream"), "nothing live needs it, so no rlib is read: {steps:?}");
    match session.apply(1, &p.dylib, &p.pairs) {
        agent::proto::Reply::Ok { .. } => {}
        other => panic!("the agent refused the patch: {other:?}"),
    }
    assert_eq!(check(&mut server), "live=2");

    // ── 2. Live code names it: the copy comes from `up`'s rlib ───────
    let (built, steps) = patch("    let v = [1.0f32, 3.0];\n    v.get(1usize).map(|f| *f as u64).unwrap_or(0)");
    let p = built.expect("a shared instantiation is available from the upstream rlib");
    assert!(steps.contains(&"upstream"), "the fixture must exercise the rlib: {steps:?}");
    match session.apply(2, &p.dylib, &p.pairs) {
        agent::proto::Reply::Ok { .. } => {}
        other => panic!("the agent refused the patch: {other:?}"),
    }
    assert_eq!(check(&mut server), "live=3");

    // ── 3. A plain function: a copy would never see a later patch ─────
    let (built, _) = patch("    up::plain_nobody_calls()");
    let message = format!("{:#}", built.expect_err("a stripped plain function has no safe copy"));
    assert!(message.contains("plain_nobody_calls"), "{message}");
    assert!(!message.contains("dyld"), "{message}");
    assert_eq!(check(&mut server), "live=3", "a refused patch changed nothing");

    // ── 4. An rlib compiled after the link does not speak for the binary ──
    std::thread::sleep(Duration::from_millis(20));
    let rewritten = std::fs::read(&up_rlib).unwrap();
    std::fs::write(&up_rlib, rewritten).unwrap();
    let (built, _) = patch("    let v = [1.0f32, 4.0];\n    v.get(1usize).map(|f| *f as u64).unwrap_or(0)");
    let message = format!("{:#}", built.expect_err("a newer rlib may hold other code"));
    assert!(message.contains("after the running binary was linked"), "{message}");
    assert_eq!(check(&mut server), "live=3");
    assert_eq!(server.child.id(), pid);
    let _ = std::fs::remove_dir_all(&dir);
}

fn clone(c: &CapturedInvocation) -> CapturedInvocation {
    CapturedInvocation { rustc: c.rustc.clone(), args: c.args.clone(), cwd: c.cwd.clone(), env: c.env.clone() }
}
