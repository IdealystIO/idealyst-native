//! The dev loop's decision: patch this save, or rebuild it?
//!
//! On a save the watcher has the new source. The build it is watching
//! left a descriptor set behind (see [`crate::archive`]). Together those
//! are enough to answer the question without starting a compiler — and
//! starting one is the thing worth avoiding, since a rebuild of a real
//! app is seconds and a patch is milliseconds.
//!
//! # The decision is conservative on purpose
//!
//! A save is OVERLAY-patchable only when EVERY one of these holds:
//!
//! 1. every changed file still parses;
//! 2. its [skeleton](runtime_macros_parse::file_skeleton) — the file
//!    with `ui!` bodies blanked — is byte-for-byte unchanged, so nothing
//!    outside a site moved;
//! 3. the file's `ui!` sites are the same sites, in the same order;
//! 4. every site whose descriptor changed diffs to a valid
//!    [`runtime_template::Patch`];
//! 5. the archive is the one this build produced.
//!
//! When (2), (3) or (4) fails the overlay is out, and the save asks the
//! hot-patch tier's question instead: is the file's SHAPE (everything
//! outside function bodies) unchanged? If so the edit is body-only —
//! a `ui!` literal becoming a variable, a nested `ui!` appearing inside
//! another's body — and a hot patch carries it, because it re-emits the
//! whole crate from source. If not, or if that cannot be proven, it
//! rebuilds. A file with both a patchable `ui!` edit and a logic edit
//! never takes the overlay, because (2) fails — and that is the case
//! worth being careful about: patching the label while skipping the
//! compiler that would have picked up the logic change is how a dev
//! loop starts lying about what is running.
//!
//! The asymmetry is deliberate. A wrong REBUILD costs seconds. A wrong
//! PATCH means the screen and the source disagree with nothing to say
//! so, and every subsequent edit is reasoned about against a program
//! that is not there.

use std::collections::BTreeMap;
use std::path::Path;

use runtime_template::{diff, Edit};

use crate::archive::{ArchivedSite, DescriptorSet, FileDigest};

/// What to do with a save.
///
/// Three live outcomes and one fallback, cheapest first. See
/// [`crate::archive::FileDigest`] for what each one is decided on.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Nothing changed that matters. No patch, no rebuild.
    Unchanged,
    /// Apply these patches; skip the compiler entirely.
    Patch(Vec<SitePatch>),
    /// Only function BODIES changed. Re-emit the user crate, link a
    /// patch dylib, rebind the jump table, re-run the mounted tree —
    /// all in the running process. Carries the files whose bodies
    /// moved, for the log.
    HotPatch(Vec<String>),
    /// Rebuild and respawn, for this reason.
    Rebuild(Reason),
}

/// One site's worth of edits, addressed by the key the compiled code
/// carries in its tags.
#[derive(Debug, Clone, PartialEq)]
pub struct SitePatch {
    pub site: u64,
    pub file: String,
    pub edits: Vec<Edit>,
}

/// Why a save could not be patched.
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    /// There is no archived descriptor set to diff against — the first
    /// save of a session started before the overlay existed, or a
    /// `cargo clean`.
    NoArchive,
    /// A changed file is not in the archive: it is new, or it was not
    /// scanned (a file outside `src/`).
    UnknownFile { file: String },
    /// A changed file no longer parses. Mid-edit, almost always.
    DoesNotParse { file: String },
    /// Something outside the `ui!` bodies changed. This is the common
    /// one, and it covers every logic edit.
    CodeChanged { file: String },
    /// The file's SHAPE moved — a signature, a props struct, a
    /// `static`, an attribute, the set of items. A hot patch cannot
    /// express this: the patch dylib is spliced into a process where
    /// every other crate is still the old build, so the two would
    /// disagree about layout. Only a rebuild is correct.
    ShapeChanged { file: String },
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::NoArchive => write!(f, "no descriptor set from this build"),
            Reason::UnknownFile { file } => write!(f, "{file} is not in this build"),
            Reason::DoesNotParse { file } => write!(f, "{file} does not parse"),
            Reason::CodeChanged { file } => write!(f, "{file} changed outside its `ui!` bodies"),
            Reason::ShapeChanged { file } => {
                write!(f, "{file} changed outside its function bodies")
            }
        }
    }
}

/// One changed file, as the watcher sees it.
pub struct ChangedFile {
    /// Package-relative, `/`-separated — the spelling the archive uses.
    pub path: String,
    /// The file's new contents.
    pub text: String,
}

/// Decide what a save means.
///
/// Pure: no filesystem, no compiler, no clock. Everything it needs is in
/// the two arguments, which is what lets the decision table be tested
/// exhaustively without a build.
pub fn decide(archive: Option<&DescriptorSet>, changed: &[ChangedFile]) -> Decision {
    let Some(archive) = archive else {
        return Decision::Rebuild(Reason::NoArchive);
    };
    // A document from an older producer has no per-site keys or
    // ordinals, so nothing here can address a patch. Rebuilding
    // regenerates it in the current format.
    if archive.overlay_version != crate::archive::OVERLAY_VERSION {
        return Decision::Rebuild(Reason::NoArchive);
    }

    // The archive's sites by file, in the ordinal order the producer
    // recorded — the same walk the on-save scan does.
    let mut by_file: BTreeMap<&str, Vec<&ArchivedSite>> = BTreeMap::new();
    for site in &archive.sites {
        by_file.entry(site.descriptor.site.file.as_ref()).or_default().push(site);
    }
    for sites in by_file.values_mut() {
        sites.sort_by_key(|s| s.ordinal);
    }

    let mut patches = Vec::new();
    // Files whose edit the overlay cannot carry but whose SHAPE did not
    // move — the subsecond tier. Collected rather than returned early
    // because a save can touch several files and the answer is the most
    // expensive one any of them needs.
    let mut body_only: Vec<String> = Vec::new();
    for file in changed {
        let Some(recorded) = archive.files.get(&file.path) else {
            return Decision::Rebuild(Reason::UnknownFile { file: file.path.clone() });
        };
        match decide_file(archive, &by_file, recorded, file) {
            FileOutcome::Unchanged => {}
            FileOutcome::Patches(mut site_patches) => patches.append(&mut site_patches),
            FileOutcome::BodyOnly => body_only.push(file.path.clone()),
            FileOutcome::Rebuild(why) => return Decision::Rebuild(why),
        }
    }

    // Most expensive outcome wins. A save that edits a literal in one
    // file and a body in another needs the compiler either way, and the
    // hot patch carries the literal along — applying the overlay patch
    // as well would address the pre-patch binary's site keys.
    if !body_only.is_empty() {
        Decision::HotPatch(body_only)
    } else if patches.is_empty() {
        Decision::Unchanged
    } else {
        Decision::Patch(patches)
    }
}

/// What ONE changed file needs, before the save's files are combined.
enum FileOutcome {
    /// Same bytes, or a change no descriptor sees.
    Unchanged,
    /// Overlay patches for this file's sites.
    Patches(Vec<SitePatch>),
    /// Not expressible as an overlay patch, but only function bodies
    /// moved: the hot-patch tier.
    BodyOnly,
    Rebuild(Reason),
}

fn decide_file(
    archive: &DescriptorSet,
    by_file: &BTreeMap<&str, Vec<&ArchivedSite>>,
    recorded: &FileDigest,
    file: &ChangedFile,
) -> FileOutcome {
    if recorded.content == crate::archive::digest(file.text.as_bytes()) {
        return FileOutcome::Unchanged;
    }

    let Ok(sites) = runtime_macros_parse::sites_in_file(&archive.package, &file.path, &file.text)
    else {
        return FileOutcome::Rebuild(Reason::DoesNotParse { file: file.path.clone() });
    };
    let skeleton =
        crate::archive::digest(runtime_macros_parse::skeleton_of(&file.text, &sites).as_bytes());
    if skeleton != recorded.skeleton {
        // Something outside the `ui!` bodies moved, so the overlay is
        // out. Its sites are not diffed: a hot patch re-emits the whole
        // crate from source, so every literal in this file arrives with
        // it, and diffing would only risk a patch addressed at keys the
        // patched binary no longer carries.
        return body_only_or_rebuild(recorded, file);
    }

    // Match by ORDINAL, not by site key. A body gaining a line moves
    // every site below it, which re-keys them — but the running binary
    // still carries the OLD keys, and the file still has the same sites
    // in the same order. Matching by position lets those saves patch;
    // keying by position alone made them all rebuild.
    //
    // The ordinal set changing means a site was added or removed — with
    // the skeleton unchanged, that is a `ui!` nested inside another
    // site's body. A new site has no compiled tag for an overlay patch to
    // address, but it is still an edit inside a function body, so it is
    // the hot-patch tier's to take if the shape agrees.
    let archived = by_file.get(file.path.as_str()).cloned().unwrap_or_default();
    if archived.len() != sites.len() {
        return body_only_or_rebuild(recorded, file);
    }
    let mut patches = Vec::new();
    for (before, mut site) in archived.iter().zip(sites) {
        // A site this save cannot parse, or one this build could not
        // describe, has nothing to diff — and it is not a hot patch
        // either: a `ui!` body that does not parse is a body that does
        // not compile, so the patch build would only fail on it.
        let Some(ui) = site.ui.as_mut() else {
            return FileOutcome::Rebuild(Reason::DoesNotParse { file: file.path.clone() });
        };
        if before.descriptor.nodes.is_empty() && before.descriptor.roots.is_empty() {
            return FileOutcome::Rebuild(Reason::DoesNotParse { file: file.path.clone() });
        }
        // Describe under the ARCHIVED site id, so the diff compares two
        // versions of one site rather than refusing them as two
        // different sites — and so the patch is addressed to the key the
        // compiled code carries.
        let Ok(after) = runtime_macros_parse::describe(before.descriptor.site.clone(), ui) else {
            // The two numbering walks disagreed — a bug in this crate's
            // own pass, not in the author's code. The overlay cannot
            // address this site; a compiler can.
            return body_only_or_rebuild(recorded, file);
        };
        match diff(&before.descriptor, &after) {
            Ok(patch) if patch.edits.is_empty() => {}
            Ok(patch) => patches.push(SitePatch {
                site: before.key,
                file: file.path.clone(),
                edits: patch.edits.into_owned(),
            }),
            // The differ refuses what a patch cannot express: a static
            // slot turning dynamic (`{ "Title" }` → `{ title }`), a
            // literal becoming a closure. That is new CODE inside a
            // function body — the hot-patch tier's case, not a rebuild's,
            // when nothing outside the bodies moved.
            Err(_) => return body_only_or_rebuild(recorded, file),
        }
    }
    FileOutcome::Patches(patches)
}

/// The hot-patch tier's question, for a file the overlay cannot carry:
/// did anything outside its FUNCTION bodies move?
///
/// This is the safety boundary, not an optimization. Routing a shape
/// change into a patch would splice code with a new layout into a
/// process that still holds the old one everywhere else — silent
/// corruption rather than a stale screen. So anything this cannot prove
/// is body-only rebuilds: a shape that moved is `ShapeChanged`, and a
/// file whose shape cannot be computed is `DoesNotParse`.
fn body_only_or_rebuild(recorded: &FileDigest, file: &ChangedFile) -> FileOutcome {
    let Some(shape) = runtime_macros_parse::shape_of(&file.text) else {
        return FileOutcome::Rebuild(Reason::DoesNotParse { file: file.path.clone() });
    };
    if crate::archive::digest(shape.as_bytes()) != recorded.shape {
        return FileOutcome::Rebuild(Reason::ShapeChanged { file: file.path.clone() });
    }
    FileOutcome::BodyOnly
}

/// Fold a decided save back into the archive, so the NEXT save is
/// decided against what is actually running.
///
/// Call it after the save's patch has been APPLIED — an overlay patch
/// sent, or a hot patch pushed. A rebuild regenerates the archive from
/// source instead, which is also what drops every staged patch. What
/// "running" means differs by tier, so this re-derives the save's
/// decision (it is pure, and the archive has not moved since the caller
/// decided) and advances accordingly:
///
/// - **Overlay patch** (or a save no descriptor sees): the DESCRIPTORS
///   move forward, the KEYS stay. The binary still carries the keys it
///   was compiled with, and no overlay patch changes them — a `ui!` body
///   gaining a line re-keys every site below it in source, but not in
///   the binary. Without this, a second edit to the same literal would
///   diff against the original and re-send an edit that is already
///   applied — harmless once, wrong the moment the two edits are not the
///   same shape.
/// - **Hot patch**: every changed file's sites are re-derived from the
///   new source, KEYS included. The patch re-emitted the crate, and the
///   remounted tree is built by the patched code, whose tags carry the
///   keys of the source it was compiled from. Keeping the old keys would
///   address the next literal edit at a site the running tree no longer
///   has whenever the hot patch shifted a line (a `let` added above a
///   `ui!`), and keeping the old SITE LIST would leave a `ui!` the patch
///   added with no archived site at all.
///
/// What this cannot do is re-key a file the save did NOT touch: a hot
/// patch re-emits those too, and one whose sites moved since the last
/// full scan (an earlier overlay patch that grew a `ui!` body, say) is
/// re-keyed in the binary but not here. The exact fix for that is the
/// one the runtime-server host already uses — re-scan the crate
/// ([`crate::scan_crate`]) after a hot patch lands, instead of calling
/// this.
pub fn advance_archive(archive: &mut DescriptorSet, changed: &[ChangedFile]) {
    let rekey = match decide(Some(archive), changed) {
        Decision::HotPatch(_) => true,
        Decision::Patch(_) | Decision::Unchanged => false,
        // Nothing was applied, so nothing is running that the archive
        // does not already describe. The caller rebuilds and re-scans.
        Decision::Rebuild(_) => return,
    };
    for file in changed {
        let Ok(sites) =
            runtime_macros_parse::sites_in_file(&archive.package, &file.path, &file.text)
        else {
            continue;
        };
        let skeleton =
            crate::archive::digest(runtime_macros_parse::skeleton_of(&file.text, &sites).as_bytes());
        archive.files.insert(
            file.path.clone(),
            FileDigest {
                content: crate::archive::digest(file.text.as_bytes()),
                skeleton,
                shape: crate::archive::digest(
                    runtime_macros_parse::shape_of(&file.text)
                        .unwrap_or_else(|| file.text.clone())
                        .as_bytes(),
                ),
            },
        );
        if rekey {
            rekey_file(archive, &file.path, sites);
        } else {
            advance_descriptors(archive, &file.path, sites);
        }
    }
}

/// The overlay half of [`advance_archive`]: new descriptors, old keys.
fn advance_descriptors(
    archive: &mut DescriptorSet,
    path: &str,
    sites: Vec<runtime_macros_parse::Site>,
) {
    for (ordinal, mut site) in sites.into_iter().enumerate() {
        let Some(ui) = site.ui.as_mut() else { continue };
        let Some(slot) = archive
            .sites
            .iter_mut()
            .find(|s| s.descriptor.site.file == path && s.ordinal == ordinal as u32)
        else {
            continue;
        };
        // Describe under the archived site id and keep `key` untouched.
        // The DESCRIPTOR moves forward so the next save diffs against
        // what is running; the KEY must not, because it is what the
        // compiled binary's tags say and no overlay patch changes that.
        let Ok(after) = runtime_macros_parse::describe(slot.descriptor.site.clone(), ui) else {
            continue;
        };
        slot.descriptor = after;
    }
}

/// The hot-patch half of [`advance_archive`]: the file's sites exactly
/// as a fresh scan of the new source would record them.
///
/// Replaces the file's whole site list, in place, so the archive keeps
/// its file-then-document order. A site that cannot be described is
/// recorded with an empty descriptor, as the producer does — never with
/// the previous build's, which would describe a site the patched code
/// no longer builds.
fn rekey_file(archive: &mut DescriptorSet, path: &str, sites: Vec<runtime_macros_parse::Site>) {
    let fresh: Vec<ArchivedSite> = sites
        .into_iter()
        .enumerate()
        .map(|(ordinal, mut site)| {
            let key = site.id.key();
            let descriptor = site
                .ui
                .as_mut()
                .and_then(|ui| runtime_macros_parse::describe(site.id.clone(), ui).ok())
                .unwrap_or_else(|| crate::archive::empty_descriptor(site.id.clone()));
            ArchivedSite { key, ordinal: ordinal as u32, descriptor }
        })
        .collect();
    let at = archive
        .sites
        .iter()
        .position(|s| s.descriptor.site.file == path)
        .unwrap_or(archive.sites.len());
    archive.sites.retain(|s| s.descriptor.site.file != path);
    let at = at.min(archive.sites.len());
    archive.sites.splice(at..at, fresh);
}

/// The payload the delivery side ships, in both dev shapes.
///
/// `wire::WireOverlayPatch`'s JSON shape, spelled here rather than by
/// depending on `wire`: this crate is the WATCHER, and taking a
/// protocol dependency to emit four fields would couple the file
/// watcher to the dev-server's wire crate for no gain. The shape is
/// pinned by a test that decodes it as the real type.
pub fn wire_payload(patch: &SitePatch) -> serde_json::Value {
    serde_json::json!({
        "site": patch.site,
        "edits": patch.edits,
    })
}

/// Read the newest descriptor set a build left under `project_root`.
///
/// Newest by modification time: a build writes one file per content
/// hash, so several can accumulate over a session and the most recent is
/// the one the running binary was built from.
pub fn load_archive(project_root: &Path, app: &str) -> Option<DescriptorSet> {
    let dir = crate::archive::overlay_dir(project_root, app);
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
            newest = Some((modified, path));
        }
    }
    let (_, path) = newest?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = r#"
use runtime_core::*;

fn count() -> i32 {
    1
}

#[component]
fn Screen() -> Element {
    ui! {
        view() {
            text { "hello" }
        }
    }
}
"#;

    /// Build an archive the way a real build does: scan a crate on disk.
    /// A real directory rather than a synthesized `DescriptorSet`,
    /// because the decision compares against what the PRODUCER wrote and
    /// a hand-built archive could agree with a decision that the real one
    /// would not.
    fn archive_of(source: &str) -> (tempfile::TempDir, DescriptorSet) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"decide-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/app.rs"), source).unwrap();
        let set = crate::archive::scan_crate(dir.path()).expect("scan");
        (dir, set)
    }

    fn changed(text: &str) -> Vec<ChangedFile> {
        vec![ChangedFile { path: "src/app.rs".into(), text: text.to_string() }]
    }

    // --- the decision table ------------------------------------------

    #[test]
    fn unchanged_source_is_neither_patched_nor_rebuilt() {
        let (_d, archive) = archive_of(APP);
        assert_eq!(decide(Some(&archive), &changed(APP)), Decision::Unchanged);
    }

    #[test]
    fn a_literal_inside_a_ui_body_is_a_patch() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace(r#""hello""#, r#""goodbye""#);
        match decide(Some(&archive), &changed(&edited)) {
            Decision::Patch(patches) => {
                assert_eq!(patches.len(), 1);
                assert_eq!(patches[0].file, "src/app.rs");
                assert_eq!(patches[0].edits.len(), 1);
                assert_ne!(patches[0].site, 0);
            }
            other => panic!("expected a patch, got {other:?}"),
        }
    }

    /// A statement inside a function body is not something the
    /// overlay can express — but it IS something a jump table can
    /// rebind, because nothing about the file's shape moved.
    #[test]
    fn a_logic_edit_is_a_hot_patch() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace("    1\n", "    2\n");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
    }

    /// One save carrying both. The overlay must not apply its half:
    /// the compiler is running either way, and a hot patch re-emits the
    /// whole crate from source, so the literal arrives with it. Applying
    /// the overlay patch as well would address site keys the patched
    /// binary no longer carries.
    #[test]
    fn a_save_with_both_a_ui_edit_and_a_logic_edit_is_one_hot_patch() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace(r#""hello""#, r#""goodbye""#).replace("    1\n", "    2\n");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
    }

    // ---------------------------------------------------------------
    // The decision table for the middle tier. Each of these changes
    // code the overlay cannot express; the question is only whether a
    // jump table can carry it, and getting THAT wrong is unsafe rather
    // than slow (see `Reason::ShapeChanged`).
    // ---------------------------------------------------------------

    /// A comment or blank line outside a body moves no shape — tokens,
    /// not text, are what the shape digest compares.
    #[test]
    fn a_comment_added_outside_a_body_is_a_hot_patch() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace("#[component]", "// a new line\n#[component]");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
    }

    /// The dangerous-looking one: a body that gains state. Body-only,
    /// so it patches. What happens to the OLD state across the re-run
    /// is the runtime's problem, not the decision's.
    #[test]
    fn adding_a_signal_call_is_a_hot_patch() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace(
            "fn Screen() -> Element {",
            "fn Screen() -> Element {\n    let extra = signal(0i32);",
        );
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
    }

    /// A signature change is the boundary. The patch dylib would
    /// compute one layout while every un-re-emitted crate in the
    /// process computes another.
    #[test]
    fn changing_a_signature_rebuilds_as_a_shape_change() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace("fn count() -> i32", "fn count() -> i64");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::ShapeChanged { file: "src/app.rs".into() })
        );
    }

    #[test]
    fn adding_a_prop_rebuilds_as_a_shape_change() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace("fn Screen() -> Element", "fn Screen(tone: String) -> Element");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::ShapeChanged { file: "src/app.rs".into() })
        );
    }

    #[test]
    fn adding_an_item_rebuilds_as_a_shape_change() {
        let (_d, archive) = archive_of(APP);
        let edited = format!("{APP}\n pub struct Extra {{ pub n: u8 }}\n");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::ShapeChanged { file: "src/app.rs".into() })
        );
    }

    #[test]
    fn changing_a_static_rebuilds_as_a_shape_change() {
        let src = format!("{APP}\n pub static LIMIT: u32 = 1;\n");
        let (_d, archive) = archive_of(&src);
        let edited = src.replace("LIMIT: u32 = 1", "LIMIT: u32 = 2");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::ShapeChanged { file: "src/app.rs".into() })
        );
    }

    /// A literal-only edit still takes the CHEAPEST tier. The hot-patch
    /// tier must not swallow saves the overlay can already handle
    /// without a compiler.
    #[test]
    fn a_literal_only_edit_still_takes_the_overlay_tier() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace(r#""hello""#, r#""goodbye""#);
        assert!(matches!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Patch(_)
        ));
    }

    /// An edit a patch cannot express — here a literal becoming a
    /// closure — is refused by the differ. That is new code inside a
    /// function body, so with the shape unchanged it is the hot-patch
    /// tier's, not a rebuild's.
    #[test]
    fn an_edit_the_differ_refuses_is_a_hot_patch_when_the_shape_holds() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace(
            r#"text { "hello" }"#,
            r#"text { move || format!("{}", count()) }"#,
        );
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
    }

    /// The lab-app repro: `text(style = heading) { "Hot reload lab" }`
    /// → `text(style = heading) { x }`, with `x` already bound in the
    /// body. The skeleton is unchanged (bodies are blanked in it), so the
    /// save reached the ordinal-matched diff, the differ refused a static
    /// slot turning dynamic — and the decision returned `Refused` on the
    /// spot, rebuilding and reloading the page (state lost) for an edit
    /// that never left a function body.
    #[test]
    fn regression_literal_becoming_dynamic_is_a_hot_patch() {
        let src = APP.replace(
            "fn Screen() -> Element {",
            "fn Screen() -> Element {\n    let x = \"123\".to_string();",
        );
        let (_d, archive) = archive_of(&src);
        let edited = src.replace(r#"text { "hello" }"#, "text { x }");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
    }

    /// A `ui!` invocation appearing inside another site's body leaves the
    /// skeleton byte-identical (the outer body is blanked in it) and
    /// changes only the ordinal count. That returned `SitesMoved` and
    /// rebuilt; it is a body-only edit, and a hot patch compiles the new
    /// site in with everything else.
    #[test]
    fn regression_new_ui_site_inside_a_body_is_a_hot_patch() {
        let src = r#"
#[component]
fn A() -> Element {
    ui! { view() { presence(visible = f) { text { "inner" } } } }
}
"#;
        let (_d, archive) = archive_of(src);
        assert_eq!(archive.sites.len(), 1);

        let edited = src.replace(r#"text { "inner" }"#, r#"ui! { text { "inner" } }"#);
        let before = runtime_macros_parse::sites_in_file(&archive.package, "src/app.rs", src)
            .expect("parses");
        let after = runtime_macros_parse::sites_in_file(&archive.package, "src/app.rs", &edited)
            .expect("parses");
        assert_eq!(
            runtime_macros_parse::skeleton_of(src, &before),
            runtime_macros_parse::skeleton_of(&edited, &after),
            "the fixture must leave the skeleton alone, or it tests the other branch"
        );
        assert_eq!(after.len(), 2, "and it must add a site");

        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
    }

    /// Falling through to the hot-patch tier asks ITS question, not a
    /// free pass: a refused `ui!` edit in a save that also moves a
    /// signature is still a shape change, and still rebuilds.
    #[test]
    fn regression_literal_becoming_dynamic_with_a_signature_change_rebuilds() {
        let src = APP.replace(
            "fn Screen() -> Element {",
            "fn Screen() -> Element {\n    let x = \"123\".to_string();",
        );
        let (_d, archive) = archive_of(&src);
        let edited = src
            .replace(r#"text { "hello" }"#, "text { x }")
            .replace("fn count() -> i32", "fn count() -> i64");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::ShapeChanged { file: "src/app.rs".into() })
        );
    }

    /// The save AFTER a literal-to-dynamic hot patch. Here the same save
    /// also adds the `let` the new slot reads, so the `ui!` moves down a
    /// line: the patched code the page remounted with tags that site with
    /// the NEW position's key. A literal edit next must be an overlay
    /// patch addressed at THAT key, diffed against the patched source's
    /// descriptor. Advancing the archive the overlay way kept the build's
    /// key, so the patch went to a site the running tree no longer had and
    /// the screen silently stayed put.
    #[test]
    fn regression_literal_edit_after_a_dynamic_hot_patch_addresses_the_patched_code() {
        let src = APP.replace(r#"text { "hello" }"#, r#"text { "hello" } text { "there" }"#);
        let (_d, mut archive) = archive_of(&src);
        let built_key = archive.sites[0].key;

        let hot = src
            .replace(
                "fn Screen() -> Element {",
                "fn Screen() -> Element {\n    let x = \"123\".to_string();",
            )
            .replace(r#"text { "hello" }"#, "text { x }");
        assert_eq!(
            decide(Some(&archive), &changed(&hot)),
            Decision::HotPatch(vec!["src/app.rs".into()])
        );
        advance_archive(&mut archive, &changed(&hot));

        let patched = runtime_macros_parse::sites_in_file(&archive.package, "src/app.rs", &hot)
            .expect("parses");
        let running_key = patched[0].id.key();
        assert_ne!(running_key, built_key, "the fixture must move the site, or this proves nothing");
        assert_eq!(archive.sites.len(), 1);
        assert_eq!(archive.sites[0].key, running_key, "the patched code's key, not the build's");

        let next = hot.replace(r#""there""#, r#""everyone""#);
        match decide(Some(&archive), &changed(&next)) {
            Decision::Patch(patches) => {
                assert_eq!(patches.len(), 1);
                assert_eq!(patches[0].site, running_key);
                // And the edits are the ones a diff against the PATCHED
                // source gives — the dynamic slot is not re-sent as a
                // literal, and nothing else is touched.
                let mut sites = runtime_macros_parse::sites_in_file(
                    &archive.package,
                    "src/app.rs",
                    &hot,
                )
                .unwrap();
                let mut next_sites = runtime_macros_parse::sites_in_file(
                    &archive.package,
                    "src/app.rs",
                    &next,
                )
                .unwrap();
                let id = sites[0].id.clone();
                let a = runtime_macros_parse::describe(id.clone(), sites[0].ui.as_mut().unwrap())
                    .unwrap();
                let b = runtime_macros_parse::describe(id, next_sites[0].ui.as_mut().unwrap())
                    .unwrap();
                assert_eq!(patches[0].edits, diff(&a, &b).unwrap().edits.into_owned());
            }
            other => panic!("expected an overlay patch, got {other:?}"),
        }
    }

    /// A site the hot patch ADDED is in the patched code, so it is in the
    /// archive afterwards, and a literal edit inside it patches. Advancing
    /// the overlay way left it out — the ordinal count then disagreed on
    /// every later save of that file until a full rebuild.
    ///
    /// The new `ui!` is a `let` in a body rather than nested inside the
    /// existing site: an edit inside a nested site is also an edit to the
    /// OUTER site's opaque child expression, which the overlay rightly
    /// refuses.
    #[test]
    fn a_site_a_hot_patch_added_is_patchable_on_the_next_save() {
        let (_d, mut archive) = archive_of(APP);
        let hot = APP.replace(
            "fn Screen() -> Element {",
            "fn Screen() -> Element {\n    let _extra = ui! { text { \"inner\" } };",
        );
        assert!(matches!(decide(Some(&archive), &changed(&hot)), Decision::HotPatch(_)));
        advance_archive(&mut archive, &changed(&hot));
        assert_eq!(archive.sites.len(), 2);
        assert_eq!(archive.sites.iter().map(|s| s.ordinal).collect::<Vec<_>>(), vec![0, 1]);

        let next = hot.replace(r#""inner""#, r#""inner!""#);
        let new_site = runtime_macros_parse::sites_in_file(&archive.package, "src/app.rs", &hot)
            .unwrap()[0]
            .id
            .key();
        match decide(Some(&archive), &changed(&next)) {
            Decision::Patch(patches) => {
                assert_eq!(patches.len(), 1);
                assert_eq!(patches[0].site, new_site);
            }
            other => panic!("expected an overlay patch, got {other:?}"),
        }
    }

    /// A body edit that shifts a line (the common `let` added above a
    /// `ui!`) re-keys the site in the patched code. The next literal edit
    /// must address the new key — the build's is gone from the tree.
    #[test]
    fn regression_literal_edit_after_a_line_shifting_hot_patch_uses_the_new_key() {
        let (_d, mut archive) = archive_of(APP);
        let built_key = archive.sites[0].key;
        let hot = APP.replace(
            "fn Screen() -> Element {",
            "fn Screen() -> Element {\n    let extra = signal(0i32);",
        );
        assert!(matches!(decide(Some(&archive), &changed(&hot)), Decision::HotPatch(_)));
        advance_archive(&mut archive, &changed(&hot));

        let next = hot.replace(r#""hello""#, r#""goodbye""#);
        match decide(Some(&archive), &changed(&next)) {
            Decision::Patch(patches) => {
                assert_eq!(patches.len(), 1);
                assert_ne!(patches[0].site, built_key, "the build's key is not in the tree");
                assert_eq!(
                    patches[0].site,
                    runtime_macros_parse::sites_in_file(&archive.package, "src/app.rs", &hot)
                        .unwrap()[0]
                        .id
                        .key()
                );
            }
            other => panic!("expected an overlay patch, got {other:?}"),
        }
    }

    /// An overlay patch must NOT re-key: the binary was not recompiled,
    /// so it still carries the build's key even when the patch grew a
    /// `ui!` body and moved a site below it in source.
    #[test]
    fn advancing_after_an_overlay_patch_keeps_the_builds_keys() {
        let two_sites = format!(
            "{APP}\n#[component]\nfn Other() -> Element {{\n    ui! {{ text {{ \"two\" }} }}\n}}\n"
        );
        let (_d, mut archive) = archive_of(&two_sites);
        let keys: Vec<u64> = archive.sites.iter().map(|s| s.key).collect();
        let edited = two_sites.replace(
            "            text { \"hello\" }\n",
            "            text { \"hello\" }\n            text { \"extra\" }\n",
        );
        assert!(matches!(decide(Some(&archive), &changed(&edited)), Decision::Patch(_)));
        advance_archive(&mut archive, &changed(&edited));
        assert_eq!(archive.sites.iter().map(|s| s.key).collect::<Vec<_>>(), keys);
    }

    /// A `ui!` body gaining a line moves every site BELOW it in the
    /// file, which re-keys them — the compiled tags still carry the old
    /// keys. Matching by ORDINAL instead of by key is what lets this
    /// save patch: the file still has the same sites in the same order,
    /// and the patch is addressed to the key the binary actually has.
    ///
    /// Before ordinal matching this rebuilt, which meant adding one line
    /// to a `ui!` body cost a full compile no matter how trivial the
    /// edit was.
    #[test]
    fn a_body_growing_a_line_patches_the_sites_below_it_instead_of_rebuilding() {
        let two_sites = format!(
            "{APP}\n#[component]\nfn Other() -> Element {{\n    ui! {{ text {{ \"two\" }} }}\n}}\n"
        );
        let (_d, archive) = archive_of(&two_sites);
        assert_eq!(archive.sites.len(), 2, "the fixture must have a site below the edit");
        let second_key = archive.sites[1].key;

        let edited = two_sites.replace(
            "            text { \"hello\" }\n",
            "            text { \"hello there\" }\n            text { \"extra\" }\n",
        );
        match decide(Some(&archive), &changed(&edited)) {
            Decision::Patch(patches) => {
                assert_eq!(patches.len(), 1, "only the edited site changed: {patches:?}");
                assert_eq!(patches[0].site, archive.sites[0].key);
            }
            other => panic!("expected a patch, got {other:?}"),
        }

        // The untouched site below must still be addressable by the key
        // the binary holds, not by the one its new line number implies.
        let moved = runtime_macros_parse::sites_in_file(
            &archive.package,
            "src/app.rs",
            &edited,
        )
        .expect("parses");
        assert_ne!(
            moved[1].id.key(),
            second_key,
            "the fixture must actually re-key the site below, or this proves nothing"
        );
    }

    /// A site APPEARING has no compiled tag to address, so there is
    /// nothing a patch could reach. It rebuilds — and in practice the
    /// SKELETON catches it one step before the ordinal check does,
    /// because writing a new `ui!` invocation means writing the code
    /// around it too. Both guards are real; this pins the outcome
    /// without pinning which one fires, since that depends on how the
    /// site was added.
    #[test]
    fn adding_a_site_rebuilds() {
        let (_d, archive) = archive_of(APP);
        let edited = format!(
            "{APP}\n#[component]\nfn New() -> Element {{ ui! {{ text {{ \"new\" }} }} }}\n"
        );
        assert!(
            matches!(decide(Some(&archive), &changed(&edited)), Decision::Rebuild(_)),
            "a new site has no compiled tag to address"
        );
    }

    /// The ordinal check on its own, reached by removing a site while
    /// leaving the skeleton byte-identical — only possible from INSIDE
    /// another site's body, which is where nested sites live. The
    /// vanished site had a compiled tag, so the overlay cannot express
    /// it; the edit is still inside a function body, so a hot patch can.
    #[test]
    fn removing_a_nested_site_is_a_hot_patch_on_the_ordinal_check() {
        let source = r#"
#[component]
fn A() -> Element {
    ui! { view() { presence(visible = f) { ui! { text { "inner" } } } } }
}
"#;
        let (_d, archive) = archive_of(source);
        assert_eq!(archive.sites.len(), 2, "outer and inner");

        let edited = source.replace(r#"ui! { text { "inner" } }"#, r#"text { "inner" }"#);
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::HotPatch(vec!["src/app.rs".into()]),
            "one site fewer: not an overlay patch, but body-only"
        );
    }

    /// A site nested in another macro's tokens counts in document
    /// order, so the sites after it keep their ordinals. Get this wrong
    /// — append nested sites at the end, say — and every patch after the
    /// nested one addresses the wrong site.
    #[test]
    fn a_nested_site_takes_its_place_in_the_ordinal_order() {
        let source = r#"
#[component]
fn A() -> Element {
    ui! { text { "first" } }
}

#[component]
fn B() -> Element {
    pressable(vec![ui! { text { "nested" } }], || {})
}

#[component]
fn C() -> Element {
    ui! { text { "third" } }
}
"#;
        let (_d, archive) = archive_of(source);
        assert_eq!(archive.sites.len(), 3, "the nested site is archived too");
        assert_eq!(
            archive.sites.iter().map(|s| s.ordinal).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        // Editing the LAST site must patch the last site. If the nested
        // one were appended rather than slotted in at ordinal 1, this
        // would address the nested site instead.
        let edited = source.replace(r#""third""#, r#""third!""#);
        match decide(Some(&archive), &changed(&edited)) {
            Decision::Patch(patches) => {
                assert_eq!(patches.len(), 1);
                assert_eq!(patches[0].site, archive.sites[2].key);
            }
            other => panic!("expected a patch, got {other:?}"),
        }

        // And the nested site itself is patchable, which it never was
        // before: `syn` does not descend into a macro's tokens.
        let edited = source.replace(r#""nested""#, r#""nested!""#);
        match decide(Some(&archive), &changed(&edited)) {
            Decision::Patch(patches) => {
                assert_eq!(patches.len(), 1);
                assert_eq!(patches[0].site, archive.sites[1].key);
            }
            other => panic!("expected a patch, got {other:?}"),
        }
    }

    #[test]
    fn a_file_that_does_not_parse_rebuilds() {
        let (_d, archive) = archive_of(APP);
        assert_eq!(
            decide(Some(&archive), &changed("fn oops( {")),
            Decision::Rebuild(Reason::DoesNotParse { file: "src/app.rs".into() })
        );
    }

    #[test]
    fn a_file_the_build_never_saw_rebuilds() {
        let (_d, archive) = archive_of(APP);
        let new_file = vec![ChangedFile { path: "src/new.rs".into(), text: APP.into() }];
        assert_eq!(
            decide(Some(&archive), &new_file),
            Decision::Rebuild(Reason::UnknownFile { file: "src/new.rs".into() })
        );
    }

    #[test]
    fn no_archive_rebuilds() {
        assert_eq!(decide(None, &changed(APP)), Decision::Rebuild(Reason::NoArchive));
    }

    /// After a patch, the archive must describe what is RUNNING. Without
    /// this the next save diffs against the original and re-sends an
    /// edit that is already applied — harmless once, wrong the moment
    /// the two edits are not the same shape.
    #[test]
    fn advancing_the_archive_makes_the_next_save_diff_against_the_patch() {
        let (_d, mut archive) = archive_of(APP);
        let once = APP.replace(r#""hello""#, r#""goodbye""#);
        assert!(matches!(decide(Some(&archive), &changed(&once)), Decision::Patch(_)));

        advance_archive(&mut archive, &changed(&once));
        assert_eq!(
            decide(Some(&archive), &changed(&once)),
            Decision::Unchanged,
            "the same source must now be a no-op"
        );

        let twice = APP.replace(r#""hello""#, r#""farewell""#);
        match decide(Some(&archive), &changed(&twice)) {
            Decision::Patch(patches) => assert_eq!(patches.len(), 1),
            other => panic!("expected a patch, got {other:?}"),
        }
    }
}

/// The watcher emits its patch payload by hand rather than depending on
/// the protocol crate. These tests are what make that safe.
#[cfg(test)]
mod payload_tests {
    use super::*;
    use std::borrow::Cow;

    fn every_shape() -> SitePatch {
        SitePatch {
            site: u64::MAX,
            file: "src/app.rs".into(),
            edits: vec![
                Edit::SetProp {
                    node: 4,
                    name: Cow::Borrowed("content"),
                    value: runtime_template::LiteralValue::Str(Cow::Borrowed("hi")),
                },
                Edit::SetProp {
                    node: 5,
                    name: Cow::Borrowed("count"),
                    value: runtime_template::LiteralValue::Int(-2),
                },
                Edit::SetProp {
                    node: 6,
                    name: Cow::Borrowed("ratio"),
                    value: runtime_template::LiteralValue::Float(1.5),
                },
                Edit::SetProp {
                    node: 7,
                    name: Cow::Borrowed("loud"),
                    value: runtime_template::LiteralValue::Bool(false),
                },
                Edit::SetProp {
                    node: 8,
                    name: Cow::Borrowed("tone"),
                    value: runtime_template::LiteralValue::Path(Cow::Borrowed("tone::Danger")),
                },
                Edit::SetChildren {
                    node: 0,
                    children: Cow::Owned(vec![runtime_template::NewNode {
                        kind: Cow::Borrowed("view"),
                        props: Cow::Owned(vec![
                            runtime_template::PropEntry {
                                name: Cow::Borrowed("style"),
                                value: runtime_template::PropValue::Slot(3),
                            },
                            runtime_template::PropEntry {
                                name: Cow::Borrowed("is_container"),
                                value: runtime_template::PropValue::Lit(
                                    runtime_template::LiteralValue::Bool(true),
                                ),
                            },
                        ]),
                        children: Cow::Owned(vec![runtime_template::NewNode {
                            kind: Cow::Borrowed("text"),
                            props: Cow::Borrowed(&[]),
                            children: Cow::Borrowed(&[]),
                        }]),
                    }]),
                },
            ],
        }
    }

    /// The watcher's hand-written JSON must decode as the protocol's
    /// own type, field for field, over every shape an edit can take.
    ///
    /// This is the seam a mirror type usually loses on: a tuple where
    /// the other side has a struct, a renamed field, a variant spelled
    /// differently. None of it would fail loudly — the patch would
    /// decode partially, or not at all, and the page would quietly not
    /// change.
    #[test]
    fn the_watchers_payload_decodes_as_the_protocol_type() {
        let patch = every_shape();
        let json = serde_json::to_string(&wire_payload(&patch)).expect("encode");
        let decoded: wire::WireOverlayPatch =
            serde_json::from_str(&json).expect("the protocol type must accept it");

        assert_eq!(decoded.site, patch.site);
        assert_eq!(
            decoded.to_edits(),
            patch.edits,
            "and converting back must give the same edits"
        );
    }

    /// The payload travels on one SSE `data:` line, so it must not
    /// contain a newline. `serde_json::to_string` is compact, and this
    /// is what stops a later switch to `to_string_pretty` from
    /// truncating every patch at the first line break.
    #[test]
    fn the_payload_is_one_line() {
        let json = serde_json::to_string(&wire_payload(&every_shape())).expect("encode");
        assert!(!json.contains('\n'), "{json}");
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;

    /// What the producer writes, the decision must be able to load.
    ///
    /// Two halves that only meet on disk: `write_for` names the file by
    /// content hash under `target/idealyst/<app>/overlay/`, and
    /// `load_archive` finds the newest one there. A mismatch in either
    /// spelling would make every save rebuild — quietly, because
    /// "no archive" is a perfectly ordinary reason to rebuild.
    #[test]
    fn the_watcher_writes_an_archive_the_decision_can_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"archive-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/app.rs"),
            "#[component]\nfn S() -> Element { ui! { text { \"x\" } } }\n",
        )
        .unwrap();

        assert!(
            load_archive(dir.path(), "archive-fixture").is_none(),
            "nothing to load before a build"
        );

        crate::archive::write_for(dir.path(), dir.path()).expect("write");
        let loaded = load_archive(dir.path(), "archive-fixture").expect("load");
        assert_eq!(loaded.package, "archive-fixture");
        assert_eq!(loaded.sites.len(), 1);
        assert_eq!(loaded, crate::archive::scan_crate(dir.path()).unwrap());
    }
}
