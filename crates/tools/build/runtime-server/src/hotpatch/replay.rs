//! Replay captured rustc invocations with `--emit=obj`.
//!
//! Captures are produced by the `rustc-capture` CLI subcommand
//! during the initial fat build (see
//! [`crate::hotpatch::fat_build_env`]). On each source change the
//! host's rebuild loop hands a `<crate-name>` to
//! [`crate::hotpatch::HotPatchBuilder::build`], which calls into
//! here to:
//!
//!  1. Load the on-disk capture file
//!  2. Rewrite the argv into a patch compile — see [`replay_args`]
//!  3. Spawn rustc with the captured env + cwd
//!  4. Collect every `.o` from the replay's private out-dir
//!
//! Rustc emits one `.rcgu.o` per codegen unit, up to
//! [`REPLAY_CODEGEN_UNITS`]. The full list is what feeds the stub
//! generator and the patch link.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Mirror of `cli::cmd::rustc_capture::CapturedInvocation`. Lives
/// here too because both crates need to deserialize without
/// taking a dep on each other.
#[derive(Serialize, Deserialize, Debug)]
pub struct CapturedInvocation {
    pub rustc: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
}

/// Find the capture file for `crate_name`. Cargo invokes rustc
/// once per crate (multiple `--crate-type` flags share an
/// invocation), so a crate with `crate-type = ["cdylib", "rlib"]`
/// has one capture file named for whichever crate-type appears
/// first in the rustc argv. We don't try to be clever — just
/// take the first file matching `<crate_name>.*.json`.
///
/// Cargo normalizes `-` to `_` in `--crate-name`, so a Cargo.toml
/// `name = "hot-reload-test"` ends up as `hot_reload_test` in the
/// rustc argv (and therefore in the capture filename). We try the
/// caller's name first, then the underscore-normalized variant.
pub fn find_capture(
    captures_dir: &Path,
    crate_name: &str,
) -> Result<CapturedInvocation> {
    let primary = format!("{}.", crate_name);
    let normalized = format!("{}.", crate_name.replace('-', "_"));
    let mut candidates: Vec<(usize, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(captures_dir)
        .with_context(|| format!("read captures dir {}", captures_dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".json") {
            continue;
        }
        if name.starts_with(&primary) || name.starts_with(&normalized) {
            candidates.push((crate_type_rank(&name), entry.path()));
        }
    }
    // A project with both a library and a binary of the same name — the
    // standard idealyst shape, where `src/main.rs` is one `entry!` line —
    // writes two captures. Directory order is not defined, so picking
    // "the first" picked either, and replaying the BINARY produced a
    // patch containing an `entry!` expansion and none of the app.
    //
    // Rank, then sort: the library is where the components live.
    candidates.sort();
    let found = candidates.into_iter().next().map(|(_, path)| path);
    let path = found.ok_or_else(|| {
        anyhow::anyhow!(
            "no capture for crate `{}` in {} — was the fat build run with \
             RUSTC_WORKSPACE_WRAPPER set?",
            crate_name,
            captures_dir.display()
        )
    })?;
    let data = std::fs::read(&path)
        .with_context(|| format!("read capture {}", path.display()))?;
    serde_json::from_slice(&data)
        .with_context(|| format!("parse capture {}", path.display()))
}

/// Replay rustc, modifying `--emit=` to emit only `.o` files.
/// Returns the absolute paths of every `.o` rustc produced.
pub fn run_rustc_emit_obj(captured: &CapturedInvocation) -> Result<Vec<PathBuf>> {
    run_rustc_emit_obj_with(captured, &[])
}

/// As [`run_rustc_emit_obj`], with extra rustc flags appended.
///
/// The wasm patch build needs `-Crelocation-model=pic`, because a wasm
/// patch is a PIC side module and its data references have to go through
/// a GOT the loader can point at the base's memory. The flag is appended
/// to the REPLAYED argv rather than set in `RUSTFLAGS` on the base build
/// for one reason that is easy to get wrong: cargo folds `RUSTFLAGS` into
/// the `-Cmetadata` it passes each crate, `-Cmetadata` seeds the symbol
/// mangling hash, and a patch whose symbols hash differently from the
/// base's pairs with nothing. Appending here leaves the captured
/// `-Cmetadata` exactly as it was.
pub fn run_rustc_emit_obj_with(
    captured: &CapturedInvocation,
    extra: &[String],
) -> Result<Vec<PathBuf>> {
    let out_dir = replay_out_dir(&captured.args)?;
    // A fresh directory per replay. With many codegen units rustc writes
    // one object per unit under a HASHED name, so a previous replay's
    // objects never get overwritten, and linking whatever is lying in
    // the directory would link stale code next to the new.
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("create {}", out_dir.display()))?;
    let args = replay_args(&captured.args, extra, &out_dir);

    let mut cmd = Command::new(&captured.rustc);
    cmd.args(&args).current_dir(&captured.cwd);
    cmd.env_clear();
    for (k, v) in &captured.env {
        cmd.env(k, v);
    }

    let output = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("spawn rustc")?;
    if !output.status.success() {
        // The captured invocation carries cargo's `--error-format=json`, so
        // rustc wrote JSON. Report each diagnostic as the text rustc would
        // have printed, not as raw JSON; the rebuild this failure falls back
        // to reports the same errors as structured diagnostics.
        let reporter = dev_events::global();
        for stream in [&output.stderr, &output.stdout] {
            for line in String::from_utf8_lossy(stream).lines() {
                match dev_events::cargo::rustc_diagnostic(line) {
                    Some(d) => {
                        for l in d.ansi.as_deref().unwrap_or(&d.rendered).lines() {
                            reporter.output("hotpatch", l);
                        }
                    }
                    None => reporter.output("hotpatch", line),
                }
            }
        }
        // Name any `--extern` input that is not on disk right now. A
        // replay runs against artifacts a concurrent cargo may be
        // rewriting, and "can't find crate" alone does not say whether
        // the file was missing, replaced, or never there.
        let missing: Vec<&str> = extern_paths(&args)
            .filter(|p| !std::path::Path::new(p).exists())
            .collect();
        if missing.is_empty() {
            anyhow::bail!("rustc --emit=obj exited with {}", output.status);
        }
        anyhow::bail!(
            "rustc --emit=obj exited with {} — {} `--extern` input(s) missing at replay time: {}",
            output.status,
            missing.len(),
            missing.join(", ")
        );
    }

    // Every object in the private out-dir is this replay's: the dir was
    // emptied first. Artifact notifications are not enough on their
    // own — with several codegen units rustc reports the objects it
    // wrote, but a unit reused from the incremental cache is copied in
    // without one.
    let listed: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .with_context(|| format!("read {}", out_dir.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    Ok(replay_objects(listed, &args))
}

/// The objects a replay's out-dir holds, minus rustc's single-unit copy.
///
/// With exactly ONE codegen unit, rustc copies that unit's
/// `<crate><extra>.<cgu>.rcgu.o` to `<crate><extra>.o` AND keeps the
/// numbered file (`produce_final_output_artifacts`: `copy_if_one_unit(
/// Object, keep_numbered = true)`). Linking both defines every symbol
/// twice and the patch link fails with `duplicate symbol` — which a small
/// library crate of the workspace hits, since it can partition into a
/// single unit where the app crate never does. So the copy is dropped
/// whenever a numbered unit is present.
pub fn replay_objects(listed: Vec<PathBuf>, args: &[String]) -> Vec<PathBuf> {
    let mut objects: Vec<PathBuf> = listed
        .into_iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("o"))
        .collect();
    let has_units = objects.iter().any(|p| is_codegen_unit(p));
    if has_units {
        let crate_name = arg_value(args, "--crate-name").unwrap_or_default();
        let extra = codegen_value(args, "extra-filename").unwrap_or_default();
        let single_copy = format!("{crate_name}{extra}.o");
        objects.retain(|p| p.file_name().and_then(|f| f.to_str()) != Some(single_copy.as_str()));
    }
    objects.sort();
    objects
}

fn is_codegen_unit(path: &Path) -> bool {
    path.file_name()
        .and_then(|f| f.to_str())
        .is_some_and(|f| f.ends_with(".rcgu.o"))
}

/// The value of a `-C <key>=<value>` / `-C<key>=<value>` flag.
fn codegen_value(args: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        let flag = if a == "-C" {
            iter.next().map(String::as_str)
        } else {
            a.strip_prefix("-C")
        };
        if let Some(v) = flag.and_then(|f| f.strip_prefix(&prefix)) {
            return Some(v.to_string());
        }
    }
    None
}

/// Preference order among several captures for one crate name: lower is
/// better. The library targets come first because that is where an app's
/// components are defined; a `bin` of the same name is the entry point
/// and holds nothing worth patching.
fn crate_type_rank(file_name: &str) -> usize {
    let stem = file_name.trim_end_matches(".json");
    match stem.rsplit('.').next().unwrap_or("") {
        "rlib" => 0,
        "lib" => 1,
        "cdylib" => 2,
        "staticlib" => 3,
        "bin" => 4,
        _ => 5,
    }
}

/// The codegen-unit count a replay asks for when the capture names none:
/// rustc's own default for an incremental build, which is what the base
/// build (`--emit=link`) got.
pub const REPLAY_CODEGEN_UNITS: usize = 256;

/// Where a replay writes its objects: beside the captured `--out-dir`,
/// never in it (see the call site).
fn replay_out_dir(captured: &[String]) -> Result<PathBuf> {
    let deps = arg_value(captured, "--out-dir")
        .context("the captured rustc invocation has no --out-dir")?;
    let crate_name = arg_value(captured, "--crate-name").unwrap_or_else(|| "crate".into());
    let deps = PathBuf::from(deps);
    let parent = deps.parent().map(Path::to_path_buf).unwrap_or(deps);
    Ok(parent.join("idealyst-hotpatch-obj").join(crate_name))
}

/// The captured argv, rewritten into a patch compile.
///
/// Four rewrites, each a measured cost when missing:
///
/// 1. `--emit=obj` instead of cargo's `--emit=dep-info,metadata,link`:
///    rustc stops at object files.
/// 2. **An explicit `-C codegen-units`**, unless the capture sets one.
///    rustc forces codegen-units to 1 when the emit set contains an
///    object-like output and no count was given, so the replay compiled
///    the whole crate as ONE unit and any edit re-codegened all of it.
///    On CrewForge (146,942 mono items) a one-token edit took 40–46 s;
///    with 256 units it takes 7.4 s, and a line-shifting edit 6.4 s
///    instead of 63 s.
/// 3. **A separate incremental dir** (`<dir>-hotpatch`). The base build
///    and a replay have different tracked options (emit, codegen units,
///    relocation model), and rustc discards a session whose options
///    differ, so sharing one directory made each side throw away the
///    other's cache: the first patch after a rebuild, and the first
///    rebuild after a patch, both compiled cold.
/// 4. `--out-dir` pointed at a private directory.
///
/// `-Cmetadata` and everything else stays byte-identical: it seeds the
/// symbol hashes the jump table pairs on.
pub fn replay_args(captured: &[String], extra: &[String], out_dir: &Path) -> Vec<String> {
    let mut args: Vec<String> = Vec::with_capacity(captured.len() + 4);
    let mut emit_set = false;
    let mut has_cgus = false;
    let mut iter = captured.iter().peekable();
    while let Some(a) = iter.next() {
        if a == "--emit" {
            let _ = iter.next();
            args.push("--emit=obj".to_string());
            emit_set = true;
            continue;
        }
        if a.starts_with("--emit=") {
            args.push("--emit=obj".to_string());
            emit_set = true;
            continue;
        }
        if a == "--out-dir" {
            let _ = iter.next();
            args.push("--out-dir".to_string());
            args.push(out_dir.display().to_string());
            continue;
        }
        if let Some(v) = a.strip_prefix("--out-dir=") {
            let _ = v;
            args.push(format!("--out-dir={}", out_dir.display()));
            continue;
        }
        // `-C incremental=<dir>` (cargo's spelling) or `-Cincremental=<dir>`.
        if a == "-C" {
            if let Some(next) = iter.peek() {
                if let Some(dir) = next.strip_prefix("incremental=") {
                    args.push("-C".to_string());
                    args.push(format!("incremental={dir}-hotpatch"));
                    let _ = iter.next();
                    continue;
                }
                if next.starts_with("codegen-units=") {
                    has_cgus = true;
                }
            }
        }
        if let Some(dir) = a.strip_prefix("-Cincremental=") {
            args.push(format!("-Cincremental={dir}-hotpatch"));
            continue;
        }
        if a.starts_with("-Ccodegen-units=") {
            has_cgus = true;
        }
        args.push(a.clone());
    }
    if !emit_set {
        args.push("--emit=obj".to_string());
    }
    if !has_cgus {
        args.push(format!("-Ccodegen-units={REPLAY_CODEGEN_UNITS}"));
    }
    args.extend(extra.iter().cloned());
    args
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == key {
            return iter.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(&format!("{key}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cargo_argv() -> Vec<String> {
        [
            "--crate-name", "app", "--edition=2021", "src/lib.rs",
            "--emit=dep-info,metadata,link", "-C", "debuginfo=2",
            "-C", "metadata=abc", "--out-dir", "/t/debug/deps",
            "-C", "incremental=/t/debug/incremental",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// Regression: rustc forces codegen-units=1 when the emit set has an
    /// object-like output and no count is given, so every patch compiled
    /// the whole crate as one unit — 40–46 s for a one-token edit on
    /// CrewForge, against 7.4 s with 256 units.
    #[test]
    fn regression_a_replay_asks_for_many_codegen_units() {
        let args = replay_args(&cargo_argv(), &[], Path::new("/t/obj"));
        assert!(args.contains(&"-Ccodegen-units=256".to_string()), "{args:?}");
        assert!(args.contains(&"--emit=obj".to_string()));
    }

    /// An explicit count in the capture is the author's, and kept.
    #[test]
    fn an_explicit_codegen_unit_count_is_kept() {
        let mut argv = cargo_argv();
        argv.extend(["-C".to_string(), "codegen-units=4".to_string()]);
        let args = replay_args(&argv, &[], Path::new("/t/obj"));
        assert!(!args.iter().any(|a| a == "-Ccodegen-units=256"), "{args:?}");
    }

    /// Regression: the base build and the replay have different tracked
    /// options, and a shared incremental dir made each discard the
    /// other's cache.
    #[test]
    fn regression_a_replay_uses_its_own_incremental_dir() {
        let args = replay_args(&cargo_argv(), &[], Path::new("/t/obj"));
        assert!(args.contains(&"incremental=/t/debug/incremental-hotpatch".to_string()), "{args:?}");
        assert!(!args.contains(&"incremental=/t/debug/incremental".to_string()));
    }

    /// Objects go to a private directory; `-Cmetadata`, which seeds the
    /// symbol hashes the jump table pairs on, is untouched.
    #[test]
    fn a_replay_writes_elsewhere_and_keeps_its_symbol_seed() {
        let args = replay_args(&cargo_argv(), &["-Crelocation-model=pic".into()], Path::new("/t/obj"));
        let at = args.iter().position(|a| a == "--out-dir").unwrap();
        assert_eq!(args[at + 1], "/t/obj");
        assert!(args.contains(&"metadata=abc".to_string()));
        assert_eq!(args.last().unwrap(), "-Crelocation-model=pic");
        assert_eq!(
            replay_out_dir(&cargo_argv()).unwrap(),
            PathBuf::from("/t/debug/idealyst-hotpatch-obj/app")
        );
    }

    /// Regression: a crate that partitions into ONE codegen unit gets its
    /// unit copied to `<crate><extra>.o` while the numbered file is kept,
    /// and linking both failed every patch of a small workspace library
    /// with `duplicate symbol` (found by the two-crate roundtrip).
    #[test]
    fn regression_a_single_unit_crates_copy_is_not_linked_twice() {
        let args: Vec<String> = ["--crate-name", "rt_shared", "-C", "extra-filename=-cc4a", "--out-dir", "/o"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let listed = vec![
            PathBuf::from("/o/rt_shared-cc4a.o"),
            PathBuf::from("/o/rt_shared-cc4a.9y8q.rcgu.o"),
            PathBuf::from("/o/rt_shared-cc4a.d"),
        ];
        assert_eq!(
            replay_objects(listed, &args),
            vec![PathBuf::from("/o/rt_shared-cc4a.9y8q.rcgu.o")]
        );
    }

    /// Many units: rustc makes no copy, and every unit is kept. An object
    /// with no numbered sibling (a non-incremental single-object compile)
    /// is the only object, and kept too.
    #[test]
    fn every_numbered_unit_is_kept_and_a_lone_object_is_not_dropped() {
        let args: Vec<String> =
            ["--crate-name", "app", "-Cextra-filename=-ab"].iter().map(|s| s.to_string()).collect();
        let many = vec![PathBuf::from("/o/app-ab.b.rcgu.o"), PathBuf::from("/o/app-ab.a.rcgu.o")];
        assert_eq!(
            replay_objects(many, &args),
            vec![PathBuf::from("/o/app-ab.a.rcgu.o"), PathBuf::from("/o/app-ab.b.rcgu.o")]
        );
        assert_eq!(
            replay_objects(vec![PathBuf::from("/o/app-ab.o")], &args),
            vec![PathBuf::from("/o/app-ab.o")]
        );
    }

    fn write(dir: &Path, name: &str) {
        let capture = CapturedInvocation {
            rustc: format!("/usr/bin/rustc-{name}"),
            args: vec!["--crate-name".into(), "app".into()],
            cwd: "/tmp".into(),
            env: Vec::new(),
        };
        std::fs::write(
            dir.join(name),
            serde_json::to_string(&capture).unwrap(),
        )
        .unwrap();
    }

    /// Regression: the standard idealyst shape is a library plus a
    /// binary of the SAME name, whose `src/main.rs` is one `entry!`
    /// line. Both get a capture, directory order is undefined, and
    /// taking "the first" replayed the binary about half the time —
    /// producing a patch that contained an `entry!` expansion and none
    /// of the app.
    #[test]
    fn regression_a_lib_and_a_bin_of_one_name_resolve_to_the_lib() {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-capture-rank-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "my_app.bin.json");
        write(&dir, "my_app.rlib.json");

        let found = find_capture(&dir, "my-app").unwrap();
        assert_eq!(
            found.rustc, "/usr/bin/rustc-my_app.rlib.json",
            "the library capture is the one with the components in it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A crate with only a binary still resolves — an app whose whole
    /// source is in `main.rs` is unusual but legal.
    #[test]
    fn a_bin_only_crate_still_resolves() {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-capture-binonly-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "solo.bin.json");
        assert!(find_capture(&dir, "solo").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The file paths named by `--extern name=path` arguments.
fn extern_paths(args: &[String]) -> impl Iterator<Item = &str> {
    args.windows(2)
        .filter(|w| w[0] == "--extern")
        .filter_map(|w| w[1].split_once('=').map(|(_, p)| p))
}

#[cfg(test)]
mod extern_path_tests {
    #[test]
    fn extern_paths_reads_name_equals_path_pairs() {
        let args: Vec<String> = ["--extern", "a=/x/liba.rmeta", "-L", "y", "--extern", "noprelude:b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(super::extern_paths(&args).collect::<Vec<_>>(), vec!["/x/liba.rmeta"]);
    }
}
