//! Build one wasm patch, from a saved source file to a module the page
//! can instantiate.
//!
//! Four steps, each a measured cost on a save:
//!
//! 1. **Replay.** The user crate's exact rustc invocation, captured
//!    during the base build, re-run with `--emit=obj` and
//!    `-Crelocation-model=pic`. One crate's codegen — not the graph's.
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
use crate::hotpatch_wasm::{build_jump_table, WasmJumpTable};

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
        format!(
            "{} (total {}ms)",
            parts.join(" · "),
            self.total().as_millis()
        )
    }
}

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
    crate_name: String,
    out_dir: PathBuf,
    wasm_ld: PathBuf,
    base: BaseIndex,
    base_wasm: Vec<u8>,
    serial: std::cell::Cell<u64>,
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
        Ok(Self {
            captures_dir: captures_dir.into(),
            aliases,
            crate_name: crate_name.into(),
            out_dir: out_dir.into(),
            wasm_ld: locate_wasm_ld()?,
            base,
            base_wasm,
            serial: std::cell::Cell::new(0),
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

    pub fn build(&self) -> Result<BuiltPatch> {
        let mut timings = Vec::new();

        let started = Instant::now();
        let captured = replay::find_capture(&self.captures_dir, &self.crate_name)?;
        let objects = replay::run_rustc_emit_obj_with(
            &captured,
            // See the module docs: this rides the replayed command line
            // so cargo's `-Cmetadata` — and therefore the symbol hashes
            // the jump table pairs on — stay exactly as the base's.
            &["-Crelocation-model=pic".to_string()],
        )
        .context("recompiling the changed crate as a PIC object")?;
        if objects.is_empty() {
            bail!(
                "rustc produced no object files for `{}` — nothing to link a patch from",
                self.crate_name
            );
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
        let path = self.out_dir.join(format!("patch-{serial}.wasm"));
        std::fs::write(&path, &resolved)
            .with_context(|| format!("write the patch {}", path.display()))?;
        // The pre-resolve module is only useful when a patch misbehaves,
        // and it is the same size as the one we serve. Keeping every one
        // of them fills a dev session's staging dir.
        let _ = std::fs::remove_file(&linked);
        timings.push(("resolve", started.elapsed()));

        let started = Instant::now();
        let jump_table = build_jump_table(&self.base_wasm, &resolved, &self.aliases)
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

        Ok(BuiltPatch {
            path,
            jump_table,
            timings,
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
        cmd.args([
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
        ]);
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
        };
        let line = patch.timing_line();
        for step in ["cargo", "link", "resolve", "jump-table"] {
            assert!(line.contains(step), "{line}");
        }
        assert!(line.contains("total 551ms"), "{line}");
    }

    /// The bundled linker has to exist for any of this to work, and a
    /// missing one is a clear message rather than a failed link later.
    #[test]
    fn the_bundled_wasm_ld_is_findable() {
        let found = locate_wasm_ld().expect("the active toolchain ships wasm-ld");
        assert!(found.is_file(), "{}", found.display());
    }
}
