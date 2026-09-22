//! The dev loop's decision: patch this save, or rebuild it?
//!
//! On a save the watcher has the new source. The build it is watching
//! left a descriptor set behind (see [`super::overlay`]). Together those
//! are enough to answer the question without starting a compiler — and
//! starting one is the thing worth avoiding, since a rebuild of a real
//! app is seconds and a patch is milliseconds.
//!
//! # The decision is conservative on purpose
//!
//! A save is patchable only when EVERY one of these holds:
//!
//! 1. every changed file still parses;
//! 2. its [skeleton](runtime_macros_parse::file_skeleton) — the file
//!    with `ui!` bodies blanked — is byte-for-byte unchanged, so nothing
//!    outside a site moved;
//! 3. the file's `ui!` sites are the same sites, at the same positions;
//! 4. every site whose descriptor changed diffs to a valid [`Patch`];
//! 5. the archive is the one this build produced.
//!
//! Anything else rebuilds. A file with both a patchable `ui!` edit and a
//! logic edit rebuilds, because (2) fails — and that is the case worth
//! being careful about: patching the label while skipping the compiler
//! that would have picked up the logic change is how a dev loop starts
//! lying about what is running.
//!
//! The asymmetry is deliberate. A wrong REBUILD costs seconds. A wrong
//! PATCH means the screen and the source disagree with nothing to say
//! so, and every subsequent edit is reasoned about against a program
//! that is not there.

use std::collections::BTreeMap;
use std::path::Path;

use runtime_template::{diff, Edit, Rejection};

use crate::overlay::DescriptorSet;

/// What to do with a save.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Nothing changed that matters. No patch, no rebuild.
    Unchanged,
    /// Apply these patches; skip the rebuild.
    Patch(Vec<SitePatch>),
    /// Rebuild, for this reason.
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
    /// A `ui!` site appeared, vanished or moved. Positions are part of
    /// site identity, so an inserted line above a site lands here.
    SitesMoved { file: String },
    /// A site's edit cannot be expressed as a patch.
    Refused { file: String, why: Rejection },
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::NoArchive => write!(f, "no descriptor set from this build"),
            Reason::UnknownFile { file } => write!(f, "{file} is not in this build"),
            Reason::DoesNotParse { file } => write!(f, "{file} does not parse"),
            Reason::CodeChanged { file } => write!(f, "{file} changed outside its `ui!` bodies"),
            Reason::SitesMoved { file } => write!(f, "{file}'s `ui!` sites moved"),
            Reason::Refused { file, why } => write!(f, "{file}: {why}"),
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
    if archive.overlay_version != crate::overlay::OVERLAY_VERSION {
        return Decision::Rebuild(Reason::NoArchive);
    }

    // The archive's sites by file, in the ordinal order the producer
    // recorded — the same walk the on-save scan does.
    let mut by_file: BTreeMap<&str, Vec<&crate::overlay::ArchivedSite>> = BTreeMap::new();
    for site in &archive.sites {
        by_file.entry(site.descriptor.site.file.as_ref()).or_default().push(site);
    }
    for sites in by_file.values_mut() {
        sites.sort_by_key(|s| s.ordinal);
    }

    let mut patches = Vec::new();
    for file in changed {
        let Some(recorded) = archive.files.get(&file.path) else {
            return Decision::Rebuild(Reason::UnknownFile { file: file.path.clone() });
        };
        if recorded.content == crate::overlay::digest(file.text.as_bytes()) {
            continue;
        }

        let sites = match runtime_macros_parse::sites_in_file(
            &archive.package,
            &file.path,
            &file.text,
        ) {
            Ok(s) => s,
            Err(_) => {
                return Decision::Rebuild(Reason::DoesNotParse { file: file.path.clone() })
            }
        };
        let skeleton =
            crate::overlay::digest(runtime_macros_parse::skeleton_of(&file.text, &sites).as_bytes());
        if skeleton != recorded.skeleton {
            return Decision::Rebuild(Reason::CodeChanged { file: file.path.clone() });
        }

        // Match by ORDINAL, not by site key. A body gaining a line moves
        // every site below it, which re-keys them — but the running
        // binary still carries the OLD keys, and the file still has the
        // same sites in the same order. Matching by position lets those
        // saves patch; keying by position alone made them all rebuild.
        //
        // The ordinal set changing means a site was added or removed,
        // and a new site has no compiled tag to address at all.
        let archived = by_file.get(file.path.as_str()).cloned().unwrap_or_default();
        if archived.len() != sites.len() {
            return Decision::Rebuild(Reason::SitesMoved { file: file.path.clone() });
        }
        for (before, mut site) in archived.iter().zip(sites) {
            // A site this build could not describe, or one this save
            // cannot parse, has nothing to diff.
            let Some(ui) = site.ui.as_mut() else {
                return Decision::Rebuild(Reason::DoesNotParse { file: file.path.clone() });
            };
            if before.descriptor.nodes.is_empty() && before.descriptor.roots.is_empty() {
                return Decision::Rebuild(Reason::DoesNotParse { file: file.path.clone() });
            }
            // Describe under the ARCHIVED site id, so the diff compares
            // two versions of one site rather than refusing them as two
            // different sites — and so the patch is addressed to the key
            // the compiled code carries.
            let after = match runtime_macros_parse::describe(before.descriptor.site.clone(), ui) {
                Ok(d) => d,
                // The two numbering walks disagreed — a bug in this
                // crate's own pass, not in the author's code. Rebuild:
                // it is the answer that is always right.
                Err(_) => {
                    return Decision::Rebuild(Reason::SitesMoved { file: file.path.clone() })
                }
            };
            match diff(&before.descriptor, &after) {
                Ok(patch) if patch.edits.is_empty() => {}
                Ok(patch) => patches.push(SitePatch {
                    site: before.key,
                    file: file.path.clone(),
                    edits: patch.edits.into_owned(),
                }),
                Err(why) => {
                    return Decision::Rebuild(Reason::Refused { file: file.path.clone(), why })
                }
            }
        }
    }

    if patches.is_empty() {
        Decision::Unchanged
    } else {
        Decision::Patch(patches)
    }
}

/// Fold a decided patch back into the archive, so the NEXT save diffs
/// against what is actually running.
///
/// Without this, a second edit to the same literal would diff against
/// the original and re-send an edit that is already applied — harmless
/// once, wrong the moment the two edits are not the same shape. A
/// rebuild regenerates the archive from source instead, which is also
/// what drops every staged patch.
pub fn advance_archive(archive: &mut DescriptorSet, changed: &[ChangedFile]) {
    for file in changed {
        let Ok(sites) =
            runtime_macros_parse::sites_in_file(&archive.package, &file.path, &file.text)
        else {
            continue;
        };
        let skeleton =
            crate::overlay::digest(runtime_macros_parse::skeleton_of(&file.text, &sites).as_bytes());
        archive.files.insert(
            file.path.clone(),
            crate::overlay::FileDigest {
                content: crate::overlay::digest(file.text.as_bytes()),
                skeleton,
            },
        );
        for (ordinal, mut site) in sites.into_iter().enumerate() {
            let Some(ui) = site.ui.as_mut() else { continue };
            let Some(slot) = archive
                .sites
                .iter_mut()
                .find(|s| s.descriptor.site.file == file.path && s.ordinal == ordinal as u32)
            else {
                continue;
            };
            // Describe under the archived site id and keep `key`
            // untouched. The DESCRIPTOR moves forward so the next save
            // diffs against what is running; the KEY must not, because
            // it is what the compiled binary's tags say and no patch
            // changes that. Only a rebuild does.
            let Ok(after) = runtime_macros_parse::describe(slot.descriptor.site.clone(), ui) else {
                continue;
            };
            slot.descriptor = after;
        }
    }
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
    let dir = crate::overlay::overlay_dir(project_root, app);
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
        let set = crate::overlay::scan_crate(dir.path()).expect("scan");
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

    #[test]
    fn a_logic_edit_rebuilds() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace("    1\n", "    2\n");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::CodeChanged { file: "src/app.rs".into() })
        );
    }

    /// The case the whole conservative design is for: one save carrying
    /// both. Patching the label and skipping the compiler that would
    /// have picked up the logic change is how a dev loop starts lying
    /// about what is running.
    #[test]
    fn a_save_with_both_a_ui_edit_and_a_logic_edit_rebuilds() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace(r#""hello""#, r#""goodbye""#).replace("    1\n", "    2\n");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::CodeChanged { file: "src/app.rs".into() })
        );
    }

    /// An edit a patch cannot express — here a literal becoming a
    /// closure — is refused by the differ and falls to a rebuild, with
    /// the differ's own reason carried along so the log can say why.
    #[test]
    fn an_edit_the_differ_refuses_rebuilds_with_its_reason() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace(
            r#"text { "hello" }"#,
            r#"text { move || format!("{}", count()) }"#,
        );
        match decide(Some(&archive), &changed(&edited)) {
            Decision::Rebuild(Reason::Refused { file, why }) => {
                assert_eq!(file, "src/app.rs");
                assert!(!why.to_string().is_empty(), "a refusal must say why");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
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
    /// another site's body, which is where nested sites live.
    #[test]
    fn removing_a_nested_site_rebuilds_on_the_ordinal_check() {
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
            Decision::Rebuild(Reason::SitesMoved { file: "src/app.rs".into() }),
            "one site fewer, and the one that vanished had a compiled tag"
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

    /// An inserted line ABOVE everything is caught one step earlier, by
    /// the skeleton — it is code that moved, whatever it does.
    #[test]
    fn a_line_inserted_outside_a_body_rebuilds_as_a_code_change() {
        let (_d, archive) = archive_of(APP);
        let edited = APP.replace("#[component]", "// a new line\n#[component]");
        assert_eq!(
            decide(Some(&archive), &changed(&edited)),
            Decision::Rebuild(Reason::CodeChanged { file: "src/app.rs".into() })
        );
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

        crate::overlay::write_for(dir.path(), dir.path()).expect("write");
        let loaded = load_archive(dir.path(), "archive-fixture").expect("load");
        assert_eq!(loaded.package, "archive-fixture");
        assert_eq!(loaded.sites.len(), 1);
        assert_eq!(loaded, crate::overlay::scan_crate(dir.path()).unwrap());
    }
}
