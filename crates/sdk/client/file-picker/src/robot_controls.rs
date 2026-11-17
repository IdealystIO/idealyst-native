//! Robot test controls: hand the app files instead of driving the OS
//! picker, which a test can't (a native dialog, a photo library). Compiled
//! only into robot builds; elsewhere `take` answers nothing and the real
//! picker opens.
//!
//! A test queues answers; each [`FilePicker::pick`](crate::FilePicker::pick)
//! takes the oldest, with no dialog: the files queued (as
//! [`PickedFile`](crate::PickedFile)s read from memory, `path()` `None`, as
//! on web) or a cancel. With nothing queued the real picker opens. Each
//! pick is logged (`file_picker.requests`) so a test can check what the
//! app asked for.

use crate::{PickOutcome, PickRequest};

runtime_vocabulary::robot_select! {
    on: {
        use std::cell::RefCell;
        use std::collections::VecDeque;
        use std::sync::Arc;

        use base64::Engine as _;
        use runtime_core::robot_fn;
        use runtime_vocabulary::robot_fn::register as register_fn;
        use serde::Deserialize;

        use crate::{GivenFile, MediaKind, PickKind, PickedFile};

        /// A queued answer: these files, or a cancel.
        enum Answer {
            Files(Vec<(String, String, Arc<Vec<u8>>)>),
            Cancel,
        }

        runtime_tls::thread_local! {
            static QUEUE: RefCell<VecDeque<Answer>> = const { RefCell::new(VecDeque::new()) };
            static REQUESTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        }

        fn describe(request: &PickRequest) -> String {
            let kind = match &request.kind {
                PickKind::Documents(m) if m.is_empty() => "documents".to_string(),
                PickKind::Documents(m) => format!("documents {}", m.join(",")),
                PickKind::Media(MediaKind::Images) => "media images".into(),
                PickKind::Media(MediaKind::Videos) => "media videos".into(),
                PickKind::Media(MediaKind::ImagesAndVideos) => "media images+videos".into(),
            };
            if request.allow_multiple { format!("{kind} multiple") } else { kind }
        }

        /// The oldest queued answer for this pick, if any.
        pub(crate) fn take(request: &PickRequest) -> Option<PickOutcome> {
            REQUESTS.with(|r| r.borrow_mut().push(describe(request)));
            let answer = QUEUE.with(|q| q.borrow_mut().pop_front())?;
            Some(match answer {
                Answer::Cancel => PickOutcome::Cancelled,
                Answer::Files(files) => PickOutcome::Picked(
                    files
                        .into_iter()
                        .map(|(name, mime, bytes)| PickedFile::given(GivenFile { name, mime, bytes }))
                        .collect(),
                ),
            })
        }

        /// One file a test hands the app.
        #[derive(Deserialize)]
        struct QueuedFile {
            name: String,
            mime: Option<String>,
            /// The contents, base64.
            base64: Option<String>,
            /// The contents as text (instead of `base64`).
            text: Option<String>,
        }

        /// Queue the answer to the next pick: `files`, each `{name, mime?,
        /// base64? | text?}` (the mime defaults from the name's extension).
        /// The pick returns them with no dialog. Queued answers are taken in
        /// order, one per pick.
        #[robot_fn(name = "file_picker.queue")]
        fn queue(files: Vec<QueuedFile>) -> Result<usize, String> {
            if files.is_empty() {
                return Err("file_picker.queue: no files (use file_picker.cancel_next for a cancel)".into());
            }
            let mut out = Vec::with_capacity(files.len());
            for f in files {
                let bytes = match (f.base64, f.text) {
                    (Some(_), Some(_)) => return Err(format!("{}: give `base64` or `text`, not both", f.name)),
                    (Some(b), None) => base64::engine::general_purpose::STANDARD
                        .decode(b.as_bytes())
                        .map_err(|e| format!("{}: base64: {e}", f.name))?,
                    (None, Some(t)) => t.into_bytes(),
                    (None, None) => Vec::new(),
                };
                let mime = f.mime.unwrap_or_else(|| crate::mime::guess_mime(&f.name));
                out.push((f.name, mime, Arc::new(bytes)));
            }
            let n = out.len();
            QUEUE.with(|q| q.borrow_mut().push_back(Answer::Files(out)));
            Ok(n)
        }

        /// The next pick is cancelled, as if the user dismissed the picker.
        #[robot_fn(name = "file_picker.cancel_next")]
        fn cancel_next() {
            QUEUE.with(|q| q.borrow_mut().push_back(Answer::Cancel));
        }

        /// The picks the app made since the last `file_picker.clear`, oldest
        /// first, described as `documents application/pdf`,
        /// `media images multiple`, ….
        #[robot_fn(name = "file_picker.requests")]
        fn requests() -> Vec<String> {
            REQUESTS.with(|r| r.borrow().clone())
        }

        /// Drop the queued answers and the request log.
        #[robot_fn(name = "file_picker.clear")]
        fn clear() {
            QUEUE.with(|q| q.borrow_mut().clear());
            REQUESTS.with(|r| r.borrow_mut().clear());
        }

        pub(crate) fn register() {
            register_fn(queue::robot());
            register_fn(cancel_next::robot());
            register_fn(requests::robot());
            register_fn(clear::robot());
        }
    }
    off: {
        pub(crate) fn take(_request: &PickRequest) -> Option<PickOutcome> {
            None
        }
        pub(crate) fn register() {}
    }
}
