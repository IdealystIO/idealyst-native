//! Housekeeping for the cargo target dirs the CLI builds into.
//!
//! Cargo never deletes a compilation unit it has stopped using, and rustc
//! only collects stale *sessions* inside the one incremental cache dir it
//! is compiling into. So every rehash of a crate (a framework release, a
//! `cargo update`, a feature change) leaves the previous unit — rlib,
//! fingerprint, build-script output, incremental cache — behind for good.
//! Measured on CrewForge (2026-10-08): an outside GC reclaimed 4.2 GB from
//! one iOS wrapper target; `crewforge_main` had 12 dead incremental caches
//! in its web build's `incremental/` and 19 in `incremental-hotpatch/`,
//! ~10 GB each.
//!
//! One module, two ways of deciding what is superseded:
//!
//! - **Records (exact)** — wherever the CLI runs the cargo build itself.
//!   Every successful build RECORDS the units it consists of
//!   ([`build_and_prune`](crate::target_gc::build_and_prune) / [`record_and_prune`](crate::target_gc::record_and_prune)), and a prune removes
//!   exactly the units no recorded variant uses. Used by every generated
//!   platform wrapper, the web build, the dev server's build and the MCP
//!   catalog build.
//! - **Recency (keep-newest-N)** — only where no record can exist: a
//!   profile dir an older CLI built (`idealyst clean --stale`), and the
//!   hot-patch replay's `incremental-hotpatch/`, which rustc writes outside
//!   cargo, so nothing reports it ([`prune_incremental_dirs`](crate::target_gc::prune_incremental_dirs),
//!   [`prune_units_keep_newest`](crate::target_gc::prune_units_keep_newest)).
//!
//! Both go through the same guard: a profile dir is only touched while
//! holding cargo's own exclusive build locks on it ([`CargoBuildLocks`](crate::target_gc::CargoBuildLocks)).
//! Deleting a unit that was still wanted is never a correctness problem —
//! cargo simply compiles it again.
//!
//! # Why records beat "keep the newest N"
//!
//! A target dir legitimately holds several live builds at once: a
//! wrapper's `idealyst dev` build (`--features dev`) beside its
//! `idealyst build` / `run` one; several apps sharing a workspace's
//! `idealyst-web-<key>` or `idealyst-dev-server`; three catalog extractor
//! flavours in `idealyst-mcp`. Recency can't tell those apart from
//! superseded generations — ten dev rebuilds would push out the one
//! current non-dev build. A record names exactly what each variant uses.
//!
//! # Where the live set comes from
//!
//! `cargo build --message-format=json…` reports every unit of the build,
//! fresh or recompiled, with its output paths; each carries cargo's 16-hex
//! unit hash (`deps/libfoo-<hash>.rlib`, `build/foo-<hash>/…`), which is
//! also the hash naming the unit's `.fingerprint/` dir (checked against
//! cargo 1.97). Two things are not reported that way, and each is
//! recovered precisely:
//!
//! - **Units whose outputs carry no hash.** For a root `bin`/`staticlib`
//!   cargo reports only the UPLIFTED copy (`<triple>/debug/libapp.a`,
//!   `debug/app-macos`), and it drops the hash from EVERY output of a path
//!   package's `cdylib`/`dylib` lib target (`backend-android-mobile` →
//!   `deps/libbackend_android.rlib`). Such a unit is found from its
//!   fingerprint instead: the `.fingerprint/<package>-<hash>/*.json` whose
//!   recorded dependency fingerprints are all fingerprints of this build's
//!   units (resolved to a fixpoint, since one can depend on another).
//!   Differently-featured variants depend on differently-featured crates,
//!   so they never match each other's build. The fingerprint FILE holds
//!   the u64 as the hex of its little-endian bytes; the JSON holds it as
//!   a number.
//! - **Incremental caches.** rustc names them by the stable crate id
//!   (`<crate>-<base36>`), not cargo's hash. A cache dir is attributed to
//!   the variant whose build wrote into it (its mtime moved during that
//!   build); a crate that was fresh keeps the cache its variant last
//!   recorded. One name is shared by many units — rustc calls EVERY build
//!   script `build_script_build` — so a crate name maps to a SET of caches,
//!   and for a shared name a recompile adds to the set instead of replacing
//!   it (one build script rebuilding says nothing about the others; the
//!   first version of this deleted all but one build script's cache after
//!   every build). The cost is one superseded build-script cache, tens of
//!   KB, per rehash of a path package with a `build.rs`.
//!
//! # Layout and scope
//!
//! Records live in the profile dir the build compiled its own units into
//! — `<target>/<triple>/<profile>` with `--target`, `<target>/<profile>`
//! without — as `.idealyst-live/<variant>.json`. A prune deletes anything
//! no record keeps, so it only ever runs on a target dir the CLI owns
//! outright; pointing it at a shared `target/` would delete the app's own
//! build.
//!
//! The host dir `<target>/<profile>` (proc-macros, build scripts, and a
//! no-`--target` build's own units) is shared by every triple built with
//! that profile, so it is pruned only when every `<target>/<triple>/<profile>`
//! beside it carries a record — an older device build with no record yet
//! keeps its host units.
//!
//! A variant that has not been built since records existed has none, so
//! its units read as superseded in a dir another variant has a record in:
//! the first build after upgrading costs the OTHER variant one rebuild.
//!
//! In a dir shared by many tenants (apps, project sets) a record can
//! outlive its tenant — an app deleted, a project set never catalogued
//! again — and would pin its units forever. Such builds pass a
//! [`BuildSpec::record_ttl`](crate::target_gc::BuildSpec::record_ttl); the record carries it, and a prune ignores
//! (and removes) a record not refreshed within it. A wrapper's records
//! have none: its variants are fixed, and a rarely-run one (a release
//! device build) must not lose its units for being rare.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The incremental cache dirs a profile dir can hold: cargo's own, and the
/// hot-patch replay's private one (`<dir>-hotpatch`, see
/// `build_runtime_server::hotpatch::replay` — the replay compiles with
/// different tracked flags, so the two can never share a cache).
pub const INCREMENTAL_DIRS: &[&str] = &["incremental", "incremental-hotpatch"];

/// Subdir of a profile dir holding one record per built variant.
pub const RECORDS_DIR: &str = ".idealyst-live";

/// The per-unit dirs of a profile dir, entries named `<name>-<16 hex>…`.
const UNIT_DIRS: &[&str] = &["deps", ".fingerprint", "build"];

// ─────────────────────────────────────────────────────────────────────
// Locks, layout, sizes
// ─────────────────────────────────────────────────────────────────────

/// Every profile dir under `target_dir` — `<target>/<profile>` and
/// `<target>/<triple>/<profile>` — recognised by a `deps`, `.fingerprint`
/// or incremental dir, or cargo's lock file. Not by incremental alone: a
/// release profile has none, and [`CargoBuildLocks::try_acquire_all`]
/// must still see (and respect) a running release build's lock.
pub fn profile_dirs(target_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(target_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if is_profile_dir(&path) {
            out.push(path.clone());
        }
        // A host profile dir is checked one level deeper too: harmless (its
        // subdirs are `deps`, `build`, … — never profile-shaped).
        if let Ok(inner) = fs::read_dir(&path) {
            out.extend(inner.flatten().map(|e| e.path()).filter(|p| is_profile_dir(p)));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn is_profile_dir(p: &Path) -> bool {
    p.is_dir()
        && (p.join("deps").is_dir()
            || p.join(".fingerprint").is_dir()
            || p.join(".cargo-lock").is_file()
            || INCREMENTAL_DIRS.iter().any(|d| p.join(d).is_dir()))
}

/// Cargo's exclusive build locks on one profile dir, held for as long as
/// this value lives.
///
/// Cargo holds `.cargo-lock` and `.cargo-build-lock` exclusively for a
/// whole build (measured with cargo 1.97), so holding both means no build
/// is writing into the profile dir and none can start until we drop them.
pub struct CargoBuildLocks(#[allow(dead_code)] Vec<fs::File>);

impl CargoBuildLocks {
    /// `None` if any lock is held by someone else (a build is running).
    /// A lock file that doesn't exist is not held by anyone.
    pub fn try_acquire(profile: &Path) -> Option<Self> {
        let mut held = Vec::new();
        for name in [".cargo-lock", ".cargo-build-lock"] {
            let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(profile.join(name))
            else {
                continue;
            };
            match file.try_lock() {
                Ok(()) => held.push(file),
                Err(_) => return None,
            }
        }
        Some(Self(held))
    }

    /// Locks on every profile dir under `target_dir`, all or nothing —
    /// what removing a whole target dir needs.
    pub fn try_acquire_all(target_dir: &Path) -> Option<Vec<Self>> {
        profile_dirs(target_dir).iter().map(|p| Self::try_acquire(p)).collect()
    }
}

/// Recursive size of `path` in bytes. A symlink counts as nothing and is
/// never followed out of the tree.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| dir_size(&e.path()))
        .sum()
}

/// Remove a file or dir (dry run: only measure it). Returns the bytes
/// freed, `None` if nothing was removed.
fn remove(path: &Path, dry_run: bool) -> Option<u64> {
    let meta = fs::symlink_metadata(path).ok()?;
    let size = dir_size(path);
    if dry_run {
        return Some(size);
    }
    let ok = if meta.is_dir() { fs::remove_dir_all(path).is_ok() } else { fs::remove_file(path).is_ok() };
    ok.then_some(size)
}

/// What a prune removed (or, in a dry run, would remove).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GcReport {
    /// Unit entries (files / dirs under `deps`, `.fingerprint`, `build`).
    pub units_removed: usize,
    /// Incremental cache dirs.
    pub incremental_removed: usize,
    /// Bytes freed.
    pub bytes_freed: u64,
    /// Profile dirs skipped because a build held cargo's lock on them.
    pub skipped_locked: usize,
    /// Packages whose current unit could not be identified from its
    /// fingerprint, so every unit of theirs was kept.
    pub unidentified: Vec<String>,
}

impl GcReport {
    /// One line for the build log; `None` when nothing happened.
    pub fn summary(&self) -> Option<String> {
        if self.units_removed + self.incremental_removed == 0 && self.unidentified.is_empty() {
            return None;
        }
        let mut s = format!(
            "pruned {} superseded unit file(s) and {} incremental cache(s), freed {:.2} GB",
            self.units_removed,
            self.incremental_removed,
            self.bytes_freed as f64 / 1e9,
        );
        if !self.unidentified.is_empty() {
            s.push_str(&format!(
                " (kept every unit of {}: current one not identified from its fingerprint)",
                self.unidentified.join(", ")
            ));
        }
        Some(s)
    }

    fn absorb(&mut self, other: GcReport) {
        self.units_removed += other.units_removed;
        self.incremental_removed += other.incremental_removed;
        self.bytes_freed += other.bytes_freed;
        self.skipped_locked += other.skipped_locked;
        self.unidentified.extend(other.unidentified);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Recording a build
// ─────────────────────────────────────────────────────────────────────

/// One CLI-run cargo build to record, and prune after.
#[derive(Debug, Clone)]
pub struct BuildSpec<'a> {
    /// The target dir the build compiles into. The prune deletes whatever
    /// no record there keeps, so nothing but recorded builds may use it.
    pub target_dir: &'a Path,
    /// `--target` triple, or `None` for a host build without `--target`
    /// (its units then live in the host dir `<target>/<profile>`).
    pub triple: Option<&'a str>,
    /// `--release`?
    pub release: bool,
    /// What this build is, among the builds sharing the dir — names its
    /// record. A rebuild of the same variant replaces its record. See
    /// [`variant_key`].
    pub variant: String,
    /// Expire this record if it is not refreshed within this long. For a
    /// dir shared by tenants that can disappear; see the module docs.
    pub record_ttl: Option<Duration>,
}

impl<'a> BuildSpec<'a> {
    /// A generated wrapper's build: the variant is its `--features` set
    /// (`dev`, or none), no expiry.
    pub fn wrapper(
        target_dir: &'a Path,
        triple: Option<&'a str>,
        release: bool,
        features: &[String],
    ) -> Self {
        let parts: Vec<&str> = features.iter().map(String::as_str).collect();
        Self { target_dir, triple, release, variant: features_variant(&parts), record_ttl: None }
    }

    fn profile(&self) -> &'static str {
        if self.release { "release" } else { "debug" }
    }

    /// The profile dir this build compiles its own units into.
    pub fn build_profile_dir(&self) -> PathBuf {
        match self.triple {
            Some(t) => self.target_dir.join(t).join(self.profile()),
            None => self.target_dir.join(self.profile()),
        }
    }

    fn host_profile_dir(&self) -> PathBuf {
        self.target_dir.join(self.profile())
    }
}

/// A feature set as a variant name: sorted, deduped, `+`-joined;
/// `default` when empty.
pub fn features_variant(features: &[&str]) -> String {
    let mut f: Vec<&str> = features.to_vec();
    f.sort_unstable();
    f.dedup();
    if f.is_empty() { "default".to_string() } else { variant_key(&f.join("+")) }
}

/// A record file name from any text (an app name, a manifest path plus
/// arguments): kept readable where it is safe, with a content hash so two
/// keys that sanitize alike never share a record.
pub fn variant_key(raw: &str) -> String {
    let clean: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "+-_.".contains(c) { c } else { '_' })
        .collect();
    if clean == raw && clean.len() <= 80 && !clean.starts_with('.') {
        return clean;
    }
    let short: String = clean.chars().rev().take(48).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{}-{:016x}", short.trim_start_matches(['.', '_']), fnv1a(raw.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Run `cmd` (a `cargo build` invocation, without `--message-format`) and,
/// once it succeeds, record this variant's units and prune everything no
/// recorded variant uses.
///
/// The build's own failure is the error; a pruning failure only logs —
/// a cache we fail to trim costs disk, never a build.
pub fn build_and_prune(cmd: &mut Command, spec: &BuildSpec<'_>) -> Result<(CargoUnits, GcReport)> {
    let before = IncrementalSnapshot::take(spec.target_dir);
    let units = run_cargo_tracked(cmd)?;
    let report = record_and_prune_or_log(spec, &units, &before);
    Ok((units, report))
}

/// [`record_and_prune`], logging (not returning) a failure: for callers
/// whose build succeeded and must not fail over housekeeping.
pub fn record_and_prune_or_log(
    spec: &BuildSpec<'_>,
    units: &CargoUnits,
    before: &IncrementalSnapshot,
) -> GcReport {
    record_and_prune(spec, units, before).unwrap_or_else(|e| {
        eprintln!("[target-gc] skipped pruning {}: {e:#}", spec.target_dir.display());
        GcReport::default()
    })
}

/// Every unit one cargo build consisted of.
#[derive(Debug, Default, Clone)]
pub struct CargoUnits {
    /// Cargo unit hashes reported for this build, fresh or not.
    pub hashes: BTreeSet<String>,
    /// Target (crate) names of PATH packages — the only ones cargo
    /// compiles incrementally — with `-` → `_`, as rustc names them.
    pub local_crates: BTreeSet<String>,
    /// Units cargo actually (re)compiled this build — 0 for a fully fresh
    /// one. For logs and tests.
    pub recompiled: usize,
    /// Packages of those units.
    pub recompiled_packages: BTreeSet<String>,
    /// Packages with a unit whose outputs carry NO hash, so the unit's
    /// hash has to be recovered from its fingerprint (see the module docs).
    pub unhashed_packages: BTreeSet<String>,
    /// The last executable the build reported (fresh or not) — a `--bin`
    /// build's binary.
    pub executable: Option<PathBuf>,
}

/// Run a cargo build with JSON messages on stdout (diagnostics still
/// rendered to stderr, so the user sees the normal output) and collect its
/// units. Anything on stdout that is not a cargo message is passed to
/// STDERR: the caller's stdout may be a protocol (the MCP server's stdio).
pub fn run_cargo_tracked(cmd: &mut Command) -> Result<CargoUnits> {
    cmd.arg("--message-format=json-render-diagnostics").stdout(Stdio::piped());
    let mut child = cmd.spawn().context("failed to spawn `cargo` — is it on your PATH?")?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut units = CargoUnits::default();
    for line in BufReader::new(stdout).lines() {
        let line = line.context("read cargo's output")?;
        if !units.absorb(&line) {
            eprintln!("{line}");
        }
    }
    let status = child.wait().context("wait for cargo")?;
    if !status.success() {
        anyhow::bail!("cargo build exited with {status}");
    }
    Ok(units)
}

impl CargoUnits {
    /// Fold one cargo JSON message in. False if it isn't one.
    pub fn absorb(&mut self, line: &str) -> bool {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        let Some(reason) = msg.get("reason").and_then(|r| r.as_str()) else {
            return false;
        };
        match reason {
            "compiler-artifact" => {
                let mut hashed = false;
                for f in msg["filenames"].as_array().into_iter().flatten() {
                    if let Some(f) = f.as_str() {
                        hashed |= path_hashes(Path::new(f)).next().is_some();
                        self.hashes.extend(path_hashes(Path::new(f)));
                    }
                }
                if let Some(exe) = msg["executable"].as_str() {
                    self.hashes.extend(path_hashes(Path::new(exe)));
                    self.executable = Some(PathBuf::from(exe));
                }
                if !hashed {
                    if let Some(name) = msg["package_id"].as_str().and_then(package_name) {
                        self.unhashed_packages.insert(name.to_string());
                    }
                }
                if msg["fresh"] == serde_json::Value::Bool(false) {
                    self.recompiled += 1;
                    if let Some(name) = msg["package_id"].as_str().and_then(package_name) {
                        self.recompiled_packages.insert(name.to_string());
                    }
                }
                let local = msg["package_id"].as_str().is_some_and(|id| id.starts_with("path+"));
                if local {
                    if let Some(name) = msg["target"]["name"].as_str() {
                        self.local_crates.insert(name.replace('-', "_"));
                    }
                }
            }
            "build-script-executed" => {
                if let Some(out) = msg["out_dir"].as_str() {
                    self.hashes.extend(path_hashes(Path::new(out)));
                }
            }
            _ => {}
        }
        true
    }
}

/// The package name in a cargo package id: `path+file:///a/b#name@1.0.0`
/// → `name`; `path+file:///a/name#1.0.0` (name = dir) → `name`.
fn package_name(id: &str) -> Option<&str> {
    let (url, frag) = id.rsplit_once('#')?;
    match frag.split_once('@') {
        Some((name, _)) => Some(name),
        None => url.rsplit('/').next().filter(|n| !n.is_empty()),
    }
}

/// Every cargo unit hash in a path's components.
fn path_hashes(path: &Path) -> impl Iterator<Item = String> + '_ {
    path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter_map(unit_hash)
        .map(str::to_string)
}

/// The cargo unit hash in an entry name: the `<16 lowercase hex>` after
/// the last `-` that is followed by exactly that and then `.` or the end.
/// `libfoo-0123456789abcdef.rlib`, `foo-0123456789abcdef`,
/// `foo-0123456789abcdef.cgu.rcgu.o` all yield the hex.
pub fn unit_hash(name: &str) -> Option<&str> {
    for (i, _) in name.match_indices('-').collect::<Vec<_>>().into_iter().rev() {
        let rest = &name[i + 1..];
        if rest.len() < 16 {
            continue;
        }
        let (hex, tail) = rest.split_at(16);
        let is_hex = hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if is_hex && (tail.is_empty() || tail.starts_with('.')) && i > 0 {
            return Some(hex);
        }
    }
    None
}

/// One variant's record: the units it consists of.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
struct Record {
    hashes: BTreeSet<String>,
    /// Profile dir (relative to the target dir) → crate name → the
    /// incremental cache dirs this variant compiles that crate into. Usually
    /// one; several for `build_script_build`, the crate name rustc gives
    /// EVERY build script.
    incremental: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    /// Seconds without a refresh after which the record no longer counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl_secs: Option<u64>,
}

/// mtimes of every incremental cache dir under a target dir.
#[derive(Debug, Default)]
pub struct IncrementalSnapshot(HashMap<PathBuf, SystemTime>);

impl IncrementalSnapshot {
    /// Take one BEFORE the build whose writes are to be attributed.
    pub fn take(target_dir: &Path) -> Self {
        let mut map = HashMap::new();
        for profile in profile_dirs(target_dir) {
            for (path, mtime) in incremental_caches(&profile.join("incremental")) {
                map.insert(path, mtime);
            }
        }
        Self(map)
    }

    fn touched(&self, path: &Path, mtime: SystemTime) -> bool {
        self.0.get(path) != Some(&mtime)
    }
}

/// `(<dir>/<crate>-<id>, mtime)` for each cache dir in one incremental dir.
fn incremental_caches(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect()
}

/// `<crate>` of a rustc incremental cache dir `<crate>-<base36 id>`.
fn incremental_crate(name: &str) -> Option<&str> {
    let (krate, id) = name.rsplit_once('-')?;
    let ok = !krate.is_empty()
        && !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
    ok.then_some(krate)
}

fn rel(target_dir: &Path, dir: &Path) -> String {
    dir.strip_prefix(target_dir).unwrap_or(dir).to_string_lossy().replace('\\', "/")
}

/// Record this build's units for its variant, then prune its profile dir
/// (and the host dir, when every triple beside it is recorded).
pub fn record_and_prune(
    spec: &BuildSpec<'_>,
    units: &CargoUnits,
    before: &IncrementalSnapshot,
) -> Result<GcReport> {
    let target = spec.target_dir;
    let build_profile = spec.build_profile_dir();
    let host_profile = spec.host_profile_dir();
    let mut profiles = vec![build_profile.clone()];
    if host_profile != build_profile {
        profiles.push(host_profile.clone());
    }
    let mut report = GcReport::default();

    // ── Units reported without a hash, from their fingerprints ──────
    //
    // A unit is current when every dependency fingerprint its own
    // fingerprint JSON records is the fingerprint of a unit of THIS build.
    // Resolved to a fixpoint, since one hash-less unit can depend on
    // another (the Android root cdylib on `backend-android-mobile`, itself
    // a path cdylib).
    let mut hashes = units.hashes.clone();
    let mut fingerprints = unit_fingerprints(&profiles, &hashes);
    let mut pending = units.unhashed_packages.clone();
    loop {
        let mut progressed = false;
        for pkg in pending.clone() {
            let matched: BTreeSet<String> = fingerprint_candidates(&profiles, &pkg)
                .into_iter()
                .filter(|(_, deps)| deps.as_ref().is_some_and(|d| d.iter().all(|f| fingerprints.contains(f))))
                .map(|(h, _)| h)
                .collect();
            if !matched.is_empty() {
                fingerprints.extend(unit_fingerprints(&profiles, &matched));
                hashes.extend(matched);
                pending.remove(&pkg);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    for pkg in &pending {
        // Format drift or a fingerprint we could not read: keep every unit
        // of the package rather than guess.
        let candidates = fingerprint_candidates(&profiles, pkg);
        if !candidates.is_empty() {
            report.unidentified.push(pkg.clone());
            hashes.extend(candidates.into_keys());
        }
    }

    // ── Incremental caches: written by this build, or carried forward ──
    let records_dir = build_profile.join(RECORDS_DIR);
    let record_path = records_dir.join(format!("{}.json", spec.variant));
    let previous: Record = fs::read(&record_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let local = &units.local_crates;
    let mut incremental = BTreeMap::new();
    for profile in &profiles {
        let key = rel(target, profile);
        let inc_dir = profile.join("incremental");
        // What this build wrote into, per crate.
        let mut touched: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (path, mtime) in incremental_caches(&inc_dir) {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(krate) = incremental_crate(name) else {
                continue;
            };
            if local.contains(krate) && before.touched(&path, mtime) {
                touched.entry(krate.to_string()).or_default().insert(name.to_string());
            }
        }
        // What this variant used before, still on disk.
        let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (krate, dirs) in previous.incremental.get(&key).into_iter().flatten() {
            if local.contains(krate) {
                let live: BTreeSet<String> = dirs.iter().filter(|d| inc_dir.join(d).is_dir()).cloned().collect();
                if !live.is_empty() {
                    map.insert(krate.clone(), live);
                }
            }
        }
        // A crate this build compiled replaces its cache — unless its name
        // is shared by several units (every build script is
        // `build_script_build`): then one recompiled build script says
        // nothing about the others, which keep theirs.
        for (krate, dirs) in touched {
            let entry = map.entry(krate).or_default();
            if dirs.len() == 1 && entry.len() <= 1 {
                *entry = dirs;
            } else {
                entry.extend(dirs);
            }
        }
        if !map.is_empty() {
            incremental.insert(key, map);
        }
    }

    let record = Record { hashes, incremental, ttl_secs: spec.record_ttl.map(|d| d.as_secs()) };
    fs::create_dir_all(&records_dir).with_context(|| format!("create {}", records_dir.display()))?;
    // Always written, even when unchanged: the file's mtime is the
    // record's "last refreshed" for `ttl_secs`.
    fs::write(&record_path, serde_json::to_vec_pretty(&record)?)
        .with_context(|| format!("write {}", record_path.display()))?;

    // ── Prune ────────────────────────────────────────────────────────
    for profile in &profiles {
        report.absorb(prune_profile_by_records(target, profile, false));
    }
    Ok(report)
}

/// Is `profile` the host dir `<target>/<profile>` (vs `<target>/<triple>/<profile>`)?
fn is_host_dir(target: &Path, profile: &Path) -> bool {
    profile.parent() == Some(target)
}

/// The record files in one profile dir, with their mtimes.
fn record_files(profile: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(profile.join(RECORDS_DIR)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| (e.path(), e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH)))
        .collect()
}

/// Every unexpired record in one profile dir, plus the paths of the
/// expired ones.
fn records_in(profile: &Path) -> (Vec<Record>, Vec<PathBuf>) {
    let now = SystemTime::now();
    let mut live = Vec::new();
    let mut expired = Vec::new();
    for (path, mtime) in record_files(profile) {
        let Some(record) = fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Record>(&b).ok())
        else {
            continue;
        };
        let age = now.duration_since(mtime).unwrap_or_default();
        match record.ttl_secs {
            Some(ttl) if age > Duration::from_secs(ttl) => expired.push(path),
            _ => live.push(record),
        }
    }
    (live, expired)
}

/// The records governing `profile`: its own, and for a host dir also
/// those of every `<target>/<triple>/<profile>` beside it. `None` when
/// the dir must not be pruned by records: it has none at all, or (host
/// dir) some triple dir beside it has never been recorded — its host
/// units are unknown.
fn governing_records(target: &Path, profile: &Path) -> Option<(Vec<Record>, Vec<PathBuf>)> {
    let (mut all, expired) = records_in(profile);
    let mut any_file = !record_files(profile).is_empty();
    if is_host_dir(target, profile) {
        let name = profile.file_name()?;
        for e in fs::read_dir(target).ok()?.flatten() {
            let triple_profile = e.path().join(name);
            if e.path() == profile || !triple_profile.join("deps").is_dir() {
                continue;
            }
            if record_files(&triple_profile).is_empty() {
                return None;
            }
            any_file = true;
            all.extend(records_in(&triple_profile).0);
        }
    }
    any_file.then_some((all, expired))
}

/// Does `profile` hold records governing it (see [`governing_records`])?
pub fn is_recorded(target: &Path, profile: &Path) -> bool {
    governing_records(target, profile).is_some()
}

/// Remove every unit entry and incremental cache in `profile` that no
/// governing record keeps (and expired records). Holds cargo's build locks
/// on the dir throughout. A dir without governing records is left alone.
fn prune_profile_by_records(target: &Path, profile: &Path, dry_run: bool) -> GcReport {
    let mut report = GcReport::default();
    let Some((records, expired)) = governing_records(target, profile) else {
        return report;
    };
    let Some(_locks) = CargoBuildLocks::try_acquire(profile) else {
        report.skipped_locked += 1;
        return report;
    };
    if !dry_run {
        for path in &expired {
            let _ = fs::remove_file(path);
        }
    }
    let live: BTreeSet<&str> = records.iter().flat_map(|r| r.hashes.iter().map(String::as_str)).collect();
    let key = rel(target, profile);
    let live_incremental: BTreeSet<&str> = records
        .iter()
        .filter_map(|r| r.incremental.get(&key))
        .flat_map(|m| m.values().flatten().map(String::as_str))
        .collect();

    for dir in UNIT_DIRS {
        let Ok(entries) = fs::read_dir(profile.join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(hash) = unit_hash(&name) else {
                continue;
            };
            if live.contains(hash) {
                continue;
            }
            if let Some(bytes) = remove(&e.path(), dry_run) {
                report.units_removed += 1;
                report.bytes_freed += bytes;
            }
        }
    }
    // `incremental/` only: `incremental-hotpatch/` is written outside
    // cargo, so no record names its caches — recency trims it
    // ([`prune_incremental_dirs`]).
    for (path, _) in incremental_caches(&profile.join("incremental")) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if incremental_crate(name).is_none() || live_incremental.contains(name) {
            continue;
        }
        if let Some(bytes) = remove(&path, dry_run) {
            report.incremental_removed += 1;
            report.bytes_freed += bytes;
        }
    }
    report
}

/// Prune every recorded profile dir under `target_dir` against its
/// records — what `idealyst clean --stale` runs, so it removes exactly
/// what the builds' own prunes would. Returns the report and the profile
/// dirs that have no governing records (for a recency fallback).
pub fn prune_recorded(target_dir: &Path, dry_run: bool) -> (GcReport, Vec<PathBuf>) {
    let mut report = GcReport::default();
    let mut unrecorded = Vec::new();
    for profile in profile_dirs(target_dir) {
        if is_recorded(target_dir, &profile) {
            report.absorb(prune_profile_by_records(target_dir, &profile, dry_run));
        } else {
            unrecorded.push(profile);
        }
    }
    (report, unrecorded)
}

/// Forget `variant`'s records in every profile dir under `target_dir`, so
/// the next prune there stops keeping its units — for a tenant that moved
/// to another dir (a web build slot whose config key changed).
pub fn forget_variant(target_dir: &Path, variant: &str) {
    for profile in profile_dirs(target_dir) {
        let _ = fs::remove_file(profile.join(RECORDS_DIR).join(format!("{variant}.json")));
    }
}

/// The fingerprint values of this build's units: the hex in each
/// `.fingerprint/<name>-<hash>/<kind>-<target>` file of a live hash.
fn unit_fingerprints(profiles: &[PathBuf], hashes: &BTreeSet<String>) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    for profile in profiles {
        let Ok(entries) = fs::read_dir(profile.join(".fingerprint")) else {
            continue;
        };
        for e in entries.flatten() {
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !unit_hash(&name).is_some_and(|h| hashes.contains(h)) {
                continue;
            }
            let Ok(files) = fs::read_dir(e.path()) else {
                continue;
            };
            for f in files.flatten() {
                let fname = f.file_name();
                let Some(fname) = fname.to_str() else {
                    continue;
                };
                // `lib-foo`, `bin-foo`, `build-script-build-script-build`,
                // `run-build-script-build-script-build`: the bare hex
                // fingerprint. Skip `*.json`, `dep-*`, `invoked.timestamp`.
                if fname.contains('.') || fname.starts_with("dep-") {
                    continue;
                }
                // Cargo writes the u64 as the hex of its LITTLE-endian
                // bytes (`09b59f…` on disk is `0x…9fb509` in the JSON).
                if let Some(v) = fs::read_to_string(f.path())
                    .ok()
                    .filter(|s| s.trim().len() == 16)
                    .and_then(|s| u64::from_str_radix(s.trim(), 16).ok())
                {
                    out.insert(v.swap_bytes());
                }
            }
        }
    }
    out
}

/// Every `.fingerprint/<package>-<hash>` in the profile dirs, with the
/// dependency fingerprints its JSON records (`None` if unreadable).
fn fingerprint_candidates(profiles: &[PathBuf], package: &str) -> BTreeMap<String, Option<Vec<u64>>> {
    let mut out = BTreeMap::new();
    for profile in profiles {
        let Ok(entries) = fs::read_dir(profile.join(".fingerprint")) else {
            continue;
        };
        for e in entries.flatten() {
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(hash) = unit_hash(&name) else {
                continue;
            };
            if name.len() != package.len() + 1 + 16 || !name.starts_with(package) {
                continue;
            }
            out.insert(hash.to_string(), fingerprint_deps(&e.path()));
        }
    }
    out
}

/// The `deps` of a unit's fingerprint JSON: `[[pkg id, name, public,
/// fingerprint], …]` → the fingerprints.
fn fingerprint_deps(dir: &Path) -> Option<Vec<u64>> {
    let json = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))?;
    let v: serde_json::Value = serde_json::from_slice(&fs::read(json).ok()?).ok()?;
    let deps = v.get("deps")?.as_array()?;
    let fps: Option<Vec<u64>> = deps.iter().map(|d| d.as_array()?.get(3)?.as_u64()).collect();
    fps.filter(|f| !f.is_empty())
}

// ─────────────────────────────────────────────────────────────────────
// Recency fallback — only where no record can exist
// ─────────────────────────────────────────────────────────────────────

/// Trim superseded incremental caches from every profile dir under
/// `target_dir`, keeping the `keep` most recently written caches per crate
/// name in each incremental dir. Returns how many cache dirs were removed.
///
/// Each profile dir is pruned only while holding cargo's exclusive build
/// locks on it; a profile whose build is running is skipped.
///
/// "Newest" is the cache dir's own mtime, which moves when rustc
/// finalises a session into it (it creates the new session dir and deletes
/// the old one), i.e. whenever that unit was last compiled.
pub fn prune_incremental(target_dir: &Path, keep: usize) -> usize {
    let per_dir: Vec<(&str, usize)> = INCREMENTAL_DIRS.iter().map(|d| (*d, keep)).collect();
    prune_incremental_dirs(target_dir, &per_dir)
}

/// [`prune_incremental`] with a keep count per incremental dir name
/// (`("incremental", 2)`, `("incremental-hotpatch", 1)`).
pub fn prune_incremental_dirs(target_dir: &Path, per_dir: &[(&str, usize)]) -> usize {
    profile_dirs(target_dir)
        .iter()
        .map(|p| prune_incremental_in_profile(p, per_dir, false).incremental_removed)
        .sum()
}

/// The keep-N incremental trim for one profile dir, under its locks.
pub fn prune_incremental_in_profile(profile: &Path, per_dir: &[(&str, usize)], dry_run: bool) -> GcReport {
    let mut report = GcReport::default();
    let Some(_locks) = CargoBuildLocks::try_acquire(profile) else {
        report.skipped_locked += 1;
        return report;
    };
    for (dir, keep) in per_dir {
        let (n, bytes) = prune_incremental_dir_with(&profile.join(dir), *keep, dry_run);
        report.incremental_removed += n;
        report.bytes_freed += bytes;
    }
    report
}

/// Pure core for one incremental dir: group the `<crate>-<hash>` cache
/// dirs by crate and remove all but the `keep` most recently written of
/// each. Entries that aren't a cache dir are left alone. Takes no lock —
/// the caller holds the profile's.
pub fn prune_incremental_dir(incremental: &Path, keep: usize) -> usize {
    prune_incremental_dir_with(incremental, keep, false).0
}

fn prune_incremental_dir_with(incremental: &Path, keep: usize, dry_run: bool) -> (usize, u64) {
    let mut by_crate: HashMap<String, Vec<(SystemTime, PathBuf)>> = HashMap::new();
    for (path, mtime) in incremental_caches(incremental) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(krate) = incremental_crate(name) {
            by_crate.entry(krate.to_string()).or_default().push((mtime, path));
        }
    }
    let (mut removed, mut bytes) = (0, 0);
    for (_, mut caches) in by_crate {
        if caches.len() <= keep {
            continue;
        }
        caches.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, stale) in caches.into_iter().skip(keep) {
            if let Some(b) = remove(&stale, dry_run) {
                removed += 1;
                bytes += b;
            }
        }
    }
    (removed, bytes)
}

/// Drop every compilation unit of `profile` except the most recently
/// built one per crate name — the fallback for a profile dir with no
/// records (built by a CLI from before them), where a single live build
/// per crate is the safe assumption (a web dir: one config, one app).
///
/// Cargo names a unit's fingerprint dir and its `deps/` / `build/` entries
/// with the same `<name>-<hash>` stem, so the fingerprint dir's mtime is a
/// reliable "when was this unit last built" and the hash links it to the
/// rest. Held under the profile's cargo locks.
pub fn prune_units_keep_newest(profile: &Path, dry_run: bool) -> GcReport {
    let mut report = GcReport::default();
    let Some(_locks) = CargoBuildLocks::try_acquire(profile) else {
        report.skipped_locked += 1;
        return report;
    };
    // crate name → (mtime, hash)
    let mut units: HashMap<String, Vec<(SystemTime, String)>> = HashMap::new();
    for e in fs::read_dir(profile.join(".fingerprint")).into_iter().flatten().flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(hash) = unit_hash(&name) else {
            continue;
        };
        let krate = name[..name.len() - hash.len() - 1].to_string();
        let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
        units.entry(krate).or_default().push((mtime, hash.to_string()));
    }
    let mut stale = BTreeSet::new();
    for (_, mut versions) in units {
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        stale.extend(versions.into_iter().skip(1).map(|(_, h)| h));
    }
    for dir in UNIT_DIRS {
        for e in fs::read_dir(profile.join(dir)).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if unit_hash(&name).is_some_and(|h| stale.contains(h)) {
                if let Some(bytes) = remove(&e.path(), dry_run) {
                    report.units_removed += 1;
                    report.bytes_freed += bytes;
                }
            }
        }
    }
    report
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Retry `f` until it returns `Some`, for up to ~1 s. A test that drops
    /// a flock and immediately re-acquires it can lose to a sibling test's
    /// `fork`: the child shares the lock's open file description until its
    /// `exec` closes it (O_CLOEXEC), so the lock outlives the drop for that
    /// window. Never needed outside tests — the CLI spawns nothing while it
    /// holds these locks.
    pub(crate) fn eventually<T>(mut f: impl FnMut() -> Option<T>) -> T {
        for _ in 0..100 {
            if let Some(v) = f() {
                return v;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        f().expect("still locked after 1 s")
    }

    fn cache(dir: &Path, name: &str, age_secs: u64) -> PathBuf {
        let p = dir.join(name);
        fs::create_dir_all(p.join("s-abc-def")).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        fs::File::open(&p).unwrap().set_modified(t).unwrap();
        p
    }

    fn touch(path: &Path) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, b"x").unwrap();
    }

    fn age(path: &Path, secs: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs);
        fs::File::open(path).unwrap().set_modified(t).unwrap();
    }

    // ── Recency fallback ─────────────────────────────────────────────

    /// Regression (CrewForge, 2026-10-08): the hot-patch replay's
    /// `incremental-hotpatch/` piled up 19 dead caches for one crate,
    /// because only `incremental/` was ever trimmed and rustc never
    /// collects another hash's dir. Both are trimmed now.
    #[test]
    fn regression_incremental_hotpatch_caches_never_pruned() {
        let target = tempfile::tempdir().unwrap();
        let profile = target.path().join("wasm32-unknown-unknown/debug");
        for dir in INCREMENTAL_DIRS {
            let inc = profile.join(dir);
            cache(&inc, "crewforge_main-1a2b", 300);
            cache(&inc, "crewforge_main-3c4d", 200);
            cache(&inc, "crewforge_main-5e6f", 100);
            cache(&inc, "ui_shared-77", 50);
        }
        assert_eq!(prune_incremental(target.path(), 1), 4, "two dead caches per incremental dir");
        for dir in INCREMENTAL_DIRS {
            let inc = profile.join(dir);
            assert!(inc.join("crewforge_main-5e6f").is_dir(), "{dir}: newest kept");
            assert!(!inc.join("crewforge_main-1a2b").exists(), "{dir}: oldest removed");
            assert!(!inc.join("crewforge_main-3c4d").exists(), "{dir}");
            assert!(inc.join("ui_shared-77").is_dir(), "{dir}: a lone cache is untouched");
        }
    }

    #[test]
    fn per_dir_keep_counts_apply_to_their_own_dir() {
        let target = tempfile::tempdir().unwrap();
        let profile = target.path().join("debug");
        for dir in INCREMENTAL_DIRS {
            for (i, age) in [300, 200, 100].iter().enumerate() {
                cache(&profile.join(dir), &format!("app-{i}"), *age);
            }
        }
        let removed = prune_incremental_dirs(target.path(), &[("incremental", 2), ("incremental-hotpatch", 1)]);
        assert_eq!(removed, 1 + 2);
        assert!(profile.join("incremental/app-1").is_dir() && profile.join("incremental/app-2").is_dir());
        assert!(!profile.join("incremental/app-0").exists());
        assert!(profile.join("incremental-hotpatch/app-2").is_dir());
        assert!(!profile.join("incremental-hotpatch/app-1").exists());
    }

    #[test]
    fn a_profile_whose_build_is_running_is_skipped() {
        let target = tempfile::tempdir().unwrap();
        let profile = target.path().join("debug");
        let inc = profile.join("incremental");
        cache(&inc, "app-1", 300);
        cache(&inc, "app-2", 100);
        let lock = fs::File::create(profile.join(".cargo-lock")).unwrap();
        lock.lock().unwrap();
        // A second handle can't take the lock while `lock` holds it.
        assert_eq!(prune_incremental(target.path(), 1), 0);
        drop(lock);
        assert_eq!(eventually(|| Some(prune_incremental(target.path(), 1)).filter(|n| *n > 0)), 1);
    }

    /// A running RELEASE build (no `incremental/`) must still block
    /// removing its target dir.
    #[test]
    fn a_running_release_build_blocks_try_acquire_all() {
        let target = tempfile::tempdir().unwrap();
        let release = target.path().join("wasm32-unknown-unknown/release");
        fs::create_dir_all(release.join(".fingerprint")).unwrap();
        let lock = fs::File::create(release.join(".cargo-lock")).unwrap();
        lock.lock().unwrap();
        assert!(CargoBuildLocks::try_acquire_all(target.path()).is_none());
        drop(lock);
        eventually(|| CargoBuildLocks::try_acquire_all(target.path()));
    }

    #[test]
    fn non_cache_entries_are_left_alone() {
        let target = tempfile::tempdir().unwrap();
        let inc = target.path().join("debug/incremental");
        cache(&inc, "app-1", 300);
        cache(&inc, "app-2", 100);
        fs::create_dir_all(inc.join("NotACache")).unwrap();
        fs::create_dir_all(inc.join("weird-HASH")).unwrap();
        fs::write(inc.join("file-1"), b"").unwrap();
        assert_eq!(prune_incremental_dir(&inc, 1), 1);
        assert!(inc.join("NotACache").is_dir() && inc.join("weird-HASH").is_dir());
        assert!(inc.join("file-1").is_file());
    }

    #[test]
    fn keep_newest_units_drops_superseded_hashes_in_every_unit_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("wasm32-unknown-unknown/debug");
        let (old, new) = ("1111111111111111", "2222222222222222");
        for h in [old, new] {
            touch(&p.join(format!(".fingerprint/web-sys-{h}/lib-web_sys")));
            touch(&p.join(format!("deps/libweb_sys-{h}.rlib")));
            touch(&p.join(format!("build/web-sys-{h}/output")));
        }
        age(&p.join(format!(".fingerprint/web-sys-{old}")), 3600);
        let dry = prune_units_keep_newest(&p, true);
        assert_eq!(dry.units_removed, 3);
        assert!(p.join(format!("deps/libweb_sys-{old}.rlib")).is_file(), "dry run deletes nothing");
        let real = prune_units_keep_newest(&p, false);
        assert_eq!(real, dry, "a dry run reports exactly what a real one does");
        assert!(!p.join(format!("deps/libweb_sys-{old}.rlib")).exists());
        assert!(!p.join(format!("build/web-sys-{old}")).exists());
        assert!(p.join(format!("deps/libweb_sys-{new}.rlib")).is_file());
    }

    // ── Records ──────────────────────────────────────────────────────

    #[test]
    fn unit_hash_reads_every_cargo_entry_shape() {
        let h = "0123456789abcdef";
        for name in [
            format!("libfoo-{h}.rlib"),
            format!("libfoo_bar-{h}.a"),
            format!("foo-bar-{h}"),
            format!("foo-{h}.d"),
            format!("foo-{h}.akm9p2g7f21dtliex8jt56m6l.03ls564.rcgu.o"),
            format!("web-sys-{h}"),
        ] {
            assert_eq!(unit_hash(&name), Some(h), "{name}");
        }
        for name in [
            "libfoo.a",
            "foo-3bzbwlg4uzxox",
            "foo-0123456789ABCDEF",
            "-0123456789abcdef",
            "foo-0123456789abcdef0",
            "build-script-build",
            "libruntime_layout-0123456789abcdefab.rlib",
            "crate-zzzzzzzzzzzzzzzz",
        ] {
            assert_eq!(unit_hash(name), None, "{name}");
        }
    }

    #[test]
    fn json_messages_yield_unit_hashes_and_local_crates() {
        let mut u = CargoUnits::default();
        assert!(u.absorb(r#"{"reason":"compiler-artifact","package_id":"registry+https://x#serde@1.0.0","target":{"name":"serde","kind":["lib"]},"filenames":["/t/a/debug/deps/libserde-1111111111111111.rlib"],"executable":null,"fresh":true}"#));
        assert!(u.absorb(r#"{"reason":"compiler-artifact","package_id":"path+file:///app#my-app@0.1.0","target":{"name":"my-app","kind":["lib"]},"filenames":["/t/a/debug/deps/libmy_app-2222222222222222.rlib"],"executable":null,"fresh":false}"#));
        assert!(u.absorb(r#"{"reason":"compiler-artifact","package_id":"registry+https://x#libc@0.2.0","target":{"name":"build-script-build","kind":["custom-build"]},"filenames":["/t/debug/build/libc-3333333333333333/build-script-build"],"executable":null,"fresh":true}"#));
        assert!(u.absorb(r#"{"reason":"build-script-executed","package_id":"x","out_dir":"/t/a/debug/build/libc-4444444444444444/out"}"#));
        assert!(u.absorb(r#"{"reason":"compiler-artifact","package_id":"path+file:///app#my-app@0.1.0","target":{"name":"srv","kind":["bin"]},"filenames":["/t/debug/srv"],"executable":"/t/debug/srv","fresh":true}"#));
        assert!(u.absorb(r#"{"reason":"build-finished","success":true}"#));
        assert!(!u.absorb("hello from a build script"));
        let want: BTreeSet<String> = ["1111111111111111", "2222222222222222", "3333333333333333", "4444444444444444"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(u.hashes, want);
        assert_eq!(u.local_crates, BTreeSet::from(["my_app".to_string(), "srv".to_string()]));
        assert_eq!(u.unhashed_packages, BTreeSet::from(["my-app".to_string()]), "the uplifted bin carries no hash");
        assert_eq!(u.executable.as_deref(), Some(Path::new("/t/debug/srv")));
    }

    #[test]
    fn variant_keys_are_file_safe_and_distinct() {
        assert_eq!(features_variant(&[]), "default");
        assert_eq!(features_variant(&["dev", "aas", "dev"]), "aas+dev");
        assert_eq!(variant_key("app-main"), "app-main");
        let a = variant_key("/ws/crates/server/Cargo.toml --bin srv");
        let b = variant_key("/ws/crates/server/Cargo.toml --bin srv2");
        assert_ne!(a, b);
        for k in [&a, &b] {
            assert!(!k.contains('/') && !k.contains(' ') && !k.starts_with('.'), "{k}");
        }
        assert_ne!(variant_key("a/b"), variant_key("a_b"), "sanitizing alike must not collide");
    }

    fn write_record(profile: &Path, variant: &str, hashes: &[&str], inc: &[(&str, &str, &str)]) {
        let mut r = Record { hashes: hashes.iter().map(|h| h.to_string()).collect(), ..Default::default() };
        for (key, krate, dir) in inc {
            r.incremental
                .entry(key.to_string())
                .or_default()
                .entry(krate.to_string())
                .or_default()
                .insert(dir.to_string());
        }
        fs::create_dir_all(profile.join(RECORDS_DIR)).unwrap();
        fs::write(profile.join(RECORDS_DIR).join(format!("{variant}.json")), serde_json::to_vec(&r).unwrap())
            .unwrap();
    }

    /// The prune keeps the union of every recorded variant and removes
    /// the rest; a host dir is left alone while any triple built with that
    /// profile has no record.
    #[test]
    fn prune_keeps_every_recorded_variant_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let sim = t.join("aarch64-apple-ios-sim/debug");
        let host = t.join("debug");
        let (dev, plain, old) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc");
        for h in [dev, plain, old] {
            touch(&sim.join(format!("deps/libapp-{h}.a")));
            touch(&sim.join(format!(".fingerprint/app-{h}/lib-app")));
            touch(&host.join(format!("deps/libmacros-{h}.dylib")));
        }
        for d in ["app-1dev", "app-2plain", "app-3old"] {
            fs::create_dir_all(sim.join("incremental").join(d)).unwrap();
        }
        write_record(&sim, "dev", &[dev], &[("aarch64-apple-ios-sim/debug", "app", "app-1dev")]);
        write_record(&sim, "default", &[plain], &[("aarch64-apple-ios-sim/debug", "app", "app-2plain")]);

        // A device dir with no record yet: the host dir must survive.
        touch(&t.join("aarch64-apple-ios/debug/deps/libapp-dddddddddddddddd.a"));
        let report = prune_profile_by_records(t, &sim, false);
        assert!(!is_recorded(t, &host), "device dir has no record");
        assert_eq!(prune_profile_by_records(t, &host, false), GcReport::default());

        for h in [dev, plain] {
            assert!(sim.join(format!("deps/libapp-{h}.a")).is_file(), "{h} is recorded");
            assert!(sim.join(format!(".fingerprint/app-{h}")).is_dir());
        }
        assert!(!sim.join(format!("deps/libapp-{old}.a")).exists(), "superseded unit removed");
        assert!(!sim.join(format!(".fingerprint/app-{old}")).exists());
        assert!(sim.join("incremental/app-1dev").is_dir() && sim.join("incremental/app-2plain").is_dir());
        assert!(!sim.join("incremental/app-3old").exists());
        assert_eq!((report.units_removed, report.incremental_removed), (2, 1));

        // Once the device dir has a record, the host dir is pruned against
        // the union of both triples' records.
        write_record(&t.join("aarch64-apple-ios/debug"), "default", &[plain], &[]);
        prune_profile_by_records(t, &host, false);
        assert!(host.join(format!("deps/libmacros-{dev}.dylib")).is_file());
        assert!(host.join(format!("deps/libmacros-{plain}.dylib")).is_file());
        assert!(!host.join(format!("deps/libmacros-{old}.dylib")).exists());
    }

    /// A no-`--target` build records in the host dir itself, and its own
    /// units there count alongside the host units of triple builds (the
    /// macOS wrapper: a host-arch dev build beside a universal publish).
    #[test]
    fn a_host_build_records_in_the_host_dir_and_keeps_triple_host_units() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let host = t.join("debug");
        let (mine, universal, old) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc");
        for h in [mine, universal, old] {
            touch(&host.join(format!("deps/libx-{h}.rlib")));
        }
        touch(&t.join(format!("x86_64-apple-darwin/debug/deps/liby-{universal}.rlib")));
        write_record(&t.join("x86_64-apple-darwin/debug"), "default", &[universal], &[]);
        write_record(&host, "dev", &[mine], &[]);
        let report = prune_profile_by_records(t, &host, false);
        assert_eq!(report.units_removed, 1);
        assert!(host.join(format!("deps/libx-{mine}.rlib")).is_file());
        assert!(host.join(format!("deps/libx-{universal}.rlib")).is_file(), "a triple build's host unit");
        assert!(!host.join(format!("deps/libx-{old}.rlib")).exists());
    }

    /// A record with a TTL that was not refreshed in time no longer keeps
    /// anything, and is removed; one without a TTL never expires.
    #[test]
    fn expired_records_stop_keeping_units() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let p = t.join("debug");
        let (gone, kept, wrapper) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc");
        for h in [gone, kept, wrapper] {
            touch(&p.join(format!("deps/libx-{h}.rlib")));
        }
        let rec = |variant: &str, hash: &str, ttl: Option<u64>| {
            let r = Record { hashes: [hash.to_string()].into(), ttl_secs: ttl, ..Default::default() };
            let path = p.join(RECORDS_DIR).join(format!("{variant}.json"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, serde_json::to_vec(&r).unwrap()).unwrap();
            path
        };
        let old_app = rec("old-app", gone, Some(60));
        age(&old_app, 3600);
        rec("live-app", kept, Some(60));
        let ancient = rec("dev", wrapper, None);
        age(&ancient, 400 * 86_400);
        prune_profile_by_records(t, &p, false);
        assert!(!p.join(format!("deps/libx-{gone}.rlib")).exists(), "expired record keeps nothing");
        assert!(!old_app.exists(), "expired record removed");
        assert!(p.join(format!("deps/libx-{kept}.rlib")).is_file());
        assert!(p.join(format!("deps/libx-{wrapper}.rlib")).is_file(), "no TTL, no expiry");
    }

    #[test]
    fn a_locked_profile_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let sim = tmp.path().join("x/debug");
        touch(&sim.join("deps/libapp-cccccccccccccccc.a"));
        write_record(&sim, "dev", &["aaaaaaaaaaaaaaaa"], &[]);
        let lock = fs::File::create(sim.join(".cargo-lock")).unwrap();
        lock.lock().unwrap();
        let report = prune_profile_by_records(tmp.path(), &sim, false);
        assert_eq!(report.skipped_locked, 1);
        assert!(sim.join("deps/libapp-cccccccccccccccc.a").is_file());
    }

    /// `prune_recorded` (what `clean --stale` runs) removes exactly what a
    /// build's own prune would, reports the unrecorded profile dirs for
    /// the fallback, and a dry run deletes nothing.
    #[test]
    fn prune_recorded_matches_the_build_prune_and_lists_unrecorded_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        let rec = t.join("wasm32-unknown-unknown/debug");
        let legacy = t.join("aarch64-apple-ios/debug");
        for h in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"] {
            touch(&rec.join(format!("deps/libx-{h}.rlib")));
            touch(&legacy.join(format!("deps/libx-{h}.rlib")));
        }
        write_record(&rec, "app", &["aaaaaaaaaaaaaaaa"], &[]);
        let (dry, unrecorded) = prune_recorded(t, true);
        assert_eq!(dry.units_removed, 1);
        assert!(rec.join("deps/libx-bbbbbbbbbbbbbbbb.rlib").is_file(), "dry run");
        assert_eq!(unrecorded, vec![legacy.clone()], "the host dir has no deps, so it is not a profile dir");
        let (real, _) = prune_recorded(t, false);
        assert_eq!(real, dry);
        assert!(!rec.join("deps/libx-bbbbbbbbbbbbbbbb.rlib").exists());
        assert!(legacy.join("deps/libx-bbbbbbbbbbbbbbbb.rlib").is_file(), "unrecorded: left to the fallback");
    }

    fn host_triple() -> String {
        let out = Command::new("rustc").arg("-vV").output().expect("rustc -vV");
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
            .expect("host triple")
    }

    struct Probe {
        _tmp: tempfile::TempDir,
        dir: PathBuf,
        target: PathBuf,
        triple: Option<String>,
    }

    impl Probe {
        /// A wrapper-shaped crate: a root (`staticlib`, or `bin`) over a
        /// path dep whose `dev` feature the root's `dev` turns on — the
        /// wrapper shape in miniature.
        fn new(root_kind: &str, triple: bool) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = fs::canonicalize(tmp.path()).unwrap();
            let w = |p: &str, s: &str| {
                let p = dir.join(p);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, s).unwrap();
            };
            let (target_section, src) = match root_kind {
                "staticlib" => (
                    "[lib]\ncrate-type = [\"staticlib\"]\n",
                    ("wrapper/src/lib.rs", "#[no_mangle] pub extern \"C\" fn probe() -> u32 { native::v() }\n"),
                ),
                _ => ("", ("wrapper/src/main.rs", "fn main() { println!(\"{}\", native::v()); }\n")),
            };
            w(
                "wrapper/Cargo.toml",
                &format!("[workspace]\n[package]\nname = \"probe-wrapper\"\nversion = \"0.0.1\"\nedition = \"2021\"\n{target_section}[dependencies]\nnative = {{ path = \"../native\" }}\n[features]\ndev = [\"native/dev\"]\n"),
            );
            w(src.0, src.1);
            // A PATH package with a `cdylib` lib target, like
            // `backend-android-mobile`: cargo drops the hash from all its
            // outputs, so only its fingerprint dir carries one.
            w("native/Cargo.toml", "[package]\nname = \"native\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n[dependencies]\nleaf = { path = \"../leaf\" }\n[features]\ndev = [\"leaf/dev\"]\n");
            w("native/src/lib.rs", "pub fn v() -> u32 { leaf::v() }\n");
            // Build scripts on two path packages: rustc names every build
            // script's incremental cache `build_script_build-<id>`, so one
            // crate name maps to several live caches.
            w("native/build.rs", "fn main() {}\n");
            w("leaf/build.rs", "fn main() { println!(\"cargo::rerun-if-changed=build.rs\"); }\n");
            w("leaf/src/lib.rs", "pub fn v() -> u32 { if cfg!(feature = \"dev\") { 2 } else { 1 } }\n");
            let p = Self {
                target: dir.join("wrapper/target"),
                dir,
                _tmp: tmp,
                triple: triple.then(host_triple),
            };
            p.release_leaf("0.1.0");
            p
        }

        /// Publish a new leaf version: every unit above it rehashes, as a
        /// framework release does to everything above the bumped crate.
        fn release_leaf(&self, version: &str) {
            fs::write(
                self.dir.join("leaf/Cargo.toml"),
                format!("[package]\nname = \"leaf\"\nversion = \"{version}\"\nedition = \"2021\"\n[features]\ndev = []\n"),
            )
            .unwrap();
        }

        fn build(&self, features: &[String]) -> (CargoUnits, GcReport) {
            let mut cmd = Command::new("cargo");
            cmd.args(["build", "--offline", "--quiet"])
                .current_dir(self.dir.join("wrapper"))
                .env("CARGO_TARGET_DIR", &self.target);
            if let Some(t) = &self.triple {
                cmd.args(["--target", t]);
            }
            if !features.is_empty() {
                cmd.arg("--features").arg(features.join(","));
            }
            let spec = BuildSpec::wrapper(&self.target, self.triple.as_deref(), false, features);
            build_and_prune(&mut cmd, &spec).expect("cargo build")
        }

        fn profile(&self) -> PathBuf {
            match &self.triple {
                Some(t) => self.target.join(t).join("debug"),
                None => self.target.join("debug"),
            }
        }

        /// Root units in `deps/` — one per live generation.
        fn root_units(&self) -> usize {
            self.count("deps", |n| {
                (n.starts_with("libprobe_wrapper-") && n.ends_with(".a"))
                    || (n.starts_with("probe_wrapper-") && !n.contains('.'))
            })
        }

        /// [`Self::count`] over the build's profile dir and the host dir
        /// (once, when they are the same dir).
        fn count_host_and_build(&self, sub: &str, f: impl Fn(&str) -> bool) -> usize {
            let host = self.target.join("debug");
            let mut dirs = vec![self.profile()];
            if host != self.profile() {
                dirs.push(host);
            }
            dirs.iter()
                .map(|d| {
                    fs::read_dir(d.join(sub))
                        .map(|r| r.flatten().filter(|e| e.file_name().to_str().is_some_and(&f)).count())
                        .unwrap_or(0)
                })
                .sum()
        }

        fn count(&self, sub: &str, f: impl Fn(&str) -> bool) -> usize {
            fs::read_dir(self.profile().join(sub))
                .map(|d| d.flatten().filter(|e| e.file_name().to_str().is_some_and(&f)).count())
                .unwrap_or(0)
        }
    }

    /// Also a regression for the prune itself: a path `cdylib` dependency
    /// (`native`, like `backend-android-mobile`) reports outputs with no
    /// hash, so its fingerprint dir read as superseded and was deleted after
    /// EVERY build — and cargo recompiled it on every run.
    ///
    /// Regression (CrewForge, 2026-10-08): every framework release left a
    /// whole superseded generation in the iOS wrapper's target — root
    /// staticlib included, ~1.1 GB each, times two for dev on/off — and
    /// nothing ever removed one. After a build exactly the units the
    /// recorded variants use survive: the current dev AND non-dev builds,
    /// nothing older, and neither variant has to recompile because of it.
    fn superseded_generations_are_pruned(p: Probe) {
        let dev = vec!["dev".to_string()];

        let (_, first) = p.build(&[]);
        assert_eq!(first.incremental_removed, 0, "a first build removed its own caches: {first:?}");
        let (_, first_dev) = p.build(&dev);
        assert_eq!(first_dev.units_removed + first_dev.incremental_removed, 0, "{first_dev:?}");
        assert_eq!(p.root_units(), 2, "dev and non-dev roots coexist");
        assert_eq!(
            p.count_host_and_build("incremental", |n| n.starts_with("build_script_build-")),
            4,
            "both build scripts keep their incremental cache, per variant (cargo compiles a \
             build script with its package's features)"
        );

        // A release: dev rebuilds against the new leaf. The old dev
        // generation goes; the current non-dev build stays.
        p.release_leaf("0.2.0");
        let (units, report) = p.build(&dev);
        assert!(units.recompiled > 0);
        assert!(report.unidentified.is_empty(), "every hash-less unit found from its fingerprint: {report:?}");
        assert_eq!(p.root_units(), 2, "old dev root pruned, non-dev root kept: {report:?}");
        assert!(report.units_removed > 0);

        // Non-dev catches up; its old generation goes too.
        p.build(&[]);
        assert_eq!(p.root_units(), 2);
        assert_eq!(p.count("deps", |n| n.starts_with("libleaf-") && n.ends_with(".rlib")), 2);
        for krate in ["probe_wrapper-", "leaf-"] {
            let n = p.count("incremental", |n| n.starts_with(krate));
            assert!(n <= 2, "{krate}: {n} incremental caches for two variants");
        }

        // A prune never deletes what the build it follows used: the same
        // variant again is fully fresh.
        let (plain_again, _) = p.build(&[]);
        assert_eq!(plain_again.recompiled_packages, BTreeSet::new(), "the non-dev build lost live units to its own prune");

        // Nor what the OTHER variant uses. `native` is the exception cargo
        // makes itself: a path cdylib writes the same hash-less outputs for
        // both variants, so cargo rebuilds it, and the root above it, on
        // every switch — prune or no prune. Everything hashed must survive.
        let (switched, _) = p.build(&dev);
        let collides: BTreeSet<String> = ["native", "probe-wrapper"].map(String::from).into();
        assert!(
            switched.recompiled_packages.is_subset(&collides),
            "switching to dev recompiled {:?}: the non-dev prune deleted dev units",
            switched.recompiled_packages
        );
        let (dev_again, _) = p.build(&dev);
        assert_eq!(dev_again.recompiled_packages, BTreeSet::new(), "the dev build lost live units to its own prune");
    }

    #[test]
    fn regression_superseded_wrapper_generations_pile_up() {
        superseded_generations_are_pruned(Probe::new("staticlib", true));
    }

    /// The desktop wrappers build a `bin` for the host WITHOUT `--target`,
    /// so units, build scripts and records all share `<target>/debug`, and
    /// the root's only reported output is the uplifted, hash-less binary.
    #[test]
    fn regression_superseded_host_bin_wrapper_generations_pile_up() {
        superseded_generations_are_pruned(Probe::new("bin", false));
    }

    #[test]
    fn incremental_snapshot_sees_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let inc = tmp.path().join("debug/incremental/app-1x");
        fs::create_dir_all(&inc).unwrap();
        fs::create_dir_all(tmp.path().join("debug/deps")).unwrap();
        let old = SystemTime::now() - Duration::from_secs(60);
        fs::File::open(&inc).unwrap().set_modified(old).unwrap();
        let snap = IncrementalSnapshot::take(tmp.path());
        assert!(!snap.touched(&inc, old));
        assert!(snap.touched(&inc, SystemTime::now()));
        assert!(snap.touched(&tmp.path().join("debug/incremental/app-2y"), old), "new dir");
    }
}
