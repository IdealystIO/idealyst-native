//! OS file drag-and-drop onto the host window: the OLE `IDropTarget` that
//! feeds `on_file_drop` views (the Windows counterpart of macOS's
//! `NSDraggingDestination`, GTK's `DropTarget` and web's `drop` events).
//!
//! Windows delivers a drag to a WINDOW, not to a view, so one target is
//! registered on the host window (lazily, when the first `on_file_drop`
//! handler is installed) and every callback is routed to views along the
//! hit chain by `crate::view_input` (`drag_moved` / `drag_left` /
//! `dropped`). This module only adapts OLE to that:
//!
//! | `IDropTarget`          | routing                                |
//! |------------------------|----------------------------------------|
//! | `DragEnter`/`DragOver` | `drag_moved` → copy effect if accepted |
//! | `DragLeave`            | `drag_left`                            |
//! | `Drop`                 | `dropped` with the `CF_HDROP` paths    |
//!
//! Only drags that carry files (`CF_HDROP`: Explorer, the desktop, most
//! file managers) are offered to views; a text or image drag gets the
//! no-drop cursor and no events. Dropped files carry real paths, like
//! macOS and Linux, so the file-picker SDK streams them with `std::fs`.
//!
//! OLE calls these methods on the UI thread, from the message loop, while
//! the source process runs its drag loop — never under a backend borrow
//! of ours, but `view_input` still uses `try_borrow` throughout.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Weak;

use windows::core::{implement, Result as WinResult};
use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, CF_HDROP,
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use runtime_shared::DroppedFile;

use crate::{view_input, WindowsBackend};

/// The window's drop target. Holds the backend weakly: the window (and
/// its OLE registration) outlives nothing that matters, and a strong
/// reference would be a cycle through the backend's own tables.
#[implement(IDropTarget)]
pub(crate) struct DropTarget {
    backend: Weak<RefCell<WindowsBackend>>,
    hwnd: HWND,
    /// Whether the drag in progress carries files; decided once on enter.
    has_files: Cell<bool>,
}

impl DropTarget {
    pub(crate) fn new(backend: Weak<RefCell<WindowsBackend>>, hwnd: HWND) -> Self {
        DropTarget { backend, hwnd, has_files: Cell::new(false) }
    }

    /// Screen point → client device pixels, as the mouse messages carry
    /// them.
    fn client_point(&self, pt: &POINTL) -> (f32, f32) {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        (p.x as f32, p.y as f32)
    }

    fn moved(&self, pt: &POINTL, effect: *mut DROPEFFECT) {
        let accepted = self.has_files.get()
            && match self.backend.upgrade() {
                Some(b) => {
                    let (x, y) = self.client_point(pt);
                    view_input::drag_moved(&b, x, y)
                }
                None => false,
            };
        set_effect(effect, accepted);
    }
}

/// Answers with a copy when accepted and the source allows copying —
/// `effect` comes in holding the source's allowed effects.
fn set_effect(effect: *mut DROPEFFECT, accepted: bool) {
    if effect.is_null() {
        return;
    }
    unsafe {
        let allowed = *effect;
        *effect = if accepted && (allowed.0 & DROPEFFECT_COPY.0) != 0 { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Option<&IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        self.has_files.set(data.is_some_and(offers_files));
        self.moved(pt, effect);
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> WinResult<()> {
        self.moved(pt, effect);
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        if let Some(b) = self.backend.upgrade() {
            view_input::drag_left(&b);
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Option<&IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let Some(b) = self.backend.upgrade() else {
            set_effect(effect, false);
            return Ok(());
        };
        let files = data.map(dropped_files).unwrap_or_default();
        let (x, y) = self.client_point(pt);
        let ok = view_input::dropped(&b, x, y, files);
        set_effect(effect, ok);
        Ok(())
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// Whether the drag carries at least one file. Opens the `CF_HDROP` list
/// and counts it: `QueryGetData` is not enough, because a shell data object
/// answers yes for `CF_HDROP` even when it holds no items. Counting reads
/// only the path list (no file is stat'ed), so it stays cheap for a large
/// selection.
fn offers_files(data: &IDataObject) -> bool {
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else {
            return false;
        };
        let count = DragQueryFileW(HDROP(medium.u.hGlobal.0), u32::MAX, None);
        ReleaseStgMedium(&mut medium);
        count > 0
    }
}

/// The files a drag carries, as `DroppedFile`s. Empty when it carries no
/// `CF_HDROP`.
fn dropped_files(data: &IDataObject) -> Vec<DroppedFile> {
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else {
            return Vec::new();
        };
        let paths = hdrop_paths(HDROP(medium.u.hGlobal.0));
        ReleaseStgMedium(&mut medium);
        paths.iter().map(|p| dropped_file(p)).collect()
    }
}

/// The paths in a `CF_HDROP` block.
///
/// # Safety
/// `hdrop` must be a valid `CF_HDROP` global.
pub(crate) unsafe fn hdrop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    let count = DragQueryFileW(hdrop, u32::MAX, None);
    (0..count)
        .filter_map(|i| {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..got])))
        })
        .collect()
}

fn dropped_file(path: &Path) -> DroppedFile {
    DroppedFile {
        name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        mime: mime_for_path(path).to_string(),
        size: std::fs::metadata(path).ok().map(|m| m.len()),
        path: Some(path.to_path_buf()),
        source: None,
    }
}

/// Best-effort MIME from the extension. The SAME table the macOS
/// (`imp/view.rs`) and Linux (`file_drop.rs`) backends use, so a dropped
/// `.png` reports `image/png` everywhere; name-only, since the file may be
/// many GB.
fn mime_for_path(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("heic") => "image/heic",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("html") | Some("htm") => "text/html",
        Some("zip") => "application/zip",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Registers the host window's drop target. OLE must be initialized
/// (single-threaded apartment) on the UI thread first; `OleInitialize` is
/// reference counted and returns `S_FALSE` when COM already is. Fails when
/// the thread joined the multithreaded apartment, or the window is already
/// registered.
pub(crate) fn register(hwnd: HWND, backend: Weak<RefCell<WindowsBackend>>) -> WinResult<IDropTarget> {
    unsafe {
        OleInitialize(None)?;
        let target: IDropTarget = DropTarget::new(backend, hwnd).into();
        RegisterDragDrop(hwnd, &target)?;
        Ok(target)
    }
}

/// Through the real COM interface, with a shell data object built from real
/// files — the same kind of object Explorer hands a drop target — so the
/// `CF_HDROP` read, the effect negotiation and the routing are all
/// exercised. Only the OS drag loop itself (a source process calling us)
/// is not; it can't be driven without synthesizing a mouse drag.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeKind, NodeMeta, ViewVisual};
    use runtime_shared::{FileDropEvent, FileDropPhase, TouchResponse};
    use std::rc::Rc;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Ole::DROPEFFECT_MOVE;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        ILFree, IShellItemArray, SHCreateShellItemArrayFromIDLists, SHParseDisplayName, BHID_DataObject,
    };

    fn data_object_for(paths: &[PathBuf]) -> IDataObject {
        unsafe {
            OleInitialize(None).unwrap();
            let pidls: Vec<*mut ITEMIDLIST> = paths
                .iter()
                .map(|p| {
                    let mut pidl = std::ptr::null_mut();
                    SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None).unwrap();
                    pidl
                })
                .collect();
            let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
            let items: IShellItemArray = SHCreateShellItemArrayFromIDLists(&consts).unwrap();
            for p in pidls {
                ILFree(Some(p));
            }
            items.BindToHandler(None, &BHID_DataObject).unwrap()
        }
    }

    fn backend_with_zone(log: Rc<RefCell<Vec<String>>>) -> Rc<RefCell<WindowsBackend>> {
        let mut b = WindowsBackend::new(HWND(std::ptr::null_mut()));
        let mut root = NodeMeta::new(NodeKind::View(ViewVisual::default()));
        root.frame = (0.0, 0.0, 800.0, 600.0);
        b.nodes.insert(1, root);
        b.root_id = Some(1);
        b.view_input.file_drop.insert(
            1,
            Rc::new(move |ev: &FileDropEvent| {
                let entry = match &ev.phase {
                    FileDropPhase::Dropped(files) => files
                        .iter()
                        .map(|f| format!("{}|{}|{:?}|{}", f.name, f.mime, f.size, f.path.is_some()))
                        .collect::<Vec<_>>()
                        .join(" "),
                    FileDropPhase::Entered => "entered".into(),
                    FileDropPhase::Exited => "exited".into(),
                    _ => "?".into(),
                };
                log.borrow_mut().push(entry);
                TouchResponse::CONSUMED
            }),
        );
        Rc::new(RefCell::new(b))
    }

    #[test]
    fn explorer_style_drop_delivers_paths_names_sizes_and_mime() {
        if crate::under_wine() {
            // Wine's shell data objects don't render CF_HDROP for item arrays.
            return;
        }
        let dir = std::env::temp_dir().join(format!("idealyst-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("notes.txt");
        let b = dir.join("photo.PNG");
        std::fs::write(&a, b"hello").unwrap();
        std::fs::write(&b, [0u8; 12]).unwrap();
        let data = data_object_for(&[a.clone(), b.clone()]);

        let log: Rc<RefCell<Vec<String>>> = Default::default();
        let backend = backend_with_zone(log.clone());
        // A null HWND: ScreenToClient leaves the point as given.
        let target: IDropTarget = DropTarget::new(Rc::downgrade(&backend), HWND(std::ptr::null_mut())).into();
        let pt = POINTL { x: 40, y: 30 };
        unsafe {
            let mut effect = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0);
            target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect).unwrap();
            assert_eq!(effect, DROPEFFECT_COPY, "an accepted file drag answers copy");
            let mut effect = DROPEFFECT_COPY;
            target.Drop(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect).unwrap();
            assert_eq!(effect, DROPEFFECT_COPY);
        }
        assert_eq!(
            *log.borrow(),
            ["entered", "notes.txt|text/plain|Some(5)|true photo.PNG|image/png|Some(12)|true"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A drag that carries no files gets the no-drop cursor and no events
    /// — including a shell data object that claims `CF_HDROP` but holds no
    /// items (an earlier `QueryGetData` check accepted it).
    #[test]
    fn a_drag_without_files_is_refused() {
        if crate::under_wine() {
            return;
        }
        let log: Rc<RefCell<Vec<String>>> = Default::default();
        let backend = backend_with_zone(log.clone());
        let target: IDropTarget = DropTarget::new(Rc::downgrade(&backend), HWND(std::ptr::null_mut())).into();
        unsafe {
            OleInitialize(None).unwrap();
            let empty: IDataObject = windows::Win32::UI::Shell::SHCreateDataObject(None, None, None).unwrap();
            let mut effect = DROPEFFECT_COPY;
            target.DragEnter(&empty, MODIFIERKEYS_FLAGS(0), POINTL { x: 40, y: 30 }, &mut effect).unwrap();
            assert_eq!(effect, DROPEFFECT_NONE);
        }
        assert!(log.borrow().is_empty());
    }

    /// The window registration itself: OLE initializes on the UI thread and
    /// accepts the target once per window.
    #[test]
    fn register_takes_the_window_once() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            assert!(register(hwnd, Weak::new()).is_ok());
            assert!(register(hwnd, Weak::new()).is_err(), "a second target for the same window is refused");
            let _ = windows::Win32::System::Ole::RevokeDragDrop(hwnd);
            let _ = DestroyWindow(hwnd);
        }
    }
}
