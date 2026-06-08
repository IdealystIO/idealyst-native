//! Apple open UI: `NSOpenPanel` on macOS, `UIDocumentPickerViewController`
//! (open) + `PHPickerViewController` on iOS. Driven through the Obj-C runtime,
//! the same posture `file-export`/`camera` take.
//!
//! macOS is **modal and synchronous** (`runModal`). iOS is **delegate-driven**:
//! we present the picker and bridge its callbacks to a oneshot, mirroring
//! `file-export`'s iOS export flow.
//!
//! Every picked file is exposed as a real on-disk path (so reads stream via the
//! shared [`fsread`](crate::fsread) reader — no buffering):
//!
//! - macOS / iOS documents: the chosen file URL. On iOS that URL is
//!   **security-scoped** — reads MUST happen between
//!   `startAccessingSecurityScopedResource` and `stop…`, or an iCloud-backed
//!   pick reads empty/EACCES. We hold the scope open for the `PickedFile`'s
//!   whole lifetime (RAII guard below) so `open`/`copy_to` "just work".
//! - iOS media (PHPicker): each asset is copied to a temp file in the block
//!   handed by `loadFileRepresentation` (a disk→disk copy, never into memory),
//!   then handed back as a plain owned path.
//!
//! VERIFICATION: macOS `NSOpenPanel` is exercised on the dev host via
//! `examples/file-picker-demo`. The iOS document-picker / PHPicker paths are
//! compile-checked for `aarch64-apple-ios` and resolve only at runtime on a
//! device (same posture `file-export`'s iOS path documents).

use std::path::{Path, PathBuf};

use crate::{PickError, PickKind, PickRequest};

// All Apple backends read a picked file from a real path.
pub(crate) use crate::fsread::FileStream;
use crate::fsread::file_meta;

/// A file the user picked on an Apple platform: a real on-disk path plus
/// metadata, and — on iOS documents — the security-scope guard that keeps the
/// path readable.
pub(crate) struct PickedFile {
    name: String,
    mime: String,
    size: Option<u64>,
    path: PathBuf,
    // Held to keep an iOS security-scoped URL accessible for this file's
    // lifetime; `None` on macOS and for iOS media temp copies (plain paths).
    #[cfg(target_os = "ios")]
    _scope: Option<ios::SecurityScope>,
}

impl PickedFile {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn mime(&self) -> &str {
        &self.mime
    }
    pub(crate) fn size(&self) -> Option<u64> {
        self.size
    }
    pub(crate) fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }
    pub(crate) async fn open(&self) -> Result<FileStream, PickError> {
        FileStream::open(&self.path)
    }
}

#[cfg(target_os = "macos")]
pub(crate) async fn pick(request: &PickRequest) -> Result<Option<Vec<PickedFile>>, PickError> {
    macos::pick(request)
}

#[cfg(target_os = "ios")]
pub(crate) async fn pick(request: &PickRequest) -> Result<Option<Vec<PickedFile>>, PickError> {
    match &request.kind {
        PickKind::Documents(_) => ios::pick_documents(request).await,
        PickKind::Media(_) => ios::pick_media(request).await,
    }
}

// ---------------------------------------------------------------------------
// macOS — NSOpenPanel.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_foundation::NSString;

    use super::*;

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}
    // `UTType` (allowedContentTypes filtering) lives in UniformTypeIdentifiers.
    #[link(name = "UniformTypeIdentifiers", kind = "framework")]
    extern "C" {}

    // NSModalResponseOK.
    const MODAL_RESPONSE_OK: isize = 1;

    pub(super) fn pick(request: &PickRequest) -> Result<Option<Vec<PickedFile>>, PickError> {
        // SAFETY: a straight transcription of the documented NSOpenPanel flow
        // (openPanel → configure → runModal → read URLs). runModal is app-modal
        // and must run on the main thread, where author code (this fn) runs.
        unsafe {
            let panel: Retained<AnyObject> = msg_send_id![class!(NSOpenPanel), openPanel];
            let _: () = msg_send![&*panel, setCanChooseFiles: true];
            let _: () = msg_send![&*panel, setCanChooseDirectories: false];
            let _: () = msg_send![&*panel, setAllowsMultipleSelection: request.allow_multiple];

            if let Some(types) = allowed_content_types(request) {
                let _: () = msg_send![&*panel, setAllowedContentTypes: &*types];
            }

            let response: isize = msg_send![&*panel, runModal];
            if response != MODAL_RESPONSE_OK {
                return Ok(None);
            }

            let urls: *mut AnyObject = msg_send![&*panel, URLs];
            if urls.is_null() {
                return Ok(Some(Vec::new()));
            }
            let count: usize = msg_send![urls, count];
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let url: *mut AnyObject = msg_send![urls, objectAtIndex: i];
                if url.is_null() {
                    continue;
                }
                let path_ns: *mut AnyObject = msg_send![url, path];
                if path_ns.is_null() {
                    continue;
                }
                let path = PathBuf::from((*(path_ns as *const NSString)).to_string());
                let (name, mime, size) = file_meta(&path);
                out.push(PickedFile {
                    name,
                    mime,
                    size,
                    path,
                });
            }
            Ok(Some(out))
        }
    }

    /// Build an `NSArray<UTType*>` from the request's MIME filters (or media
    /// kind), or `None` to leave the panel unfiltered (any file). Built via
    /// `NSMutableArray`/`addObject:` since `NSArray::from_slice` requires a
    /// concrete retainable element type we don't have a binding for (UTType).
    unsafe fn allowed_content_types(request: &PickRequest) -> Option<Retained<AnyObject>> {
        let mimes: Vec<&str> = match &request.kind {
            PickKind::Documents(m) if m.is_empty() => return None,
            PickKind::Documents(m) => m.iter().map(String::as_str).collect(),
            PickKind::Media(k) => crate::mime::media_mimes(*k).to_vec(),
        };
        let arr: Retained<AnyObject> = msg_send_id![class!(NSMutableArray), array];
        let mut any = false;
        for mime in mimes {
            let id = NSString::from_str(crate::mime::apple_uttype(mime));
            let ty: *mut AnyObject = msg_send![class!(UTType), typeWithIdentifier: &*id];
            if !ty.is_null() {
                let _: () = msg_send![&*arr, addObject: ty];
                any = true;
            }
        }
        any.then_some(arr)
    }
}

// ---------------------------------------------------------------------------
// iOS — UIDocumentPickerViewController (open) + PHPickerViewController.
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod ios {
    use std::ptr;
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use futures_channel::oneshot;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol};
    use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
    use objc2_foundation::{NSObject, NSString, NSURL};

    use super::*;

    #[link(name = "UIKit", kind = "framework")]
    extern "C" {}
    #[link(name = "PhotosUI", kind = "framework")]
    extern "C" {}
    #[link(name = "UniformTypeIdentifiers", kind = "framework")]
    extern "C" {}

    type Outcome = Result<Option<Vec<PickedFile>>, PickError>;

    /// RAII guard over an iOS security-scoped URL: starts access on acquire,
    /// stops it on drop (only if `start…` actually granted it — the API
    /// contract is "balance a YES, never balance a NO").
    pub(super) struct SecurityScope {
        url: Retained<NSURL>,
        started: bool,
    }

    impl SecurityScope {
        unsafe fn acquire(url: Retained<NSURL>) -> Self {
            let started: bool = msg_send![&*url, startAccessingSecurityScopedResource];
            Self { url, started }
        }
    }

    impl Drop for SecurityScope {
        fn drop(&mut self) {
            if self.started {
                unsafe {
                    let _: () = msg_send![&*self.url, stopAccessingSecurityScopedResource];
                }
            }
        }
    }

    // ----- Document picker (open) -----------------------------------------

    struct DocDelegateIvars {
        tx: Mutex<Option<oneshot::Sender<Outcome>>>,
    }

    declare_class!(
        struct DocDelegate;

        unsafe impl ClassType for DocDelegate {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystFilePickerDocDelegate";
        }

        impl DeclaredClass for DocDelegate {
            type Ivars = DocDelegateIvars;
        }

        unsafe impl NSObjectProtocol for DocDelegate {}

        // UIDocumentPickerDelegate — UIKit calls these selectors by name.
        unsafe impl DocDelegate {
            #[method(documentPicker:didPickDocumentsAtURLs:)]
            fn did_pick(&self, _picker: *mut AnyObject, urls: *mut AnyObject) {
                let files = unsafe { picked_from_urls(urls) };
                self.resolve(Ok(Some(files)));
            }

            #[method(documentPickerWasCancelled:)]
            fn was_cancelled(&self, _picker: *mut AnyObject) {
                self.resolve(Ok(None));
            }
        }
    );

    impl DocDelegate {
        fn resolve(&self, outcome: Outcome) {
            if let Some(tx) = self.ivars().tx.lock().unwrap().take() {
                let _ = tx.send(outcome);
            }
        }
    }

    /// Build `PickedFile`s from an `NSArray<NSURL*>*` of security-scoped URLs.
    unsafe fn picked_from_urls(urls: *mut AnyObject) -> Vec<PickedFile> {
        let mut out = Vec::new();
        if urls.is_null() {
            return out;
        }
        let count: usize = msg_send![urls, count];
        for i in 0..count {
            let url: *mut AnyObject = msg_send![urls, objectAtIndex: i];
            if url.is_null() {
                continue;
            }
            let Some(retained) = Retained::retain(url as *mut NSURL) else {
                continue;
            };
            let scope = SecurityScope::acquire(retained);
            let path_ns: *mut AnyObject = msg_send![url, path];
            if path_ns.is_null() {
                continue;
            }
            let path = PathBuf::from((*(path_ns as *const NSString)).to_string());
            let (name, mime, size) = file_meta(&path);
            out.push(PickedFile {
                name,
                mime,
                size,
                path,
                _scope: Some(scope),
            });
        }
        out
    }

    pub(super) async fn pick_documents(request: &PickRequest) -> Outcome {
        let (tx, rx) = oneshot::channel::<Outcome>();

        // SAFETY: documented UIDocumentPickerViewController open flow; runs on
        // the main thread (author code does).
        let (_picker, _delegate) = unsafe {
            let types = content_types(request);

            let alloc = DocDelegate::alloc().set_ivars(DocDelegateIvars {
                tx: Mutex::new(Some(tx)),
            });
            let delegate: Retained<DocDelegate> = msg_send_id![super(alloc), init];

            let picker_alloc: objc2::rc::Allocated<AnyObject> =
                msg_send_id![class!(UIDocumentPickerViewController), alloc];
            // forOpeningContentTypes: (asCopy defaults to NO) → open-in-place
            // with a security-scoped URL, so a huge file isn't copied up front.
            let picker: Retained<AnyObject> =
                msg_send_id![picker_alloc, initForOpeningContentTypes: &*types];
            let _: () = msg_send![&*picker, setAllowsMultipleSelection: request.allow_multiple];
            let _: () = msg_send![&*picker, setDelegate: &*delegate];

            let root = root_view_controller().ok_or(PickError::NoPresenter)?;
            let _: () = msg_send![
                &*root,
                presentViewController: &*picker,
                animated: true,
                completion: ptr::null::<AnyObject>(),
            ];
            (picker, delegate)
        };

        rx.await
            .unwrap_or_else(|_| Err(PickError::Backend("picker dropped".into())))
    }

    /// `NSArray<UTType*>` for the request — a concrete filter, or `public.item`
    /// (any) when the document request is unfiltered. Elements typed as
    /// `NSObject` to satisfy `NSArray::from_slice` (see the macOS note).
    unsafe fn content_types(request: &PickRequest) -> Retained<AnyObject> {
        let mimes: Vec<&str> = match &request.kind {
            PickKind::Documents(m) if m.is_empty() => vec!["*/*"],
            PickKind::Documents(m) => m.iter().map(String::as_str).collect(),
            PickKind::Media(k) => crate::mime::media_mimes(*k).to_vec(),
        };
        let arr: Retained<AnyObject> = msg_send_id![class!(NSMutableArray), array];
        let mut any = false;
        for mime in mimes {
            let id = NSString::from_str(crate::mime::apple_uttype(mime));
            let ty: *mut AnyObject = msg_send![class!(UTType), typeWithIdentifier: &*id];
            if !ty.is_null() {
                let _: () = msg_send![&*arr, addObject: ty];
                any = true;
            }
        }
        if !any {
            // Fall back to the root "any item" type so the picker still opens.
            let id = NSString::from_str("public.item");
            let ty: *mut AnyObject = msg_send![class!(UTType), typeWithIdentifier: &*id];
            if !ty.is_null() {
                let _: () = msg_send![&*arr, addObject: ty];
            }
        }
        arr
    }

    // ----- Media picker (PHPicker) ----------------------------------------

    /// Shared sink the per-asset load blocks write into; resolves the oneshot
    /// once every selected asset has been copied out (or failed).
    struct Collector {
        remaining: usize,
        files: Vec<PickedFile>,
        tx: Option<oneshot::Sender<Outcome>>,
    }

    impl Collector {
        fn finish_one(&mut self, file: Option<PickedFile>) {
            if let Some(f) = file {
                self.files.push(f);
            }
            self.remaining -= 1;
            if self.remaining == 0 {
                if let Some(tx) = self.tx.take() {
                    let _ = tx.send(Ok(Some(std::mem::take(&mut self.files))));
                }
            }
        }
    }

    struct MediaDelegateIvars {
        collector: Arc<Mutex<Collector>>,
    }

    declare_class!(
        struct MediaDelegate;

        unsafe impl ClassType for MediaDelegate {
            type Super = NSObject;
            type Mutability = mutability::InteriorMutable;
            const NAME: &'static str = "IdealystFilePickerMediaDelegate";
        }

        impl DeclaredClass for MediaDelegate {
            type Ivars = MediaDelegateIvars;
        }

        unsafe impl NSObjectProtocol for MediaDelegate {}

        // PHPickerViewControllerDelegate.
        unsafe impl MediaDelegate {
            #[method(picker:didFinishPicking:)]
            fn did_finish(&self, picker: *mut AnyObject, results: *mut AnyObject) {
                // Dismiss immediately; item providers stay valid afterward.
                unsafe {
                    let _: () = msg_send![picker, dismissViewControllerAnimated: true,
                        completion: ptr::null::<AnyObject>()];
                }
                unsafe { self.start_loads(results) };
            }
        }
    );

    impl MediaDelegate {
        /// Kick off a `loadFileRepresentation` for each result; each block copies
        /// its temp URL into our temp dir and reports back to the collector.
        unsafe fn start_loads(&self, results: *mut AnyObject) {
            let count: usize = if results.is_null() {
                0
            } else {
                msg_send![results, count]
            };

            let collector = self.ivars().collector.clone();
            {
                let mut c = collector.lock().unwrap();
                c.remaining = count;
                // Empty selection (or cancel) → resolve as cancelled now.
                if count == 0 {
                    if let Some(tx) = c.tx.take() {
                        let _ = tx.send(Ok(None));
                    }
                    return;
                }
            }

            for i in 0..count {
                let result: *mut AnyObject = msg_send![results, objectAtIndex: i];
                let provider: *mut AnyObject = msg_send![result, itemProvider];
                if provider.is_null() {
                    collector.lock().unwrap().finish_one(None);
                    continue;
                }
                // Pick the concrete type the asset conforms to.
                let movie = NSString::from_str("public.movie");
                let is_movie: bool =
                    msg_send![provider, hasItemConformingToTypeIdentifier: &*movie];
                let type_id = if is_movie {
                    NSString::from_str("public.movie")
                } else {
                    NSString::from_str("public.image")
                };

                let sink = collector.clone();
                // `loadFileRepresentation` calls this block on an arbitrary
                // queue; the temp URL is valid only synchronously inside it, so
                // we copy it out immediately (disk→disk, not into memory).
                let block = RcBlock::new(move |url: *mut AnyObject, _err: *mut AnyObject| {
                    let file = unsafe { copy_asset_to_temp(url) };
                    sink.lock().unwrap().finish_one(file);
                });
                let _progress: *mut AnyObject = msg_send![
                    provider,
                    loadFileRepresentationForTypeIdentifier: &*type_id,
                    completionHandler: &*block,
                ];
            }
        }
    }

    /// Copy the asset's transient temp URL into our own temp dir, returning a
    /// `PickedFile` over the owned copy.
    unsafe fn copy_asset_to_temp(url: *mut AnyObject) -> Option<PickedFile> {
        if url.is_null() {
            return None;
        }
        let path_ns: *mut AnyObject = msg_send![url, path];
        if path_ns.is_null() {
            return None;
        }
        let src = PathBuf::from((*(path_ns as *const NSString)).to_string());
        let name = src
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "media".to_string());
        let dest = std::env::temp_dir().join(format!("idealyst-pick-{}", name));
        std::fs::copy(&src, &dest).ok()?;
        let (name, mime, size) = file_meta(&dest);
        Some(PickedFile {
            name,
            mime,
            size,
            path: dest,
            _scope: None,
        })
    }

    pub(super) async fn pick_media(request: &PickRequest) -> Outcome {
        let kind = match &request.kind {
            PickKind::Media(k) => *k,
            // pick() only routes Media here.
            PickKind::Documents(_) => return Err(PickError::Backend("not a media request".into())),
        };

        let (tx, rx) = oneshot::channel::<Outcome>();
        let collector = Arc::new(Mutex::new(Collector {
            remaining: 0,
            files: Vec::new(),
            tx: Some(tx),
        }));

        // SAFETY: documented PHPickerViewController flow; presented from the
        // main thread.
        let (_picker, _delegate) = unsafe {
            let filter = media_filter(kind);
            let config: Retained<AnyObject> =
                msg_send_id![class!(PHPickerConfiguration), new];
            let _: () = msg_send![&*config, setFilter: filter];
            let limit: isize = if request.allow_multiple { 0 } else { 1 };
            let _: () = msg_send![&*config, setSelectionLimit: limit];

            let picker_alloc: objc2::rc::Allocated<AnyObject> =
                msg_send_id![class!(PHPickerViewController), alloc];
            let picker: Retained<AnyObject> =
                msg_send_id![picker_alloc, initWithConfiguration: &*config];

            let alloc = MediaDelegate::alloc().set_ivars(MediaDelegateIvars {
                collector: collector.clone(),
            });
            let delegate: Retained<MediaDelegate> = msg_send_id![super(alloc), init];
            let _: () = msg_send![&*picker, setDelegate: &*delegate];

            let root = root_view_controller().ok_or(PickError::NoPresenter)?;
            let _: () = msg_send![
                &*root,
                presentViewController: &*picker,
                animated: true,
                completion: ptr::null::<AnyObject>(),
            ];
            (picker, delegate)
        };

        rx.await
            .unwrap_or_else(|_| Err(PickError::Backend("picker dropped".into())))
    }

    /// A `PHPickerFilter` for the media kind (images / videos / both).
    unsafe fn media_filter(kind: crate::MediaKind) -> *mut AnyObject {
        use crate::MediaKind::*;
        match kind {
            Images => msg_send![class!(PHPickerFilter), imagesFilter],
            Videos => msg_send![class!(PHPickerFilter), videosFilter],
            ImagesAndVideos => {
                let images: *mut AnyObject = msg_send![class!(PHPickerFilter), imagesFilter];
                let videos: *mut AnyObject = msg_send![class!(PHPickerFilter), videosFilter];
                let arr: Retained<AnyObject> = msg_send_id![class!(NSMutableArray), array];
                if !images.is_null() {
                    let _: () = msg_send![&*arr, addObject: images];
                }
                if !videos.is_null() {
                    let _: () = msg_send![&*arr, addObject: videos];
                }
                msg_send![class!(PHPickerFilter), anyFilterMatchingSubfilters: &*arr]
            }
        }
    }

    /// The key window's root view controller, to present the picker from.
    /// (Identical posture to `file-export`'s presenter lookup.)
    unsafe fn root_view_controller() -> Option<Retained<AnyObject>> {
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        if app.is_null() {
            return None;
        }
        let window: *mut AnyObject = msg_send![app, keyWindow];
        let window = if window.is_null() {
            let windows: *mut AnyObject = msg_send![app, windows];
            if windows.is_null() {
                return None;
            }
            let count: usize = msg_send![windows, count];
            if count == 0 {
                return None;
            }
            msg_send![windows, objectAtIndex: 0usize]
        } else {
            window
        };
        if window.is_null() {
            return None;
        }
        let root: *mut AnyObject = msg_send![window, rootViewController];
        let mut top = root;
        loop {
            if top.is_null() {
                return None;
            }
            let presented: *mut AnyObject = msg_send![top, presentedViewController];
            if presented.is_null() {
                break;
            }
            top = presented;
        }
        Retained::retain(top)
    }
}
