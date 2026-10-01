//! Resuming the previous session's base: a restart after hot patches
//! serves the bundle the last session built and replays its patches,
//! instead of compiling the patched sources into a new base.
//!
//! # Why
//!
//! A hot patch never rebuilds the base: the page runs the base plus the
//! patch. So a session stopped after patches leaves sources that differ
//! from the last built base, and the next session's start compiled that
//! difference — measured on CrewForge (aarch64), one patched screen
//! crate: launch → page 11–18 s, all of it the cargo build (7–15 s) and
//! the packaging passes (~2 s); with nothing changed it is 3–7 s. The
//! session that was stopped had already PAID for that code: the base on
//! disk, its captured rustc invocations, and the latest patch module are
//! all still there, and are exactly what the page was running.
//!
//! # What is kept, and when it is used
//!
//! After every base build and every pushed patch the watch loop writes a
//! [`State`] beside the base's captures: the archives the next save is
//! decided against, the crates patched since the base, the patches a
//! page loading the base needs (`ReloadSignal::connect_snapshot`), the
//! newest hot patch's module (hard-linked, since restaging clears the
//! served bundle), and what the base was built FROM.
//!
//! A session resumes only when nothing outside the workspace crates'
//! `.rs` sources can have moved since that base was built — the same
//! inputs an uninterrupted session would have had to rebuild for:
//!
//! - the same `idealyst` executable (its captured wrapper, its scanner),
//!   the same `rustc -vV`, and the same `CARGO_*` / `RUST*` environment;
//! - every other file under the watched roots (manifests, assets under
//!   `src/`, every source of a path dependency outside the workspace —
//!   a `[patch]`ed framework included), every `Cargo.lock`, cargo config
//!   and `rust-toolchain` file from the project up, at the same length
//!   and modification time;
//! - the packaged base module unchanged, and cargo's own output still the
//!   one it was packaged from ([`build_web::restage`] refuses otherwise).
//!
//! Anything else is a cold start, as before. Workspace `.rs` files that
//! moved since the last patch — edits made while no session ran — are
//! handed to the loop as a save, and decided like any save: an overlay
//! patch, a hot patch, or the rebuild a shape change needs.
//!
//! A state is only recorded for a base whose inputs were all older than
//! its build's start: a file written during the build may or may not be
//! in it, and that session's own watcher deals with it, not a resume.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// The document's format. A state of another version is ignored.
pub(crate) const STATE_VERSION: u32 = 1;

/// A file's identity as cargo's fingerprints see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileStamp {
    pub len: u64,
    pub mtime_ns: u128,
}

impl FileStamp {
    pub fn of(path: &Path) -> Option<Self> {
        let md = std::fs::metadata(path).ok()?;
        let mtime_ns = md
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            len: md.len(),
            mtime_ns,
        })
    }

    fn older_than(&self, t: SystemTime) -> bool {
        let t = t
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.mtime_ns < t
    }
}

/// One patch a page loading the base needs, as `connect_snapshot` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Replayed {
    pub hot: bool,
    pub json: String,
}

/// What a resuming session needs from the one before it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct State {
    pub version: u32,
    /// [`dev_overlay::archive::scanner_identity`]: the `idealyst` that
    /// built the base. Its path is the captured rustc wrapper.
    pub program: String,
    /// [`environment`] when the base was built.
    pub environment: String,
    /// The packaged module the page runs, and its stamp.
    pub served: PathBuf,
    pub served_stamp: FileStamp,
    /// [`inputs`] when the base was built.
    pub inputs: Vec<(PathBuf, FileStamp)>,
    /// Each workspace crate's archive, as the next save would be decided
    /// against it.
    pub archives: BTreeMap<String, Option<dev_overlay::DescriptorSet>>,
    /// Every crate a hot patch re-emitted since the base.
    pub patched: BTreeSet<String>,
    /// What a page loading the base is replayed, in order.
    pub replay: Vec<Replayed>,
    /// The newest hot patch's file name under the served `pkg/hotpatch/`,
    /// kept as `<state dir>/resume/<name>`.
    pub patch_file: Option<String>,
    /// The last patch number written: a resumed builder counts on from it.
    pub serial: u64,
    /// The replayed objects the builder could reuse, `(crate, source key,
    /// objects)` (`WasmPatchBuilder::reusable_objects`): a resumed
    /// builder's first patch then replays only what moved.
    #[serde(default)]
    pub objects: Vec<(String, String, Vec<PathBuf>)>,
}

/// Where a web target dir keeps the state: beside the base's captures.
pub(crate) fn state_path(target_dir: &Path) -> PathBuf {
    hotpatch_dir(target_dir).join("resume.json")
}

/// Where the newest patch module is kept between sessions.
pub(crate) fn kept_patches_dir(target_dir: &Path) -> PathBuf {
    hotpatch_dir(target_dir).join("resume")
}

fn hotpatch_dir(target_dir: &Path) -> PathBuf {
    let captures = build_web::captures_dir(target_dir);
    captures.parent().map(Path::to_path_buf).unwrap_or(captures)
}

/// The toolchain and the environment cargo and rustc read: `rustc -vV`
/// (as the `rustc` cargo would run resolves it, `RUSTC` or the PATH one,
/// under any `RUSTUP_TOOLCHAIN`), and every `CARGO_*` / `RUST*` variable
/// except the ones that only change logging.
///
/// Run in `project_dir`, as cargo runs it: rustup picks the toolchain
/// from the `rust-toolchain(.toml)` it finds there.
pub(crate) fn environment(project_dir: &Path) -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("-vV")
        .current_dir(project_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_else(|| "rustc: unavailable".into());
    let mut out = version;
    out.push_str(&environment_vars(std::env::vars()));
    out
}

fn environment_vars(vars: impl Iterator<Item = (String, String)>) -> String {
    const NOISE: &[&str] = &[
        "CARGO_LOG",
        "RUST_LOG",
        "RUST_BACKTRACE",
        "RUST_LIB_BACKTRACE",
    ];
    let mut keep: Vec<(String, String)> = vars
        .filter(|(k, _)| k.starts_with("CARGO_") || k.starts_with("RUST"))
        .filter(|(k, _)| !NOISE.contains(&k.as_str()) && !k.starts_with("CARGO_TERM_"))
        .collect();
    keep.sort();
    keep.into_iter()
        .map(|(k, v)| format!("\n{k}={v}"))
        .collect()
}

/// Every input of the base that is NOT a workspace crate's `.rs` source:
/// each file under `roots` (the watch roots: each local crate's `src/` and
/// `Cargo.toml`), minus the `.rs` files under the `src/` of a crate in
/// `workspace_dirs`, plus every `Cargo.lock`, `.cargo/config(.toml)` and
/// `rust-toolchain(.toml)` from `project_dir` up. Sorted by path.
pub(crate) fn inputs(
    project_dir: &Path,
    roots: &[PathBuf],
    workspace_dirs: &[PathBuf],
) -> Vec<(PathBuf, FileStamp)> {
    let skip: Vec<PathBuf> = workspace_dirs.iter().map(|d| d.join("src")).collect();
    let mut out = BTreeMap::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
            continue;
        }
        let workspace_source =
            path.extension().is_some_and(|e| e == "rs") && skip.iter().any(|s| path.starts_with(s));
        if workspace_source {
            continue;
        }
        if let Some(stamp) = FileStamp::of(&path) {
            out.insert(path, stamp);
        }
    }
    for dir in project_dir.ancestors() {
        for name in [
            "Cargo.lock",
            ".cargo/config.toml",
            ".cargo/config",
            "rust-toolchain",
            "rust-toolchain.toml",
        ] {
            let path = dir.join(name);
            if let Some(stamp) = FileStamp::of(&path) {
                out.insert(path, stamp);
            }
        }
    }
    out.into_iter().collect()
}

/// [`inputs`], but `None` when any of them was written at or after
/// `build_started`: such a file may or may not be in the base, and only
/// a full build can tell. With `sources_too`, the workspace's `.rs`
/// sources are held to the same rule — for the session's FIRST base,
/// whose archives are scanned after the build rather than read before it.
pub(crate) fn inputs_before(
    project_dir: &Path,
    roots: &[PathBuf],
    workspace_dirs: &[PathBuf],
    build_started: SystemTime,
    sources_too: bool,
) -> Option<Vec<(PathBuf, FileStamp)>> {
    let stamped = inputs(project_dir, roots, workspace_dirs);
    if !stamped.iter().all(|(_, s)| s.older_than(build_started)) {
        return None;
    }
    if !sources_too {
        return Some(stamped);
    }
    let mut stack: Vec<PathBuf> = workspace_dirs.iter().map(|d| d.join("src")).collect();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            if !FileStamp::of(&path).is_some_and(|s| s.older_than(build_started)) {
                return None;
            }
        }
    }
    Some(stamped)
}

/// Read a state, or `None` when there is none, or it is unreadable or of
/// another version.
pub(crate) fn load(path: &Path) -> Option<State> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: State = serde_json::from_str(&text).ok()?;
    (state.version == STATE_VERSION).then_some(state)
}

/// Write `state` atomically: a session killed mid-write leaves the old
/// state or the new one, never half of one.
pub(crate) fn save(path: &Path, state: &State) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(state)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Forget the state: the base it describes is about to be replaced.
pub(crate) fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// What the current program and machine look like, to check a state
/// against.
pub(crate) struct Current<'a> {
    pub program: Option<String>,
    pub environment: String,
    pub inputs: &'a [(PathBuf, FileStamp)],
}

/// Why `state` cannot be resumed under `now`, or `None` when it can.
pub(crate) fn refusal(state: &State, now: &Current<'_>) -> Option<String> {
    if now.program.as_deref() != Some(state.program.as_str()) {
        return Some("the idealyst executable changed".into());
    }
    if now.environment != state.environment {
        return Some("the toolchain or the CARGO_* / RUST* environment changed".into());
    }
    if FileStamp::of(&state.served) != Some(state.served_stamp) {
        return Some("the packaged base module changed".into());
    }
    if now.inputs != state.inputs.as_slice() {
        let before: BTreeMap<&PathBuf, &FileStamp> =
            state.inputs.iter().map(|(p, s)| (p, s)).collect();
        let after: BTreeMap<&PathBuf, &FileStamp> =
            now.inputs.iter().map(|(p, s)| (p, s)).collect();
        let moved = before
            .keys()
            .chain(after.keys())
            .find(|p| before.get(*p) != after.get(*p))
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        return Some(format!("{moved} changed since the base was built"));
    }
    None
}

/// The `.rs` files of the workspace whose contents differ from what
/// `archives` describe — edits made while no session ran — as absolute
/// paths: changed, added, or removed. A crate with no archive is reported
/// whole (every file it has), so the decision rebuilds it.
pub(crate) fn moved_sources(
    archives: &BTreeMap<String, Option<dev_overlay::DescriptorSet>>,
    read: &[(String, PathBuf, Option<dev_overlay::archive::CrateSources>)],
) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();
    for (package, dir, sources) in read {
        let archive = archives.get(package).and_then(Option::as_ref);
        let Some(sources) = sources else { continue };
        let Some(archive) = archive else {
            out.extend(sources.files.iter().map(|(rel, _)| dir.join(rel)));
            continue;
        };
        if sources.build_key() == archive.build_key() {
            continue;
        }
        let now: BTreeMap<&str, &str> = sources
            .files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        for (rel, text) in &now {
            let same = archive
                .files
                .get(*rel)
                .is_some_and(|d| d.content == dev_overlay::archive::digest(text.as_bytes()));
            if !same {
                out.insert(dir.join(rel));
            }
        }
        for rel in archive.files.keys() {
            if !now.contains_key(rel.as_str()) {
                out.insert(dir.join(rel));
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_environment_ignores_logging_but_not_flags() {
        let a = environment_vars(
            [
                ("CARGO_LOG", "x"),
                ("RUSTFLAGS", "-Cfoo"),
                ("HOME", "/h"),
                ("CARGO_TERM_COLOR", "always"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string())),
        );
        assert_eq!(a, "\nRUSTFLAGS=-Cfoo");
    }

    /// The inputs a resume checks are everything cargo reads EXCEPT the
    /// workspace's `.rs` sources, which the archives cover by content.
    #[test]
    fn inputs_skip_workspace_sources_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        let ws = root.join("crates/screen");
        let outside = root.join("vendor/fw");
        write(&ws.join("src/lib.rs"), "fn a() {}");
        write(&ws.join("src/logo.svg"), "<svg/>");
        write(&ws.join("Cargo.toml"), "[package]");
        write(&outside.join("src/lib.rs"), "fn b() {}");
        write(&root.join("Cargo.lock"), "lock");
        let roots = vec![ws.join("src"), ws.join("Cargo.toml"), outside.join("src")];
        let got: Vec<PathBuf> = inputs(&ws, &roots, &[ws.clone()])
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert!(got.contains(&ws.join("src/logo.svg")));
        assert!(got.contains(&ws.join("Cargo.toml")));
        assert!(
            got.contains(&outside.join("src/lib.rs")),
            "a path dependency's sources count"
        );
        assert!(got.contains(&root.join("Cargo.lock")));
        assert!(
            !got.contains(&ws.join("src/lib.rs")),
            "the archives cover workspace sources"
        );
    }

    /// A base whose inputs moved during its build is never recorded: the
    /// file may or may not be in it.
    #[test]
    fn a_file_written_during_the_build_makes_the_base_unresumable() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("screen");
        write(&ws.join("src/lib.rs"), "fn a() {}");
        write(&ws.join("Cargo.toml"), "[package]");
        let roots = vec![ws.join("src"), ws.join("Cargo.toml")];
        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        assert!(inputs_before(&ws, &roots, &[ws.clone()], later, true).is_some());
        let earlier = SystemTime::now() - std::time::Duration::from_secs(60);
        assert!(
            inputs_before(&ws, &roots, &[ws.clone()], earlier, false).is_none(),
            "a manifest written after"
        );
        // A workspace source written after the build started counts for
        // the first base, whose archive is scanned after the build; not
        // for a rebuild, whose archive was read before it.
        let only_src = tmp.path().join("other");
        write(&only_src.join("src/lib.rs"), "fn a() {}");
        assert!(inputs_before(&only_src, &[], &[only_src.clone()], earlier, true).is_none());
        assert!(inputs_before(&only_src, &[], &[only_src.clone()], earlier, false).is_some());
    }

    fn state(tmp: &Path, inputs: Vec<(PathBuf, FileStamp)>) -> State {
        let served = tmp.join("app_bg.wasm");
        write(&served, "base");
        State {
            version: STATE_VERSION,
            program: "idealyst".into(),
            environment: "env".into(),
            served_stamp: FileStamp::of(&served).unwrap(),
            served,
            inputs,
            archives: BTreeMap::new(),
            patched: BTreeSet::new(),
            replay: vec![Replayed {
                hot: true,
                json: "{}".into(),
            }],
            patch_file: Some("patch-3.wasm".into()),
            serial: 3,
            objects: vec![("app".into(), "key".into(), vec![tmp.join("app.o")])],
        }
    }

    #[test]
    fn a_state_round_trips_and_resumes_only_when_nothing_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let toml = tmp.path().join("Cargo.toml");
        write(&toml, "[package]");
        let inputs = vec![(toml.clone(), FileStamp::of(&toml).unwrap())];
        let s = state(tmp.path(), inputs.clone());
        let path = tmp.path().join("hp/resume.json");
        save(&path, &s).unwrap();
        assert_eq!(load(&path), Some(s.clone()));

        let now = |program: &str, env: &str, inputs: &[(PathBuf, FileStamp)]| {
            refusal(
                &s,
                &Current {
                    program: Some(program.into()),
                    environment: env.into(),
                    inputs: &inputs.to_vec(),
                },
            )
        };
        assert_eq!(now("idealyst", "env", &inputs), None);
        assert!(now("idealyst-2", "env", &inputs).is_some(), "another CLI");
        assert!(
            now("idealyst", "env2", &inputs).is_some(),
            "another toolchain"
        );
        let moved = vec![(
            toml.clone(),
            FileStamp {
                len: 1,
                mtime_ns: 1,
            },
        )];
        assert!(now("idealyst", "env", &moved)
            .unwrap()
            .contains("Cargo.toml"));
        let added = vec![
            inputs[0].clone(),
            (
                tmp.path().join("build.rs"),
                FileStamp {
                    len: 1,
                    mtime_ns: 1,
                },
            ),
        ];
        assert!(now("idealyst", "env", &added).is_some(), "a new input");
        write(&s.served, "rebuilt base");
        assert!(
            now("idealyst", "env", &inputs).is_some(),
            "another packaged base"
        );

        clear(&path);
        assert_eq!(load(&path), None);
    }

    /// Sources edited while no session ran are handed to the loop as a
    /// save: exactly the files whose contents moved, added or removed.
    #[test]
    fn moved_sources_are_the_files_whose_contents_differ() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("screen");
        write(
            &dir.join("Cargo.toml"),
            "[package]\nname = \"screen\"\nversion = \"0.1.0\"\n",
        );
        write(&dir.join("src/lib.rs"), "mod a;\nfn x() {}\n");
        write(&dir.join("src/a.rs"), "fn y() {}\n");
        write(&dir.join("src/gone.rs"), "fn z() {}\n");
        let sources = dev_overlay::archive::read_crate(&dir).unwrap();
        let archive = dev_overlay::archive::scan_sources(&sources);
        let archives = BTreeMap::from([("screen".to_string(), Some(archive))]);
        let read = |d: &Path| {
            vec![(
                "screen".to_string(),
                d.to_path_buf(),
                dev_overlay::archive::read_crate(d).ok(),
            )]
        };
        assert!(moved_sources(&archives, &read(&dir)).is_empty());

        write(&dir.join("src/a.rs"), "fn y() { 1; }\n");
        write(&dir.join("src/new.rs"), "fn n() {}\n");
        std::fs::remove_file(dir.join("src/gone.rs")).unwrap();
        assert_eq!(
            moved_sources(&archives, &read(&dir)),
            vec![
                dir.join("src/a.rs"),
                dir.join("src/gone.rs"),
                dir.join("src/new.rs")
            ]
        );
    }
}
