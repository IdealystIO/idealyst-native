//! [`FileDropZone`] — accept OS files **dropped** onto a view.
//!
//! This is the drag-*in* counterpart to [`FilePicker::pick`](crate::FilePicker::pick):
//! instead of the app popping an open-panel, the user drags files from *outside*
//! the app (Finder, the desktop, another browser tab) and releases them over a
//! view. A dropped file surfaces as the same lazy [`PickedFile`](crate::PickedFile)
//! handle a picker returns, so upload code is identical however the file arrived.
//!
//! It builds on the framework's OS file-drop channel
//! ([`runtime_core::file_drop`]): make any view a drop target by handing it the
//! zone's [`handler`](FileDropZone::handler), and style it against the reactive
//! [`active`](FileDropZone::active) signal that goes `true` while a file drag
//! hovers.
//!
//! ```no_run
//! # use file_picker::{FileDropZone, PickedFile};
//! let dz = FileDropZone::new().on_drop(|files: Vec<PickedFile>| {
//!     for f in files {
//!         // stream `f` to your uploader — identical to a picked file
//!         let _ = f.name();
//!     }
//! });
//! let active = dz.active();
//! # let _ = active;
//! // ui! { view().on_file_drop(dz.handler()) { text("Drop files here") } }
//! ```
//!
//! Delivered on **web**, **macOS**, and **Linux** (GTK4) — the desktop-class
//! backends. On iOS / Android (no OS file-drag) and the scaffold Windows
//! backend the `on_file_drop` view slot is a no-op, so the handler simply
//! never fires — the zone renders inertly, exactly like a wheel handler on a
//! phone.

use std::rc::Rc;

use runtime_core::{signal, FileDropEvent, FileDropPhase, Signal, TouchResponse};

use crate::PickedFile;

/// A drop target for OS files. Cheap to construct and clone (it's a handle over
/// a reactive signal + the callbacks). Attach it to a view with
/// [`handler`](Self::handler) and style the view against
/// [`active`](Self::active).
#[derive(Clone)]
pub struct FileDropZone {
    /// `true` while a file drag is hovering the zone; back to `false` on leave
    /// or drop. Read it inside a reactive scope to restyle the drop target.
    active: Signal<bool>,
    on_enter: Option<Rc<dyn Fn()>>,
    on_leave: Option<Rc<dyn Fn()>>,
    on_drop: Option<Rc<dyn Fn(Vec<PickedFile>)>>,
}

impl Default for FileDropZone {
    fn default() -> Self {
        Self::new()
    }
}

impl FileDropZone {
    /// Create an empty drop zone. Chain [`on_drop`](Self::on_drop) (and
    /// optionally [`on_enter`](Self::on_enter) / [`on_leave`](Self::on_leave))
    /// to react to drops.
    ///
    /// Creates the `active` signal, so it must run where signal creation is
    /// legal — a component body or any world-entered build scope, not an
    /// event handler. The returned handler itself IS handler-safe.
    pub fn new() -> Self {
        Self {
            active: signal(false),
            on_enter: None,
            on_leave: None,
            on_drop: None,
        }
    }

    /// Fire when a file drag enters the zone (in addition to flipping
    /// [`active`](Self::active) to `true`).
    pub fn on_enter(mut self, f: impl Fn() + 'static) -> Self {
        self.on_enter = Some(Rc::new(f));
        self
    }

    /// Fire when a file drag leaves without dropping (in addition to flipping
    /// [`active`](Self::active) back to `false`).
    pub fn on_leave(mut self, f: impl Fn() + 'static) -> Self {
        self.on_leave = Some(Rc::new(f));
        self
    }

    /// Fire when files are dropped on the zone, with each as a lazy, streamable
    /// [`PickedFile`] — the same handle [`FilePicker::pick`](crate::FilePicker::pick)
    /// yields.
    pub fn on_drop(mut self, f: impl Fn(Vec<PickedFile>) + 'static) -> Self {
        self.on_drop = Some(Rc::new(f));
        self
    }

    /// The reactive drag-active signal: `true` while a file drag hovers the
    /// zone. Read it inside a reactive style scope to highlight the target.
    pub fn active(&self) -> Signal<bool> {
        self.active.clone()
    }

    /// The handler to hand a view via `view().on_file_drop(dz.handler())`.
    /// Accepts the drag (so the browser doesn't navigate to the file / macOS
    /// shows the copy cursor), tracks [`active`](Self::active), and converts
    /// dropped OS files into [`PickedFile`]s for [`on_drop`](Self::on_drop).
    pub fn handler(&self) -> impl Fn(&FileDropEvent) -> TouchResponse + 'static {
        let active = self.active.clone();
        let on_enter = self.on_enter.clone();
        let on_leave = self.on_leave.clone();
        let on_drop = self.on_drop.clone();
        move |ev: &FileDropEvent| match &ev.phase {
            FileDropPhase::Entered => {
                // `set` is equality-guarded at commit, so a redundant write is
                // already a no-op — no read-back guard needed (and a read-back
                // could not see a same-turn staged write anyway).
                active.set(true);
                if let Some(cb) = &on_enter {
                    cb();
                }
                // Accept the drag — web `preventDefault`, macOS copy operation.
                TouchResponse::CONSUMED
            }
            FileDropPhase::Exited => {
                active.set(false);
                if let Some(cb) = &on_leave {
                    cb();
                }
                TouchResponse::IGNORED
            }
            FileDropPhase::Dropped(files) => {
                active.set(false);
                if let Some(cb) = &on_drop {
                    let picked: Vec<PickedFile> = files
                        .iter()
                        .filter_map(|f| crate::imp::picked_from_dropped(f).map(PickedFile::from_inner))
                        .collect();
                    cb(picked);
                }
                TouchResponse::CONSUMED
            }
            // `FileDropPhase` is `#[non_exhaustive]`; a future phase we don't
            // yet handle neither accepts the drag nor fires a callback.
            _ => TouchResponse::IGNORED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{DroppedFile, TouchPoint};
    use std::cell::RefCell;

    /// The zone mints a signal (world required) and its writes STAGE, so a
    /// read-back needs the commit the backend's driver would perform after the
    /// drop event returns.
    fn world<R>(f: impl FnOnce() -> R) -> R {
        runtime_core::__with_fresh_world(f)
    }

    fn flush() {
        runtime_core::__flush_test_world();
    }

    fn ev(phase: FileDropPhase) -> FileDropEvent {
        FileDropEvent {
            phase,
            position: TouchPoint::new(0.0, 0.0),
        }
    }

    #[test]
    fn active_toggles_on_enter_and_exit() {
        world(|| {
            let dz = FileDropZone::new();
            let active = dz.active();
            let h = dz.handler();
            assert!(!active.get());

            let resp = h(&ev(FileDropPhase::Entered));
            assert!(resp.consumed, "Entered is accepted (so the drag isn't rejected)");
            flush();
            assert!(active.get(), "active flips true while a drag hovers");

            h(&ev(FileDropPhase::Exited));
            flush();
            assert!(!active.get(), "active flips back false on leave");
        });
    }

    #[test]
    fn on_drop_receives_picked_files_and_resets_active() {
        // A real temp file so the path-based host backend can build a PickedFile
        // from the dropped path.
        let path = std::env::temp_dir().join("file_drop_zone_unit_test.txt");
        std::fs::write(&path, b"hello").unwrap();

        world(|| {
            let names: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
            let names_c = names.clone();
            let dz = FileDropZone::new().on_drop(move |files| {
                names_c
                    .borrow_mut()
                    .extend(files.iter().map(|f| f.name().to_string()));
            });
            let active = dz.active();
            let h = dz.handler();

            h(&ev(FileDropPhase::Entered));
            flush();
            assert!(active.get());

            h(&ev(FileDropPhase::Dropped(vec![DroppedFile {
                name: "file_drop_zone_unit_test.txt".to_string(),
                mime: "text/plain".to_string(),
                size: Some(5),
                path: Some(path.clone()),
                source: None,
            }])));
            flush();

            assert!(!active.get(), "active resets after a drop");
            assert_eq!(
                names.borrow().as_slice(),
                &["file_drop_zone_unit_test.txt".to_string()],
                "the dropped file surfaces as a PickedFile with the right name"
            );
        });

        let _ = std::fs::remove_file(&path);
    }
}
