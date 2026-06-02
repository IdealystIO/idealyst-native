//! Cross-platform blob/file storage for **binary data** — recordings,
//! images, downloads, caches. The third storage primitive, alongside
//! [`storage`](../storage) (plaintext key-value) and
//! [`credentials`](../credentials) (secrets). This one is for *bytes*.
//!
//! A [`FileStore`] reads/writes/deletes/lists binary blobs addressed by a
//! relative path, rooted in a **per-app private directory**. Get one for the
//! current platform with [`app_files`]:
//!
//! ```no_run
//! use files::app_files;
//!
//! # async fn demo() -> Result<(), files::FileError> {
//! let store = app_files("myapp")?;                 // Arc<dyn FileStore>
//! store.write("recordings/note1.wav", &[0u8; 16]).await?;
//! let bytes = store.read("recordings/note1.wav").await?;   // Option<Vec<u8>>
//! let names = store.list("recordings").await?;            // Vec<String>
//! store.delete("recordings/note1.wav").await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Backends
//!
//! - **iOS / macOS / Windows / Linux / Android** — the real filesystem,
//!   rooted in the app's private data dir (sandbox Application Support /
//!   `%APPDATA%` / XDG data dir / `getFilesDir()`). [`FileStore::local_path`]
//!   returns the real path, so you can hand it to a native API.
//! - **web (wasm32)** — IndexedDB (a browser has no filesystem); blobs are
//!   keyed by their path, and `local_path` returns `None`.
//!
//! Async because blob I/O can be large. On native the work is synchronous
//! `std::fs` inside the returned future (fine for the modest blobs this is
//! meant for; a high-throughput caller should front it with its own
//! offloading); on web it's genuinely async IndexedDB.

#![deny(missing_docs)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

/// A blob-store failure.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// A path escaped the store root (contained `..` or was absolute).
    #[error("unsafe path '{0}': paths must be relative and may not contain '..'")]
    UnsafePath(String),
    /// The app's data directory couldn't be resolved (no `HOME`, no Android
    /// context, etc.).
    #[error("could not resolve the app data directory: {0}")]
    NoAppDir(String),
    /// The underlying backend failed (I/O, IndexedDB, platform API).
    #[error("file store backend error: {0}")]
    Backend(String),
}

// The future a `FileStore` op returns. Web's IndexedDB futures hold non-`Send`
// JS values across `.await`, so the `Send` bound is cfg'd off there; native
// futures are `Send`. (The store itself stays `Send + Sync` everywhere — the
// web impl holds only `String`s and opens the DB per op.)
/// A future returned by a [`FileStore`] op (native: `Send`; web: not).
#[cfg(not(target_arch = "wasm32"))]
pub type FileFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, FileError>> + Send + 'a>>;
/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub type FileFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, FileError>> + 'a>>;

/// Async binary blob storage rooted in a per-app private directory. Paths
/// are relative (`"sub/dir/file.bin"`); a leading `/` or any `..` component
/// is rejected as [`FileError::UnsafePath`].
pub trait FileStore: Send + Sync {
    /// The bytes at `path`, or `None` if no such blob exists.
    fn read(&self, path: &str) -> FileFuture<'_, Option<Vec<u8>>>;
    /// Write `bytes` to `path`, creating parent directories and replacing
    /// any existing blob.
    fn write(&self, path: &str, bytes: &[u8]) -> FileFuture<'_, ()>;
    /// Delete `path`. `Ok(())` whether or not it existed.
    fn delete(&self, path: &str) -> FileFuture<'_, ()>;
    /// Whether a blob exists at `path`.
    fn exists(&self, path: &str) -> FileFuture<'_, bool>;
    /// The blob/entry names directly under `dir` (not recursive). An empty
    /// `dir` lists the store root. Missing directory → empty list.
    fn list(&self, dir: &str) -> FileFuture<'_, Vec<String>>;
    /// The real filesystem path for `path` on file-backed (native) stores —
    /// hand it to a native API that wants a path. `None` on web, which has
    /// no filesystem (pass the bytes around instead).
    fn local_path(&self, path: &str) -> Option<PathBuf>;
}

/// Validate a caller-supplied relative path and split it into components.
/// Rejects absolute paths and any `..` (parent-dir escape). Returns the
/// normalized relative `PathBuf` for joining onto the store root.
pub(crate) fn safe_relative(path: &str) -> Result<PathBuf, FileError> {
    use std::path::Component;
    let p = std::path::Path::new(path);
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            // Ignore `.` and a leading `/`-rooted CurDir; reject the rest.
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => {
                return Err(FileError::UnsafePath(path.to_string()));
            }
        }
    }
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(not(target_arch = "wasm32"))]
mod native;

/// An `Arc<dyn FileStore>` over the current platform's blob storage,
/// namespaced by `name` (a subdirectory of the app's private data dir on
/// native; an IndexedDB database on web). Created/opened on first use.
///
/// `Err` if the app data directory can't be resolved (native).
pub fn app_files(name: &str) -> Result<std::sync::Arc<dyn FileStore>, FileError> {
    #[cfg(target_arch = "wasm32")]
    {
        return Ok(std::sync::Arc::new(web::IndexedDbFileStore::new(name)));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        return Ok(std::sync::Arc::new(native::FsFileStore::open(name)?));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_accepts_nested() {
        assert_eq!(
            safe_relative("a/b/c.bin").unwrap(),
            PathBuf::from("a/b/c.bin")
        );
        assert_eq!(safe_relative("./x").unwrap(), PathBuf::from("x"));
    }

    #[test]
    fn safe_relative_rejects_escapes() {
        assert!(matches!(
            safe_relative("../secret"),
            Err(FileError::UnsafePath(_))
        ));
        assert!(matches!(
            safe_relative("a/../../b"),
            Err(FileError::UnsafePath(_))
        ));
        assert!(matches!(
            safe_relative("/etc/passwd"),
            Err(FileError::UnsafePath(_))
        ));
    }
}
