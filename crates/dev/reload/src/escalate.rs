//! Escalating an overlay patch the page could not show.
//!
//! The overlay tier is decided from SOURCE, and source cannot say whether
//! an edit will actually reach the screen. A primitive's literal has a
//! setter; a `#[component]`'s literal prop is shown live only when the
//! prop is `Reactive` and the component's body reads it in a binding
//! (`runtime_vocabulary::overlay::cells`). A `#[prop(static)]` prop, or a
//! body that bakes the value in while building, has nothing live to
//! write — and when its props are not `Clone` either, the page can only
//! stage the edit for that site's next render, which may never come.
//! That was the user's report: "static props don't hot reload at all".
//!
//! The page knows, and says so: its ack carries how many edits it
//! `refused`. This module turns that into the tier that CAN show them:
//!
//! 1. when a save decides as an overlay patch, the hot patch (or rebuild)
//!    that would carry the same save is decided too, against the build
//!    that is running — [`dev_overlay::Workspace::escalate`], BEFORE the
//!    archive advances — and kept as PREPARED;
//! 2. pushing the overlay patch makes it PENDING;
//! 3. an overlay ack with `refused > 0` TRIGGERS it: the watch loop is
//!    woken with the saved files as a synthetic file event, and the next
//!    decision is the pending one merged with whatever that batch holds
//!    ([`decide`]) — a hot patch when the tier is armed (the page remounts
//!    with its state carried), a rebuild when it is not or cannot be.
//!
//! A pending escalation is also folded into any hot patch or rebuild that
//! happens first (a hot patch re-emits from source, so it carries the
//! literal whether or not the ack has arrived yet), and dropped once a
//! hot patch or a rebuild has reached the page.

use std::path::PathBuf;

use crate::{ReloadSignal, WatchMsg};

/// The escalation state of a session. See the module docs.
#[derive(Default)]
pub(crate) struct Escalation {
    /// Decided for the overlay save being handled now; not yet pushed.
    prepared: Option<Pending>,
    /// Pushed overlay saves whose page acks may still refuse.
    pending: Vec<Pending>,
    /// A page refused an edit: the next decision carries `pending`.
    triggered: bool,
}

struct Pending {
    /// The saved files, as a watcher would report them.
    paths: Vec<PathBuf>,
    /// What carries them when the overlay cannot.
    decision: dev_overlay::WorkspaceDecision,
}

/// Decide a save, folding in an escalation that is owed.
///
/// What the watch loop's `handle_save` matches on in place of
/// `ws.decide`. Without an escalation in play it IS `ws.decide`.
pub(crate) fn decide(
    ws: &dev_overlay::Workspace,
    signal: &ReloadSignal,
    reporter: &dev_events::Reporter,
    saved: &[dev_overlay::SavedFile],
    premint: bool,
) -> dev_overlay::WorkspaceDecision {
    use dev_overlay::WorkspaceDecision as D;
    let decision = ws.decide(saved, premint);
    let mut esc = signal.escalation.lock().unwrap();
    esc.prepared = None;

    if esc.triggered {
        // The page refused an overlay edit: carry every pending save,
        // plus this batch's own edits (nothing, for the synthetic event
        // the ack sent — its files are already what the archive says).
        esc.triggered = false;
        let files: std::collections::BTreeSet<String> =
            esc.pending.iter().flat_map(|p| p.paths.iter()).map(|p| p.display().to_string()).collect();
        let files: Vec<String> = files.into_iter().collect();
        reporter.log(
            "dev-reload",
            format!(
                "the page could not show an overlay edit live; carrying it with a hot patch: {}",
                files.join(", ")
            ),
        );
        let owed = esc.pending.drain(..).fold(D::Unchanged, |a, p| ws.merge(a, p.decision));
        return ws.merge(owed, ws.escalate(saved, premint));
    }

    match &decision {
        // A hot patch or a rebuild re-emits from source: it carries what
        // is pending, whether or not the page has refused it yet. Kept
        // pending until one actually reaches the page (it may still be
        // superseded).
        D::HotPatch(_) | D::Rebuild(_) if !esc.pending.is_empty() => {
            let owed = esc
                .pending
                .iter()
                .fold(D::Unchanged, |a, p| ws.merge(a, p.decision.clone()));
            ws.merge(owed, decision)
        }
        D::Patch(_) => {
            esc.prepared = Some(Pending {
                paths: saved.iter().filter_map(|s| absolute(ws, s)).collect(),
                decision: ws.escalate(saved, premint),
            });
            decision
        }
        _ => decision,
    }
}

/// Where a saved file lives on disk, as the watcher names it.
fn absolute(ws: &dev_overlay::Workspace, saved: &dev_overlay::SavedFile) -> Option<PathBuf> {
    let dir = ws
        .crates
        .get(&saved.package)
        .map(|c| c.dir.clone())
        .or_else(|| ws.outside.get(&saved.package).cloned())?;
    Some(dir.join(&saved.file.path))
}

impl ReloadSignal {
    /// An overlay patch was pushed: what was prepared for it is now owed
    /// if the page refuses.
    pub(crate) fn escalation_pushed(&self) {
        let mut esc = self.escalation.lock().unwrap();
        if let Some(p) = esc.prepared.take() {
            esc.pending.push(p);
        }
    }

    /// A hot patch or a rebuild reached the page: it carried every pending
    /// save (see [`decide`]), so nothing is owed any more.
    pub(crate) fn escalation_settled(&self) {
        let mut esc = self.escalation.lock().unwrap();
        esc.pending.clear();
        esc.triggered = false;
    }

    /// A page acked an overlay patch with `refused` edits it could not
    /// show. Wakes the watch loop with the pending saves' files, once per
    /// escalation. Returns whether it did.
    pub(crate) fn escalate_refused(&self, refused: u64) -> bool {
        if refused == 0 {
            return false;
        }
        let paths: Vec<PathBuf> = {
            let mut esc = self.escalation.lock().unwrap();
            if esc.triggered || esc.pending.is_empty() {
                return false;
            }
            esc.triggered = true;
            let unique: std::collections::BTreeSet<PathBuf> =
                esc.pending.iter().flat_map(|p| p.paths.iter().cloned()).collect();
            unique.into_iter().collect()
        };
        let events = paths
            .into_iter()
            .map(|path| notify_debouncer_mini::DebouncedEvent {
                path,
                kind: notify_debouncer_mini::DebouncedEventKind::Any,
            })
            .collect();
        match self.rebuild.lock().unwrap().as_ref() {
            Some(tx) => tx.send(WatchMsg::Fs(Ok(events))).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{mpsc, Arc};

    use super::*;
    use crate::{handle_save, read_saved, Handled, PatchEvent, PatchFailure, PatchKind};

    /// A one-crate app whose screen passes a literal to a component prop.
    /// Whether that prop is `#[prop(static)]` is invisible here — it is
    /// declared in another crate — which is the whole problem.
    fn app(root: &Path, label: &str) -> dev_overlay::Workspace {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), screen(label)).unwrap();
        let meta = serde_json::json!({
            "packages": [
                {"id": "app", "name": "app", "manifest_path": root.join("Cargo.toml").display().to_string(),
                 "source": null, "targets": [{"kind": ["rlib"], "name": "app"}]}
            ],
            "workspace_members": ["app"],
            "resolve": {"root": "app", "nodes": [{"id": "app", "deps": []}]}
        });
        let mut ws = dev_overlay::Workspace::from_metadata(&meta, root).expect("the tip");
        ws.rescan_all(root);
        ws
    }

    fn screen(label: &str) -> String {
        format!(
            "use runtime_core::*;\n\n#[component]\nfn Root() -> Element {{\n    \
             ui! {{ view() {{ Fixed(label = \"{label}\") }} }}\n}}\n"
        )
    }

    fn built() -> std::result::Result<PatchEvent, PatchFailure> {
        Ok(PatchEvent {
            json: "{\"hot\":1}".into(),
            redirected: 1,
            steps: Vec::new(),
            crates: Vec::new(),
            skipped: Vec::new(),
            bytes: 1,
        })
    }

    fn kinds(signal: &ReloadSignal) -> Vec<PatchKind> {
        signal.patches_since(0).into_iter().map(|p| p.kind).collect()
    }

    /// Regression: "static props don't hot reload at all". A literal edit
    /// to a component prop decides as an overlay patch; when the page
    /// acks it with `refused` (a `#[prop(static)]` prop nothing
    /// re-renders), the save used to stop there, parked on "waiting for
    /// the next render" forever. Now the ack wakes the loop and the save
    /// goes out as the hot patch that carries it.
    #[test]
    fn regression_static_prop_literal_edit_is_never_parked_on_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut ws = app(&root, "before");
        let file = root.join("src/lib.rs");
        std::fs::write(&file, screen("after")).unwrap();

        let signal = ReloadSignal::new();
        let (tx, rx) = mpsc::channel();
        *signal.rebuild.lock().unwrap() = Some(tx);
        let reporter = dev_events::Reporter::new();
        let builds = std::cell::Cell::new(0);
        let mut build = |crates: &[build_web::hotpatch_build::PatchCrate]| {
            builds.set(builds.get() + 1);
            assert_eq!(crates.len(), 1, "the app crate is re-emitted");
            built()
        };

        // The save: an overlay patch, pushed.
        let saved = read_saved(&ws, &[file.clone()]);
        assert!(matches!(
            handle_save(&mut ws, &root, &saved, false, &signal, &reporter, &mut build, &|| false),
            Handled::Done
        ));
        assert_eq!(kinds(&signal), vec![PatchKind::Overlay]);
        assert_eq!(builds.get(), 0);

        // The page shows it: nothing more to do.
        assert!(signal.page_ack(r#"{"kind":"overlay","applied":1,"refused":0}"#));
        assert!(rx.try_recv().is_err(), "an applied overlay edit is not escalated");

        // The page could NOT show it: the loop is woken with the file.
        assert!(signal.page_ack(r#"{"kind":"overlay","applied":0,"refused":1}"#));
        let paths = match rx.try_recv() {
            Ok(WatchMsg::Fs(Ok(events))) => {
                events.into_iter().map(|e| e.path).collect::<Vec<_>>()
            }
            _ => panic!("the refusal must wake the watch loop"),
        };
        assert_eq!(paths, vec![file.clone()]);
        // A second refusal of the same escalation does not queue another.
        assert!(signal.page_ack(r#"{"kind":"overlay","applied":0,"refused":1}"#));
        assert!(rx.try_recv().is_err());

        // The woken loop re-reads the file (unchanged since the overlay
        // advanced the archive) and hot-patches it instead of deciding
        // "unchanged".
        let again = read_saved(&ws, &paths);
        assert!(matches!(
            handle_save(&mut ws, &root, &again, false, &signal, &reporter, &mut build, &|| false),
            Handled::Done
        ));
        assert_eq!(builds.get(), 1, "the refused edit was hot-patched");
        assert_eq!(kinds(&signal), vec![PatchKind::Overlay, PatchKind::Hot]);

        // Settled: a later overlay refusal with nothing pending is a no-op.
        assert!(!signal.escalate_refused(1));
    }

    /// With the hot-patch tier unavailable (unarmed, or the patch fails)
    /// the escalation rebuilds — still never parked.
    #[test]
    fn an_escalation_rebuilds_when_the_hot_patch_cannot_be_built() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut ws = app(&root, "before");
        let file = root.join("src/lib.rs");
        std::fs::write(&file, screen("after")).unwrap();
        let signal = ReloadSignal::new();
        let (tx, _rx) = mpsc::channel();
        *signal.rebuild.lock().unwrap() = Some(tx);
        let reporter = dev_events::Reporter::new();
        let mut unarmed = |_: &[build_web::hotpatch_build::PatchCrate]| {
            Err(PatchFailure::Failed("the hot-patch tier is not armed".into()))
        };

        let saved = read_saved(&ws, &[file.clone()]);
        handle_save(&mut ws, &root, &saved, false, &signal, &reporter, &mut unarmed, &|| false);
        assert!(signal.escalate_refused(1));
        let again = read_saved(&ws, &[file]);
        assert!(matches!(
            handle_save(&mut ws, &root, &again, false, &signal, &reporter, &mut unarmed, &|| false),
            Handled::Rebuild
        ));
    }

    /// A hot patch decided before the page's ack arrives carries the
    /// pending overlay save too: it re-emits from source.
    #[test]
    fn a_hot_patch_carries_a_pending_overlay_save() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut ws = app(&root, "before");
        let file = root.join("src/lib.rs");
        std::fs::write(&file, screen("after")).unwrap();
        let signal = Arc::new(ReloadSignal::default());
        let reporter = dev_events::Reporter::new();
        let mut build = |_: &[build_web::hotpatch_build::PatchCrate]| built();
        let saved = read_saved(&ws, &[file.clone()]);
        handle_save(&mut ws, &root, &saved, false, &signal, &reporter, &mut build, &|| false);
        assert_eq!(signal.escalation.lock().unwrap().pending.len(), 1);

        // A body edit before the ack: one hot patch, and nothing owed after.
        let body = screen("after").replace("fn Root() -> Element {\n", "fn Root() -> Element {\n    let _x = 1;\n");
        std::fs::write(&file, body).unwrap();
        let saved = read_saved(&ws, &[file]);
        assert!(matches!(
            handle_save(&mut ws, &root, &saved, false, &signal, &reporter, &mut build, &|| false),
            Handled::Done
        ));
        assert!(signal.escalation.lock().unwrap().pending.is_empty());
        assert!(!signal.escalate_refused(1), "the hot patch settled it");
    }
}
