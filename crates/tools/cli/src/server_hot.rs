//! Hot patches for a full-stack session's SERVER: a body edit in a crate
//! the server is built from is patched into the RUNNING server process,
//! where it would otherwise rebuild the binary and respawn it.
//!
//! The mechanism is `build_runtime_server::hotpatch::detour` (the patch)
//! and its agent (`agent/hot_agent.rs`, the half inside the server); their
//! docs explain how a running function is redirected. This module is the
//! session's side:
//!
//! - **Arming** ([`ServerHot::arm`]): compile the agent, open the Unix
//!   socket it reports to, read the server's local packages.
//! - **The server's builds** run through the `idealyst` rustc wrapper
//!   ([`ServerHot::build_env`]), so every crate cargo compiles leaves its
//!   exact rustc invocation behind for a patch to replay. The wrapper does
//!   not enter cargo's fingerprint: arming costs no rebuild.
//! - **Every successful build** is the new BASE ([`ServerHot::rebased`]):
//!   the binary's symbols (parsed in the background) and the sources it
//!   was built from, read when the build started.
//! - **The server's start**: `idealyst run-linked` loads the agent into
//!   the process ([`ServerHot::run_env`]); the agent says hello on the
//!   socket with its pid and ASLR slide.
//! - **Each save** ([`ServerHot::try_patch`]) is decided against the base's
//!   sources (`dev_overlay::native`); a patchable one replays only the
//!   edited crates, links the patch and has the agent apply it. Anything
//!   else — and any failure on the way — returns `Rebuild`, and the
//!   watcher runs today's `cargo build` + respawn.
//!
//! macOS on Apple silicon only (the patch builder links Mach-O arm64);
//! elsewhere the session never arms it and every server save rebuilds.
//! `IDEALYST_SERVER_HOT_PATCH=0` turns it off.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use build_runtime_server::hotpatch::detour::{agent, PatchCrate, ServerPatchBuilder, AGENT_CFG, AGENT_CRATE, AGENT_SOURCE};
use build_runtime_server::hotpatch::{replay, CapturedInvocation};
use dev_events::{DevEvent, SERVER_TARGET};
use dev_reload::{native::NativeDecision, Patched, Supersede};

/// The environment variable that turns the tier off (`0`).
pub(crate) const OFF_ENV: &str = "IDEALYST_SERVER_HOT_PATCH";

/// Read by `idealyst run-linked`: the agent library to load into the
/// server it execs.
pub(crate) const AGENT_LIB_ENV: &str = "IDEALYST_HOT_AGENT_LIB";

/// How long the agent may take to apply a patch before the session
/// gives up on it and rebuilds. Loading a patch and swapping its pages
/// takes milliseconds; this only bounds a server that stopped answering.
const APPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether this host can patch a native server at all.
pub(crate) fn supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// The session's server hot-patch tier. Cheap to clone.
#[derive(Clone)]
pub(crate) struct ServerHot {
    inner: Arc<Inner>,
}

struct Inner {
    agent_lib: PathBuf,
    socket: PathBuf,
    captures: PathBuf,
    patches: PathBuf,
    idealyst: PathBuf,
    packages: Vec<LocalPackage>,
    agent: Mutex<Option<AgentConn>>,
    base: Mutex<Option<Arc<Base>>>,
    loading: Mutex<Option<std::thread::JoinHandle<Result<Base>>>>,
}

/// A build's binary and the sources it was compiled from.
struct Base {
    exe: PathBuf,
    stamp: Option<FileStamp>,
    builder: ServerPatchBuilder,
    sources: Mutex<HashMap<PathBuf, String>>,
}

/// The agent of the server process now running.
struct AgentConn {
    hello: agent::proto::Hello,
    /// The executable it runs, as it was when it connected: which build
    /// it is.
    stamp: Option<FileStamp>,
    writer: std::os::unix::net::UnixStream,
    reader: BufReader<std::os::unix::net::UnixStream>,
}

/// Length + modification time: enough to tell two links of a binary
/// apart, and kept by the clone cargo uplifts (see `run_linked`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp(u64, Option<SystemTime>);

impl FileStamp {
    fn of(path: &Path) -> Option<Self> {
        let md = std::fs::metadata(path).ok()?;
        Some(FileStamp(md.len(), md.modified().ok()))
    }
}

/// One local package of the server's build.
#[derive(Debug, Clone)]
struct LocalPackage {
    name: String,
    dir: PathBuf,
    /// The library target's crate name, if it has one.
    lib: Option<String>,
    /// `(crate name, src path)` of each binary target.
    bins: Vec<(String, PathBuf)>,
}

/// Which compilation unit a saved file belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Unit {
    crate_name: String,
    bin: bool,
}

impl ServerHot {
    /// Arm the tier for the server whose manifest is `server_manifest`,
    /// building into `target_dir`. `None` (logged) when this host or
    /// session cannot have it.
    pub(crate) fn arm(server_manifest: &Path, target_dir: &Path) -> Option<ServerHot> {
        if !supported() {
            crate::dlog!("dev server", "server hot patches need macOS on Apple silicon; server saves rebuild");
            return None;
        }
        if std::env::var(OFF_ENV).is_ok_and(|v| v == "0") {
            crate::dlog!("dev server", "{OFF_ENV}=0: server saves rebuild");
            return None;
        }
        match Self::try_arm(server_manifest, target_dir) {
            Ok(hot) => Some(hot),
            Err(e) => {
                dev_events::global()
                    .warn("dev server", format!("server hot patches are off for this session: {e:#}"));
                None
            }
        }
    }

    fn try_arm(server_manifest: &Path, target_dir: &Path) -> Result<ServerHot> {
        let root = target_dir.join("idealyst-hotpatch");
        let agent_lib = build_agent(&root.join("agent"))?;
        let packages = local_packages(server_manifest)?;
        // `sun_path` holds 104 bytes on macOS, which a project's target
        // dir can exceed; the temp dir is short and per-user.
        let socket = std::env::temp_dir().join(format!("idealyst-srv-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket)
            .with_context(|| format!("bind the agent socket {}", socket.display()))?;
        crate::cmd::dev::remove_on_exit(vec![socket.clone()]);
        let hot = ServerHot {
            inner: Arc::new(Inner {
                agent_lib,
                socket,
                captures: root.join("captures"),
                patches: root.join("patches"),
                idealyst: std::env::current_exe().context("locate the idealyst executable")?,
                packages,
                agent: Mutex::new(None),
                base: Mutex::new(None),
                loading: Mutex::new(None),
            }),
        };
        let accept = hot.clone();
        std::thread::Builder::new()
            .name("idealyst-server-agent".into())
            .spawn(move || accept.accept(listener))
            .context("spawn the agent socket thread")?;
        Ok(hot)
    }

    /// Take each server's hello. The newest connection is the server now
    /// running: a restart kills the old process, and its socket with it.
    fn accept(&self, listener: std::os::unix::net::UnixListener) {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let Ok(read) = stream.try_clone() else { continue };
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let Some(hello) = agent::proto::parse_hello(&line) else {
                dev_events::global().warn("dev server", format!("hot-patch agent sent no hello: {line:?}"));
                continue;
            };
            if hello.protocol != agent::PROTOCOL {
                dev_events::global().warn(
                    "dev server",
                    format!("hot-patch agent speaks protocol {}, not {}", hello.protocol, agent::PROTOCOL),
                );
                continue;
            }
            let _ = stream.set_read_timeout(Some(APPLY_TIMEOUT));
            crate::dlog!("dev server", "hot-patch agent connected: pid {} ({})", hello.pid, hello.exe);
            let stamp = FileStamp::of(Path::new(&hello.exe));
            *self.inner.agent.lock().unwrap() = Some(AgentConn { hello, stamp, writer: stream, reader });
        }
    }

    /// The environment of the server's `cargo build`: every crate cargo
    /// compiles goes through the capture wrapper. Never `cargo run`'s:
    /// `idealyst run-linked` would inherit it and take itself for the
    /// wrapper (`main` dispatches on these variables before parsing
    /// arguments).
    pub(crate) fn build_env(&self) -> Vec<(String, String)> {
        vec![
            ("RUSTC_WRAPPER".into(), self.inner.idealyst.display().to_string()),
            ("IDEALYST_RUSTC_CAPTURE_DIR".into(), self.inner.captures.display().to_string()),
            ("IDEALYST_RUSTC_WRAPPER_ACTIVE".into(), "1".into()),
        ]
    }

    /// The environment of the server's start: the agent to load, and
    /// where it reports.
    pub(crate) fn run_env(&self) -> Vec<(String, String)> {
        vec![
            (AGENT_LIB_ENV.into(), self.inner.agent_lib.display().to_string()),
            (agent::SOCKET_ENV.into(), self.inner.socket.display().to_string()),
        ]
    }

    /// The sources a build is about to compile: every `.rs` file under
    /// each local package's `src/`. Read when the build starts — a save
    /// made while it runs is the next save, decided against these.
    pub(crate) fn read_sources(&self) -> HashMap<PathBuf, String> {
        let mut out = HashMap::new();
        for p in &self.inner.packages {
            read_tree(&p.dir.join("src"), &mut out);
        }
        out
    }

    /// A build linked `exe` from `sources`: the next patch is built
    /// against it. Parsing the binary's symbols takes a while on a large
    /// server, so it runs in the background; the next patch waits for it.
    pub(crate) fn rebased(&self, exe: PathBuf, sources: HashMap<PathBuf, String>) {
        *self.inner.base.lock().unwrap() = None;
        let patches = self.inner.patches.clone();
        let handle = std::thread::Builder::new()
            .name("idealyst-server-base".into())
            .spawn(move || -> Result<Base> {
                let stamp = FileStamp::of(&exe);
                let builder = ServerPatchBuilder::new(&exe, patches)?;
                Ok(Base { exe, stamp, builder, sources: Mutex::new(sources) })
            })
            .ok();
        *self.inner.loading.lock().unwrap() = handle;
    }

    fn base(&self) -> Result<Arc<Base>> {
        if let Some(handle) = self.inner.loading.lock().unwrap().take() {
            let loaded = handle.join().map_err(|_| anyhow::anyhow!("reading the server binary panicked"))?;
            *self.inner.base.lock().unwrap() = Some(Arc::new(loaded?));
        }
        self.inner.base.lock().unwrap().clone().context("no server build to patch yet")
    }

    /// Offer a save to the tier. See the module docs; every way it can
    /// fail is reported and answers `Rebuild`.
    pub(crate) fn try_patch(&self, paths: &[PathBuf], token: &Supersede) -> Patched {
        let reporter = dev_events::global();
        let started = Instant::now();
        let base = match self.base() {
            Ok(b) => b,
            Err(e) => {
                reporter.log("dev server", format!("not hot patching: {e:#}"));
                return Patched::Rebuild;
            }
        };
        let saved: Vec<Saved> = paths.iter().map(|p| self.saved(p, &base)).collect();
        let files: Vec<String> = saved.iter().map(|s| s.display.clone()).collect();
        let decision = {
            let saves: Vec<dev_reload::native::NativeSave<'_>> = saved
                .iter()
                .map(|s| dev_reload::native::NativeSave {
                    package: &s.package,
                    display: &s.display,
                    source: s.unit.is_some(),
                    before: s.before.as_deref(),
                    after: s.after.as_deref(),
                })
                .collect();
            dev_reload::native::decide_native(&saves)
        };
        let decided = |decision| reporter.emit(DevEvent::Decided { target: SERVER_TARGET.into(), decision });
        let units: BTreeSet<Unit> = match decision {
            NativeDecision::Unchanged => {
                decided(dev_events::Decision::Unchanged);
                return Patched::Applied;
            }
            NativeDecision::Rebuild(why) => {
                decided(dev_events::Decision::Rebuild { reason: Some(why.to_string()) });
                return Patched::Rebuild;
            }
            NativeDecision::Patch { .. } => {
                saved.iter().filter(|s| s.before != s.after).filter_map(|s| s.unit.clone()).collect()
            }
        };
        decided(dev_events::Decision::HotPatch {
            crates: units.iter().map(|u| u.crate_name.clone()).collect(),
            files: files.clone(),
        });
        let steps = Mutex::new(Vec::<dev_events::Timing>::new());
        let on_step = |name: &'static str, took: Duration| {
            reporter.emit(DevEvent::PatchStep {
                target: SERVER_TARGET.into(),
                step: name.into(),
                ms: took.as_millis() as u64,
            });
            steps.lock().unwrap().push(dev_events::Timing::new(name, took));
        };
        on_step("decide", started.elapsed());
        let failed = |reason: String| {
            reporter.emit(DevEvent::PatchFailed { target: SERVER_TARGET.into(), files: files.clone(), reason });
            Patched::Rebuild
        };
        let superseded = || {
            reporter.emit(DevEvent::Superseded {
                target: SERVER_TARGET.into(),
                work: dev_events::SupersededWork::HotPatch,
                ms: started.elapsed().as_millis() as u64,
            });
            Patched::Superseded
        };

        let crates = match self.captures(&units) {
            Ok(c) => c,
            Err(e) => return failed(format!("{e:#}")),
        };
        // The agent of the process running THIS base's binary: a server
        // still running the previous build has other addresses.
        let slide = {
            let agent = self.inner.agent.lock().unwrap();
            match agent.as_ref() {
                None => {
                    return failed(
                        "the server has no hot-patch agent connected (a project `runner` replaces \
                         `idealyst run-linked`, which loads it)"
                            .into(),
                    )
                }
                Some(a) if a.stamp.is_none() || a.stamp != base.stamp => {
                    return failed(format!(
                        "the running server (pid {}) is not the last build of {} yet",
                        a.hello.pid,
                        base.exe.display()
                    ))
                }
                Some(a) => a.hello.slide,
            }
        };
        let cancel = || token.is_superseded();
        let patch = match base.builder.build(&crates, slide, &cancel, &on_step) {
            Ok(p) => p,
            Err(e) if e.is::<replay::Cancelled>() => return superseded(),
            Err(e) => return failed(format!("{e:#}")),
        };
        // What the patch compiled must be what was decided: a save that
        // landed during the replay, past the window that supersedes, may
        // have moved a crate's shape into the patch.
        if token.is_superseded() || !self.sources_still(&base, &saved, &units) {
            return superseded();
        }
        let t = Instant::now();
        let applied = {
            let mut agent = self.inner.agent.lock().unwrap();
            match agent.as_mut() {
                Some(a) => apply(a, &patch),
                None => Err(anyhow::anyhow!("the server's hot-patch agent disconnected")),
            }
        };
        on_step("apply", t.elapsed());
        match applied {
            Ok(detoured) => {
                let mut sources = base.sources.lock().unwrap();
                for s in &saved {
                    if let (Some(path), Some(after)) = (&s.path, &s.after) {
                        sources.insert(path.clone(), after.clone());
                    }
                }
                drop(sources);
                let pid = self.inner.agent.lock().unwrap().as_ref().map(|a| a.hello.pid).unwrap_or(0);
                reporter.log(
                    "dev server",
                    format!("patched the running server (pid {pid}): {detoured} function(s) redirected"),
                );
                reporter.emit(DevEvent::PatchBuilt {
                    target: SERVER_TARGET.into(),
                    files,
                    crates: patch
                        .crates
                        .iter()
                        .map(|(name, took)| dev_events::CrateTiming { name: name.clone(), ms: Some(took.as_millis() as u64) })
                        .collect(),
                    redirected: detoured,
                    steps: std::mem::take(&mut *steps.lock().unwrap()),
                    skipped: Vec::new(),
                    bytes: patch.bytes,
                    ms: started.elapsed().as_millis() as u64,
                });
                Patched::Applied
            }
            Err(e) => failed(format!("the server did not take the patch: {e:#}")),
        }
    }

    /// A saved path, with what the base and the disk hold for it.
    fn saved(&self, path: &Path, base: &Base) -> Saved {
        let canonical = canonical(path);
        let package = self.inner.packages.iter().filter(|p| canonical.starts_with(&p.dir)).max_by_key(|p| p.dir.as_os_str().len());
        let display = match package {
            Some(p) => format!(
                "{}/{}",
                p.name,
                canonical.strip_prefix(&p.dir).unwrap_or(&canonical).display()
            ),
            None => path.display().to_string(),
        };
        let unit = package.and_then(|p| unit_of(p, &canonical));
        let before = base.sources.lock().unwrap().get(&canonical).cloned();
        let after = std::fs::read_to_string(&canonical).ok();
        Saved {
            package: package.map(|p| p.name.clone()).unwrap_or_default(),
            display,
            unit,
            path: Some(canonical),
            before,
            after,
        }
    }

    /// The capture of each unit, from the server's builds.
    fn captures(&self, units: &BTreeSet<Unit>) -> Result<Vec<PatchCrate>> {
        units
            .iter()
            .map(|u| {
                let kinds: &[&str] = if u.bin { &["bin"] } else { &["lib", "rlib"] };
                let file = kinds
                    .iter()
                    .map(|k| self.inner.captures.join(format!("{}.{k}.json", u.crate_name)))
                    .find(|p| p.is_file())
                    .with_context(|| {
                        format!(
                            "no captured rustc invocation for `{}` yet: the server's builds have not \
                             compiled it since server hot patches were armed (this rebuild will)",
                            u.crate_name
                        )
                    })?;
                let capture: CapturedInvocation = serde_json::from_slice(
                    &std::fs::read(&file).with_context(|| format!("read {}", file.display()))?,
                )
                .with_context(|| format!("parse {}", file.display()))?;
                Ok(PatchCrate { crate_name: u.crate_name.clone(), capture })
            })
            .collect()
    }

    /// Whether every source file of the patched packages still holds what
    /// the decision read (the saved ones) or the base (the rest).
    fn sources_still(&self, base: &Base, saved: &[Saved], units: &BTreeSet<Unit>) -> bool {
        let decided: HashMap<&PathBuf, &Option<String>> =
            saved.iter().filter_map(|s| s.path.as_ref().map(|p| (p, &s.after))).collect();
        let sources = base.sources.lock().unwrap();
        for p in &self.inner.packages {
            let patched = p.lib.iter().chain(p.bins.iter().map(|(n, _)| n)).any(|n| units.iter().any(|u| &u.crate_name == n));
            if !patched {
                continue;
            }
            let mut now = HashMap::new();
            read_tree(&p.dir.join("src"), &mut now);
            for (path, text) in &now {
                let expected = match decided.get(path) {
                    Some(after) => after.as_ref(),
                    None => sources.get(path),
                };
                if expected != Some(text) {
                    return false;
                }
            }
            let in_base = sources.keys().filter(|k| k.starts_with(&p.dir)).count();
            if now.len() != in_base {
                return false;
            }
        }
        true
    }
}

struct Saved {
    package: String,
    display: String,
    unit: Option<Unit>,
    path: Option<PathBuf>,
    before: Option<String>,
    after: Option<String>,
}

/// Send a patch to the agent and wait for its answer.
fn apply(agent: &mut AgentConn, patch: &build_runtime_server::hotpatch::detour::ServerPatch) -> Result<usize> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    agent
        .writer
        .write_all(agent::proto::format_apply(seq, &patch.dylib, &patch.pairs).as_bytes())
        .context("send the patch to the server")?;
    let mut line = String::new();
    agent.reader.read_line(&mut line).context("read the server's answer")?;
    match agent::proto::parse_reply(&line) {
        Some(agent::proto::Reply::Ok { seq: s, detoured, .. }) if s == seq => Ok(detoured),
        Some(agent::proto::Reply::Err { message, .. }) => bail!("{message}"),
        _ => bail!("unexpected answer {line:?}"),
    }
}

/// Which unit a source file of `p` compiles into: a binary target's root
/// or a file under `src/bin/` is that binary's; any other file under
/// `src/` is the library's, or the only binary's when there is no
/// library. `None` for anything else (`build.rs`, tests, examples).
fn unit_of(p: &LocalPackage, file: &Path) -> Option<Unit> {
    let src = p.dir.join("src");
    if !file.starts_with(&src) || file.extension().is_none_or(|e| e != "rs") {
        return None;
    }
    if let Some((name, _)) = p.bins.iter().find(|(_, root)| root == file) {
        return Some(Unit { crate_name: name.clone(), bin: true });
    }
    if let Ok(rest) = file.strip_prefix(src.join("bin")) {
        let stem = rest.components().next()?.as_os_str().to_string_lossy().trim_end_matches(".rs").replace('-', "_");
        return p.bins.iter().find(|(n, _)| *n == stem).map(|(n, _)| Unit { crate_name: n.clone(), bin: true });
    }
    match (&p.lib, p.bins.as_slice()) {
        (Some(lib), _) => Some(Unit { crate_name: lib.clone(), bin: false }),
        (None, [(only, _)]) => Some(Unit { crate_name: only.clone(), bin: true }),
        _ => None,
    }
}

fn canonical(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    // A deleted file: canonicalize what is left of it.
    match (path.parent().and_then(|d| std::fs::canonicalize(d).ok()), path.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

fn read_tree(dir: &Path, out: &mut HashMap<PathBuf, String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => read_tree(&path, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|x| x == "rs") => {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.insert(canonical(&path), text);
                }
            }
            _ => {}
        }
    }
}

/// The server's local packages (path dependencies and the server's own),
/// with their library and binary crate names.
fn local_packages(manifest: &Path) -> Result<Vec<LocalPackage>> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .output()
        .context("run `cargo metadata` for the server")?;
    if !out.status.success() {
        bail!("`cargo metadata` for the server failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).context("parse `cargo metadata`")?;
    Ok(packages_of(&meta))
}

fn packages_of(meta: &serde_json::Value) -> Vec<LocalPackage> {
    let mut out = Vec::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        if !p["source"].is_null() {
            continue;
        }
        let Some(manifest) = p["manifest_path"].as_str() else { continue };
        let dir = canonical(Path::new(manifest).parent().unwrap_or(Path::new(".")));
        let mut lib = None;
        let mut bins = Vec::new();
        for t in p["targets"].as_array().into_iter().flatten() {
            let kinds: Vec<&str> = t["kind"].as_array().into_iter().flatten().filter_map(|k| k.as_str()).collect();
            let name = t["name"].as_str().unwrap_or_default().replace('-', "_");
            if kinds.iter().any(|k| matches!(*k, "lib" | "rlib" | "dylib" | "cdylib" | "staticlib")) {
                lib = Some(name);
            } else if kinds.contains(&"bin") {
                if let Some(src) = t["src_path"].as_str() {
                    bins.push((name, canonical(Path::new(src))));
                }
            }
        }
        out.push(LocalPackage { name: p["name"].as_str().unwrap_or_default().to_string(), dir, lib, bins });
    }
    out
}

/// Compile the agent for this host, once per agent source and toolchain.
fn build_agent(dir: &Path) -> Result<PathBuf> {
    use sha2::Digest as _;
    let version = Command::new("rustc").arg("-vV").output().context("run `rustc -vV`")?;
    let mut h = sha2::Sha256::new();
    h.update(AGENT_SOURCE.as_bytes());
    h.update(&version.stdout);
    let key: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    let out_dir = dir.join(&key);
    let lib = out_dir.join(format!("lib{AGENT_CRATE}.dylib"));
    if lib.is_file() {
        return Ok(lib);
    }
    std::fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    let src = out_dir.join("hot_agent.rs");
    std::fs::write(&src, AGENT_SOURCE).with_context(|| format!("write {}", src.display()))?;
    let tmp = out_dir.join(format!("lib{AGENT_CRATE}.tmp.dylib"));
    let out = Command::new("rustc")
        .args(["--edition=2021", "--crate-type", "cdylib", "--crate-name", AGENT_CRATE, "--cfg", AGENT_CFG])
        .args(["-C", "opt-level=1", "-C", "debuginfo=0"])
        .arg(&src)
        .arg("-o")
        .arg(&tmp)
        .output()
        .context("run rustc for the hot-patch agent")?;
    if !out.status.success() {
        bail!("the hot-patch agent did not compile:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    std::fs::rename(&tmp, &lib).with_context(|| format!("move {} into place", lib.display()))?;
    // Older agents of this target dir (another CLI or toolchain).
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name() != key.as_str() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(lib)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(lib: Option<&str>, bins: &[&str]) -> LocalPackage {
        LocalPackage {
            name: "p".into(),
            dir: PathBuf::from("/w/p"),
            lib: lib.map(String::from),
            bins: bins
                .iter()
                .map(|b| (b.to_string(), if *b == "p" { PathBuf::from("/w/p/src/main.rs") } else { PathBuf::from(format!("/w/p/src/bin/{b}.rs")) }))
                .collect(),
        }
    }

    fn unit(name: &str, bin: bool) -> Option<Unit> {
        Some(Unit { crate_name: name.into(), bin })
    }

    #[test]
    fn a_module_file_belongs_to_the_library_and_a_binary_root_to_its_binary() {
        let p = pkg(Some("p"), &["p", "seed"]);
        assert_eq!(unit_of(&p, Path::new("/w/p/src/domains/checkin.rs")), unit("p", false));
        assert_eq!(unit_of(&p, Path::new("/w/p/src/main.rs")), unit("p", true));
        assert_eq!(unit_of(&p, Path::new("/w/p/src/bin/seed.rs")), unit("seed", true));
    }

    /// A binary-only package (CrewForge's `server-bin`): its modules are
    /// the binary's.
    #[test]
    fn without_a_library_every_source_file_is_the_binarys() {
        let p = pkg(None, &["p"]);
        assert_eq!(unit_of(&p, Path::new("/w/p/src/routes.rs")), unit("p", true));
    }

    #[test]
    fn build_scripts_tests_and_other_files_are_no_unit() {
        let p = pkg(Some("p"), &[]);
        assert_eq!(unit_of(&p, Path::new("/w/p/build.rs")), None);
        assert_eq!(unit_of(&p, Path::new("/w/p/tests/e2e.rs")), None);
        assert_eq!(unit_of(&p, Path::new("/w/p/src/schema.sql")), None);
    }

    #[test]
    fn packages_come_from_path_dependencies_with_their_crate_names() {
        let meta = serde_json::json!({ "packages": [
            { "name": "crewforge-core", "source": null, "manifest_path": "/nonexistent/core/Cargo.toml",
              "targets": [{ "kind": ["lib"], "name": "crewforge-core", "src_path": "/nonexistent/core/src/lib.rs" }] },
            { "name": "server-bin", "source": null, "manifest_path": "/nonexistent/server-bin/Cargo.toml",
              "targets": [{ "kind": ["bin"], "name": "crewforge-server", "src_path": "/nonexistent/server-bin/src/main.rs" }] },
            { "name": "serde", "source": "registry+https://github.com/rust-lang/crates.io-index",
              "manifest_path": "/r/serde/Cargo.toml", "targets": [] },
        ]});
        let p = packages_of(&meta);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].lib.as_deref(), Some("crewforge_core"));
        assert_eq!(p[1].bins[0].0, "crewforge_server");
    }
}
