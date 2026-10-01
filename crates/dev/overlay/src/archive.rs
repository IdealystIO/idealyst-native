//! The dev-time overlay's BUILD-TIME half: a crate's `ui!` sites, as
//! descriptors, written next to the build.
//!
//! The compiled app carries only numbers — a site key and a node index
//! on each `Element` a `ui!` site builds. What those numbers MEAN is
//! this file. Producing it here rather than emitting it into the binary
//! is a measured decision: a `static` descriptor per site cost +1.4 s on
//! every one-edit rebuild of a real app, while the tags alone cost
//! nothing measurable (`runtime_template`'s README has the table).
//!
//! # What is produced
//!
//! One JSON document per build, under
//! `target/idealyst/<app>/overlay/<build>.json`, where `<build>` is a
//! digest of every scanned file's contents plus
//! [`runtime_template::SPLIT_VERSION`]. Keying by content means two
//! builds of the same sources share a file and a rebuild that changed
//! nothing writes nothing new; keying by the split version means a
//! change to node numbering cannot be mistaken for a change to the app.
//!
//! A differ's job is then: take the previous build's document and this
//! one, and turn the difference into patches. That differ is NOT built
//! here.
//!
//! # Why the same library as the macro
//!
//! `runtime_macros_parse` is the `ui!` parser, split pass and node
//! numbering — the exact code the proc macro runs. A second
//! implementation that disagreed by one node would mis-address every
//! patch after it, silently, because both halves would stay internally
//! consistent. `crates/dev/ui-lowering-parity` holds the two tests that
//! keep them honest: one compiles a real `ui!` and checks its tag
//! against this scanner's site key, the other checks node-for-node
//! agreement across the whole fixture corpus.
//!
//! # Why this lives in the dev-reload crate
//!
//! Because the DECISION does. A save's outcome — patch or rebuild —
//! needs the archived descriptor set and the freshly scanned one side
//! by side, and the watch loop is what has both. Producing the archive
//! and reading it back are two halves of one thing, and splitting them
//! across a crate boundary would mean the CLI owning the format and the
//! watcher re-deriving it.
//!
//! # Failure is per-file
//!
//! A file that does not parse is reported and skipped; every other file
//! still contributes. The author is mid-edit most of the time this runs,
//! and one broken file must not cost the descriptor set for the rest of
//! the crate. Same forgiveness `catalog-scan` has, for the same reason.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use runtime_template::{Descriptor, SPLIT_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// What a build recorded about one source file.
///
/// Three digests, because a dev loop has to sort a save into three
/// outcomes, each strictly more expensive than the last:
///
/// - **`content`** — did this file change at all? Cheapest possible
///   filter, and what makes a build identifiable.
/// - **`skeleton`** — did anything change OUTSIDE its `ui!` bodies? A
///   descriptor cannot answer that: `let x = 1;` becoming `let x = 2;`
///   moves no site and changes no descriptor, and it is compiled code.
///   Unchanged ⇒ the overlay can apply the save with no compiler at
///   all.
/// - **`shape`** — did anything change outside its FUNCTION bodies?
///   Unchanged ⇒ the save is a subsecond hot patch: a jump table
///   rebinds function addresses, so new statements inside a function
///   can be spliced into the running process. CHANGED ⇒ rebuild, and
///   this one is a safety boundary rather than a speed tier. A patch
///   dylib is spliced into a process where everything else is the old
///   build; if a props struct gained a field, the framework's own
///   generic instantiations over that type — compiled into an rlib
///   that is NOT re-emitted — still use the old layout. That is memory
///   corruption, not a stale render.
///
/// Together they partition a file into "an overlay can patch it",
/// "subsecond can patch it", and "only a compiler and a fresh process
/// can".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDigest {
    pub content: String,
    pub skeleton: String,
    /// See the struct docs. Defaulted so a document written before
    /// this field existed still deserializes — it then equals no
    /// file's real shape, so every save from such an archive rebuilds,
    /// which is the safe reading. (`overlay_version` refuses those
    /// documents outright; the default is belt and braces.)
    #[serde(default)]
    pub shape: String,
    /// Digest of the file's `stylesheet!` invocations, values included
    /// (`runtime_macros_parse::stylesheet_tokens`). A premint session
    /// generated its class names from exactly these tokens, so there a
    /// value edit the shape ignores still has to rebuild. Defaulted: an
    /// older document matches no file, which rebuilds — the safe reading.
    #[serde(default)]
    pub sheets: String,
    /// One digest per function whose body a DEPENDENT crate compiles
    /// from this crate's metadata — generic, `#[inline]`, `const`,
    /// `async`, `impl Trait`, trait default bodies — and per `const`
    /// item, whose value a dependent evaluates from the same metadata
    /// (`runtime_macros_parse::downstream_bodies`), keyed by a label that
    /// names the item.
    ///
    /// Only consulted for a library crate of the app's workspace, never
    /// for the app crate itself (nothing depends on it). A hot patch of a
    /// library re-emits it and its dependents, but a dependent's replay
    /// reads the library's metadata from the BASE build, so its copy of
    /// such a body is the old one; an edit that moves one of these has to
    /// rebuild. Defaulted: an older document records none, which a file
    /// that has any never matches — it rebuilds, the safe reading.
    #[serde(default)]
    pub downstream: BTreeMap<String, String>,
    /// What the file says about `const` values
    /// (`runtime_macros_parse::const_facts`): a digest of every value the
    /// shape blanks, and where every const is read. The shape no longer
    /// holds a const's value, so this is what decides whether a value
    /// edit can be a hot patch — see [`ConstDigest`]. Defaulted: an older
    /// document records no values, which a file that has any never
    /// matches, and `overlay_version` refuses it anyway.
    #[serde(default)]
    pub consts: ConstDigest,
}

/// [`runtime_macros_parse::ConstFacts`], with each value as a digest.
///
/// The decider needs two things from it. Per file: which const values a
/// save moved (`values`). Across the whole crate: which names are read
/// somewhere a value reaches a type, a layout or a `static` — the rest
/// of the fields, which `runtime_macros_parse::compile_time_reach` closes
/// over. A value edit to a const in that set rebuilds; any other is a
/// body edit, because the patch re-emits every body of the crate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstDigest {
    /// Label -> `(name, digest of the initializer)`.
    #[serde(default)]
    pub values: BTreeMap<String, (String, String)>,
    #[serde(default)]
    pub reads: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub compile_time: BTreeSet<String>,
    #[serde(default)]
    pub sheet_reads: BTreeSet<String>,
    #[serde(default)]
    pub renames: BTreeSet<(String, String)>,
    /// The file pulls in source the scan cannot read, or did not parse:
    /// nothing about the crate's const reads can be trusted.
    #[serde(default)]
    pub opaque: bool,
    /// Names read in bodies a dependent compiles itself — what a LIBRARY
    /// crate's const value edit is checked against, with the other
    /// crates' `mentions` (`runtime_macros_parse::library_reach`).
    #[serde(default)]
    pub downstream_reads: BTreeSet<String>,
    /// Every identifier in the file: what it could be reading of another
    /// crate's.
    #[serde(default)]
    pub mentions: BTreeSet<String>,
}

impl ConstDigest {
    /// The digest of `text`. A file that does not parse is
    /// [`Self::opaque`]: a read hidden in it could be anywhere.
    pub fn of(text: &str) -> ConstDigest {
        let Some(facts) = runtime_macros_parse::const_facts(text) else {
            return ConstDigest { opaque: true, ..ConstDigest::default() };
        };
        ConstDigest {
            values: facts
                .values
                .into_iter()
                .map(|(label, v)| (label, (v.name, hex(&Sha256::digest(v.value.as_bytes())))))
                .collect(),
            reads: facts.reads,
            compile_time: facts.compile_time,
            sheet_reads: facts.sheet_reads,
            renames: facts.renames,
            opaque: facts.opaque,
            downstream_reads: facts.downstream_reads,
            mentions: facts.mentions,
        }
    }

    /// Back to facts, for `compile_time_reach` — which reads only where
    /// names are used, never the values.
    pub fn uses(&self) -> runtime_macros_parse::ConstFacts {
        runtime_macros_parse::ConstFacts {
            values: BTreeMap::new(),
            reads: self.reads.clone(),
            compile_time: self.compile_time.clone(),
            sheet_reads: self.sheet_reads.clone(),
            renames: self.renames.clone(),
            opaque: self.opaque,
            downstream_reads: self.downstream_reads.clone(),
            mentions: self.mentions.clone(),
        }
    }
}

/// One build's descriptor set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescriptorSet {
    /// Format version of this document. `2` added per-site keys and
    /// ordinals; a `1` document has no way to express them, so a reader
    /// that meets one rebuilds.
    pub overlay_version: u32,
    /// The node-numbering version the scanning library used. A differ
    /// must refuse a document whose value disagrees with the binary's
    /// `IDEALYST_UI_SPLIT_VERSION`.
    pub split_version: u32,
    /// `[package] name` of the scanned crate, exactly as it feeds the
    /// site key.
    pub package: String,
    /// Per-file digests, by package-relative path. See [`FileDigest`].
    pub files: BTreeMap<String, FileDigest>,
    /// Every site found, in file then document order.
    pub sites: Vec<ArchivedSite>,
    /// The program that scanned it ([`scanner_identity`]). A set read back
    /// from disk is reused in place of a scan only when this matches the
    /// running program: a scanner change that bumps neither version above
    /// (a parser fix between releases) must not hand the dev loop sites
    /// numbered by the old code. `None` on documents from before the
    /// field, which are never reused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanner: Option<String>,
}

/// The running program, as `len:mtime` of its executable: what produced
/// a scan, for [`read_set`] to reuse only its own. `None` when the
/// executable cannot be stat'd; nothing is reused then.
pub fn scanner_identity() -> Option<String> {
    static ID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let md = std::fs::metadata(&exe).ok()?;
        let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(format!("{}:{}:{}", exe.display(), md.len(), mtime.as_nanos()))
    })
    .clone()
}

/// The scan of `sources` this program already wrote into `dir`, if any:
/// `<dir>/<key>.json` from an earlier session, read back instead of
/// scanned again. Every file there is a scan (only scans are written,
/// never an archive a patch advanced — see `ScanMemo`), and it is reused
/// only when its versions, package, key and scanner all match.
///
/// Why: a session start scanned every crate of the workspace before the
/// hot tier could take a save — 14–16 s on CrewForge, every restart, on
/// sources the previous session had already scanned.
pub fn read_set(dir: &Path, sources: &CrateSources) -> Option<DescriptorSet> {
    let scanner = scanner_identity()?;
    let key = sources.build_key();
    let text = std::fs::read_to_string(dir.join(format!("{key}.json"))).ok()?;
    let set: DescriptorSet = serde_json::from_str(&text).ok()?;
    let reusable = set.overlay_version == OVERLAY_VERSION
        && set.split_version == SPLIT_VERSION
        && set.package == sources.package
        && set.scanner.as_deref() == Some(scanner.as_str())
        && set.build_key() == key;
    reusable.then_some(set)
}

/// The document format's own version. See
/// [`DescriptorSet::overlay_version`].
///
/// `3` added [`FileDigest::shape`], which the hot-patch tier decides
/// on. A `2` document cannot answer "did only function bodies change",
/// and guessing would route a shape change into a patch — so a reader
/// that meets one rebuilds.
///
/// `4` added [`FileDigest::downstream`], which a hot patch of a library
/// crate in the app's workspace needs (see [`crate::workspace`]).
///
/// `5`: descriptors record each slot's source and wrapped literal
/// (`runtime_template::SlotInfo::{code, literal}`), and blank nested
/// `ui!` bodies out of the text they record. A `4` document diffed
/// against a `5` scan would read every slot as changed code.
///
/// `6`: the shape blanks `const` initializers, and [`FileDigest::consts`]
/// carries what decides a const value edit instead. A `5` document's
/// shape still holds the values, so every const edit against it would
/// read as a shape change at best — and it has no `consts` to decide
/// one with.
pub const OVERLAY_VERSION: u32 = 6;

/// One `ui!` site as a build recorded it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchivedSite {
    /// The key the COMPILED code carries in this site's tags.
    ///
    /// Stored rather than derived from `descriptor.site`, and never
    /// updated by a patch, because those two stop agreeing the moment an
    /// edit shifts a line: the source says the site is at 44:5 now, and
    /// the running binary still says 43:5. The binary is what a patch
    /// has to address, so THIS is the number that travels.
    pub key: u64,
    /// Position among the file's `ui!` invocations, in document order,
    /// starting at 0.
    ///
    /// What survives a line shift. A save is matched to the archive by
    /// (file, ordinal), so a body gaining a line re-keys nothing as far
    /// as the dev loop is concerned — it patches, where keying by
    /// position alone would have rebuilt every site below the edit.
    pub ordinal: u32,
    pub descriptor: Descriptor,
}

impl DescriptorSet {
    /// The build key: a digest over the split version and every file's
    /// path and CONTENT hash. Also the document's file name.
    ///
    /// The skeleton hash is deliberately not in it: two builds whose
    /// sources differ only inside `ui!` bodies are still different
    /// builds, and a key that could not tell them apart would let a
    /// stale descriptor set be mistaken for the current one.
    pub fn build_key(&self) -> String {
        self.build_key_with(&BTreeMap::new())
    }

    /// [`Self::build_key`] of the crate as it would scan with some files'
    /// contents replaced: `changed` maps a package-relative path to the
    /// file's new TEXT. What a save turns the crate into, before any
    /// rescan — so a patch built from the save can be keyed by the
    /// sources it compiled.
    pub fn build_key_with(&self, changed: &BTreeMap<String, String>) -> String {
        key_of(
            self.split_version,
            self.files.iter().map(|(path, digest)| {
                let content = match changed.get(path) {
                    Some(text) => self::digest(text.as_bytes()),
                    None => digest.content.clone(),
                };
                (path.as_str(), content)
            }),
        )
    }
}

/// The build key over `(path, content digest)` pairs, in path order.
/// One function for [`DescriptorSet::build_key`] and
/// [`CrateSources::build_key`]: the two must agree byte for byte, or a
/// scan the dev loop could have reused is redone (or worse, one it must
/// redo is reused).
fn key_of<'a>(split_version: u32, files: impl Iterator<Item = (&'a str, String)>) -> String {
    let mut h = Sha256::new();
    h.update(split_version.to_le_bytes());
    for (path, content) in files {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update(content.as_bytes());
        h.update([0u8]);
    }
    hex(&h.finalize())
}

/// Scan one crate directory into a [`DescriptorSet`].
///
/// `dir` is the directory holding `Cargo.toml` and `src/`.
pub fn scan_crate(dir: &Path) -> Result<DescriptorSet> {
    Ok(scan_sources(&read_crate(dir)?))
}

/// A crate's `.rs` sources as read at one moment: the package name and
/// every file under `src/`, package-relative. Reading is milliseconds;
/// scanning ([`scan_sources`]) is the slow half (3.4 s for CrewForge's
/// app crate), so a caller that needs an archive describing EXACTLY the
/// moment a compile started reads first and scans alongside the compile.
#[derive(Debug, Clone)]
pub struct CrateSources {
    pub package: String,
    /// `(package-relative path, text)`, sorted by path. A file that could
    /// not be read is left out, as the scan always did.
    pub files: Vec<(String, String)>,
}

impl CrateSources {
    /// The [`DescriptorSet::build_key`] a scan of these sources would
    /// have — without scanning them, which is the slow half.
    ///
    /// A scan is a pure function of the sources (the package, each file's
    /// path and text) and the scanner, so a set already scanned from
    /// sources with this key IS the set a scan would produce now. That is
    /// what lets the dev loop skip re-scanning the crates a rebuild did
    /// not touch ([`crate::ScanMemo`]).
    pub fn build_key(&self) -> String {
        // Path order, as the set's `files` map holds them: `files` is
        // sorted by `PathBuf`, which orders `a/b` and `a-b` differently
        // from the string the set is keyed by.
        let files: BTreeMap<&str, &str> =
            self.files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        key_of(SPLIT_VERSION, files.into_iter().map(|(p, t)| (p, digest(t.as_bytes()))))
    }
}

/// Read `dir`'s sources (see [`CrateSources`]).
pub fn read_crate(dir: &Path) -> Result<CrateSources> {
    let package = package_name(dir)?;
    let src = dir.join("src");
    let mut paths = Vec::new();
    collect_rs_files(&src, &mut paths);
    // Also scan `tests/` and `examples/`? No: only what the app's
    // binary is built from can carry tags, and a descriptor for a site
    // that is never compiled into the running program is noise a differ
    // would have to filter.
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    for file in &paths {
        let Some(relative) = relative_to(dir, file) else { continue };
        match std::fs::read_to_string(file) {
            Ok(text) => files.push((relative, text)),
            Err(e) => eprintln!("[overlay] skipping {}: {e}", file.display()),
        }
    }
    Ok(CrateSources { package, files })
}

/// Scan sources already read into a [`DescriptorSet`].
pub fn scan_sources(sources: &CrateSources) -> DescriptorSet {
    scan_sources_cancellable(sources, &|| false).expect("a scan that is never cancelled finishes")
}

/// [`scan_sources`], abandoned between files once `cancel` says so:
/// `None`, and nothing half-built escapes.
///
/// The dev loop scans the edited crate BESIDE a hot patch's compile, and
/// a newer save kills that compile within milliseconds — but the attempt
/// could not return until the scan it had started finished: 1.4 s on
/// CrewForge's projects crate, paid on every save its editor reported
/// twice. A file is the unit of cancellation: one file's scan is
/// milliseconds, the whole crate's is seconds.
pub fn scan_sources_cancellable(
    sources: &CrateSources,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Option<DescriptorSet> {
    let package = sources.package.clone();
    let mut set = DescriptorSet {
        overlay_version: OVERLAY_VERSION,
        split_version: SPLIT_VERSION,
        package: package.clone(),
        files: BTreeMap::new(),
        sites: Vec::new(),
        scanner: scanner_identity(),
    };

    for (relative, text) in &sources.files {
        if cancel() {
            return None;
        }
        let relative = relative.clone();
        let content = hex(&Sha256::digest(text.as_bytes()));

        let sites = match runtime_macros_parse::sites_in_file(&package, &relative, &text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[overlay] skipping {package}/{relative} (does not parse: {e}); other files \
                     still scanned"
                );
                // Still hashed, with the skeleton standing in as the
                // whole file: a file this scan could not read is a file
                // any change to must rebuild, and recording it that way
                // is what makes that happen.
                set.files.insert(
                    relative,
                    FileDigest {
                        content: content.clone(),
                        skeleton: content.clone(),
                        shape: content.clone(),
                        sheets: content,
                        downstream: BTreeMap::new(),
                        consts: ConstDigest { opaque: true, ..ConstDigest::default() },
                    },
                );
                continue;
            }
        };
        let skeleton = hex(&Sha256::digest(
            runtime_macros_parse::skeleton_of(&text, &sites).as_bytes(),
        ));
        // A file whose shape cannot be computed records its own content
        // digest as the shape — no later save can match it, so every
        // change to it rebuilds. Same posture as the unparseable branch
        // above, for the same reason.
        let shape = hex(&Sha256::digest(
            runtime_macros_parse::shape_of(text)
                .unwrap_or_else(|| text.to_string())
                .as_bytes(),
        ));
        let sheets = sheets_digest(text);
        let downstream = downstream_digests(text);
        let consts = ConstDigest::of(text);
        set.files.insert(
            relative.clone(),
            FileDigest { content, skeleton, shape, sheets, downstream, consts },
        );
        for (ordinal, mut site) in sites.into_iter().enumerate() {
            let key = site.id.key();
            let Some(ui) = site.ui.as_mut() else {
                // Recorded with no descriptor: the ordinal has to stay
                // in step with the on-save scan, which counts this site
                // too. A site with no descriptor is never patched.
                set.sites.push(ArchivedSite {
                    key,
                    ordinal: ordinal as u32,
                    descriptor: empty_descriptor(site.id.clone()),
                });
                continue;
            };
            match runtime_macros_parse::describe(site.id.clone(), ui) {
                Ok(descriptor) => set.sites.push(ArchivedSite {
                    key,
                    ordinal: ordinal as u32,
                    descriptor,
                }),
                // A stamp mismatch is a bug in this crate's own two
                // walks, not in the author's code. Loud on stderr, and
                // recorded WITHOUT a descriptor so the ordinal still
                // lines up: a wrong descriptor is worse than a missing
                // one, and a missing ordinal is worse than both.
                Err(e) => {
                    eprintln!("[overlay] {}: {e}", site.id);
                    set.sites.push(ArchivedSite {
                        key,
                        ordinal: ordinal as u32,
                        descriptor: empty_descriptor(site.id.clone()),
                    });
                }
            }
        }
    }

    Some(set)
}

/// Scan `crate_dir` and write its descriptor set under
/// `target/idealyst/<app>/overlay/`, returning the file written.
///
/// `<app>` is the scanned crate's own package name, so a project with
/// several app crates gets one directory each.
///
/// Idempotent: the name is the build key, so re-running on unchanged
/// sources rewrites the same bytes to the same path.
/// Digest of a file's `stylesheet!` tokens — see [`FileDigest::sheets`].
pub fn sheets_digest(text: &str) -> String {
    hex(&Sha256::digest(
        runtime_macros_parse::stylesheet_tokens(text)
            .unwrap_or_else(|| text.to_string())
            .as_bytes(),
    ))
}

/// [`FileDigest::downstream`] for a file's text.
///
/// A file whose downstream bodies cannot be computed (it does not parse)
/// records one entry holding its content digest: no later save matches
/// it, so a change to that file in a library crate rebuilds. The shape
/// digest already forces that for an unparseable file; this keeps the
/// two digests telling the same story.
pub fn downstream_digests(text: &str) -> BTreeMap<String, String> {
    match runtime_macros_parse::downstream_bodies(text) {
        Some(bodies) => bodies
            .into_iter()
            .map(|(label, tokens)| (label, hex(&Sha256::digest(tokens.as_bytes()))))
            .collect(),
        None => BTreeMap::from([(
            "<file does not parse>".to_string(),
            hex(&Sha256::digest(text.as_bytes())),
        )]),
    }
}

pub fn write_for(project_root: &Path, crate_dir: &Path) -> Result<PathBuf> {
    let set = scan_crate(crate_dir)?;
    let dir = overlay_dir(project_root, &set.package);
    write_set(&dir, &set)
}

/// Scan `crate_dir` and write its descriptor set into `dir`, returning
/// the set as well — the workspace path keeps it in memory rather than
/// reading back the file it just wrote.
pub fn write_into(dir: &Path, crate_dir: &Path) -> Result<DescriptorSet> {
    let set = scan_crate(crate_dir)?;
    write_set(dir, &set)?;
    Ok(set)
}

/// [`write_into`], from sources already read.
pub fn write_scanned(dir: &Path, sources: &CrateSources) -> Result<DescriptorSet> {
    let set = scan_sources(sources);
    write_set(dir, &set)?;
    Ok(set)
}

/// [`write_scanned`], abandoned between files once `cancel` says so:
/// `Ok(None)`, and nothing is written — a cancelled scan leaves the
/// archive directory exactly as it found it.
pub fn write_scanned_cancellable(
    dir: &Path,
    sources: &CrateSources,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<DescriptorSet>> {
    let Some(set) = scan_sources_cancellable(sources, cancel) else { return Ok(None) };
    write_set(dir, &set)?;
    Ok(Some(set))
}

pub(crate) fn write_set(dir: &Path, set: &DescriptorSet) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{}.json", set.build_key()));
    let json = serde_json::to_string(set)?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Where one crate of the app's workspace keeps its descriptor sets.
///
/// The app crate's go where they always have, [`overlay_dir`] — the
/// runtime-server host reads them from there. A LIBRARY crate's go in
/// `crates/<package>/` INSIDE the app's directory rather than in an
/// `overlay_dir` of its own: `target/idealyst/<name>/` is a per-APP
/// staging tree, and a library named like some app would otherwise
/// share one. Two apps of one workspace that both depend on the library
/// each keep their own copy, under their own project root.
pub fn crate_overlay_dir(project_root: &Path, app: &str, package: &str) -> PathBuf {
    let dir = overlay_dir(project_root, app);
    if package == app {
        dir
    } else {
        dir.join("crates").join(package)
    }
}

/// `target/idealyst/<app>/overlay` — alongside the other per-app CLI
/// staging dirs, so `idealyst clean` already sweeps it.
pub fn overlay_dir(project_root: &Path, app: &str) -> PathBuf {
    project_root.join("target").join("idealyst").join(app).join("overlay")
}

/// Every `.rs` file under a directory, recursively.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A placeholder for a site this build could not describe.
///
/// Marked by having no roots AND no nodes, which `check_well_formed`
/// treats as legal (it is what `ui! {}` produces) — so a reader can tell
/// "nothing here" from "something here" without a second field, and a
/// differ against one produces no edits.
pub(crate) fn empty_descriptor(site: runtime_template::SiteId) -> Descriptor {
    Descriptor {
        site,
        slots: runtime_template::SlotSig { slots: Vec::new().into() },
        nodes: Vec::new().into(),
        roots: Vec::new().into(),
    }
}

/// `[package] name` of the crate, spelled as cargo spells it — hyphens
/// intact, because that is what `CARGO_PKG_NAME` gives the proc macro
/// and therefore what feeds the site key.
fn package_name(dir: &Path) -> Result<String> {
    let manifest_path = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parse {}", manifest_path.display()))?;
    manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string)
        .with_context(|| format!("{} has no [package] name", manifest_path.display()))
}

/// A file's path relative to the package root, `/`-separated — the
/// spelling the proc macro normalizes its span's file to.
fn relative_to(dir: &Path, file: &Path) -> Option<String> {
    Some(file.strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The digest spelling this module records, so the decision half can
/// produce a comparable one from a file it has in memory.
pub fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: every session start scanned the whole workspace again
    /// before the hot tier could take a save (14–16 s on CrewForge), on
    /// sources the last session had scanned and written to disk. The
    /// written scan is read back when nothing it depends on moved.
    #[test]
    fn regression_a_scan_written_by_an_earlier_session_is_reused() {
        let krate = fixture_crate();
        let out = tempfile::tempdir().unwrap();
        let sources = read_crate(krate.path()).unwrap();
        let written = write_scanned(out.path(), &sources).unwrap();
        assert!(written.scanner.is_some());
        assert_eq!(read_set(out.path(), &sources), Some(written.clone()));

        // Other sources: another key, nothing to reuse.
        let mut edited = sources.clone();
        edited.files[0].1.push_str("\n// edit\n");
        assert_eq!(read_set(out.path(), &edited), None);

        // A set another build of the scanner wrote is never reused.
        let path = out.path().join(format!("{}.json", sources.build_key()));
        let mut foreign = written.clone();
        foreign.scanner = Some("another-idealyst".into());
        std::fs::write(&path, serde_json::to_string(&foreign).unwrap()).unwrap();
        assert_eq!(read_set(out.path(), &sources), None);
        // Nor one from before the field existed.
        foreign.scanner = None;
        std::fs::write(&path, serde_json::to_string(&foreign).unwrap()).unwrap();
        assert_eq!(read_set(out.path(), &sources), None);
    }

    /// Build a throwaway crate on disk and scan it.
    ///
    /// A real directory rather than a string: the scan's job includes
    /// walking `src/`, reading `Cargo.toml`, and turning absolute paths
    /// back into package-relative ones, and none of that is exercised by
    /// handing it text.
    fn fixture_crate() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"overlay-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src/screens")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            r#"
mod screens;

#[component]
fn Root() -> Element {
    ui! {
        view(style = sheet()) {
            text { "Hello" }
        }
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/screens/login.rs"),
            r#"
#[component]
fn Login(count: i32) -> Element {
    ui! {
        view() {
            text { "Sign in" }
            if count > 0 {
                text { "again" }
            }
        }
    }
}
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn a_scan_finds_every_site_and_keys_it_by_where_it_is_written() {
        let dir = fixture_crate();
        let set = scan_crate(dir.path()).expect("scan");

        assert_eq!(set.package, "overlay-fixture");
        assert_eq!(set.split_version, SPLIT_VERSION);
        assert_eq!(set.sites.len(), 2, "one `ui!` per file");

        let mut files: Vec<&str> =
            set.sites.iter().map(|s| s.descriptor.site.file.as_ref()).collect();
        files.sort();
        assert_eq!(files, vec!["src/lib.rs", "src/screens/login.rs"]);
        // Package-relative and `/`-separated, never the tempdir's path:
        // the key must not depend on where the crate was checked out.
        assert!(set.files.keys().all(|f| !f.starts_with('/')), "{:?}", set.files);
    }

    /// The `if` occupies a node index between its siblings, and its body
    /// hangs under it. That is the property a differ addresses children
    /// by, so it is pinned on real scanned output rather than assumed.
    #[test]
    fn control_flow_is_a_node_with_its_body_underneath() {
        let dir = fixture_crate();
        let set = scan_crate(dir.path()).expect("scan");
        let login = set
            .sites
            .iter()
            .find(|s| s.descriptor.site.file == "src/screens/login.rs")
            .expect("login site");

        let opaque = login
            .descriptor
            .nodes
            .iter()
            .enumerate()
            .find_map(|(i, n)| match n {
                // The `if` is the Opaque node that HAS children; a bare
                // literal child of `text` is also Opaque (it is an
                // expression the descriptor cannot patch in place — the
                // patchable form is the parent's `content` prop) but it
                // is a leaf.
                runtime_template::Node::Opaque { expr: Some(e), children }
                    if !children.is_empty() =>
                {
                    Some((i as u32, e.to_string(), children.to_vec()))
                }
                _ => None,
            })
            .expect("the `if` is an Opaque node");
        assert_eq!(opaque.1, "count>0", "the condition is recorded as source text");
        assert_eq!(opaque.2.len(), 1, "its body is one node, hanging under it");
        assert!(opaque.2[0] > opaque.0, "a child is numbered after its parent");
        assert_eq!(runtime_template::check_well_formed(&login.descriptor), Ok(()));
    }

    /// Two scans of the same sources must produce the same build key,
    /// and any edit must change it. That is the whole contract of the
    /// file name.
    #[test]
    fn the_build_key_is_the_content_and_the_split_version() {
        let dir = fixture_crate();
        let first = scan_crate(dir.path()).expect("scan").build_key();
        assert_eq!(first, scan_crate(dir.path()).expect("scan").build_key());

        let login = dir.path().join("src/screens/login.rs");
        let text = std::fs::read_to_string(&login).unwrap().replace("Sign in", "Sign in.");
        std::fs::write(&login, text).unwrap();
        assert_ne!(first, scan_crate(dir.path()).expect("scan").build_key());
    }

    /// The key of sources read but not scanned is the key their scan
    /// gets — the dev loop reuses a scan on that equality, so a mismatch
    /// would rescan every crate on every rebuild (or, the other way, reuse
    /// a set for sources it does not describe). Covers an unparseable file
    /// (still hashed) and paths whose `PathBuf` order differs from their
    /// string order (`a/b.rs` vs `a-b.rs`).
    #[test]
    fn read_sources_key_like_their_scan() {
        let dir = fixture_crate();
        std::fs::write(dir.path().join("src/broken.rs"), "fn oops( {").unwrap();
        std::fs::create_dir_all(dir.path().join("src/a")).unwrap();
        std::fs::write(dir.path().join("src/a/b.rs"), "pub fn b() {}\n").unwrap();
        std::fs::write(dir.path().join("src/a-b.rs"), "pub fn ab() {}\n").unwrap();
        let read = read_crate(dir.path()).unwrap();
        assert_eq!(read.build_key(), scan_sources(&read).build_key());
        std::fs::write(dir.path().join("src/a-b.rs"), "pub fn ab() { }\n").unwrap();
        let moved = read_crate(dir.path()).unwrap();
        assert_ne!(read.build_key(), moved.build_key());
        assert_eq!(moved.build_key(), scan_sources(&moved).build_key());
    }

    /// One unparseable file costs its own descriptors and nothing else.
    #[test]
    fn a_file_that_does_not_parse_does_not_take_the_crate_down() {
        let dir = fixture_crate();
        std::fs::write(dir.path().join("src/broken.rs"), "fn oops( {").unwrap();
        let set = scan_crate(dir.path()).expect("scan");
        assert_eq!(set.sites.len(), 2, "the other two files still contributed");
        assert!(set.files.contains_key("src/broken.rs"), "and it is still hashed");
    }

    #[test]
    fn writing_puts_the_set_under_the_app_staging_dir() {
        let dir = fixture_crate();
        let project = tempfile::tempdir().expect("tempdir");
        let path = write_for(project.path(), dir.path()).expect("write");
        assert!(
            path.starts_with(project.path().join("target/idealyst/overlay-fixture/overlay")),
            "{path:?}"
        );
        let read: DescriptorSet =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, scan_crate(dir.path()).unwrap());
        assert_eq!(path.file_stem().unwrap().to_string_lossy(), read.build_key());
    }
}
