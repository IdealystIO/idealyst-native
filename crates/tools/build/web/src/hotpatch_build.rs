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
//! 3. **Resolve.** [`crate::hotpatch_patch::resolve_against_base`] turns
//!    those imports into things `subsecond::apply_patch` can supply.
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
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use build_runtime_server::hotpatch::replay;

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
}

impl PatchCrate {
    pub fn new(crate_name: impl Into<String>, source_key: Option<String>) -> Self {
        Self { crate_name: crate_name.into(), source_key }
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
}

impl BuiltPatch {
    pub fn total(&self) -> Duration {
        self.timings.iter().map(|(_, d)| *d).sum()
    }

    pub fn timing_line(&self) -> String {
        let parts: Vec<String> = self
            .timings
            .iter()
            .map(|(name, d)| format!("{name} {}ms", d.as_millis()))
            .collect();
        let line = format!("{} (total {}ms)", parts.join(" · "), self.total().as_millis());
        if self.crates.len() < 2 {
            return line;
        }
        // Several crates replay concurrently, so `cargo` above is the
        // slowest of them; this says which.
        let crates: Vec<String> = self
            .crates
            .iter()
            .map(|(name, d)| match d {
                Some(d) => format!("{name} {}ms", d.as_millis()),
                None => format!("{name} reused"),
            })
            .collect();
        format!("{line} [{}]", crates.join(", "))
    }
}

/// Objects a replay wrote, and the source key they were compiled from.
type ObjectCache = std::collections::HashMap<String, (String, Vec<PathBuf>)>;

/// Everything that does not change between patches, resolved once so a
/// save only pays for the four steps.
///
/// The base index in particular is the expensive part — parsing a
/// debug-profile wasm module with every function in its table is tens of
/// megabytes of walrus work — and it is valid for the life of one base
/// build.
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
}

impl WasmPatchBuilder {
    /// `served_wasm` must be the module the BROWSER actually runs —
    /// after wasm-bindgen and after the command-export neutralize pass.
    /// Both rewrite the module, and a table index read before either one
    /// names a different function than the one the page will call.
    /// `symbol_aliases` is the file the base build wrote from the LINKED
    /// module (see [`crate::hotpatch_aliases`]). A base built without one
    /// still patches; it just cannot resolve a symbol the linker knew by
    /// a second name.
    pub fn new(
        served_wasm: &Path,
        symbol_aliases: Option<&Path>,
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
        let base = BaseIndex::of(&base_wasm, &aliases).with_context(|| {
            format!("indexing the served base module {}", served_wasm.display())
        })?;
        let base_slots = base_slots(&base_wasm)?;
        Ok(Self {
            captures_dir: captures_dir.into(),
            aliases,
            crate_name: crate_name.into(),
            out_dir: out_dir.into(),
            wasm_ld: locate_wasm_ld()?,
            base,
            base_slots,
            serial: std::cell::Cell::new(0),
            objects: Default::default(),
        })
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

    /// A patch of the app crate alone — [`Self::build_crates`] over the
    /// crate this builder was made for.
    pub fn build(&self) -> Result<BuiltPatch> {
        self.build_crates(&[PatchCrate::new(self.crate_name.clone(), None)])
    }

    /// A patch re-emitting every crate in `crates`, linked into one
    /// module. See the module docs for why one module, and why the
    /// replays run concurrently.
    pub fn build_crates(&self, crates: &[PatchCrate]) -> Result<BuiltPatch> {
        if crates.is_empty() {
            bail!("a patch of no crates");
        }
        let mut timings = Vec::new();

        let started = Instant::now();
        let replayed = collect_objects(&self.captures_dir, &self.objects, crates)?;
        let mut objects = Vec::new();
        let mut carried = Vec::with_capacity(crates.len());
        for (krate, (objs, took)) in crates.iter().zip(replayed) {
            objects.extend(objs);
            carried.push((krate.crate_name.clone(), took));
        }
        timings.push(("cargo", started.elapsed()));

        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        std::fs::create_dir_all(&self.out_dir)
            .with_context(|| format!("create {}", self.out_dir.display()))?;
        let linked = self.out_dir.join(format!("patch-{serial}.linked.wasm"));

        let started = Instant::now();
        self.link(&objects, &linked)?;
        timings.push(("link", started.elapsed()));

        let started = Instant::now();
        let raw = std::fs::read(&linked)
            .with_context(|| format!("read the linked patch {}", linked.display()))?;
        let resolved = crate::hotpatch_patch::resolve_against_base(&raw, &self.base)?;
        // The pre-resolve module is only useful when a patch misbehaves,
        // and it is larger than the one we serve. Keeping every one of
        // them fills a dev session's staging dir.
        let _ = std::fs::remove_file(&linked);
        timings.push(("resolve", started.elapsed()));

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
        // time reads it: pairing is done, and subsecond applies by table
        // index. What it costs is function names in a stack trace through
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
        })
    }

    /// Link the patch's objects — and ONLY those — into a PIC side
    /// module.
    ///
    /// Everything the objects reference but do not define is left
    /// undefined on purpose: `--allow-undefined` turns each into an
    /// import, which is exactly the set `resolve_against_base` then
    /// points back at the running module. Linking the rlibs instead
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
                (i, scope.spawn(move || replay_crate(&captures, &name)))
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
                // A failed replay emptied that crate's object dir, so
                // whatever the cache said about it is gone too.
                cache.remove(&krate.crate_name);
                first_error.get_or_insert(e);
            }
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(out.into_iter().map(|o| o.expect("every crate reused or replayed")).collect())
}

/// Replay one crate's captured invocation as a PIC object compile.
fn replay_crate(captures_dir: &Path, crate_name: &str) -> Result<(Vec<PathBuf>, Duration)> {
    let started = Instant::now();
    let captured = replay::find_capture(captures_dir, crate_name)?;
    let objects = replay::run_rustc_emit_obj_with(
        &captured,
        // See the module docs: this rides the replayed command line so
        // cargo's `-Cmetadata` — and therefore the symbol hashes the jump
        // table pairs on — stay exactly as the base's.
        &["-Crelocation-model=pic".to_string()],
    )
    .with_context(|| format!("recompiling `{crate_name}` as a PIC object"))?;
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
        };
        let line = patch.timing_line();
        assert!(line.contains("[ui_shared 900ms, app reused]"), "{line}");
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
    }

    impl FakeCaptures {
        fn new(crates: &[&str]) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().to_path_buf();
            let log = root.join("calls.log");
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
                     echo \"$name\" >> {log}\n\
                     case \"$name\" in broken*) exit 3;; esac\n\
                     : > \"$out/$name.o\"\n",
                    log = log.display()
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
            Self { _tmp: tmp, dir, log }
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
        let got = collect_objects(&caps.dir, &cache, &crates).unwrap();
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

        collect_objects(&caps.dir, &cache, &[PatchCrate::new("lab_shared", key("s1")), PatchCrate::new("app", key("a1"))]).unwrap();
        assert_eq!(caps.calls().len(), 2);

        // The next save edits lab-shared again: app's sources did not move.
        let got = collect_objects(&caps.dir, &cache, &[PatchCrate::new("lab_shared", key("s2")), PatchCrate::new("app", key("a1"))]).unwrap();
        assert_eq!(caps.calls(), vec!["app", "lab_shared", "lab_shared"], "app was reused");
        assert!(got[1].1.is_none(), "the reuse reports no replay time");
        assert_eq!(names(&got)[1], vec!["app.o"]);

        // app moved (an overlay edit advanced its key): replayed.
        collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", key("a2"))]).unwrap();
        assert_eq!(caps.calls().iter().filter(|c| *c == "app").count(), 2);

        // No key: never trusted.
        collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", None)]).unwrap();
        collect_objects(&caps.dir, &cache, &[PatchCrate::new("app", None)]).unwrap();
        assert_eq!(caps.calls().iter().filter(|c| *c == "app").count(), 4);
    }

    /// A replay that fails fails the patch (the caller rebuilds), and
    /// forgets anything cached for that crate.
    #[test]
    fn a_failed_replay_fails_the_patch_and_forgets_the_crate() {
        let caps = FakeCaptures::new(&["broken_lib", "app"]);
        let cache: std::cell::RefCell<ObjectCache> = Default::default();
        cache.borrow_mut().insert("broken_lib".into(), ("k".into(), vec![PathBuf::from("/nope.o")]));
        let err = collect_objects(
            &caps.dir,
            &cache,
            &[PatchCrate::new("broken_lib", Some("k2".into())), PatchCrate::new("app", None)],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("broken_lib"), "{err:#}");
        assert!(!cache.borrow().contains_key("broken_lib"));
    }

    /// The bundled linker has to exist for any of this to work, and a
    /// missing one is a clear message rather than a failed link later.
    #[test]
    fn the_bundled_wasm_ld_is_findable() {
        let found = locate_wasm_ld().expect("the active toolchain ships wasm-ld");
        assert!(found.is_file(), "{}", found.display());
    }
}
