//! Hot patches for a native process that has no jump table: the dev
//! session's SERVER.
//!
//! The sidecar and the web page redirect functions through a table the
//! framework's own code consults (`#[component]`'s `__hot::call`, the wasm
//! element table). A full-stack app's server is the app's own binary — an
//! axum router, `#[server]` fns registered through `inventory`,
//! middleware, background tasks — and nothing in it asks a table where a
//! function lives. So a patch here redirects functions IN PLACE: the
//! first instruction of each of the running binary's functions becomes a
//! branch to the patch's copy. Every caller then reaches the new body —
//! a direct call from a crate the patch did not re-emit, a function
//! pointer stored at startup, a vtable — which is what lets a patch
//! re-emit ONLY the edited crates. A dependent's base code calls the
//! edited crate directly, and lands in the patch.
//!
//! The half inside the server is the agent (`agent/hot_agent.rs`, loaded
//! at exec by `idealyst run-linked`); its docs explain how a running
//! function is rewritten without a thread ever seeing a torn instruction.
//! This module is the half in the dev session.
//!
//! # A patch, step by step
//!
//! 1. **Replay** each edited crate's captured rustc invocation with
//!    `--emit=obj` (see [`super::replay`]: 256 codegen units, its own
//!    incremental directory, `-Cmetadata` untouched so symbols are named
//!    exactly as in the binary). Crates replay concurrently.
//! 2. **Bind** statics: the patch's references to writable statics and
//!    thread-locals the binary already has are pointed at the binary's
//!    storage ([`super::bind`], [`super::statics`]), and constructors the
//!    binary already ran are neutralized.
//! 3. **Choose the functions to redirect** ([`detour_candidate`]): every
//!    function the replayed crates define that the binary has too, by
//!    exact mangled name, except closures, async bodies, shims and generic
//!    instantiations (below).
//! 4. **Stub**: every symbol the patch uses but does not define resolves
//!    into the running binary ([`super::stub`]).
//! 5. **Link** with `-dead_strip` (`--gc-sections` on Linux), rooted at
//!    exactly the functions of step 3 ([`mark_roots`]) and `main`, the
//!    anchor the agent finds the patch's load address by. The binary was
//!    linked with dead code stripped, so a re-emitted crate holds code
//!    the binary never kept, and that code references functions the
//!    binary does not have. Stripped from the patch, it needs nothing.
//!    What the live code still imports is real: a Rust symbol the binary
//!    lacks is a generic instantiation rustc shared from an upstream crate,
//!    taken from that crate's rlib under [`super::upstream`]'s rules, with
//!    steps 2 to 5 run again over the larger set of objects. One with no
//!    safe copy refuses the patch before anything loads.
//! 6. **Pair** each root's address in the binary with its address in the
//!    patch, and write them for the agent ([`Pairs`]).
//!
//! # What is never redirected, and why that is safe
//!
//! - **Closures and coroutines** (`{closure#N}`: closure bodies, and the
//!   state machine of every `async fn` and `async` block). A future built
//!   by the old code is resumed by the old code: its state layout and
//!   suspend points belong to it. Redirecting its resume function would
//!   run the new body over the old state. Unredirected, an in-flight
//!   request finishes on the code it started on, and the next request,
//!   entering through redirected functions, builds the patch's futures.
//! - **Generic instantiations** (`::<…>`, or a generic self type) and
//!   shims. The decision refuses an edit to a generic body, so every
//!   instantiation in the binary still runs the same code; the ones it
//!   calls are redirected.
//!
//! The decision (`dev_overlay::native`) refuses everything this cannot
//! carry before a replay starts: a changed shape, a generic / `#[inline]`
//! / `const fn` / `async fn` / `impl Trait` body (their copies or their
//! state layouts live where a redirect does not reach), an edit inside a
//! closure or `async` block (one built before the patch — a route handler
//! registered at startup, a spawned task — would keep its old body while
//! the session reported the edit applied), and `fn main`, which never
//! runs again.
//!
//! arm64 only: Mach-O (macOS) and ELF (Linux) binaries, the object
//! format read from the binary ([`HostBinCache::format`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};

use super::cache::HostBinCache;
use super::replay::{self, CapturedInvocation};
use super::upstream::Rlibs;

/// The agent's source, compiled by the session on its own
/// (`rustc --crate-type cdylib --cfg idealyst_hot_agent`).
pub const AGENT_SOURCE: &str = include_str!("../../agent/hot_agent.rs");

/// The agent's protocol and planning code, compiled into this crate too:
/// the session speaks the protocol, and the tests exercise the planning.
#[path = "../../agent/hot_agent.rs"]
pub mod agent;

/// The `--cfg` that turns the agent's initializer on. Without it the
/// file is a plain library (what this crate compiles it as).
pub const AGENT_CFG: &str = "idealyst_hot_agent";

/// The agent dylib's crate name. The agent removes any library path
/// containing it from `DYLD_INSERT_LIBRARIES` / `LD_PRELOAD`, so the
/// server's own child processes do not load it.
pub const AGENT_CRATE: &str = "idealyst_hot_agent";

/// The address pairs a patch hands the agent.
pub use agent::proto::Pairs;

/// One crate a patch re-emits.
pub struct PatchCrate {
    /// rustc's `--crate-name` (underscores): what the symbols spell.
    pub crate_name: String,
    pub capture: CapturedInvocation,
}

/// A built patch, ready for the agent.
#[derive(Debug)]
pub struct ServerPatch {
    pub dylib: PathBuf,
    pub pairs: PathBuf,
    /// How many functions the agent will redirect.
    pub detours: usize,
    pub bytes: u64,
    /// Per crate, the replay's wall time.
    pub crates: Vec<(String, Duration)>,
}

/// A step's end, for the session's `patch_step` events.
pub type OnStep<'a> = &'a (dyn Fn(&'static str, Duration) + Sync);

/// Builds server patches against one base binary. Re-created (or
/// [`Self::rebase`]d) whenever the binary is relinked: every address it
/// holds belongs to one link.
pub struct ServerPatchBuilder {
    cache: HostBinCache,
    text: TextIndex,
    /// When the binary was linked: the newest an rlib may be and still speak
    /// for it ([`super::upstream`]).
    linked: Option<std::time::SystemTime>,
    out_dir: PathBuf,
    seq: std::sync::atomic::AtomicU64,
    /// Niceness increment of a patch's replays ([`Self::with_replay_nice`]).
    replay_nice: i32,
}

/// Where each function of a linked image ends, and its bytes: what tells
/// the agent how much of a function it may overwrite, and whether a
/// function too short for a long jump changed at all.
struct TextIndex {
    path: PathBuf,
    /// Every distinct function start in the text section, sorted.
    starts: Vec<u64>,
    /// The text section: address, size, file offset.
    section: (u64, u64, u64),
}

impl TextIndex {
    fn of(path: &Path, data: &[u8]) -> Result<Self> {
        let file = object::File::parse(data).with_context(|| format!("parse {}", path.display()))?;
        let text = if file.format() == object::BinaryFormat::Elf { ".text" } else { "__text" };
        let section =
            file.section_by_name(text).with_context(|| format!("{} has no {text} section", path.display()))?;
        let (offset, _) = section.file_range().with_context(|| format!("the {text} section has no file range"))?;
        let index = section.index();
        let mut starts: Vec<u64> = file
            .symbols()
            .filter(|s| s.section_index() == Some(index) && !s.is_undefined())
            .map(|s| s.address())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        Ok(TextIndex { path: path.to_path_buf(), starts, section: (section.address(), section.size(), offset) })
    }

    /// Bytes from `addr` to the next function (or the section's end).
    fn size(&self, addr: u64) -> u64 {
        let i = self.starts.partition_point(|&a| a <= addr);
        let end = self.starts.get(i).copied().unwrap_or(self.section.0 + self.section.1);
        end.saturating_sub(addr)
    }

    /// The bytes of the function at `addr`, from `data` (the image's
    /// contents) when the caller has them, else read from the file.
    fn bytes(&self, addr: u64, data: Option<&[u8]>) -> Result<Vec<u8>> {
        let len = self.size(addr) as usize;
        let at = (addr - self.section.0 + self.section.2) as usize;
        if let Some(d) = data {
            return d.get(at..at + len).map(<[u8]>::to_vec).context("a function outside its image");
        }
        use std::io::{Read, Seek};
        let mut f = std::fs::File::open(&self.path).with_context(|| format!("open {}", self.path.display()))?;
        f.seek(std::io::SeekFrom::Start(at as u64))?;
        let mut out = vec![0u8; len];
        f.read_exact(&mut out)?;
        Ok(out)
    }
}

impl ServerPatchBuilder {
    /// Parse `exe` (the server binary the session runs) and prepare
    /// `out_dir` for patches.
    pub fn new(exe: &Path, out_dir: PathBuf) -> Result<Self> {
        let cache = HostBinCache::load(exe).with_context(|| format!("reading the server binary {}", exe.display()))?;
        let data = std::fs::read(exe).with_context(|| format!("read {}", exe.display()))?;
        let text = TextIndex::of(exe, &data)?;
        drop(data);
        std::fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
        let linked = std::fs::metadata(exe).and_then(|m| m.modified()).ok();
        Ok(Self { cache, text, linked, out_dir, seq: std::sync::atomic::AtomicU64::new(0), replay_nice: 0 })
    }

    /// Run each patch's replays at niceness increment `nice` (`0`, the
    /// default: this process's priority). A full-stack save starts a web
    /// hot patch beside this replay, and the web patch is the one on the
    /// developer's screen, so a session gives this the increment its server
    /// build runs at. Not the seeds' 19: someone is waiting on this patch
    /// too, and on a busy machine 19 would starve it.
    pub fn with_replay_nice(mut self, nice: i32) -> Self {
        self.replay_nice = nice;
        self
    }

    /// Build a patch of `crates` for a server running at ASLR `slide`.
    /// `cancel` is polled throughout; a cancelled build returns an error
    /// that `is::<replay::Cancelled>()`.
    pub fn build(
        &self,
        crates: &[PatchCrate],
        slide: u64,
        cancel: &(dyn Fn() -> bool + Sync),
        on_step: OnStep<'_>,
    ) -> Result<ServerPatch> {
        anyhow::ensure!(!crates.is_empty(), "a patch needs at least one crate");
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = self.out_dir.join(format!("patch-{seq}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        // Earlier patches' work files. Their dylibs stay loaded in the
        // server — a mapped file outlives its directory entry.
        self.prune_before(seq);

        // 1. Replay, concurrently.
        let started = Instant::now();
        let results: Vec<(String, Result<Vec<PathBuf>>, Duration)> = std::thread::scope(|s| {
            let handles: Vec<_> = crates
                .iter()
                .map(|c| {
                    s.spawn(move || {
                        let t = Instant::now();
                        let r = replay::run_rustc_emit_obj_niced(&c.capture, &[], cancel, self.replay_nice);
                        (c.crate_name.clone(), r, t.elapsed())
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().expect("a replay thread panicked")).collect()
        });
        let mut objects = Vec::new();
        let mut timings = Vec::new();
        for (name, result, took) in results {
            let objs = result.with_context(|| format!("replaying `{name}`"))?;
            anyhow::ensure!(!objs.is_empty(), "replaying `{name}` produced no objects");
            objects.extend(objs);
            timings.push((name, took));
        }
        on_step("replay", started.elapsed());
        if cancel() {
            return Err(replay::Cancelled.into());
        }

        // 2–5, repeated while the live code still imports a Rust symbol.
        let names: Vec<&str> = crates.iter().map(|c| c.crate_name.as_str()).collect();
        let elf = self.cache.format == object::BinaryFormat::Elf;
        let rlibs = Rlibs::new(rlib_dirs(crates), self.linked);
        let vouch = |key: &str| crates.iter().find_map(|c| rlibs.vouch(&crate_rlib(&c.capture)?, key));
        let dylib = dir.join(format!("libpatch-{seq}.{}", if elf { "so" } else { "dylib" }));
        let mut pulled: HashSet<(PathBuf, String)> = HashSet::new();
        let mut steps = [Duration::ZERO; 5];
        let mut timed = |i: usize, t: Instant| steps[i] += t.elapsed();
        let roots = loop {
            // 2. Statics.
            let t = Instant::now();
            let (bound, bindings) = if elf {
                super::elf::bind(&objects, &self.cache, &dir.join("bound"), slide, &vouch)
            } else {
                super::bind::bind_vouched(&objects, &self.cache, &dir.join("bound"), &vouch)
            }
            .context("binding the patch's statics to the running server's")?;
            timed(0, t);

            // 3. Roots, marked in copies of the objects.
            let t = Instant::now();
            let (linked, roots) = mark_roots(&bound, &self.cache, &names, &dir.join("marked"))?;
            if roots.is_empty() {
                bail!(
                    "none of the replayed functions of {} is in the running server — its build of \
                     the crate does not match the capture (another build of it ran without the \
                     session's rustc wrapper)",
                    names.join(", ")
                );
            }
            timed(1, t);

            // 4. Stub.
            let t = Instant::now();
            let stub = dir.join("stub.o");
            super::stub::synthesize(
                &linked,
                &self.cache,
                &bindings,
                self.cache.main_addr.wrapping_add(slide),
                super::stub::MissingRust::AfterLink,
                &dir.join("stub.s"),
                &stub,
            )
            .context("synthesizing the stub object")?;
            timed(2, t);

            // 5. Link.
            let t = Instant::now();
            if elf {
                super::link::link_patch_elf(&linked, &stub, &dylib, &dir).context("linking the patch")?;
            } else {
                super::link::link_patch(&linked, &stub, &dylib, &dir.join("link.args")).context("linking the patch")?;
            }
            let missing = super::stub::undefined_rust_symbols(&dylib)?;
            timed(3, t);
            if missing.is_empty() {
                break roots;
            }

            let t = Instant::now();
            let more = rlibs.pull(&missing, &mut pulled, &dir.join("upstream")).with_context(|| {
                format!(
                    "the patched code needs {} Rust function(s) the running server does not contain \
                     (a generic instantiation its link stripped, say). Examples: {}",
                    missing.len(),
                    missing.iter().take(3).map(String::as_str).collect::<Vec<_>>().join(", "),
                )
            })?;
            objects.extend(more.into_iter().map(|p| p.path));
            timed(4, t);
        };
        for (name, took) in ["bind", "roots", "stub", "link"].into_iter().zip(steps) {
            on_step(name, took);
        }
        if !steps[4].is_zero() {
            on_step("upstream", steps[4]);
        }

        // 6. Pairs.
        let t = Instant::now();
        let pairs = pair(&dylib, &self.cache, &self.text, &roots)?;
        let pairs_file = dir.join(format!("patch-{seq}.pairs"));
        std::fs::write(&pairs_file, agent::proto::format_pairs(&pairs))
            .with_context(|| format!("write {}", pairs_file.display()))?;
        on_step("pairs", t.elapsed());
        let bytes = std::fs::metadata(&dylib).map(|m| m.len()).unwrap_or(0);
        Ok(ServerPatch { dylib, pairs: pairs_file, detours: pairs.pairs.len(), bytes, crates: timings })
    }

    /// Remove the work directories of patches before `seq`.
    fn prune_before(&self, seq: u64) {
        let Ok(entries) = std::fs::read_dir(&self.out_dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(n) = name.to_str().and_then(|n| n.strip_prefix("patch-")).and_then(|n| n.parse::<u64>().ok())
            else {
                continue;
            };
            if n < seq {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Whether the function `symbol` (as the object files spell it, with
/// Mach-O's leading underscore or ELF's none) is one a patch of `crates`
/// redirects. See the
/// module docs for what is left out and why.
///
/// Only Rust-mangled names: a `#[no_mangle]` function is the decision's
/// to refuse (`dev_overlay::native`), and the unmangled names a crate
/// defines besides (`main`, allocator shims) are entry points nothing
/// should redirect.
pub fn detour_candidate(symbol: &str, crates: &[&str]) -> bool {
    plain_path(symbol).is_some_and(|path| crates.iter().any(|c| path.contains(&format!("{c}::"))))
}

/// Whether `symbol` is a function some patch of its crate could redirect, so
/// its body can differ from the base's: a Rust function that is no closure,
/// shim or generic instantiation.
///
/// Sharper than [`detour_candidate`], which only looks at crates a patch names
/// and cannot err toward leaving a function out. Here the name decides whether
/// a copy may be carried, and `<usize as SliceIndex<[f32]>>::get` reads as a
/// plain method in its demangled path. Its symbol gives it away: v0 appends the
/// instantiating crate to an instantiation, so the name does not end in the
/// path's last segment.
pub(super) fn plain_function(symbol: &str) -> bool {
    let bare = if symbol.starts_with("__") { &symbol[1..] } else { symbol };
    plain_path(symbol).is_some_and(|path| {
        let last = path.rsplit("::").next().unwrap_or(&path);
        !bare.starts_with("_R") || bare.ends_with(&format!("{}{last}", last.len()))
    })
}

/// The demangled path of a plain Rust function.
fn plain_path(symbol: &str) -> Option<String> {
    // Mach-O prefixes every C name with `_` (`__R…`, `__ZN…`); ELF does
    // not (`_R…`, `_ZN…`).
    let bare = if symbol.starts_with("__") { &symbol[1..] } else { symbol };
    if !(bare.starts_with("_R") || bare.starts_with("_ZN")) {
        return None;
    }
    let demangled = rustc_demangle::try_demangle(bare).ok()?;
    let path = format!("{demangled:#}");
    // `{closure#0}`, `{shim:vtable#0}`, `{constant#0}`, `{{closure}}`.
    if path.contains('{') || path.contains("::<") {
        return None;
    }
    if let Some(rest) = path.strip_prefix('<') {
        let close = matching_angle(rest)?;
        let header = &rest[..close];
        let self_ty = header.split(" as ").next().unwrap_or(header);
        if self_ty.contains('<') {
            return None;
        }
    }
    Some(path)
}

/// Where the rlibs of the replayed crates' dependency graphs are: their
/// `-L dependency=` directories, then the toolchain's own.
fn rlib_dirs(crates: &[PatchCrate]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for c in crates {
        let mut args = c.capture.args.iter();
        while let Some(a) = args.next() {
            let value = if a == "-L" { args.next().map(String::as_str) } else { a.strip_prefix("-L") };
            if let Some(dir) = value.and_then(|v| v.strip_prefix("dependency=")).map(PathBuf::from) {
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
        // `<sysroot>/bin/rustc` → `<sysroot>/lib/rustlib/<triple>/lib`.
        let Some(sysroot) = Path::new(&c.capture.rustc).parent().and_then(Path::parent) else { continue };
        let Ok(triples) = std::fs::read_dir(sysroot.join("lib/rustlib")) else { continue };
        for triple in triples.flatten() {
            let lib = triple.path().join("lib");
            if lib.is_dir() && !dirs.contains(&lib) {
                dirs.push(lib);
            }
        }
    }
    dirs
}

/// The rlib a capture's compile writes: `<out-dir>/lib<crate><extra>.rlib`.
fn crate_rlib(capture: &CapturedInvocation) -> Option<PathBuf> {
    let value = |key: &str| {
        let mut args = capture.args.iter();
        while let Some(a) = args.next() {
            if a == key {
                return args.next().cloned();
            }
            if let Some(v) = a.strip_prefix(&format!("{key}=")) {
                return Some(v.to_string());
            }
        }
        None
    };
    let codegen = |prefix: &str| {
        let mut args = capture.args.iter();
        while let Some(a) = args.next() {
            let flag = if a == "-C" { args.next().map(String::as_str) } else { a.strip_prefix("-C") };
            if let Some(v) = flag.and_then(|f| f.strip_prefix(prefix)) {
                return Some(v.to_string());
            }
        }
        None
    };
    let rlib = PathBuf::from(value("--out-dir")?).join(format!(
        "lib{}{}.rlib",
        value("--crate-name")?,
        codegen("extra-filename=").unwrap_or_default()
    ));
    rlib.is_file().then_some(rlib)
}

/// The index of the `>` closing the `<` already consumed.
fn matching_angle(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

// Mach-O constants (`<mach-o/loader.h>`, `<mach-o/nlist.h>`).
const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_SYMTAB: u32 = 0x2;
const N_STAB: u8 = 0xe0;
const N_TYPE: u8 = 0x0e;
const N_SECT: u8 = 0xe;
/// `n_desc` flag: the linker must not dead-strip this symbol.
const N_NO_DEAD_STRIP: u16 = 0x0020;

/// Copy each object into `out_dir` with every detour candidate it
/// defines marked so the linker keeps it (Mach-O: the symbol
/// `N_NO_DEAD_STRIP`; ELF: its section `SHF_GNU_RETAIN`), and return the
/// copies with the set of marked names (the roots).
///
/// A name defined by two objects is left out: a function the patch
/// defines twice cannot be paired with one of the binary's.
pub fn mark_roots(
    objects: &[PathBuf],
    cache: &HostBinCache,
    crates: &[&str],
    out_dir: &Path,
) -> Result<(Vec<PathBuf>, BTreeSet<String>)> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    let mut files = Vec::with_capacity(objects.len());
    let mut defined: HashMap<String, usize> = HashMap::new();
    for path in objects {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        for (name, _) in text_definitions(&bytes).with_context(|| format!("parse {}", path.display()))? {
            *defined.entry(name).or_default() += 1;
        }
        files.push((path, bytes));
    }
    let roots: BTreeSet<String> = defined
        .into_iter()
        .filter(|(name, count)| {
            *count == 1
                && detour_candidate(name, crates)
                && cache.symbols.get(name).is_some_and(|s| s.kind == SymbolKind::Text)
        })
        .map(|(name, _)| name)
        .collect();
    let mut out = Vec::with_capacity(files.len());
    for (i, (path, mut bytes)) in files.into_iter().enumerate() {
        if cache.format == object::BinaryFormat::Elf {
            super::elf::retain_roots(&mut bytes, &roots).with_context(|| format!("mark {}", path.display()))?;
        } else {
            set_no_dead_strip(&mut bytes, &roots).with_context(|| format!("mark {}", path.display()))?;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let to = out_dir.join(format!("{i:04}-{name}"));
        std::fs::write(&to, &bytes).with_context(|| format!("write {}", to.display()))?;
        out.push(to);
    }
    Ok((out, roots))
}

/// Every symbol defined in a text section of an object, with its
/// symbol-table index.
fn text_definitions(bytes: &[u8]) -> Result<Vec<(String, usize)>> {
    let file = object::File::parse(bytes)?;
    let mut out = Vec::new();
    for sym in file.symbols() {
        if sym.is_undefined() || sym.kind() != SymbolKind::Text {
            continue;
        }
        let Ok(name) = sym.name() else { continue };
        if name.is_empty() {
            continue;
        }
        out.push((name.to_string(), sym.index().0));
    }
    Ok(out)
}

fn read_u32(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).context("truncated Mach-O")
}

/// Set `N_NO_DEAD_STRIP` on every section-defined symbol named in
/// `roots`, in place.
fn set_no_dead_strip(bytes: &mut [u8], roots: &BTreeSet<String>) -> Result<()> {
    if read_u32(bytes, 0)? != MH_MAGIC_64 {
        bail!("not a 64-bit Mach-O object");
    }
    let ncmds = read_u32(bytes, 16)? as usize;
    let mut at = 32usize;
    let mut symtab = None;
    for _ in 0..ncmds {
        let cmd = read_u32(bytes, at)?;
        let size = read_u32(bytes, at + 4)? as usize;
        if cmd == LC_SYMTAB {
            symtab = Some((
                read_u32(bytes, at + 8)? as usize,
                read_u32(bytes, at + 12)? as usize,
                read_u32(bytes, at + 16)? as usize,
                read_u32(bytes, at + 20)? as usize,
            ));
        }
        at += size;
    }
    let Some((symoff, nsyms, stroff, strsize)) = symtab else { return Ok(()) };
    for i in 0..nsyms {
        let entry = symoff + i * 16;
        let strx = read_u32(bytes, entry)? as usize;
        let n_type = *bytes.get(entry + 4).context("truncated nlist")?;
        if n_type & N_STAB != 0 || n_type & N_TYPE != N_SECT || strx >= strsize {
            continue;
        }
        let names = &bytes[stroff + strx..stroff + strsize];
        let end = names.iter().position(|&b| b == 0).unwrap_or(names.len());
        let Ok(name) = std::str::from_utf8(&names[..end]) else { continue };
        if roots.contains(name) {
            let desc = u16::from_le_bytes([bytes[entry + 6], bytes[entry + 7]]) | N_NO_DEAD_STRIP;
            bytes[entry + 6..entry + 8].copy_from_slice(&desc.to_le_bytes());
        }
    }
    Ok(())
}

/// Pair each root's address in the binary with its address in the
/// linked patch, with how many bytes the binary's function has.
///
/// A function shorter than the agent's long jump (16 bytes) is left out
/// when its patch copy is the same size and byte for byte the same: it
/// did not change, and its neighbour would be overwritten. One that
/// differs stays in, and the agent then needs the patch within `b` reach
/// of it, or refuses the patch.
fn pair(dylib: &Path, cache: &HostBinCache, base: &TextIndex, roots: &BTreeSet<String>) -> Result<Pairs> {
    let data = std::fs::read(dylib).with_context(|| format!("read {}", dylib.display()))?;
    let patch_text = TextIndex::of(dylib, &data)?;
    let file = object::File::parse(&*data).with_context(|| format!("parse {}", dylib.display()))?;
    let text: std::collections::HashSet<object::SectionIndex> = file
        .sections()
        .filter(|s| s.kind() == object::SectionKind::Text)
        .map(|s| s.index())
        .collect();
    let mut in_patch: BTreeMap<&str, u64> = BTreeMap::new();
    let mut anchor = 0u64;
    for sym in file.symbols() {
        let Ok(name) = sym.name() else { continue };
        if name == "_main" || (file.format() == object::BinaryFormat::Elf && name == "main") {
            anchor = sym.address();
        }
        if sym.section_index().is_some_and(|i| text.contains(&i)) {
            if let Some(root) = roots.get(name) {
                in_patch.insert(root.as_str(), sym.address());
            }
        }
    }
    anyhow::ensure!(anchor != 0, "the patch has no `main` anchor");
    let mut pairs = Vec::with_capacity(roots.len());
    for root in roots {
        let base_addr = cache.symbols.get(root).map(|s| s.address).context("a root left the binary's symbols")?;
        let patch = *in_patch.get(root.as_str()).with_context(|| {
            format!("`{root}` was rooted but is not in the linked patch")
        })?;
        let size = base.size(base_addr);
        if size < agent::arm64::LONG_JUMP
            && patch_text.size(patch) == size
            && base.bytes(base_addr, None)? == patch_text.bytes(patch, Some(&data))?
        {
            continue;
        }
        pairs.push(agent::proto::Pair { base: base_addr, size, patch });
    }
    Ok(Pairs { anchor, pairs })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CrewForge A4: the concurrent web hot patch slowed to 13.6–19.9 s
    /// because a patch's replay ran at normal priority even though the
    /// server's `cargo build` was niced. The whole path through a patch: a
    /// fake rustc reports the niceness it was started at.
    #[cfg(unix)]
    #[test]
    fn regression_a_server_patch_replays_at_the_builders_niceness() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let rustc = tmp.path().join("fake-rustc.sh");
        let report = tmp.path().join("nice.txt");
        std::fs::write(&rustc, format!("#!/bin/sh\nps -o nice= -p $$ | tr -d ' ' > {}\n", report.display())).unwrap();
        std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).unwrap();
        let patch_crate = || PatchCrate {
            crate_name: "app".into(),
            capture: CapturedInvocation {
                rustc: rustc.display().to_string(),
                args: vec![
                    "--crate-name".into(),
                    "app".into(),
                    "--out-dir".into(),
                    tmp.path().join("deps").display().to_string(),
                ],
                cwd: tmp.path().display().to_string(),
                env: vec![("PATH".into(), std::env::var("PATH").unwrap_or_default())],
            },
        };
        let exe = std::env::current_exe().unwrap();
        let replayed_at = |builder: ServerPatchBuilder| {
            // The fake compiles nothing, so the patch stops after its replay.
            let err = builder.build(&[patch_crate()], 0, &|| false, &|_, _| {}).err().expect("no objects");
            assert!(format!("{err:#}").contains("produced no objects"), "{err:#}");
            std::fs::read_to_string(&report).unwrap().trim().parse::<i32>().unwrap()
        };
        let base = replayed_at(ServerPatchBuilder::new(&exe, tmp.path().join("patches")).unwrap());
        let niced = ServerPatchBuilder::new(&exe, tmp.path().join("patches")).unwrap().with_replay_nice(7);
        assert_eq!(replayed_at(niced), (base + 7).min(19));
    }

    // Real v0 symbols (Mach-O spelling) from a probe crate named
    // `detour_probe`, as `nm` lists them.
    const PLAIN_FN: &str = "__RNvCs7YUasx9yL1O_12detour_probe6detour";
    const CLOSURE: &str = "__RNCNvCs7YUasx9yL1O_12detour_probe4main0B3_";
    const STD_GENERIC_OVER_US: &str =
        "__RINvMNtCsgtPOCBgevO_4core6optionINtB3_6OptionNtNtCslJsSqIQKwiA_5alloc6string6StringE3mapjNCNvCs7YUasx9yL1O_12detour_probe4main0EB1t_";

    #[test]
    fn a_plain_function_of_the_crate_is_redirected() {
        assert!(detour_candidate(PLAIN_FN, &["detour_probe"]));
    }

    /// ELF spells the same symbols without Mach-O's leading underscore;
    /// stripping one anyway left `RNv…`, which is no Rust symbol, and no
    /// Linux function was ever redirected.
    #[test]
    fn the_elf_spelling_of_a_function_is_redirected_too() {
        assert!(detour_candidate(&PLAIN_FN[1..], &["detour_probe"]));
        assert!(!detour_candidate(&CLOSURE[1..], &["detour_probe"]));
        assert!(!detour_candidate("main", &["detour_probe"]));
        assert!(!detour_candidate("__rust_alloc", &["detour_probe"]));
    }

    #[test]
    fn a_function_of_another_crate_is_not() {
        assert!(!detour_candidate(PLAIN_FN, &["other"]));
    }

    /// A closure or `async` body is resumed by the code that built it;
    /// redirecting it would run a new body over an old state.
    #[test]
    fn a_closure_or_coroutine_is_never_redirected() {
        assert!(!detour_candidate(CLOSURE, &["detour_probe"]));
    }

    #[test]
    fn a_generic_instantiation_mentioning_the_crate_is_not_redirected() {
        assert!(!detour_candidate(STD_GENERIC_OVER_US, &["detour_probe"]));
    }

    /// `<str>::parse::<usize>`, instantiated in the crate but not about
    /// it: neither generic nor owned.
    #[test]
    fn a_foreign_generic_instantiated_in_the_crate_is_not_redirected() {
        let parse = "__RINvMNtCsgtPOCBgevO_4core3stre5parsejECs7YUasx9yL1O_12detour_probe";
        assert!(!detour_candidate(parse, &["detour_probe"]));
    }

    /// What may be copied into a patch from an upstream rlib: anything a patch
    /// can never have redirected.
    #[test]
    fn a_shared_instantiation_is_not_plain_and_a_plain_function_is() {
        assert!(plain_function(PLAIN_FN));
        assert!(plain_function(&PLAIN_FN[1..]));
        assert!(!plain_function(CLOSURE));
        assert!(!plain_function(STD_GENERIC_OVER_US));
        // `<usize as SliceIndex<[f32]>>::get`, instantiated in `up`.
        let trait_generic = "_RNvXs0_NtNtCskxtBwYRO61I_4core5slice5indexjINtB5_10SliceIndexSfE3getCs7bn3dqLlFam_2up";
        assert!(!plain_function(trait_generic));
        // `<probe::Thing as From<u32>>::from`: no instantiating crate, editable.
        let trait_impl = "_RNvXCs7YUasx9yL1O_12detour_probeNtB2_5ThingINtNtCskxtBwYRO61I_4core7convert4FrommE4from";
        assert!(plain_function(trait_impl));
    }

    #[test]
    fn unmangled_names_are_left_alone() {
        assert!(!detour_candidate("_main", &["detour_probe"]));
        assert!(!detour_candidate("___rust_alloc", &["detour_probe"]));
    }

    #[test]
    fn the_angle_matcher_skips_nested_brackets() {
        assert_eq!(matching_angle("a<b<c>> as T>::m"), Some(12));
        assert_eq!(matching_angle("unclosed<"), None);
    }
}
