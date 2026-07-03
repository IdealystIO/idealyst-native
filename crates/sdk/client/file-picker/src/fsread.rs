//! Shared chunked file reader for the backends that read a picked file from a
//! real filesystem path (macOS, iOS, Windows, Linux). Android reads over a
//! file descriptor and web over a `Blob`, so each of those supplies its own
//! `FileStream` instead.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::PickError;

/// Display name + best-effort MIME + size for an on-disk path — the metadata
/// the path-based backends (macOS/iOS/Windows/Linux) attach to a `PickedFile`.
pub(crate) fn file_meta(path: &Path) -> (String, String, Option<u64>) {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mime = crate::mime::guess_mime(&name);
    let size = std::fs::metadata(path).ok().map(|m| m.len());
    (name, mime, size)
}

/// A forward-only reader that hands back the file's bytes in [`READ_CHUNK`]
/// (1 MiB) pieces, so a multi-GB pick never lands in memory at once.
///
/// [`READ_CHUNK`]: crate::READ_CHUNK
pub(crate) struct FileStream {
    file: File,
}

impl FileStream {
    /// Open `path` for streaming.
    pub(crate) fn open(path: &Path) -> Result<Self, PickError> {
        let file = File::open(path).map_err(|e| PickError::Io(e.to_string()))?;
        Ok(Self { file })
    }

    /// Read the next chunk, or `Ok(None)` at EOF.
    pub(crate) async fn chunk(&mut self) -> Result<Option<Vec<u8>>, PickError> {
        let mut buf = vec![0u8; crate::READ_CHUNK];
        let n = self
            .file
            .read(&mut buf)
            .map_err(|e| PickError::Io(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial executor so we can drive the async `chunk()` in a sync test
    /// without pulling in a runtime — every future here is immediately ready.
    fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    /// Streams a file larger than one chunk back in bounded pieces, never
    /// buffering it whole — the core no-RAM-blowup guarantee. Verifies chunk
    /// sizing, the multi-chunk split, EOF, and byte-exact reassembly.
    #[test]
    fn streams_large_file_in_bounded_chunks() {
        // 2.5 MiB of position-derived bytes → spans 3 chunks (1 + 1 + 0.5 MiB).
        let len = crate::READ_CHUNK * 2 + crate::READ_CHUNK / 2;
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();

        let path = std::env::temp_dir().join("file-picker-fsread-test.bin");
        std::fs::write(&path, &data).unwrap();

        let mut stream = FileStream::open(&path).unwrap();
        let mut reassembled = Vec::new();
        let mut chunk_count = 0;
        while let Some(chunk) = block_on(stream.chunk()).unwrap() {
            assert!(
                chunk.len() <= crate::READ_CHUNK,
                "a chunk exceeded READ_CHUNK ({} bytes) — would defeat bounded memory",
                chunk.len()
            );
            reassembled.extend_from_slice(&chunk);
            chunk_count += 1;
        }
        // Next read past EOF stays None (idempotent end).
        assert!(block_on(stream.chunk()).unwrap().is_none());

        assert_eq!(chunk_count, 3, "expected 1MiB + 1MiB + 0.5MiB chunks");
        assert_eq!(reassembled, data, "streamed bytes must match the file exactly");

        let _ = std::fs::remove_file(&path);
    }
}
