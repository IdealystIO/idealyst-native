//! Build one wasm patch, from a saved source file to a module the page
//! can instantiate.
//!
//! Four steps, each a measured cost on a save:
//!
//! 1. **Replay.** Each re-emitted crate's exact rustc invocation,
//!    captured during the base build, re-run with `--emit=obj` and
//!    `-Crelocation-model=pic`. Those crates' codegen — not the graph's.
//! 2. **Link.** `wasm-ld --pie --experimental-pic` over those objects
//!    alone, with `--allow-undefined`, so every reference into a crate
//!    the patch did not recompile comes out as an import.
//! 3. **Resolve.** [`crate::hotpatch_prepare::prepare`] decides where
//!    each of those imports comes from and appends that plan to the
//!    module; the page's loader supplies them. Nothing is rewritten.
//! 4. **Pair.** [`crate::hotpatch_wasm::build_jump_table`] matches every
//!    function the patch takes the address of to the base's table slot
//!    for it.
//!
//! # Why the invocation is replayed rather than re-derived
//!
//! Cargo folds `RUSTFLAGS` into the `-Cmetadata` it gives each crate,
//! `-Cmetadata` seeds the symbol mangling hash, and the jump table pairs
//! symbols BY NAME. Running `cargo rustc` again with an extra flag would
//! therefore produce a patch whose every symbol hashes differently from
//! the base's, pairing with nothing and reporting no error. Replaying the
//! captured argv keeps `-Cmetadata` byte-identical; the one flag we add,
//! `-Crelocation-model=pic`, goes on the replayed command line where
//! cargo's fingerprint cannot see it.
//!
//! # Several crates, one patch
//!
//! A save in a library crate of the app's workspace re-emits that crate
//! AND every workspace crate depending on it (`dev_overlay::workspace`
//! says why: a dependent calls the library directly, and only a crate
//! the patch re-emits calls the new body). [`WasmPatchBuilder::build_crates`]
//! links all of their objects into ONE module rather than one patch per
//! crate applied in order, because a patch REPLACES the page's jump table
//! rather than adding to it: a second module carrying only the dependent
//! would pair the library's functions with nothing and send them back to
//! the base. One module, one table, every crate's slots in it. The same
//! reason makes the dev loop carry every crate patched since the base in
//! each later patch, even one the save did not touch.
//!
//! The replays are independent — each reads its dependencies' METADATA
//! from the base build, never from another replay, since a replay emits
//! objects only — so they run concurrently, and a save pays for the
//! slowest one rather than the sum. A crate that is in the patch only to
//! carry another's edit, and whose sources have not moved since its last
//! replay against this base, compiles to the same objects; those are
//! reused rather than replayed (see [`PatchCrate::source_key`]).
//!
//! # Why every failure here is a rebuild, not a warning
//!
//! A patch that half-applies is worse than no patch: the page keeps
//! running, dispatches through a table that points somewhere arbitrary,
//! and reports nothing. So each step returns an error naming what it
//! could not do, and the caller's answer is the rebuild-and-reload it
//! would have done anyway.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use build_runtime_server::hotpatch::replay;
/// What [`WasmPatchBuilder::build_crates_cancellable`] fails with when it
/// was cancelled rather than broken.
pub use build_runtime_server::hotpatch::replay::Cancelled;

use crate::hotpatch_patch::BaseIndex;
use crate::hotpatch_wasm::{base_slots, build_jump_table_with, WasmJumpTable};

/// One crate a patch re-emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchCrate {
    /// rustc's `--crate-name`, which is what a capture is keyed by.
    pub crate_name: String,
    /// A digest of the sources this patch must reflect for the crate
    /// (`dev_overlay`'s `DescriptorSet::build_key`: every file's
    /// content). Objects from an earlier replay of the same crate under
    /// the same key, against the same base, are the objects a replay
    /// would write now, and are reused. `None` always replays.
    pub source_key: Option<String>,
    /// Whether the save edited this crate. A crate with no capture in
    /// this base was never compiled into the wasm program — a host-only
    /// crate the workspace's closure reaches through a feature the web
    /// build does not turn on (CrewForge's `crewforge-api-server`). One
    /// carried only as a dependent is skipped; an EDITED one fails the
    /// patch, and the rebuild decides whether the edit matters.
    pub edited: bool,
}

impl PatchCrate {
    pub fn new(crate_name: impl Into<String>, source_key: Option<String>) -> Self {
        Self { crate_name: crate_name.into(), source_key, edited: true }
    }

    /// A crate re-emitted only to carry another's edit.
    pub fn carried(crate_name: impl Into<String>, source_key: Option<String>) -> Self {
        Self { crate_name: crate_name.into(), source_key, edited: false }
    }
}

/// A patch ready to be served and applied.
#[derive(Debug, Clone)]
pub struct BuiltPatch {
    /// Where the patch module was written.
    pub path: PathBuf,
    /// Base table index → patch element index, plus how far to grow the
    /// table.
    pub jump_table: WasmJumpTable,
    /// Per-step wall time, for the `[hotpatch]` timing line. A save that
    /// stops being subsecond should say which step grew.
    pub timings: Vec<(&'static str, Duration)>,
    /// Each crate the patch carries, in link order, with its replay's
    /// wall time — or `None` when its objects were reused.
    pub crates: Vec<(String, Option<Duration>)>,
    /// Crates of the plan left out because this base never compiled
    /// them (see [`PatchCrate::edited`]).
    pub skipped: Vec<String>,
}

impl BuiltPatch {
    pub fn total(&self) -> Duration {
        self.timings.iter().map(|(_, d)| *d).sum()
    }

    /// Each step's wall time, as event data.
    pub fn steps(&self) -> Vec<dev_events::Timing> {
        self.timings.iter().map(|(name, d)| dev_events::Timing::new(*name, *d)).collect()
    }

    /// Each carried crate's replay time (`None`: objects reused).
    pub fn crate_timings(&self) -> Vec<dev_events::CrateTiming> {
        self.crates
            .iter()
            .map(|(name, d)| dev_events::CrateTiming {
                name: name.clone(),
                ms: d.map(|d| d.as_millis() as u64),
            })
            .collect()
    }

    /// Bytes of the module the page fetches.
    pub fn bytes(&self) -> u64 {
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    /// The `[hotpatch]` line's timing summary. Several crates replay
    /// concurrently, so `cargo` is the slowest of them, and the per-crate
    /// list says which. Formatted by `dev_events::plain`, which renders
    /// the same summary from a `PatchBuilt` event — one formatter, so the
    /// two cannot drift.
    pub fn timing_line(&self) -> String {
        dev_events::plain::patch_timing_line(&self.steps(), &self.crate_timings(), &self.skipped)
    }
}

/// Objects a replay wrote, and the source key they were compiled from.
type ObjectCache = std::collections::HashMap<String, (String, Vec<PathBuf>)>;

/// The background replays that warm the replay cache after a base build:
/// see [`WasmPatchBuilder::seed_all`].
///
/// A queue of crates replayed with no edit, a few at a time, at the
/// lowest CPU priority. A save comes first: [`Self::pause_for`] stops new
/// seeds, kills the running ones the save does not need (they go back on
/// the queue), and waits for the ones it does — killing those would only
/// make the save replay them cold. [`Self::resume`] starts them again
/// when the patch is built.
struct SeedQueue {
    shared: Arc<(Mutex<SeedState>, Condvar)>,
    workers: std::cell::RefCell<Vec<std::thread::JoinHandle<()>>>,
    captures_dir: PathBuf,
    reporter: dev_events::Reporter,
    jobs: usize,
}

#[derive(Default)]
struct SeedState {
    queue: std::collections::VecDeque<PatchCrate>,
    /// Each running seed and the flag that kills it.
    running: Vec<(PatchCrate, Arc<AtomicBool>)>,
    /// Finished seeds, not yet filed into the builder's object cache.
    done: Vec<(PatchCrate, Result<(Vec<PathBuf>, Duration)>)>,
    /// Every crate queued, seeded or replayed against this base: never
    /// queued again.
    known: std::collections::HashSet<String>,
    paused: bool,
    stopped: bool,
    /// The current run of seeds, for the one line that reports it.
    run: Option<SeedRun>,
}

struct SeedRun {
    started: Instant,
    seeded: usize,
    slowest: Option<(String, Duration)>,
}

impl SeedQueue {
    fn new(captures_dir: &Path, reporter: dev_events::Reporter) -> Self {
        Self {
            shared: Default::default(),
            workers: Default::default(),
            captures_dir: captures_dir.to_path_buf(),
            reporter,
            jobs: seed_jobs(std::env::var("IDEALYST_HOTPATCH_SEED_JOBS").ok().as_deref(), available_cores()),
        }
    }

    /// Queue `crates`, in order, skipping any already known.
    fn push(&self, crates: Vec<PatchCrate>) {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap();
        for krate in crates {
            if st.known.insert(krate.crate_name.clone()) {
                st.queue.push_back(krate);
            }
        }
        if st.queue.is_empty() {
            return;
        }
        st.run.get_or_insert_with(|| SeedRun { started: Instant::now(), seeded: 0, slowest: None });
        drop(st);
        let mut workers = self.workers.borrow_mut();
        while workers.len() < self.jobs {
            let shared = self.shared.clone();
            let captures = self.captures_dir.clone();
            let reporter = self.reporter.clone();
            workers.push(std::thread::spawn(move || seed_worker(&shared, &captures, &reporter)));
        }
        cv.notify_all();
    }

    /// Make way for a patch of `plan`: see the type docs. Returns with no
    /// seed running.
    fn pause_for(&self, plan: &[PatchCrate]) {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap();
        st.paused = true;
        let needed = |name: &str| plan.iter().any(|k| k.crate_name == name);
        for (krate, kill) in &st.running {
            if !needed(&krate.crate_name) {
                kill.store(true, Ordering::SeqCst);
            }
        }
        // The save replays these itself, which warms them.
        st.queue.retain(|k| !needed(&k.crate_name));
        for k in plan {
            st.known.insert(k.crate_name.clone());
        }
        while !st.running.is_empty() {
            st = cv.wait(st).unwrap();
        }
    }

    fn resume(&self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap().paused = false;
        cv.notify_all();
    }

    /// File every finished seed's objects under its source key, so a patch
    /// that carries the crate unchanged reuses them.
    fn file_into(&self, objects: &std::cell::RefCell<ObjectCache>) {
        let done = std::mem::take(&mut self.shared.0.lock().unwrap().done);
        let mut cache = objects.borrow_mut();
        for (krate, result) in done {
            match (result, krate.source_key) {
                (Ok((objs, _)), Some(key)) => {
                    cache.insert(krate.crate_name, (key, objs));
                }
                // Reported by the worker; a failed seed's dir is empty.
                _ => {
                    cache.remove(&krate.crate_name);
                }
            }
        }
    }

    /// Kill every seed and wait for the workers: the base is going away,
    /// and the next builder must not replay a crate a seed still is.
    fn stop(&self) {
        {
            let (lock, cv) = &*self.shared;
            let mut st = lock.lock().unwrap();
            st.stopped = true;
            for (_, kill) in &st.running {
                kill.store(true, Ordering::SeqCst);
            }
            cv.notify_all();
        }
        for w in self.workers.borrow_mut().drain(..) {
            let _ = w.join();
        }
    }
}

fn seed_worker(shared: &(Mutex<SeedState>, Condvar), captures: &Path, reporter: &dev_events::Reporter) {
    let (lock, cv) = shared;
    let mut st = lock.lock().unwrap();
    loop {
        if st.stopped {
            return;
        }
        let next = if st.paused { None } else { st.queue.pop_front() };
        let Some(krate) = next else {
            st = cv.wait(st).unwrap();
            continue;
        };
        let kill = Arc::new(AtomicBool::new(false));
        st.running.push((krate.clone(), kill.clone()));
        drop(st);
        // A crate this base never compiled (a host-only one the workspace
        // closure reaches) has nothing to seed; said nowhere, as a patch
        // that carries it says nothing either.
        let result = if replay::find_capture(captures, &krate.crate_name).is_ok() {
            Some(replay_crate_at(captures, &krate.crate_name, &|| kill.load(Ordering::SeqCst), true))
        } else {
            None
        };
        st = lock.lock().unwrap();
        st.running.retain(|(k, _)| k.crate_name != krate.crate_name);
        match result {
            None => {}
            // Killed to make way for a save: seeded after it.
            Some(Err(e)) if e.is::<replay::Cancelled>() => {
                if !st.stopped {
                    st.queue.push_front(krate);
                }
            }
            Some(result) => {
                match &result {
                    Ok((_, took)) => {
                        if let Some(run) = st.run.as_mut() {
                            run.seeded += 1;
                            if run.slowest.as_ref().is_none_or(|(_, d)| took > d) {
                                run.slowest = Some((krate.crate_name.clone(), *took));
                            }
                        }
                    }
                    // Not fatal: the first save that needs the crate
                    // replays it cold, and says why if it fails too.
                    Err(e) => reporter.warn(
                        "hotpatch",
                        format!("seeding the replay cache for `{}` failed: {e:#}", krate.crate_name),
                    ),
                }
                st.done.push((krate, result));
            }
        }
        if st.queue.is_empty() && st.running.is_empty() {
            if let Some(run) = st.run.take().filter(|r| r.seeded > 0) {
                let slowest = run
                    .slowest
                    .map(|(n, d)| format!(", slowest `{n}` {} ms", d.as_millis()))
                    .unwrap_or_default();
                // The session log is how anyone knows the next save is warm.
                reporter.log(
                    "hotpatch",
                    format!(
                        "replay cache seeded for {} crate(s) in {} ms{slowest}",
                        run.seeded,
                        run.started.elapsed().as_millis()
                    ),
                );
            }
        }
        cv.notify_all();
    }
}

/// How many seeds run at once: `IDEALYST_HOTPATCH_SEED_JOBS` when it is a
/// positive number, else a quarter of the cores (at least one, at most
/// four). Each replay is itself parallel (256 codegen units), so a few
/// fill the machine; more only add memory.
fn seed_jobs(env: Option<&str>, cores: usize) -> usize {
    env.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| (cores / 4).clamp(1, 4))
}

fn available_cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Everything that does not change between patches, resolved once so a
/// save only pays for the four steps.
///
/// The base index in particular is the expensive part — a pass over a
/// debug-profile wasm module with every function in its table — and it
/// is valid for the life of one base build.
pub struct WasmPatchBuilder {
    captures_dir: PathBuf,
    aliases: crate::hotpatch_aliases::AliasMap,
    /// The app crate, for [`Self::build`].
    crate_name: String,
    out_dir: PathBuf,
    wasm_ld: PathBuf,
    base: BaseIndex,
    /// The base's table by name, for pairing. Computed once, like
    /// `base`: both describe the base, which does not change between
    /// patches.
    base_slots: std::collections::BTreeMap<String, u32>,
    serial: std::cell::Cell<u64>,
    /// Per crate, the objects its last replay against THIS base wrote.
    /// Valid for the builder's life, which is one base's: a rebuild
    /// drops the builder and with it every entry.
    objects: std::cell::RefCell<ObjectCache>,
    seeds: SeedQueue,
    reporter: dev_events::Reporter,
}

impl Drop for WasmPatchBuilder {
    /// Seeds still running when the base is replaced are killed and waited
    /// for before the next builder can seed or replay the same crate: both
    /// would run in one incremental session and one object dir.
    fn drop(&mut self) {
        self.seeds.stop();
    }
}

impl WasmPatchBuilder {
    /// `served_wasm` must be the module the BROWSER actually runs — in
    /// hybrid mode after wasm-bindgen and after the command-export
    /// neutralize pass, which both rewrite the module (a table index read
    /// before either one names a different function than the one the page
    /// will call); in own mode the prepared base `pkg/` holds as written.
    /// `symbol_aliases` is the file the base build wrote from the LINKED
    /// module (see [`crate::hotpatch_aliases`]). A base built without one
    /// still patches; it just cannot resolve a symbol the linker knew by
    /// a second name. `data_symbols` is its sibling, the base's static
    /// addresses (see [`crate::hotpatch_aliases::read_data_symbols_from_linked`]);
    /// without it every `GOT.mem` import is unresolved and a patch that
    /// touches a static falls back to a rebuild.
    pub fn new(
        served_wasm: &Path,
        symbol_aliases: Option<&Path>,
        data_symbols: Option<&Path>,
        captures_dir: impl Into<PathBuf>,
        crate_name: impl Into<String>,
        out_dir: impl Into<PathBuf>,
    ) -> Result<Self> {
        let base_wasm = std::fs::read(served_wasm)
            .with_context(|| format!("read the served base module {}", served_wasm.display()))?;
        let aliases = match symbol_aliases {
            Some(path) => crate::hotpatch_aliases::read(path)?,
            None => Default::default(),
        };
        let data = match data_symbols {
            Some(path) => crate::hotpatch_aliases::read_data_symbols(path)?,
            None => Default::default(),
        };
        let base = BaseIndex::of(&base_wasm, &aliases, &data).with_context(|| {
            format!("indexing the served base module {}", served_wasm.display())
        })?;
        let base_slots = base_slots(&base_wasm)?;
        let captures_dir = captures_dir.into();
        let reporter = dev_events::Reporter::default();
        Ok(Self {
            seeds: SeedQueue::new(&captures_dir, reporter.clone()),
            captures_dir,
            aliases,
            crate_name: crate_name.into(),
            out_dir: out_dir.into(),
            wasm_ld: locate_wasm_ld()?,
            base,
            base_slots,
            serial: std::cell::Cell::new(0),
            objects: Default::default(),
            reporter,
        })
    }

    /// Report through `reporter` instead of plain stderr.
    pub fn reporting_to(mut self, reporter: dev_events::Reporter) -> Self {
        // Before any seed is queued, so no worker holds the old one.
        self.seeds.reporter = reporter.clone();
        self.reporter = reporter;
        self
    }

    /// Number this builder's patches after `serial`: the first one it
    /// writes is `patch-<serial + 1>.wasm`.
    ///
    /// For a dev session resuming the previous one's base, whose patches
    /// a page may still hold under their URLs: a builder counting from 1
    /// again would serve NEW code at a URL the browser has seen.
    pub fn numbered_after(self, serial: u64) -> Self {
        self.serial.set(serial);
        self
    }

    /// Every crate's replayed objects this builder would reuse, with the
    /// source key they were compiled from: `(crate, key, objects)`.
    ///
    /// For a dev session resuming this base: the objects stay on disk in
    /// each crate's replay dir until that crate is replayed again, and a
    /// resumed builder given them ([`Self::reusing_objects`]) skips the
    /// replay of every carried crate whose sources have not moved — where
    /// a fresh one replayed them all on its first patch (8 s for the 12
    /// crates a CrewForge session had patched).
    ///
    /// A seed still running is not waited for: its objects are filed by
    /// the next patch, or the next call.
    pub fn reusable_objects(&self) -> Vec<(String, String, Vec<PathBuf>)> {
        self.seeds.file_into(&self.objects);
        let mut out: Vec<_> = self
            .objects
            .borrow()
            .iter()
            .map(|(name, (key, objs))| (name.clone(), key.clone(), objs.clone()))
            .collect();
        out.sort();
        out
    }

    /// Reuse objects an earlier builder over the SAME base replayed (see
    /// [`Self::reusable_objects`]). Each is reused only under its source
    /// key, and only while every one of its files is still there.
    pub fn reusing_objects(
        self,
        entries: impl IntoIterator<Item = (String, String, Vec<PathBuf>)>,
    ) -> Self {
        self.objects
            .borrow_mut()
            .extend(entries.into_iter().map(|(name, key, objs)| (name, (key, objs))));
        self
    }

    /// The number of the last patch this builder wrote (or was numbered
    /// after).
    pub fn serial(&self) -> u64 {
        self.serial.get()
    }

    /// How many functions the base left reachable through its table.
    /// Near-zero means `hotpatch_base` did not run, and every patch is
    /// about to fail on its first `env` import.
    pub fn base_table_size(&self) -> usize {
        self.base.ifunc.len()
    }

    /// How many of those names are a second spelling of another
    /// function. Zero on a base linked without `--emit-relocs`, which is
    /// the shape that refuses ordinary patches over `<usize as
    /// Display>::fmt`.
    pub fn alias_count(&self) -> usize {
        self.aliases.len()
    }

    /// Replay each of `crates` once, in the background, with no edit, in
    /// the order given.
    ///
    /// A replay has its own incremental directory (see
    /// [`replay::replay_args`]), which the base build never fills; so the
    /// first patch of a session compiled each crate it re-emits cold —
    /// 45.7 s of a 49.4 s first save on CrewForge when only the app crate
    /// was seeded, and 30.5–35.0 s for a library crate's first patch, whose
    /// ~30 dependents all replayed cold (32.6 s for the slowest). Seeding
    /// every patchable crate right after each base build, while the page
    /// loads, moves that off the first save. A seed's objects are the
    /// unedited crate's, filed under its `source_key`, so a patch that
    /// only CARRIES a crate (every dependent of an edited library) reuses
    /// them outright instead of replaying at all.
    ///
    /// Seeds run a few at a time ([`seed_jobs`]) at the lowest CPU
    /// priority ([`replay::run_rustc_emit_obj_background`]), and a save
    /// always comes first (see [`SeedQueue`]): nobody typing waits on one.
    /// A crate already queued, seeded or replayed against this base is not
    /// queued again.
    pub fn seed_all(&self, crates: Vec<PatchCrate>) {
        self.seeds.push(crates);
    }

    /// [`Self::seed_all`] of one crate.
    pub fn seed(&self, krate: PatchCrate) {
        self.seed_all(vec![krate]);
    }

    /// A patch of the app crate alone — [`Self::build_crates`] over the
    /// crate this builder was made for.
    pub fn build(&self) -> Result<BuiltPatch> {
        self.build_crates(&[PatchCrate::new(self.crate_name.clone(), None)])
    }

    /// A patch re-emitting every crate in `crates`, linked into one
    /// module. See the module docs for why one module, and why the
    /// replays run concurrently.
    pub fn build_crates(&self, crates: &[PatchCrate]) -> Result<BuiltPatch> {
        self.build_crates_cancellable(crates, &|| false)
    }

    /// [`Self::build_crates`], abandoned as soon as `cancel` returns true:
    /// the dev loop's answer to a newer save arriving mid-patch (the
    /// patch would describe sources that are already stale).
    ///
    /// A running replay is killed ([`replay::run_rustc_emit_obj_cancellable`]
    /// says why that is safe); between steps `cancel` is asked again, so
    /// a patch is never written once it is known to be stale. The error is
    /// [`replay::Cancelled`] (`err.is::<replay::Cancelled>()`). What a
    /// cancelled build leaves behind is consistent: a killed crate's cache
    /// entry is dropped with its object dir (the next replay empties the
    /// dir), a crate that finished is cached as usual, and no patch file
    /// is written.
    pub fn build_crates_cancellable(
        &self,
        crates: &[PatchCrate],
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<BuiltPatch> {
        if crates.is_empty() {
            bail!("a patch of no crates");
        }
        let checkpoint = || -> Result<()> {
            if cancel() {
                return Err(replay::Cancelled.into());
            }
            Ok(())
        };
        let mut timings = Vec::new();

        // Seeds make way: the ones this patch needs are waited for (their
        // wait is part of this save), the rest are killed and run again
        // once the patch is built, whatever becomes of it.
        let started = Instant::now();
        self.seeds.pause_for(crates);
        struct Resume<'a>(&'a SeedQueue);
        impl Drop for Resume<'_> {
            fn drop(&mut self) {
                self.0.resume();
            }
        }
        let _resume = Resume(&self.seeds);
        self.seeds.file_into(&self.objects);
        let (in_build, skipped) = in_this_build(&self.captures_dir, crates)?;
        checkpoint()?;
        let replayed = collect_objects(&self.captures_dir, &self.objects, &in_build, cancel)?;
        let mut objects = Vec::new();
        let mut carried = Vec::with_capacity(in_build.len());
        for (krate, (objs, took)) in in_build.iter().zip(replayed) {
            objects.extend(objs);
            carried.push((krate.crate_name.clone(), took));
        }
        timings.push(("cargo", started.elapsed()));
        checkpoint()?;

        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        std::fs::create_dir_all(&self.out_dir)
            .with_context(|| format!("create {}", self.out_dir.display()))?;
        let linked = self.out_dir.join(format!("patch-{serial}.linked.wasm"));

        let started = Instant::now();
        self.link(&objects, &linked)?;
        timings.push(("link", started.elapsed()));
        if let Err(e) = checkpoint() {
            let _ = std::fs::remove_file(&linked);
            return Err(e);
        }

        let started = Instant::now();
        let raw = std::fs::read(&linked)
            .with_context(|| format!("read the linked patch {}", linked.display()))?;
        // Read, not rewritten: the page supplies each import as the plan
        // appended here says (see `hotpatch_prepare`).
        let resolved = crate::hotpatch_prepare::prepare(&raw, &self.base)?.wasm;
        // The pre-resolve module is only useful when a patch misbehaves,
        // and it is larger than the one we serve. Keeping every one of
        // them fills a dev session's staging dir.
        let _ = std::fs::remove_file(&linked);
        timings.push(("resolve", started.elapsed()));
        checkpoint()?;

        // Pair while the names are still there: the jump table matches
        // functions BY NAME, from the `name` section.
        let started = Instant::now();
        let jump_table = build_jump_table_with(&self.base_slots, &resolved, &self.aliases)
            .context("pairing the patch's functions to the base's table slots")?;
        if jump_table.is_empty() {
            bail!(
                "the patch redirects nothing: not one of the functions it takes the address of \
                 matched a slot in the base's table. Usually the base was built without \
                 `runtime-core/hot-reload`, so `#[component]` never split any body and there is \
                 nothing pointing a `fn` pointer at one."
            );
        }
        timings.push(("jump-table", started.elapsed()));

        // Then serve it without them. The `name` section is more than half
        // of a real patch (33 of 60 MB on CrewForge) and nothing at run
        // time reads it: pairing is done, and the page loads by table
        // index and import order. What it costs is function names in a stack trace through
        // the patch, so the named module is kept on disk beside the build
        // (`last-patch.named.wasm`), and `IDEALYST_HOTPATCH_KEEP_NAMES=1`
        // serves it instead when a trace has to be read in the browser.
        let started = Instant::now();
        let path = self.out_dir.join(format!("patch-{serial}.wasm"));
        let keep_names = std::env::var_os("IDEALYST_HOTPATCH_KEEP_NAMES").is_some();
        let served = if keep_names {
            resolved.clone()
        } else {
            crate::hotpatch_patch::strip_debug_sections(&resolved)?
        };
        std::fs::write(&path, &served)
            .with_context(|| format!("write the patch {}", path.display()))?;
        if let Some(debug_dir) = self.captures_dir.parent() {
            let _ = std::fs::write(debug_dir.join("last-patch.named.wasm"), &resolved);
        }
        // Only the newest patch is ever fetched again; the one before it
        // is kept in case a page is still fetching it. Every older one is
        // tens of megabytes of staging dir for nothing (228 MB after four
        // CrewForge saves).
        if serial > 2 {
            for old in 1..serial - 1 {
                let _ = std::fs::remove_file(self.out_dir.join(format!("patch-{old}.wasm")));
            }
        }
        timings.push(("strip+write", started.elapsed()));

        Ok(BuiltPatch {
            path,
            jump_table,
            timings,
            crates: carried,
            skipped,
        })
    }

    /// Link the patch's objects — and ONLY those — into a PIC side
    /// module.
    ///
    /// Everything the objects reference but do not define is left
    /// undefined on purpose: `--allow-undefined` turns each into an
    /// import, which is exactly the set `hotpatch_prepare` then plans
    /// against the running module. Linking the rlibs instead
    /// would produce a self-contained module that duplicates the whole
    /// program in the page's memory.
    fn link(&self, objects: &[PathBuf], out: &Path) -> Result<()> {
        let mut cmd = Command::new(&self.wasm_ld);
        cmd.args(PATCH_LINK_ARGS);
        cmd.arg("-o").arg(out);
        cmd.args(objects);

        let output = cmd
            .output()
            .with_context(|| format!("exec {} — the patch link", self.wasm_ld.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "linking the patch failed ({}).\n{}\n\
                 An undefined symbol here usually means the edit calls something rustc never \
                 codegened into the base — a rebuild picks it up.",
                output.status,
                stderr.trim(),
            );
        }
        Ok(())
    }
}

/// The patch link's flags, in one place so the roundtrip test links
/// with exactly what ships.
pub const PATCH_LINK_ARGS: &[&str] = &[
    // The patch is a library, not a program.
    "--no-entry",
    // Resolve against the page, not against ourselves.
    "--allow-undefined",
    "--import-memory",
    "--import-table",
    // `apply_patch` grows the table before instantiating; a
    // fixed-size import would make that throw.
    "--growable-table",
    // Position-independent, which is what makes `__memory_base`
    // and `__table_base` the offsets everything is written
    // relative to.
    "--pie",
    "--experimental-pic",
    // The jump table pairs symbols by name against a base linked
    // `--no-demangle`. Demangling one side pairs nothing.
    "--no-demangle",
    // Without this the linker strips every function in the
    // patch: nothing is exported and nothing is an entry point,
    // so from its point of view the whole module is dead. The
    // functions we want are precisely the ones only the jump
    // table will ever reach.
    "--no-gc-sections",
    // No DWARF in the linked patch. walrus drops it on the
    // resolve emit anyway, so the served module never carried
    // any; linking it only cost time. Measured on CrewForge:
    // link 0.73 -> 0.34 s, linked file 92 -> 63 MB (less for
    // resolve to read). The `name` section is not debug info
    // and survives, which is what the jump table pairs on. The
    // BASE link is untouched.
    "--strip-debug",
];

/// Each crate's objects, in `crates`' order: reused where the cache
/// holds a replay of the same sources, replayed — all at once —
/// otherwise. The `Duration` is the replay's, `None` for a reuse.
fn collect_objects(
    captures_dir: &Path,
    objects: &std::cell::RefCell<ObjectCache>,
    crates: &[PatchCrate],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<(Vec<PathBuf>, Option<Duration>)>> {
    let mut out: Vec<Option<(Vec<PathBuf>, Option<Duration>)>> = vec![None; crates.len()];
    let mut to_replay = Vec::new();
    {
        let cache = objects.borrow();
        for (i, krate) in crates.iter().enumerate() {
            let cached = krate.source_key.as_ref().and_then(|key| {
                cache
                    .get(&krate.crate_name)
                    .filter(|(k, objs)| k == key && objs.iter().all(|o| o.is_file()))
            });
            match cached {
                Some((_, objs)) => out[i] = Some((objs.clone(), None)),
                None => to_replay.push(i),
            }
        }
    }

    // Concurrently: no replay reads another's output (each reads its
    // dependencies' metadata from the base build), each writes into a
    // private per-crate object dir, and each crate has its own
    // session under the shared incremental dir.
    let results: Vec<(usize, Result<(Vec<PathBuf>, Duration)>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = to_replay
            .iter()
            .map(|&i| {
                let name = crates[i].crate_name.clone();
                let captures = captures_dir.to_path_buf();
                (i, scope.spawn(move || replay_crate(&captures, &name, cancel)))
            })
            .collect();
        handles
            .into_iter()
            .map(|(i, h)| {
                let r = h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("the replay thread panicked")));
                (i, r)
            })
            .collect()
    });

    let mut cache = objects.borrow_mut();
    let mut first_error = None;
    for (i, result) in results {
        let krate = &crates[i];
        match result {
            Ok((objs, took)) => {
                match &krate.source_key {
                    Some(key) => {
                        cache.insert(krate.crate_name.clone(), (key.clone(), objs.clone()));
                    }
                    None => {
                        cache.remove(&krate.crate_name);
                    }
                }
                out[i] = Some((objs, Some(took)));
            }
            Err(e) => {
                // A failed (or killed) replay emptied that crate's object
                // dir, so whatever the cache said about it is gone too.
                cache.remove(&krate.crate_name);
                // A cancellation outranks a failure: when a newer save
                // killed the replays, one that failed on the way down is
                // not what the caller should report.
                if e.is::<replay::Cancelled>() {
                    first_error = Some(e);
                } else if !first_error.as_ref().is_some_and(|f| f.is::<replay::Cancelled>()) {
                    first_error.get_or_insert(e);
                }
            }
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(out.into_iter().map(|o| o.expect("every crate reused or replayed")).collect())
}

/// Split a plan's crates into those this base compiled (a capture
/// exists) and those it did not; see [`PatchCrate::edited`].
fn in_this_build(captures_dir: &Path, crates: &[PatchCrate]) -> Result<(Vec<PatchCrate>, Vec<String>)> {
    let mut in_build = Vec::with_capacity(crates.len());
    let mut skipped = Vec::new();
    for krate in crates {
        if replay::find_capture(captures_dir, &krate.crate_name).is_ok() {
            in_build.push(krate.clone());
        } else if krate.edited {
            bail!(
                "no capture for `{}` in this base build: the crate is not compiled into the \
                 wasm program, so there is nothing to patch",
                krate.crate_name
            );
        } else {
            skipped.push(krate.crate_name.clone());
        }
    }
    Ok((in_build, skipped))
}

/// Replay one crate's captured invocation as a PIC object compile.
fn replay_crate(
    captures_dir: &Path,
    crate_name: &str,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(Vec<PathBuf>, Duration)> {
    replay_crate_at(captures_dir, crate_name, cancel, false)
}

/// [`replay_crate`], at background priority when `background` (a seed).
fn replay_crate_at(
    captures_dir: &Path,
    crate_name: &str,
    cancel: &(dyn Fn() -> bool + Sync),
    background: bool,
) -> Result<(Vec<PathBuf>, Duration)> {
    let started = Instant::now();
    let captured = replay::find_capture(captures_dir, crate_name)?;
    // See the module docs: this rides the replayed command line so
    // cargo's `-Cmetadata` — and therefore the symbol hashes the jump
    // table pairs on — stay exactly as the base's.
    let extra = ["-Crelocation-model=pic".to_string()];
    let run = if background {
        replay::run_rustc_emit_obj_background
    } else {
        replay::run_rustc_emit_obj_cancellable
    };
    let objects = match run(&captured, &extra, cancel) {
        Ok(objects) => objects,
        // Not wrapped: the caller tests for it by type.
        Err(e) if e.is::<replay::Cancelled>() => return Err(e),
        Err(e) => return Err(e.context(format!("recompiling `{crate_name}` as a PIC object"))),
    };
    if objects.is_empty() {
        bail!("rustc produced no object files for `{crate_name}` — nothing to link a patch from");
    }
    Ok((objects, started.elapsed()))
}

/// Find the `wasm-ld` that ships with the active toolchain.
///
/// The bundled one, not one on `PATH`: it is the same LLD rustc links
/// with, so it understands the object files rustc just wrote. A system
/// `wasm-ld` from a different LLVM can be a major version apart and
/// reject the relocation records in them.
fn locate_wasm_ld() -> Result<PathBuf> {
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .context("exec `rustc --print sysroot` to find the bundled wasm-ld")?;
    if !sysroot.status.success() {
        bail!("`rustc --print sysroot` exited with {}", sysroot.status);
    }
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim().to_string());

    let host = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("exec `rustc -vV` to find the host triple")?;
    let host = String::from_utf8_lossy(&host.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .context("`rustc -vV` printed no host triple")?;

    let candidate = sysroot
        .join("lib/rustlib")
        .join(&host)
        .join("bin/gcc-ld/wasm-ld");
    if candidate.is_file() {
        return Ok(candidate);
    }
    // Some toolchain layouts put it one level up.
    let flat = sysroot.join("lib/rustlib").join(&host).join("bin/wasm-ld");
    if flat.is_file() {
        return Ok(flat);
    }
    bail!(
        "no bundled wasm-ld under {} — hot patching needs the linker that ships with the \
         active toolchain, because it is the one that wrote the object files",
        sysroot.join("lib/rustlib").join(&host).display()
    )
}

/// The env a wasm base build needs so its rustc invocations are
/// captured for replay.
///
/// Deliberately NOT `build_runtime_server::hotpatch::fat_build_env`,
/// which is the native pipeline's: that one adds
/// `-Csave-temps=true -Clink-dead-code`, and `-Clink-dead-code` panics
/// wasm-bindgen 0.2.128 in its descriptor interpreter. `save-temps` is
/// also pointless here, since the replay emits its own objects rather
/// than reusing the base link's.
pub fn capture_env(idealyst_bin: &Path, captures_dir: &Path) -> Vec<(String, String)> {
    vec![
        ("RUSTC_WRAPPER".into(), idealyst_bin.display().to_string()),
        (
            "IDEALYST_RUSTC_CAPTURE_DIR".into(),
            captures_dir.display().to_string(),
        ),
        ("IDEALYST_RUSTC_WRAPPER_ACTIVE".into(), "1".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The native pipeline's env is not reusable here, and the reason is
    /// not cosmetic: `-Clink-dead-code` panics wasm-bindgen 0.2.128 at
    /// descriptor.rs:324. A base build that inherited it would fail
    /// outright, so this stays a separate function rather than a call
    /// into the native one.
    #[test]
    fn regression_the_wasm_capture_env_carries_no_link_dead_code() {
        let env = capture_env(Path::new("/usr/local/bin/idealyst"), Path::new("/tmp/caps"));
        assert!(
            !env.iter().any(|(_, v)| v.contains("link-dead-code")),
            "{env:?}"
        );
        assert!(
            !env.iter().any(|(k, _)| k == "RUSTFLAGS"),
            "the wasm base build sets its flags through CARGO_ENCODED_RUSTFLAGS; \
             a plain RUSTFLAGS here would be ignored and mislead whoever reads it: {env:?}"
        );
    }

    /// Cargo runs every crate's rustc through the wrapper, and the
    /// wrapper needs both the discriminator that puts the CLI into
    /// capture mode and somewhere to write.
    #[test]
    fn the_capture_env_points_the_wrapper_at_a_directory() {
        let env = capture_env(Path::new("/bin/idealyst"), Path::new("/tmp/caps"));
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("RUSTC_WRAPPER"), "/bin/idealyst");
        assert_eq!(get("IDEALYST_RUSTC_CAPTURE_DIR"), "/tmp/caps");
        assert_eq!(get("IDEALYST_RUSTC_WRAPPER_ACTIVE"), "1");
    }

    /// The timing line is what a person reads when a save stops feeling
    /// subsecond, so it has to attribute the time rather than total it.
    #[test]
    fn the_timing_line_names_every_step() {
        let patch = BuiltPatch {
            path: PathBuf::from("/tmp/patch-1.wasm"),
            jump_table: WasmJumpTable::default(),
            timings: vec![
                ("cargo", Duration::from_millis(410)),
                ("link", Duration::from_millis(38)),
                ("resolve", Duration::from_millis(91)),
                ("jump-table", Duration::from_millis(12)),
            ],
            crates: vec![("app".into(), Some(Duration::from_millis(410)))],
            skipped: Vec::new(),
        };
        let line = patch.timing_line();
        for step in ["cargo", "link", "resolve", "jump-table"] {
            assert!(line.contains(step), "{line}");
        }
        assert!(line.contains("total 551ms"), "{line}");
        assert!(!line.contains('['), "one crate needs no per-crate breakdown: {line}");
    }

    /// With several crates replaying at once, `cargo` is the slowest of
    /// them; the line has to say which, and which were reused.
    #[test]
    fn the_timing_line_names_each_crate_of_a_multi_crate_patch() {
        let patch = BuiltPatch {
            path: PathBuf::from("/tmp/patch-1.wasm"),
            jump_table: WasmJumpTable::default(),
            timings: vec![("cargo", Duration::from_millis(900))],
            crates: vec![
                ("ui_shared".into(), Some(Duration::from_millis(900))),
                ("app".into(), None),
            ],
            skipped: vec!["api_server".into()],
        };
        let line = patch.timing_line();
        assert!(line.contains("[ui_shared 900ms, app reused]"), "{line}");
        assert!(line.contains("(not in the wasm build: api_server)"), "{line}");
    }

    // --- object collection, against a stand-in rustc ------------------

    /// A captures dir whose "rustc" is a shell script: it writes one
    /// `<crate>.o` into whatever `--out-dir` the replay passes, appends
    /// the crate's name to `calls.log` (so a test can count replays), and
    /// fails for any crate whose name starts with `broken`.
    struct FakeCaptures {
        _tmp: tempfile::TempDir,
        dir: PathBuf,
        log: PathBuf,
        /// `start <crate>` / `end <crate>` per replay, in order.
        trace: PathBuf,
    }

    impl FakeCaptures {
        fn new(crates: &[&str]) -> Self {
            Self::with_delay(crates, "0")
        }

        /// Each replay takes 0.2 s, long enough for an overlap to show.
        fn slow(crates: &[&str]) -> Self {
            Self::with_delay(crates, "0.2")
        }

        fn with_delay(crates: &[&str], delay: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().to_path_buf();
            let log = root.join("calls.log");
            let trace = root.join("trace.log");
            let rustc = root.join("fake-rustc.sh");
            std::fs::write(
                &rustc,
                format!(
                    "#!/bin/sh\n\
                     out=''; name=''; prev=''\n\
                     for a in \"$@\"; do\n\
                       [ \"$prev\" = --out-dir ] && out=\"$a\"\n\
                       [ \"$prev\" = --crate-name ] && name=\"$a\"\n\
                       prev=\"$a\"\n\
                     done\n\
                     echo \"start $name\" >> {trace}\n\
                     /bin/sleep {delay}\n\
                     echo \"$name\" >> {log}\n\
                     case \"$name\" in broken*) exit 3;; esac\n\
                     : > \"$out/$name.o\"\n\
                     echo \"end $name\" >> {trace}\n",
                    log = log.display(),
                    trace = trace.display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).unwrap();
            let dir = root.join("captures");
            std::fs::create_dir_all(&dir).unwrap();
            for name in crates {
                // By hand: this crate has no JSON dependency, and the
                // shape is `replay::CapturedInvocation`'s four fields.
                let json = format!(
                    r#"{{"rustc":"{rustc}","args":["--crate-name","{name}","--out-dir","{deps}"],"cwd":"{cwd}","env":[]}}"#,
                    rustc = rustc.display(),
                    deps = root.join("deps").display(),
                    cwd = root.display(),
                );
                std::fs::write(dir.join(format!("{name}.lib.json")), json).unwrap();
            }
            Self { _tmp: tmp, dir, log, trace }
        }

        fn calls(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect();
            v.sort();
            v
        }
    }

    fn names(objs: &[(Vec<PathBuf>, Option<Duration>)]) -> Vec<Vec<String>> {
        objs.iter()
            .map(|(o, _)| o.iter().map(|p| p.file_name().unwrap().to_string_lossy().into()).collect())
            .collect()
    }

    /// Every crate of a multi-crate patch contributes its own objects, in
    /// the order asked for (dependencies first), however the concurrent
    /// replays happened to finish.
    #[test]
    fn a_multi_crate_patch_collects_every_crates_objects_in_order() {
        let caps = FakeCaptures::new(&["lab_shared", "app"]);
        let cache = Default::default();
        let crates = [PatchCrate::new("lab_shared", None), PatchCrate::new("app", None)];
        let got = collect_objects(&caps.dir, &cache, &crates, &|| false).unwrap();
        assert_eq!(names(&got), vec![vec!["lab_shared.o"], vec!["app.o"]]);
        assert!(got.iter().all(|(_, took)| took.is_some()), "both were replayed");
        assert_eq!(caps.calls(), vec!["app", "lab_shared"]);
    }

    /// A crate carried only for a dependency's sake, with the same
    /// sources as its last replay against this base, is not replayed
    /// again; a moved key, or none, replays it.
    #[test]
    fn a_carried_crate_with_unmoved_sources_reuses_its_objects() {
        let caps = FakeCaptures::new(&["lab_shared", "app"]);
        let cache = Default::default();
        let key = |k: &str| Some(k.to_string());

        collect_objects(&caps.dir, &cache, &[PatchCrate::new("lab_shared", key("s1")), PatchCrate::new("app", key("a1"))], &|| false).unwrap();
        assert_eq!(caps.calls().len(), 2);

        // The next save edits lab-shared again: app's sources did not move.
        let got = collect_objects(&caps.dir, &cache, &[PatchCrate::new("lab_shared", key("s2")), PatchCrate::new("app", key("a1"))], &|| false).unwrap();
        assert_eq!(caps.calls(), vec!["app", "lab_shared", "lab_shared"], "app was reused");
        assert!(got[1].1.is_none(), "the reuse reports no replay time");
        assert_eq!(names(&got)[1], vec!["app.o"]);

        // app moved (an overlay edit advanced its key): replayed.
        collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", key("a2"))], &|| false).unwrap();
        assert_eq!(caps.calls().iter().filter(|c| *c == "app").count(), 2);

        // No key: never trusted.
        collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", None)], &|| false).unwrap();
        collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", None)], &|| false).unwrap();
        assert_eq!(caps.calls().iter().filter(|c| *c == "app").count(), 4);
    }

    /// A queue running `jobs` seeds at once, over `caps`.
    fn queue(caps: &FakeCaptures, jobs: usize) -> SeedQueue {
        let mut q = SeedQueue::new(&caps.dir, dev_events::Reporter::new());
        q.jobs = jobs;
        q
    }

    /// Wait until no seed is queued or running.
    fn wait_idle(q: &SeedQueue) {
        let (lock, cv) = &*q.shared;
        let mut st = lock.lock().unwrap();
        while !(st.queue.is_empty() && st.running.is_empty()) {
            st = cv.wait(st).unwrap();
        }
    }

    fn trace(caps: &FakeCaptures) -> Vec<String> {
        std::fs::read_to_string(&caps.trace).unwrap_or_default().lines().map(str::to_string).collect()
    }

    /// Regression (CrewForge): only the app crate was seeded, so a
    /// library crate's first patch replayed its ~30 dependents cold
    /// (30.5–35.0 s, then 3.6 s once warm). Every crate seeded is filed
    /// under its source key, and a patch carrying them unchanged replays
    /// only the edited one; a crate this base never compiled is skipped.
    #[test]
    fn regression_seeding_every_crate_leaves_a_library_patch_one_replay() {
        let caps = FakeCaptures::new(&["lib", "screen_a", "screen_b", "app"]);
        let cache = Default::default();
        let q = queue(&caps, 2);
        let key = |k: &str| Some(k.to_string());
        q.push(vec![
            PatchCrate::carried("app", key("a")),
            PatchCrate::carried("screen_a", key("sa")),
            PatchCrate::carried("screen_b", key("sb")),
            PatchCrate::carried("lib", key("l")),
            PatchCrate::carried("host_only", key("h")),
        ]);
        wait_idle(&q);
        q.file_into(&cache);
        assert_eq!(caps.calls(), vec!["app", "lib", "screen_a", "screen_b"], "host_only has no capture");

        // The library's first patch: only it replays.
        let plan = [
            PatchCrate::new("lib", key("l2")),
            PatchCrate::carried("screen_a", key("sa")),
            PatchCrate::carried("screen_b", key("sb")),
            PatchCrate::carried("app", key("a")),
        ];
        q.pause_for(&plan);
        let got = collect_objects(&caps.dir, &cache, &plan, &|| false).unwrap();
        q.resume();
        assert!(got[1..].iter().all(|(_, took)| took.is_none()), "every dependent came from its seed");
        assert_eq!(caps.calls().iter().filter(|c| *c == "lib").count(), 2);
        assert_eq!(caps.calls().len(), 5);
        q.stop();
    }

    /// A save comes first. A running seed of a crate the save needs is
    /// waited for, never killed (that would only make the save replay it
    /// cold); one it does not need is killed, so the save has the cores,
    /// and seeded again after it. Two replays of one crate never overlap:
    /// they would share an incremental session and an object dir.
    #[test]
    fn a_save_waits_for_the_seeds_it_needs_and_kills_the_rest() {
        let caps = FakeCaptures::with_delay(&["app", "other"], "0.6");
        let cache = Default::default();
        let q = queue(&caps, 2);
        q.push(vec![PatchCrate::carried("app", Some("a".into())), PatchCrate::carried("other", Some("o".into()))]);
        // Both running.
        let deadline = Instant::now() + Duration::from_secs(10);
        while trace(&caps).iter().filter(|l| l.starts_with("start")).count() < 2 {
            assert!(Instant::now() < deadline, "the seeds never started");
            std::thread::sleep(Duration::from_millis(10));
        }

        let plan = [PatchCrate::carried("app", Some("a".into()))];
        q.pause_for(&plan);
        let t = trace(&caps);
        assert!(t.contains(&"end app".to_string()), "the needed seed finished: {t:?}");
        assert!(!t.contains(&"end other".to_string()), "the other seed was killed: {t:?}");
        q.file_into(&cache);
        let got = collect_objects(&caps.dir, &cache, &plan, &|| false).unwrap();
        assert!(got[0].1.is_none(), "the save reused the seed's objects");
        assert!(!trace(&caps).iter().skip(t.len()).any(|l| l.starts_with("start")), "nothing ran during the save");

        q.resume();
        wait_idle(&q);
        let t = trace(&caps);
        assert_eq!(t.last().map(String::as_str), Some("end other"), "the killed seed ran again: {t:?}");
        // `other` started, was killed by the save (no `end`), and ran
        // again only after it; `app` ran once.
        let mut sorted = t.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["end app", "end other", "start app", "start other", "start other"], "{t:?}");
        assert_eq!(&t[2..], ["end app", "start other", "end other"], "{t:?}");
        q.stop();
    }

    /// The base going away stops every seed, and nothing runs after.
    #[test]
    fn stopping_kills_running_seeds_and_drops_the_queue() {
        let caps = FakeCaptures::with_delay(&["a", "b", "c"], "2");
        let q = queue(&caps, 1);
        q.push(vec![PatchCrate::carried("a", None), PatchCrate::carried("b", None), PatchCrate::carried("c", None)]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while trace(&caps).is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        q.stop();
        // Killed rather than waited for: it never wrote its object. (Not
        // timed: the stand-in rustc is a shell script whose `sleep` keeps
        // the pipes open past the kill; rustc --emit=obj has no children.)
        assert!(caps.calls().is_empty(), "the running seed ran to the end");
        assert_eq!(trace(&caps), vec!["start a"]);
    }

    #[test]
    fn seed_jobs_default_to_a_quarter_of_the_cores() {
        assert_eq!(seed_jobs(None, 8), 2);
        assert_eq!(seed_jobs(None, 2), 1);
        assert_eq!(seed_jobs(None, 64), 4);
        assert_eq!(seed_jobs(Some("6"), 8), 6);
        assert_eq!(seed_jobs(Some("0"), 8), 2);
        assert_eq!(seed_jobs(Some("x"), 8), 2);
    }

    /// Regression (CrewForge): the workspace closure reaches host-only
    /// crates (`crewforge-api-server`, through a feature the web build
    /// never enables), so a save in `crewforge-core` planned a replay of
    /// one with no capture and every such patch failed. A dependent the
    /// base never compiled is skipped; an EDITED one still refuses.
    #[test]
    fn regression_a_dependent_outside_the_wasm_build_is_skipped() {
        let caps = FakeCaptures::new(&["core", "app"]);
        let plan = [
            PatchCrate::new("core", None),
            PatchCrate::carried("api_server", None),
            PatchCrate::carried("app", None),
        ];
        let (in_build, skipped) = in_this_build(&caps.dir, &plan).unwrap();
        assert_eq!(in_build.iter().map(|c| c.crate_name.as_str()).collect::<Vec<_>>(), vec!["core", "app"]);
        assert_eq!(skipped, vec!["api_server"]);

        let err = in_this_build(&caps.dir, &[PatchCrate::new("api_server", None)]).unwrap_err();
        assert!(format!("{err:#}").contains("not compiled into the wasm program"), "{err:#}");
    }

    /// A replay that fails fails the patch (the caller rebuilds), and
    /// forgets anything cached for that crate.
    #[test]
    fn a_failed_replay_fails_the_patch_and_forgets_the_crate() {
        let caps = FakeCaptures::new(&["broken_lib", "app"]);
        let cache: std::cell::RefCell<ObjectCache> = Default::default();
        cache.borrow_mut().insert("broken_lib".into(), ("k".into(), vec![PathBuf::from("/nope.o")]));
        let err = collect_objects(&caps.dir, &cache, &[PatchCrate::new("broken_lib", Some("k2".into())), PatchCrate::new("app", None)], &|| false)
        .unwrap_err();
        assert!(format!("{err:#}").contains("broken_lib"), "{err:#}");
        assert!(!cache.borrow().contains_key("broken_lib"));
    }

    /// A newer save cancels the patch mid-replay: the replay is killed
    /// (well before it would have finished), the error says CANCELLED
    /// rather than failed — the loop restarts instead of rebuilding — and
    /// the killed crate is forgotten, so the next patch replays it afresh.
    #[test]
    fn a_cancelled_replay_is_killed_reported_as_cancelled_and_forgotten() {
        // A replay that takes 2 s unless it is killed.
        let caps = FakeCaptures::with_delay(&["app"], "2");
        let cache: std::cell::RefCell<ObjectCache> = Default::default();
        cache.borrow_mut().insert("app".into(), ("old".into(), vec![PathBuf::from("/nope.o")]));
        let started = Instant::now();
        let cancel_at = Duration::from_millis(50);
        let err = collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", Some("k".into()))], &|| {
            started.elapsed() >= cancel_at
        })
        .unwrap_err();
        assert!(err.is::<replay::Cancelled>(), "{err:#}");
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "the replay ran to the end instead of being killed"
        );
        assert!(!cache.borrow().contains_key("app"), "a killed replay's objects must not be reused");
        assert!(caps.calls().is_empty(), "the fake rustc was killed before it finished");

        let got = collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", Some("k".into()))], &|| false)
            .unwrap();
        assert_eq!(names(&got), vec![vec!["app.o"]]);
    }

    /// The bundled linker has to exist for any of this to work, and a
    /// missing one is a clear message rather than a failed link later.
    #[test]
    fn the_bundled_wasm_ld_is_findable() {
        let found = locate_wasm_ld().expect("the active toolchain ships wasm-ld");
        assert!(found.is_file(), "{}", found.display());
    }
}
