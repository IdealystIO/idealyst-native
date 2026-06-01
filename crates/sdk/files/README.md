# `files`

Cross-platform **blob/file storage** — read and write *binary* data
(recordings, images, downloads, caches) in a per-app private directory,
addressed by relative path. It's the third storage primitive:

| Crate | For | Shape |
| --- | --- | --- |
| [`storage`](../storage) | non-secret app data (preferences) | plaintext key → string |
| [`credentials`](../credentials) | secrets (tokens, keys) | secure key → string |
| **`files`** | **binary blobs** | path → bytes |

```rust
use files::app_files;

# async fn demo() -> Result<(), files::FileError> {
let store = app_files("myapp")?;                       // Arc<dyn FileStore>
store.write("recordings/note1.wav", &wav_bytes).await?;
let bytes = store.read("recordings/note1.wav").await?; // Option<Vec<u8>>
let names = store.list("recordings").await?;           // ["note1.wav", ...]
store.delete("recordings/note1.wav").await?;
# Ok(())
# }
```

## Backends

| Platform | Backend |
| --- | --- |
| macOS | `~/Library/Application Support/<name>/…` |
| Windows | `%APPDATA%\<name>\…` |
| Linux | `$XDG_DATA_HOME` (or `~/.local/share`)`/<name>/…` |
| iOS | the app sandbox's Application Support dir (via NSFileManager) |
| Android | `Context.getFilesDir()/<name>/…` |
| web (wasm32) | IndexedDB — blobs keyed by path (no filesystem in a browser) |

On native, [`FileStore::local_path`] returns the real filesystem path, so you
can hand it to a native API (an audio encoder, an image loader). On web it
returns `None` — there's no path; pass the bytes around instead.

## API

One async, object-safe `FileStore` (held as `Arc<dyn FileStore>`):

- `read(path) -> Option<Vec<u8>>` — bytes, or `None` if absent.
- `write(path, &[u8])` — creates parent dirs, replaces any existing blob.
- `delete(path)` — idempotent.
- `exists(path) -> bool`.
- `list(dir) -> Vec<String>` — immediate child names (non-recursive); missing
  dir → empty.
- `local_path(path) -> Option<PathBuf>` — real path on native, `None` on web.

Paths are **relative** within the store. An absolute path or any `..`
component is rejected with `FileError::UnsafePath`, so a caller can't escape
the store root.

It's async because blob I/O can be large and shouldn't block. On native the
work is synchronous `std::fs` inside the returned future (fine for the modest
blobs this is meant for; a high-throughput caller should front it with its own
offloading); on web it's genuinely async IndexedDB.

## Scope

Whole-blob read/write (no streaming) and a flat per-store namespace are the
first cut — enough to save and load recordings/images. Streaming large files
and richer directory operations are natural follow-ons behind the same trait.

## Verification

- **Native filesystem** — host-tested on macOS: round-trip read/write/delete,
  overwrite, idempotent delete, `list`, `local_path`, and unsafe-path
  rejection (`cargo test -p files`).
- **iOS / Android** — compile-checked; the app-dir resolution (objc2 /
  JNI) isn't device-run here.
- **web (IndexedDB)** — compile-checked for `wasm32`; not browser-run here.

[`FileStore::local_path`]: src/lib.rs
